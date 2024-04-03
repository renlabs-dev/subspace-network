use core::marker::PhantomData;

use sp_std::{borrow::Cow, collections::btree_map::BTreeMap};
use substrate_fixed::types::{I32F32, I64F64, I96F32};

use crate::{
    math::*, vec, Active, Bonds, BondsMovingAverage, Config, Consensus, Dividends, Emission,
    Incentive, Kappa, Keys, MaxAllowedValidators, MaxWeightAge, Pallet, PruningScores, Rank, Stake,
    Trust, ValidatorPermits, ValidatorTrust, Weights,
};
use frame_support::dispatch::Vec;

type EmissionMap<T> = BTreeMap<ModuleKey<T>, BTreeMap<AccountKey<T>, u64>>;

pub struct YumaCalc<T: Config> {
    /// The amount of modules on the subnet
    module_count: u16,
    /// The UID of the subnet
    netuid: u16,
    /// Consensus majority ratio, e.g. 51%.
    kappa: I32F32,

    founder_key: AccountKey<T>,
    founder_emission: u64,
    to_be_emitted: u64,

    current_block: u64,
    activity_cutoff: u64,
    last_update: Vec<u64>,
    block_at_registration: Vec<u64>,

    validator_permits: Vec<bool>,
    validator_forbids: Vec<bool>,
    max_allowed_validators: Option<u16>,

    _pd: PhantomData<T>,
}

impl<T: Config> YumaCalc<T> {
    pub fn new(netuid: u16, to_be_emitted: u64) -> Self {
        let validator_permits = ValidatorPermits::<T>::get(netuid);
        let validator_forbids = validator_permits.iter().map(|&b| !b).collect();

        let founder_key = Pallet::<T>::get_founder(netuid);
        let (to_be_emitted, founder_emission) =
            Pallet::<T>::calculate_founder_emission(netuid, to_be_emitted, &founder_key);

        Self {
            module_count: Pallet::<T>::get_subnet_n(netuid),
            netuid,
            kappa: Pallet::<T>::get_float_kappa(),

            founder_key: AccountKey(founder_key),
            founder_emission,
            to_be_emitted,

            current_block: Pallet::<T>::get_current_block_number(),
            activity_cutoff: MaxWeightAge::<T>::get(netuid),
            last_update: Pallet::<T>::get_last_update(netuid),
            block_at_registration: Pallet::<T>::get_last_update(netuid),

            validator_forbids,
            validator_permits,
            max_allowed_validators: MaxAllowedValidators::<T>::get(netuid),

            _pd: Default::default(),
        }
    }

