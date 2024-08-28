use core::fmt::Debug;

use sp_std::collections::btree_map::BTreeMap;

use frame_support::DebugNoBound;
use pallet_subspace::{
    math::*, BalanceOf, Bonds, BondsMovingAverage, Config, Founder, Kappa, Keys, LastUpdate,
    MaxAllowedValidators, MaxWeightAge, Pallet as PalletSubspace, ValidatorPermits, Vec, Weights,
};
use substrate_fixed::types::{I32F32, I64F64};

#[derive(Clone)]
pub struct AccountKey<AccountId>(pub AccountId);

#[derive(Clone)]
pub struct ModuleKey<AccountId>(pub AccountId);

#[derive(DebugNoBound)]
pub struct YumaParams<T: Config> {
    pub subnet_id: u16,
    pub token_emission: BalanceOf<T>,

    pub modules: BTreeMap<ModuleKey<T::AccountId>, ModuleParams>,
    pub kappa: I32F32,

    pub founder_key: AccountKey<T::AccountId>,
    pub founder_emission: BalanceOf<T>,

    pub current_block: u64,
    pub activity_cutoff: u64,
    pub max_allowed_validators: Option<u16>,
    pub bonds_moving_average: u64,
}

#[derive(DebugNoBound)]
pub struct ModuleParams {
    pub uid: u16,
    pub last_update: u64,
    pub block_at_registration: u64,
    pub validator_permit: bool,
    pub stake: I32F32,
    pub bonds: Vec<(u16, u16)>,
    pub weight_unencrypted_hash: Vec<u8>,
    pub weight_encrypted: Vec<u8>,
    pub weight_unencrypted: Vec<(u16, u16)>,
}

#[derive(DebugNoBound)]
pub(super) struct FlattenedModules<AccountId: Debug> {
    pub keys: Vec<ModuleKey<AccountId>>,
    pub last_update: Vec<u64>,
    pub block_at_registration: Vec<u64>,
    pub validator_permit: Vec<bool>,
    pub validator_forbid: Vec<bool>,
    pub stake: Vec<I32F32>,
    pub bonds: Vec<Vec<(u16, I32F32)>>,
    pub weight_unencrypted_hash: Vec<Vec<u8>>,
    pub weight_encrypted: Vec<Vec<u8>>,
    pub weight_unencrypted: Vec<Vec<(u16, I32F32)>>,
}

impl<AccountId: Debug> From<BTreeMap<ModuleKey<AccountId>, ModuleParams>>
    for FlattenedModules<AccountId>
{
    fn from(value: BTreeMap<ModuleKey<AccountId>, ModuleParams>) -> Self {
        let mut modules = FlattenedModules {
            keys: Vec::with_capacity(value.len()),
            last_update: Vec::with_capacity(value.len()),
            block_at_registration: Vec::with_capacity(value.len()),
            validator_permit: Vec::with_capacity(value.len()),
            validator_forbid: Vec::with_capacity(value.len()),
            stake: Vec::with_capacity(value.len()),
            bonds: Vec::with_capacity(value.len()),
            weight_unencrypted_hash: Vec::with_capacity(value.len()),
            weight_encrypted: Vec::with_capacity(value.len()),
            weight_unencrypted: Vec::with_capacity(value.len()),
        };

        for (key, module) in value {
            modules.keys.push(key);
            modules.last_update.push(module.last_update);
            modules.block_at_registration.push(module.block_at_registration);
            modules.validator_permit.push(module.validator_permit);
            modules.validator_forbid.push(!module.validator_permit);
            modules.stake.push(module.stake);
            modules
                .bonds
                .push(module.bonds.into_iter().map(|(k, m)| (k, I32F32::from_num(m))).collect());
            modules.weight_unencrypted_hash.push(module.weight_unencrypted_hash);
            modules.weight_encrypted.push(module.weight_encrypted);
            modules.weight_unencrypted.push(
                module
                    .weight_unencrypted
                    .into_iter()
                    .map(|(k, m)| (k, I32F32::from_num(m)))
                    .collect(),
            );
        }

        modules
    }
}

impl<T: Config> YumaParams<T> {
    pub fn new(subnet_id: u16, token_emission: u64) -> Result<Self, &'static str> {
        let uids: BTreeMap<_, _> = Keys::<T>::iter_prefix(subnet_id).collect();

        let stake = Self::compute_stake(&uids);
        let bonds = Self::compute_bonds(subnet_id, &uids);
        let weights = Self::compute_weights(subnet_id, &uids);

        let last_update = LastUpdate::<T>::get(subnet_id);
        let block_at_registration = PalletSubspace::<T>::get_block_at_registration(subnet_id);
        let validator_permits = ValidatorPermits::<T>::get(subnet_id);

