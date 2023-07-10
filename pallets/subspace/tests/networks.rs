mod mock;
use mock::*;
use pallet_subspace::{Error};
use frame_support::weights::{GetDispatchInfo, DispatchInfo, DispatchClass, Pays};
use frame_system::Config;
use frame_support::{sp_std::vec};
use frame_support::{assert_ok};
use sp_core::U256;

/*TO DO SAM: write test for LatuUpdate after it is set */


#[test]
fn test_add_network() { 
        new_test_ext().execute_with(|| {
        let modality = 0;
        let tempo: u16 = 13;
        add_network(0, U256::from(0));
        assert_eq!(SubspaceModule::get_number_of_subnets(), 1);
        add_network( 1, U256::from(0));
        assert_eq!(SubspaceModule::get_number_of_subnets(), 2); 
});}


#[test]
fn test_add_many_subnets() { 
        new_test_ext().execute_with(|| {
        for i in 0..100 {
            add_network(i, U256::from(0));
            assert_eq!(SubspaceModule::get_number_of_subnets(), i+1);
        }
});}



#[test]
fn test_set_max_allowed_uids() { 
        new_test_ext().execute_with(|| {
        let netuid = 0;
        let stake = 1_000_000_000;
        let max_uids = 100;
        SubspaceModule::set_max_allowed_uids(netuid, max_uids);
        for i in 0..max_uids {
            register_module(netuid, U256::from(0), stake);
            assert_eq!(SubspaceModule::get_subnet_n(netuid), i+1);
        }
        assert_eq!(SubspaceModule::get_subnet_n(netuid), max_uids);

        for i in max_uids..max_uids+10 {
            register_module(netuid, U256::from(0), stake);
            assert_eq!(SubspaceModule::get_subnet_n(netuid), max_uids);
        }

        assert_eq!(SubspaceModule::get_subnet_n(netuid), max_uids);
});}