    /// Runs the YUMA consensus calculation on the network and distributes the emissions. Returns a
    /// map of emissions distributed per module key.
    pub fn run(self) -> EmissionMap<T> {
        let (inactive, active): (Vec<_>, Vec<_>) = self
            .last_update
            .iter()
            .map(|&updated| {
                let is_inactive = updated + self.activity_cutoff < self.current_block;
                (is_inactive, !is_inactive)
            })
            .unzip();
        log::trace!("Inactive: {inactive:?}");
        log::trace!("Active: {active:?}");

        let mut weights = self.compute_weights();
        let stake = self.compute_stake();

        let new_permits: Vec<bool> = if let Some(max) = self.max_allowed_validators {
            is_topk(stake.as_ref(), max as usize)
        } else {
            vec![true; stake.as_ref().len()]
        };

        let active_stake = self.compute_active_stake(&inactive, &stake);

        let ConsensusAndTrust {
            consensus,
            validator_trust,
            preranks,
        } = self.compute_consensus_and_trust(&mut weights, &active_stake);

        let IncentivesAndTrust {
            incentives,
            ranks,
            trust,
        } = self.compute_incentive_and_trust(&weights, &active_stake, &preranks);

        let BondsAndDividends {
            ema_bonds,
            dividends,
        } = self.compute_bonds_and_dividends(&weights, &active_stake, &incentives);

        let Emissions {
            pruning_scores,
            validator_emissions,
            server_emissions,
            combined_emissions,
        } = self.compute_emissions(&stake, &active_stake, &incentives, &dividends);

        let consensus: Vec<_> =
            consensus.into_inner().into_iter().map(fixed_proportion_to_u16).collect();
        let incentives: Vec<_> =
            incentives.into_inner().into_iter().map(fixed_proportion_to_u16).collect();
        let dividends: Vec<_> =
            dividends.into_inner().into_iter().map(fixed_proportion_to_u16).collect();
        let trust: Vec<_> = trust.into_inner().into_iter().map(fixed_proportion_to_u16).collect();
        let ranks: Vec<_> = ranks.into_inner().into_iter().map(fixed_proportion_to_u16).collect();
        let pruning_scores = vec_max_upscale_to_u16(pruning_scores.as_ref());
        let validator_trust: Vec<_> =
            validator_trust.into_inner().into_iter().map(fixed_proportion_to_u16).collect();

        Active::<T>::insert(self.netuid, active.clone());
        Consensus::<T>::insert(self.netuid, consensus);
        Dividends::<T>::insert(self.netuid, dividends);
        Emission::<T>::insert(self.netuid, combined_emissions);
        Incentive::<T>::insert(self.netuid, incentives);
        PruningScores::<T>::insert(self.netuid, pruning_scores);
        Rank::<T>::insert(self.netuid, ranks);
        Trust::<T>::insert(self.netuid, trust);
        ValidatorPermits::<T>::insert(self.netuid, &new_permits);
        ValidatorTrust::<T>::insert(self.netuid, validator_trust);

        for i in 0..self.module_count as usize {
            // Set bonds only if uid retains validator permit, otherwise clear bonds.
            if new_permits[i] {
                let new_bonds_row: Vec<(u16, u16)> = ema_bonds[i]
                    .iter()
                    .map(|(j, value)| (*j, fixed_proportion_to_u16(*value)))
                    .collect();
                Bonds::<T>::insert(self.netuid, i as u16, new_bonds_row);
            } else {
                if self.max_allowed_validators.is_none() || self.validator_permits[i] {
                    // Only overwrite the intersection.
                    let new_empty_bonds_row: Vec<(u16, u16)> = vec![];
                    Bonds::<T>::insert(self.netuid, i as u16, new_empty_bonds_row);
                }
            }
        }

        // Emission tuples ( key, server_emission, validator_emission )
        let mut result: Vec<(ModuleKey<T>, u64, u64)> = vec![];
        for (uid_i, module_key) in Keys::<T>::iter_prefix(self.netuid) {
            result.push((
                ModuleKey(module_key),
                server_emissions[uid_i as usize],
                validator_emissions[uid_i as usize],
            ));
        }

        self.distribute_emissions(result)
    }

