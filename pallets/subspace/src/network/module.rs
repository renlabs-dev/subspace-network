use crate::*;

use frame_support::pallet_prelude::DispatchResult;
use pallet_subnet_emission_api::SubnetEmissionApi;
pub struct SubnetDistributionParameters;

impl<T: Config> Pallet<T> {
    pub fn do_update_module(
        origin: T::RuntimeOrigin,
        netuid: u16,
        changeset: ModuleChangeset,
    ) -> DispatchResult {
        let key = ensure_signed(origin)?;
        let uid: u16 = Self::get_uid_for_key(netuid, &key).ok_or(Error::<T>::ModuleDoesNotExist)?;
        changeset.apply::<T>(netuid, key, uid)?;
        Ok(())
    }

    pub fn append_module(
        netuid: u16,
        key: &T::AccountId,
        changeset: ModuleChangeset,
    ) -> Result<u16, sp_runtime::DispatchError> {
        // --- Get The Next Uid ---
        let uid: u16 = N::<T>::get(netuid);
        log::debug!("append_module( netuid: {netuid:?} | uid: {key:?} | new_key: {uid:?})");

        // -- Initialize All Storages ---
        StorageHandler::initialize_all::<T>(netuid, uid, &key)?;
        // Make sure this overwrites the defaults (keep it second)
        changeset.apply::<T>(netuid, key.clone(), uid)?;

        // --- Update The Network Module Size ---
        N::<T>::mutate(netuid, |n| *n = n.saturating_add(1));

        // --- Initilaize Stake Storage ---
        Self::increase_stake(key, key, 0);

        Ok(uid)
    }

    /// Replace the module under this uid.
    pub fn remove_module(
        netuid: u16,
        uid: u16,
        deregister_subnet_if_empty: bool,
    ) -> DispatchResult {
        // 1. Check if network has any modules
        let n = N::<T>::get(netuid);
        if n == 0 {
            return Ok(());
        }

        // --- Get the keys for the current and replacement positions ---
        let module_key: T::AccountId =
            Keys::<T>::get(netuid, uid).ok_or(Error::<T>::ModuleDoesNotExist)?;
        let replace_uid = n.saturating_sub(1);
        let replace_key: T::AccountId =
            Keys::<T>::get(netuid, replace_uid).expect("this is infallible");

        log::debug!(
            "remove_module( netuid: {:?} | uid : {:?} | key: {:?} ) ",
            netuid,
            uid,
            module_key
        );

        // --- Remove All Module Related Storage ---
        StorageHandler::remove_all::<T>(netuid, uid, replace_uid, &module_key, &replace_key)?;
        <T as SubnetEmissionApi<T::AccountId>>::clear_module_includes(
            netuid,
            uid,
            replace_uid,
            &module_key,
            &replace_key,
        )?;

        // --- Delete Global-Module Storage ---
        // This will remove storages if the module is only registered on this network.
        // So the values are not "just hanging around" in the storage. Without module actually being
        // registered on any subnet.
        if Uids::<T>::iter().all(|(_, key, _)| key != module_key) {
            DelegationFee::<T>::remove(&module_key);
            Self::remove_stake_from_storage(&module_key);
        }

        // 9. Update network size
        let module_count = N::<T>::mutate(netuid, |v| {
            *v = v.saturating_sub(1);
            *v
        });

        // 10. Handle rootnet deregistration
        if let Some(key) = Self::get_key_for_uid(uid, netuid) {
            Self::handle_rootnet_power_delegation(key, netuid);
        }

        // 11. Remove subnet if empty
        if deregister_subnet_if_empty && module_count == 0 {
            Self::remove_subnet(netuid);
        }

        Ok(())
    }

    fn handle_rootnet_power_delegation(key: T::AccountId, netuid: u16) {
        if Self::is_rootnet(netuid) {
            // Remove the direct delegation for the key
            RootnetControlDelegation::<T>::remove(&key);

            // Remove all delegations to the key
            RootnetControlDelegation::<T>::translate(
                |_, v: T::AccountId| {
                    if v == key {
                        None
                    } else {
                        Some(v)
                    }
                },
            );
        }
    }
}