        let modules = uids
            .into_iter()
            .zip(stake)
            .zip(bonds)
            .zip(weights)
            .map(|((((uid, key), stake), bonds), weights)| {
                let uid = uid as usize;
                let last_update =
                    last_update.get(uid).copied().ok_or("LastUpdate storage is broken")?;
                let block_at_registration = block_at_registration
                    .get(uid)
                    .copied()
                    .ok_or("RegistrationBlock storage is broken")?;
                let validator_permit = validator_permits
                    .get(uid)
                    .copied()
                    .ok_or("ValidatorPermits storage is broken")?;

                let module = ModuleParams {
                    uid: uid as u16,
                    last_update,
                    block_at_registration,
                    validator_permit,
                    stake,
                    bonds,
                    // TODO: implement weights
                    weight_unencrypted_hash: Default::default(),
                    // TODO: implement weights
                    weight_encrypted: Default::default(),
                    // TODO: remove once we encrypt weights
                    weight_unencrypted: weights,
                };

                Result::<_, &'static str>::Ok((ModuleKey(key), module))
            })
            .collect::<Result<_, _>>()?;

        let founder_key = AccountKey(Founder::<T>::get(subnet_id));
        let (token_emission, founder_emission) =
            PalletSubspace::<T>::calculate_founder_emission(subnet_id, token_emission);

        let token_emission = token_emission.try_into().ok().unwrap_or_default();
        let founder_emission = founder_emission.try_into().ok().unwrap_or_default();

        Ok(Self {
            subnet_id,
            token_emission,

            modules,
            kappa: I32F32::from_num(Kappa::<T>::get())
                .checked_div(I32F32::from_num(u16::MAX))
                .unwrap_or_default(),

            founder_key,
            founder_emission,

            current_block: PalletSubspace::<T>::get_current_block_number(),
            activity_cutoff: MaxWeightAge::<T>::get(subnet_id),
            max_allowed_validators: MaxAllowedValidators::<T>::get(subnet_id),
            bonds_moving_average: BondsMovingAverage::<T>::get(subnet_id),
        })
    }

    fn compute_stake(uids: &BTreeMap<u16, T::AccountId>) -> Vec<I32F32> {
        // BTreeMap provides natural order, so iterating and collecting
        // will result in a vector with the same order as the uid map.
        let mut stake: Vec<_> = uids
            .values()
            .map(PalletSubspace::<T>::get_delegated_stake)
            .map(I64F64::from_num)
            .collect();
        log::trace!(target: "stake", "original: {stake:?}");

        inplace_normalize_64(&mut stake);
        log::trace!(target: "stake", "normalized: {stake:?}");

        vec_fixed64_to_fixed32(stake)
    }

    fn compute_bonds(subnet_id: u16, uids: &BTreeMap<u16, T::AccountId>) -> Vec<Vec<(u16, u16)>> {
        let mut bonds: BTreeMap<_, _> = Bonds::<T>::iter_prefix(subnet_id).collect();
        // BTreeMap provides natural order, so iterating and collecting
        // will result in a vector with the same order as the uid map.
        uids.keys().map(|uid| bonds.remove(uid).unwrap_or_default()).collect()
    }

    fn compute_weights(subnet_id: u16, uids: &BTreeMap<u16, T::AccountId>) -> Vec<Vec<(u16, u16)>> {
        let mut weights: BTreeMap<_, _> = Weights::<T>::iter_prefix(subnet_id).collect();
        // BTreeMap provides natural order, so iterating and collecting
        // will result in a vector with the same order as the uid map.
        uids.keys().map(|uid| weights.remove(uid).unwrap_or_default()).collect()
    }
}

macro_rules! impl_things {
    ($ty:ident) => {
        impl<T: PartialEq> PartialEq for $ty<T> {
            fn eq(&self, other: &Self) -> bool {
                self.0 == other.0
            }
        }

        impl<T: Eq> Eq for $ty<T> {}

        impl<T: PartialOrd + Ord> PartialOrd for $ty<T> {
            fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }

        impl<T: Ord> Ord for $ty<T> {
            fn cmp(&self, other: &Self) -> scale_info::prelude::cmp::Ordering {
                self.0.cmp(&other.0)
            }
        }

        impl<T: core::fmt::Debug> core::fmt::Debug for $ty<T> {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_fmt(format_args!("{}({:?})", stringify!($ty), self.0))
            }
        }

        impl<T: Default> Default for $ty<T> {
            fn default() -> Self {
                Self(T::default())
            }
        }
    };
}

impl_things!(AccountKey);
impl_things!(ModuleKey);