    fn distribute_emissions(&self, result: Vec<(ModuleKey<T>, u64, u64)>) -> EmissionMap<T> {
        let mut emissions: EmissionMap<T> = Default::default();
        let mut emitted = 0;

        for (module_key, mut server_emission, mut validator_emission) in result {
            if module_key.0 == self.founder_key.0 {
                server_emission = server_emission.saturating_add(self.founder_emission);
            }

            let mut increase_stake = |account_key: &AccountKey<T>, amount: u64| {
                Pallet::<T>::increase_stake(self.netuid, &account_key.0, &module_key.0, amount);
                *emissions
                    .entry(module_key.clone())
                    .or_default()
                    .entry(account_key.clone())
                    .or_default() += amount;
                emitted += amount;
            };

            if validator_emission > 0 {
                let ownership_vector =
                    Pallet::<T>::get_ownership_ratios(self.netuid, &module_key.0);
                let delegation_fee = Pallet::<T>::get_delegation_fee(self.netuid, &module_key.0);

                let total_validator_emission = I64F64::from_num(validator_emission);
                for (delegate_key, delegate_ratio) in ownership_vector {
                    if delegate_key == module_key.0 {
                        continue;
                    }

                    let dividends_from_delegate: u64 =
                        (total_validator_emission * delegate_ratio).to_num::<u64>();

                    let to_module: u64 = delegation_fee.mul_floor(dividends_from_delegate);
                    let to_delegate: u64 = dividends_from_delegate.saturating_sub(to_module);

                    increase_stake(&AccountKey(delegate_key), to_delegate);

                    validator_emission = validator_emission
                        .checked_sub(to_delegate)
                        .expect("more validator emissions were done than expected");
                }
            }

            let mut remaining_emission = server_emission + validator_emission;
            if remaining_emission > 0 {
                let profit_share_emissions =
                    Pallet::<T>::get_profit_share_emissions(&module_key.0, remaining_emission);

                if !profit_share_emissions.is_empty() {
                    for (profit_share_key, profit_share_emission) in profit_share_emissions {
                        increase_stake(&AccountKey(profit_share_key), profit_share_emission);

                        remaining_emission = remaining_emission
                            .checked_sub(profit_share_emission)
                            .expect("more remaining emissions were done than expected");
                    }
                } else {
                    increase_stake(&AccountKey(module_key.0.clone()), remaining_emission);

                    remaining_emission = 0;
                }
            }

            assert_eq!(remaining_emission, 0);
        }

        assert!(
            emitted <= self.founder_emission + self.to_be_emitted,
            "emitted more than was supposed to: {emitted} > {}",
            self.founder_emission + self.to_be_emitted
        );

        emissions
    }

    fn compute_weights(&self) -> WeightsVal {
        // Access network weights row unnormalized.
        let mut weights = Pallet::<T>::get_weights_sparse(self.netuid);
        log::trace!("W: {weights:?}");

        if self.max_allowed_validators.is_some() {
            // Mask weights that are not from permitted validators.
            weights = mask_rows_sparse(&self.validator_forbids, &weights);
            log::trace!("W (permit): {weights:?}");
        }

        // Remove self-weight by masking diagonal.
        weights = mask_diag_sparse(&weights);
        log::trace!("W (permit+diag): {weights:?}");

        // Remove weights referring to deregistered modules.
        weights = vec_mask_sparse_matrix(
            &weights,
            &self.last_update,
            &self.block_at_registration,
            |updated, registered| updated <= registered,
        );
        log::trace!("W (permit+diag+outdate): {weights:?}");
        // Normalize remaining weights.
        inplace_row_normalize_sparse(&mut weights);

        WeightsVal::unchecked_from_inner(weights)
    }

    fn compute_stake(&self) -> StakeVal {
        // Access network stake as normalized vector.
        let mut stake: Vec<_> =
            Stake::<T>::iter_prefix_values(self.netuid).map(I64F64::from_num).collect();
        assert_eq!(stake.len(), self.module_count as usize);

        inplace_normalize_64(&mut stake);

        log::trace!("S: {:?}", &stake);
        StakeVal::unchecked_from_inner(vec_fixed64_to_fixed32(stake)) // range: I32F32(0, 1)
    }

    fn compute_active_stake(&self, inactive: &[bool], stake: &StakeVal) -> ActiveStake {
        let mut active_stake = stake.as_ref().clone();

        // Remove inactive stake.
        inplace_mask_vector(&inactive, &mut active_stake);

        if self.max_allowed_validators.is_some() {
            // Remove non-validator stake.
            inplace_mask_vector(&self.validator_forbids, &mut active_stake);
        }

        // Normalize active stake.
        inplace_normalize(&mut active_stake);

        ActiveStake::unchecked_from_inner(active_stake)
    }

