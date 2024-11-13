use crate::*;
use frame_support::{
    pallet_prelude::Weight,
    traits::{Get, OnRuntimeUpgrade, StorageVersion},
};

// pub mod v1 {
//     use super::*;

//     pub struct MigrateToV8<T>(sp_std::marker::PhantomData<T>);

//     impl<T: Config> OnRuntimeUpgrade for MigrateToV8<T> {
//         fn on_runtime_upgrade() -> frame_support::weights::Weight {
//             let on_chain_version = StorageVersion::get::<Pallet<T>>();
//             if on_chain_version != 0 {
//                 log::info!("Storage v1 already updated");
//                 return Weight::zero();
//             }

//             StorageVersion::new(1).put::<Pallet<T>>();

//             log::info!("Migrated to v1");

//             T::DbWeight::get().reads_writes(2, 2)
//         }
//     }
// }

pub mod v8 {
    use super::*;

    pub struct MigrateToV8<T>(sp_std::marker::PhantomData<T>);

    impl<T: Config> OnRuntimeUpgrade for MigrateToV8<T> {
        fn on_runtime_upgrade() -> frame_support::weights::Weight {
            let on_chain_version = StorageVersion::get::<Pallet<T>>();
            if on_chain_version != 11 {
                log::info!("Storage v4 already updated");
                return Weight::zero();
            }

            StorageVersion::new(12).put::<Pallet<T>>();

            let _ = ConsensusParameters::<T>::clear(u32::MAX, None);
            let _ = SubnetDecryptionData::<T>::clear(u32::MAX, None);
            let _ = ConsensusParameters::<T>::clear(u32::MAX, None);
            let _ = WeightEncryptionData::<T>::clear(u32::MAX, None);
            let _ = DecryptedWeights::<T>::clear(u32::MAX, None);
            let _ = BannedDecryptionNodes::<T>::clear(u32::MAX, None);
            let _ = DecryptionNodes::<T>::kill();

            log::info!("Migrated to v2");

            T::DbWeight::get().reads_writes(2, 2)
        }
    }
}
