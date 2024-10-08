use crate::{
    subnet_consensus::util::{consensus::*, params},
    Config, EmissionError,
};
use core::marker::PhantomData;
use frame_support::DebugNoBound;
use pallet_subspace::math::*;
use sp_std::{collections::btree_map::BTreeMap, vec, vec::Vec};

#[derive(DebugNoBound)]
pub struct YumaEpoch<T: Config> {
    subnet_id: u16,

    pub(crate) params: params::ConsensusParams<T>,
    modules: params::FlattenedModules<T::AccountId>,

    _pd: PhantomData<T>,
}

impl<T: Config> YumaEpoch<T> {
    pub fn new(subnet_id: u16, mut params: params::ConsensusParams<T>) -> Self {
        let modules = sp_std::mem::take(&mut params.modules).into();

        Self {
            subnet_id,

            params,
            modules,

            _pd: Default::default(),
        }
    }

    fn prepare_weights(
        &self,
        input_weights: Vec<(u16, Vec<(u16, u16)>)>,
    ) -> Vec<(u16, Vec<(u16, u16)>)> {
        let uids: BTreeMap<u16, ()> = self
            .modules
            .keys
            .iter()
            .enumerate()
            .map(|(index, _)| (index as u16, ()))
            .collect();

        // Convert input weights to a BTreeMap for easier manipulation
        let mut weights: BTreeMap<u16, Vec<(u16, u16)>> = input_weights.into_iter().collect();

        // Map over uids, keeping the uid and collecting weights
        uids.keys()
            .map(|&uid| (uid, weights.remove(&uid).unwrap_or_default()))
            .collect()
    }

    pub fn run(
        self,
        input_weights: Vec<(u16, Vec<(u16, u16)>)>,
    ) -> Result<ConsensusOutput<T>, EmissionError> {
        log::debug!(
            "running yuma for subnet_id {}, will emit {:?} modules and {:?} to founder",
            self.subnet_id,
            self.params.token_emission,
            self.params.founder_emission
        );
        log::trace!("yuma for subnet_id {} parameters: {self:?}", self.subnet_id);

        let weights = self.prepare_weights(input_weights);

        let (inactive, active) = split_modules_by_activity(
            &self.modules.last_update,
            &self.modules.block_at_registration,
            self.params.activity_cutoff,
            self.params.current_block,
        );

        let mut weights = compute_weights(&self.modules, &self.params, weights)
            .ok_or(EmissionError::Other("weights are broken"))?;

        let stake = StakeVal::unchecked_from_inner(self.modules.stake_normalized.clone());
        log::trace!("final stake: {stake:?}");

        let new_permits: Vec<bool> = if let Some(max) = self.params.max_allowed_validators {
            is_topk(stake.as_ref(), max as usize)
        } else {
            vec![true; stake.as_ref().len()]
        };
        log::trace!("new permis: {new_permits:?}");

        let mut sorted_indexed_stake: Vec<(u16, u64)> = (0u16..(stake.as_ref().len() as u16))
            .map(|idx| {
                self.weight_counter.read(1);
                let key = match PalletSubspace::<T>::get_key_for_uid(self.netuid, idx) {
                    Some(key) => key,
                    None => return Err(EmissionError::Other("module doesn't have a key")),
                };

                self.weight_counter.read(1);
                let stake = PalletSubspace::<T>::get_delegated_stake(&key);
                Ok((idx, stake))
            })
            .collect::<Result<Vec<_>, EmissionError>>()?;
        sorted_indexed_stake.sort_by_key(|(_, stake)| *stake);
        sorted_indexed_stake.reverse();

        let current_block = PalletSubspace::<T>::get_current_block_number();
        self.weight_counter.read(1);
        let min_stake = pallet_subspace::MinValidatorStake::<T>::get(self.netuid);
        self.weight_counter.read(1);
        let mut validator_count = 0;
        for (idx, stake) in sorted_indexed_stake {
            if max_validators.is_some_and(|max| max <= validator_count) {
                break;
            }

            if stake < min_stake {
                continue;
            }

            self.weight_counter.read(1);
            match pallet_subspace::WeightSetAt::<T>::get(self.netuid, idx) {
                Some(weight_block) => {
                    if current_block.saturating_sub(weight_block) > 7200 {
                        continue;
                    }
                }
                None => continue,
            }

            if let Some(permit) = new_permits.get_mut(idx as usize) {
                validator_count = validator_count.saturating_add(1);
                *permit = true;
            }
        }

        let active_stake = compute_active_stake(&self.modules, &self.params, &inactive, &stake);
        log::trace!("final active stake: {active_stake:?}");

        let ConsensusAndTrust {
            consensus,
            validator_trust,
            preranks,
        } = compute_consensus_and_trust_yuma(
            &self.modules,
            &self.params,
            &mut weights,
            &active_stake,
        );

        let IncentivesAndTrust {
            incentives,
            ranks,
            trust,
        } = compute_incentive_and_trust::<T>(&self.modules, &weights, &active_stake, &preranks);

        let BondsAndDividends {
            ema_bonds,
            dividends,
        } = compute_bonds_and_dividends_yuma(
            &self.params,
            &self.modules,
            &consensus,
            &weights,
            &active_stake,
            &incentives,
        )
        .ok_or(EmissionError::Other("bonds storage is broken"))?;

        process_consensus_output::<T>(
            &self.params,
            &self.modules,
            stake,
            active_stake,
            consensus,
            incentives,
            dividends,
            trust,
            ranks,
            active,
            validator_trust,
            new_permits,
            &ema_bonds,
        )
    }
}