    fn compute_consensus_and_trust(
        &self,
        weights: &mut WeightsVal,
        active_stake: &ActiveStake,
    ) -> ConsensusAndTrust {
        // Compute preranks: r_j = SUM(i) w_ij * s_i
        let preranks = matmul_sparse(weights.as_ref(), active_stake.as_ref(), self.module_count);
        log::trace!("R (before): {:?}", &preranks);

        // Clip weights at majority consensus
        let consensus = weighted_median_col_sparse(
            active_stake.as_ref(),
            weights.as_ref(),
            self.module_count,
            self.kappa,
        );
        log::trace!("C: {:?}", &consensus);
        *weights = WeightsVal::unchecked_from_inner(col_clip_sparse(weights.as_ref(), &consensus));
        log::trace!("W: {:?}", &weights);

        let validator_trust = row_sum_sparse(weights.as_ref());
        log::trace!("Tv: {:?}", &validator_trust);

        ConsensusAndTrust {
            consensus: ConsensusVal::unchecked_from_inner(consensus),
            validator_trust: ValidatorTrustVal::unchecked_from_inner(validator_trust),
            preranks: Preranks::unchecked_from_inner(preranks),
        }
    }

    fn compute_incentive_and_trust(
        &self,
        weights: &WeightsVal,
        active_stake: &ActiveStake,
        preranks: &Preranks,
    ) -> IncentivesAndTrust {
        // Compute ranks: r_j = SUM(i) w_ij * s_i.
        let ranks = matmul_sparse(weights.as_ref(), active_stake.as_ref(), self.module_count);
        log::trace!("R (after): {:?}", &ranks);

        // Compute server trust: ratio of rank after vs. rank before.
        let trust = vecdiv(&ranks, preranks.as_ref()); // range: I32F32(0, 1)
        log::trace!("T: {:?}", &trust);

        let mut incentives = ranks.clone();
        inplace_normalize(&mut incentives); // range: I32F32(0, 1)
        log::trace!("I (=R): {:?}", &incentives);

        IncentivesAndTrust {
            incentives: IncentivesVal::unchecked_from_inner(incentives),
            ranks: RanksVal::unchecked_from_inner(ranks),
            trust: TrustVal::unchecked_from_inner(trust),
        }
    }

    fn compute_bonds_and_dividends(
        &self,
        weights: &WeightsVal,
        active_stake: &ActiveStake,
        incentives: &IncentivesVal,
    ) -> BondsAndDividends {
        // Access network bonds.
        let mut bonds = Pallet::<T>::get_bonds_sparse(self.netuid);
        log::trace!("B: {:?}", &bonds);

        // Remove bonds referring to deregistered modules.
        bonds = vec_mask_sparse_matrix(
            &bonds,
            &self.last_update,
            &self.block_at_registration,
            &|updated, registered| updated <= registered,
        );
        log::trace!("B (outdatedmask): {:?}", &bonds);

        // Normalize remaining bonds: sum_i b_ij = 1.
        inplace_col_normalize_sparse(&mut bonds, self.module_count);
        log::trace!("B (mask+norm): {:?}", &bonds);

        // Compute bonds delta column normalized.
        let mut bonds_delta = row_hadamard_sparse(weights.as_ref(), active_stake.as_ref()); // ΔB = W◦S (outdated W masked)
        log::trace!("ΔB: {:?}", &bonds_delta);

        // Normalize bonds delta.
        inplace_col_normalize_sparse(&mut bonds_delta, self.module_count); // sum_i b_ij = 1
        log::trace!("ΔB (norm): {:?}", &bonds_delta);

        // Compute bonds moving average.
        let bonds_moving_average = I64F64::from_num(BondsMovingAverage::<T>::get(self.netuid))
            / I64F64::from_num(1_000_000);
        let alpha = I32F32::from_num(1) - I32F32::from_num(bonds_moving_average);
        let mut ema_bonds = mat_ema_sparse(&bonds_delta, &bonds, alpha);

        // Normalize EMA bonds.
        inplace_col_normalize_sparse(&mut ema_bonds, self.module_count); // sum_i b_ij = 1
        log::trace!("emaB: {:?}", &ema_bonds);

        // Compute dividends: d_i = SUM(j) b_ij * inc_j.
        // range: I32F32(0, 1)
        let mut dividends = matmul_transpose_sparse(&ema_bonds, incentives.as_ref());
        inplace_normalize(&mut dividends);
        log::trace!("D: {:?}", &dividends);

        // Column max-upscale EMA bonds for storage: max_i w_ij = 1.
        inplace_col_max_upscale_sparse(&mut ema_bonds, self.module_count);

        BondsAndDividends {
            ema_bonds,
            dividends: DividendsVal::unchecked_from_inner(dividends),
        }
    }

    fn compute_emissions<'a>(
        &self,
        stake: &'a StakeVal,
        active_stake: &'a ActiveStake,
        incentives: &IncentivesVal,
        dividends: &DividendsVal,
    ) -> Emissions {
        let stake = stake.as_ref();
        let active_stake = active_stake.as_ref();

        // Compute normalized emission scores. range: I32F32(0, 1)
        let combined_emission: Vec<I32F32> = incentives
            .as_ref()
            .iter()
            .zip(dividends.as_ref().iter())
            .map(|(ii, di)| ii + di)
            .collect();
        let emission_sum: I32F32 = combined_emission.iter().sum();

        let mut normalized_server_emission = incentives.as_ref().clone(); // Servers get incentive.
        inplace_normalize_using_sum(&mut normalized_server_emission, emission_sum);

        let normalized_validator_emission: Cow<'a, [I32F32]>;
        let normalized_combined_emission: Cow<'a, [I32F32]>;

        // If emission is zero, replace emission with normalized stake.
        if emission_sum == I32F32::from_num(0) {
            // no weights set | outdated weights | self_weights
            if is_zero(active_stake) {
                // no active stake
                // do not mask inactive, assumes stake is normalized
                normalized_validator_emission = Cow::Borrowed(stake);
                normalized_combined_emission = Cow::Borrowed(stake);
            } else {
                // emission proportional to inactive-masked normalized stake
                normalized_validator_emission = Cow::Borrowed(active_stake);
                normalized_combined_emission = Cow::Borrowed(active_stake);
            }
        } else {
            let mut validator_emission = dividends.as_ref().clone(); // Validators get dividends.
            inplace_normalize_using_sum(&mut validator_emission, emission_sum);
            normalized_validator_emission = Cow::Owned(validator_emission);

            let mut combined_emission = combined_emission;
            inplace_normalize(&mut combined_emission);
            normalized_combined_emission = Cow::Owned(combined_emission);
        }

        // Compute rao based emission scores. range: I96F32(0, rao_emission)
        let to_be_emitted = I96F32::from_num(self.to_be_emitted as usize);

        let server_emissions: Vec<u64> = normalized_server_emission
            .iter()
            .map(|&se| I96F32::from_num(se) * to_be_emitted)
            .map(I96F32::to_num)
            .collect();

        let validator_emissions: Vec<u64> = normalized_validator_emission
            .iter()
            .map(|&ve| I96F32::from_num(ve) * to_be_emitted)
            .map(I96F32::to_num)
            .collect();

        // Only used to track emission in storage.
        let combined_emissions: Vec<u64> = normalized_combined_emission
            .iter()
            .map(|&ce| I96F32::from_num(ce) * to_be_emitted)
            .map(I96F32::to_num)
            .collect();

        log::trace!("nSE: {:?}", &normalized_server_emission);
        log::trace!("SE: {:?}", &server_emissions);
        log::trace!("nVE: {:?}", &normalized_validator_emission);
        log::trace!("VE: {:?}", &validator_emissions);
        log::trace!("nCE: {:?}", &normalized_combined_emission);
        log::trace!("CE: {:?}", &combined_emissions);

        // Set pruning scores using combined emission scores.
        let pruning_scores = normalized_combined_emission.into_owned();
        log::trace!("P: {:?}", &pruning_scores);

        Emissions {
            pruning_scores: PruningScoresVal::unchecked_from_inner(pruning_scores),
            validator_emissions,
            server_emissions,
            combined_emissions,
        }
    }
}

bty::brand! {
    pub type ActiveStake = Vec<I32F32>;
    pub type ConsensusVal = Vec<I32F32>;
    pub type DividendsVal = Vec<I32F32>;
    pub type IncentivesVal = Vec<I32F32>;
    pub type Preranks = Vec<I32F32>;
    pub type PruningScoresVal = Vec<I32F32>;
    pub type RanksVal = Vec<I32F32>;
    pub type StakeVal = Vec<I32F32>;
    pub type TrustVal = Vec<I32F32>;
    pub type ValidatorTrustVal = Vec<I32F32>;
    pub type WeightsVal = Vec<Vec<(u16, I32F32)>>;
}

#[derive(Clone, Debug)]
pub struct ModuleKey<T: Config>(T::AccountId);

#[derive(Clone, Debug)]
pub struct AccountKey<T: Config>(T::AccountId);

macro_rules! impl_things {
    ($ty:ident) => {
        impl<T: Config> PartialEq for $ty<T> {
            fn eq(&self, other: &Self) -> bool {
                self.0 == other.0
            }
        }

        impl<T: Config> Eq for $ty<T> {}

        impl<T: Config> PartialOrd for $ty<T> {
            fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
                self.0.partial_cmp(&other.0)
            }
        }

        impl<T: Config> Ord for $ty<T> {
            fn cmp(&self, other: &Self) -> scale_info::prelude::cmp::Ordering {
                self.0.cmp(&other.0)
            }
        }
    };
}

impl_things!(ModuleKey);
impl_things!(AccountKey);

struct ConsensusAndTrust {
    consensus: ConsensusVal,
    validator_trust: ValidatorTrustVal,
    preranks: Preranks,
}

struct BondsAndDividends {
    ema_bonds: Vec<Vec<(u16, I32F32)>>,
    dividends: DividendsVal,
}

struct IncentivesAndTrust {
    incentives: IncentivesVal,
    ranks: RanksVal,
    trust: TrustVal,
}

struct Emissions {
    pruning_scores: PruningScoresVal,
    validator_emissions: Vec<u64>,
    server_emissions: Vec<u64>,
    combined_emissions: Vec<u64>,
}

impl<T: Config> Pallet<T> {
    pub fn get_float_kappa() -> I32F32 {
        I32F32::from_num(Kappa::<T>::get()) / I32F32::from_num(u16::MAX)
    }

    fn get_weights_sparse(netuid: u16) -> Vec<Vec<(u16, I32F32)>> {
        let n = Self::get_subnet_n(netuid) as usize;
        let mut weights: Vec<Vec<(u16, I32F32)>> = vec![vec![]; n];
        for (uid_i, weights_i) in Weights::<T>::iter_prefix(netuid) {
            if uid_i >= n as u16 {
                continue;
            }

            for (uid_j, weight_ij) in weights_i.iter() {
                if *uid_j >= n as u16 {
                    continue;
                }
                weights[uid_i as usize].push((*uid_j, I32F32::from_num(*weight_ij)));
            }
        }
        weights
    }

    fn get_bonds_sparse(netuid: u16) -> Vec<Vec<(u16, I32F32)>> {
        let n: usize = Self::get_subnet_n(netuid) as usize;
        let mut bonds: Vec<Vec<(u16, I32F32)>> = vec![vec![]; n];
        for (uid_i, bonds_i) in Bonds::<T>::iter_prefix(netuid) {
            for (uid_j, bonds_ij) in bonds_i {
                bonds[uid_i as usize].push((uid_j, I32F32::from_num(bonds_ij)));
            }
        }
        bonds
    }
}
