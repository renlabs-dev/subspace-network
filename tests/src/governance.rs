// ---------
// Proposal
// ---------
use crate::mock::*;
pub use frame_support::{assert_err, assert_noop, assert_ok};
use pallet_governance::{
    dao::ApplicationStatus, proposal::get_reward_allocation, Curator, CuratorApplications,
    DaoTreasuryAddress, Error, GeneralSubnetApplicationCost, GlobalGovernanceConfig, GovernanceApi,
    ProposalStatus, Proposals, SubnetGovernanceConfig, VoteMode,
};
use pallet_governance_api::GovernanceConfiguration;
use pallet_subspace::{subnet::SubnetChangeset, GlobalParams, SubnetParams};
use sp_core::U256;
use substrate_fixed::{types::extra::U32, FixedI128};

#[test]
fn global_governance_config_validates_parameters_correctly() {
    new_test_ext().execute_with(|| {
        GovernanceMod::validate(GovernanceConfiguration {
            proposal_cost: 0,
            ..Default::default()
        })
        .expect_err("invalid proposal cost was applied");

        GovernanceMod::validate(GovernanceConfiguration {
            proposal_expiration: 0,
            ..Default::default()
        })
        .expect_err("invalid proposal cost was applied");

        GovernanceMod::validate(GovernanceConfiguration {
            proposal_cost: 1,
            proposal_expiration: 1,
            ..Default::default()
        })
        .expect("valid config failed to be applied applied");
    });
}

#[test]
fn global_proposal_validates_parameters() {
    new_test_ext().execute_with(|| {
        const KEY: u32 = 0;
        add_balance(KEY.into(), to_nano(100_000));

        let test = |global_params| {
            let GlobalParams {
                max_name_length,
                min_name_length,
                max_allowed_subnets,
                max_allowed_modules,
                max_registrations_per_block,
                max_allowed_weights,
                floor_delegation_fee,
                floor_founder_share,
                min_weight_stake,
                curator,
                general_subnet_application_cost,
                burn_config,
                governance_config,
                kappa,
                rho,
            } = global_params;

            GovernanceMod::add_global_params_proposal(
                get_origin(KEY.into()),
                vec![b'0'; 64],
                max_name_length,
                min_name_length,
                max_allowed_subnets,
                max_allowed_modules,
                max_registrations_per_block,
                max_allowed_weights,
                burn_config.max_burn,
                burn_config.min_burn,
                floor_delegation_fee,
                floor_founder_share,
                min_weight_stake,
                curator,
                governance_config.proposal_cost,
                governance_config.proposal_expiration,
                general_subnet_application_cost,
                kappa,
                rho,
            )
        };

        test(GlobalParams {
            governance_config: GovernanceConfiguration {
                proposal_cost: 0,
                ..Default::default()
            },
            ..SubspaceMod::global_params()
        })
        .expect_err("created proposal with invalid max name length");

        test(SubspaceMod::global_params())
            .expect("failed to create proposal with valid parameters");
    });
}

#[test]
fn global_custom_proposal_is_accepted_correctly() {
    new_test_ext().execute_with(|| {
        const FOR: u32 = 0;
        const AGAINST: u32 = 1;

        zero_min_burn();
        let key: U256 = 0.into();
        let origin = get_origin(key);

        register(FOR, 0, 0, to_nano(10));
        register(AGAINST, 0, 1, to_nano(5));

        config(1, 100);

        assert_ok!(GovernanceMod::do_add_global_custom_proposal(
            origin,
            vec![b'0'; 64]
        ));

        vote(FOR, 0, true);
        vote(AGAINST, 0, false);

        step_block(100);

        assert_eq!(
            Proposals::<Test>::get(0).unwrap().status,
            ProposalStatus::Accepted {
                block: 100,
                stake_for: 10_000_000_000,
                stake_against: 5_000_000_000,
            }
        );
    });
}

#[test]
fn subnet_custom_proposal_is_accepted_correctly() {
    new_test_ext().execute_with(|| {
        const FOR: u32 = 0;
        const AGAINST: u32 = 1;

        zero_min_burn();
        let origin = get_origin(0.into());

        register(FOR, 0, 0, to_nano(10));
        register(AGAINST, 0, 1, to_nano(5));
        register(AGAINST, 1, 0, to_nano(10));

        config(1, 100);

        assert_ok!(GovernanceMod::do_add_subnet_custom_proposal(
            origin,
            0,
            vec![b'0'; 64]
        ));

        vote(FOR, 0, true);
        vote(AGAINST, 0, false);

        step_block(100);

        assert_eq!(
            Proposals::<Test>::get(0).unwrap().status,
            ProposalStatus::Accepted {
                block: 100,
                stake_for: 10_000_000_000,
                stake_against: 5_000_000_000,
            }
        );
    });
}

#[test]
fn global_proposal_is_refused_correctly() {
    new_test_ext().execute_with(|| {
        const FOR: u32 = 0;
        const AGAINST: u32 = 1;

        zero_min_burn();
        let origin = get_origin(0.into());

        register(FOR, 0, 0, to_nano(5));
        register(AGAINST, 0, 1, to_nano(10));

        config(1, 100);

        assert_ok!(GovernanceMod::do_add_global_custom_proposal(
            origin,
            vec![b'0'; 64]
        ));

        vote(FOR, 0, true);
        vote(AGAINST, 0, false);

        step_block(100);

        assert_eq!(
            Proposals::<Test>::get(0).unwrap().status,
            ProposalStatus::Refused {
                block: 100,
                stake_for: 5_000_000_000,
                stake_against: 10_000_000_000,
            }
        );
    });
}

#[test]
fn global_params_proposal_accepted() {
    new_test_ext().execute_with(|| {
        const KEY: u32 = 0;
        zero_min_burn();

        register(KEY, 0, 0, to_nano(10));
        config(1, 100);

        let GlobalParams {
            max_name_length,
            min_name_length,
            max_allowed_subnets,
            max_allowed_modules,
            max_registrations_per_block,
            max_allowed_weights,
            floor_delegation_fee,
            floor_founder_share,
            min_weight_stake,
            curator,
            general_subnet_application_cost,
            burn_config,
            mut governance_config,
            rho,
            kappa,
        } = SubspaceMod::global_params();

        governance_config.proposal_cost = 69_420;

        GovernanceMod::add_global_params_proposal(
            get_origin(KEY.into()),
            vec![b'0'; 64],
            max_name_length,
            min_name_length,
            max_allowed_subnets,
            max_allowed_modules,
            max_registrations_per_block,
            max_allowed_weights,
            burn_config.max_burn,
            100_000_000,
            floor_delegation_fee,
            floor_founder_share,
            min_weight_stake,
            curator,
            governance_config.proposal_cost,
            governance_config.proposal_expiration,
            general_subnet_application_cost,
            kappa,
            rho,
        )
        .unwrap();

        vote(KEY, 0, true);
        step_block(100);

        assert_eq!(GlobalGovernanceConfig::<Test>::get().proposal_cost, 69_420);
    });
}

#[test]
fn subnet_params_proposal_accepted() {
    new_test_ext().execute_with(|| {
        const KEY: u32 = 0;
        zero_min_burn();

        register(KEY, 0, 0, to_nano(10));
        config(1, 100);

        SubnetChangeset::update(
            0,
            SubnetParams {
                governance_config: Default::default(),
                ..SubspaceMod::subnet_params(0)
            },
        )
        .unwrap()
        .apply(0)
        .unwrap();

        let SubnetParams {
            founder,
            founder_share,
            immunity_period,
            incentive_ratio,
            max_allowed_uids,
            max_allowed_weights,
            min_allowed_weights,
            max_weight_age,
            min_stake,
            name,
            tempo,
            trust_ratio,
            maximum_set_weight_calls_per_epoch,
            bonds_ma,
            target_registrations_interval,
            target_registrations_per_interval,
            max_registrations_per_interval,
            adjustment_alpha,
            mut governance_config,
        } = SubspaceMod::subnet_params(0);

        governance_config.vote_mode = VoteMode::Authority;

        GovernanceMod::add_subnet_params_proposal(
            get_origin(KEY.into()),
            0,
            vec![b'0'; 64],
            founder,
            name,
            founder_share,
            immunity_period,
            incentive_ratio,
            max_allowed_uids,
            max_allowed_weights,
            min_allowed_weights,
            min_stake,
            max_weight_age,
            tempo,
            trust_ratio,
            maximum_set_weight_calls_per_epoch,
            governance_config.vote_mode,
            bonds_ma,
            target_registrations_interval,
            target_registrations_per_interval,
            max_registrations_per_interval,
            adjustment_alpha,
        )
        .unwrap();

        vote(KEY, 0, true);
        step_block(100);

        assert_eq!(
            SubnetGovernanceConfig::<Test>::get(0).vote_mode,
            VoteMode::Authority
        );
    });
}

#[test]
fn global_proposals_counts_delegated_stake() {
    new_test_ext().execute_with(|| {
        const FOR: u32 = 0;
        const AGAINST: u32 = 1;
        const FOR_DELEGATED: u32 = 2;
        const AGAINST_DELEGATED: u32 = 3;

        zero_min_burn();
        let origin = get_origin(0.into());

        register(FOR, 0, 0, to_nano(5));
        delegate(FOR);
        register(AGAINST, 0, 1, to_nano(10));

        stake(FOR_DELEGATED, 0, to_nano(10));
        delegate(FOR_DELEGATED);
        stake(AGAINST_DELEGATED, 1, to_nano(3));
        delegate(AGAINST_DELEGATED);

        config(1, 100);

        assert_ok!(GovernanceMod::do_add_global_custom_proposal(
            origin,
            vec![b'0'; 64]
        ));

        vote(FOR, 0, true);
        vote(AGAINST, 0, false);

        step_block(100);

        assert_eq!(
            Proposals::<Test>::get(0).unwrap().status,
            ProposalStatus::Accepted {
                block: 100,
                stake_for: 15_000_000_000,
                stake_against: 13_000_000_000,
            }
        );
    });
}

#[test]
fn subnet_proposals_counts_delegated_stake() {
    new_test_ext().execute_with(|| {
        const FOR: u32 = 0;
        const AGAINST: u32 = 1;
        const FOR_DELEGATED: u32 = 2;
        const AGAINST_DELEGATED: u32 = 3;
        const FOR_DELEGATED_WRONG: u32 = 4;
        const AGAINST_DELEGATED_WRONG: u32 = 5;

        zero_min_burn();
        let origin = get_origin(0.into());

        register(FOR, 0, 0, to_nano(5));
        register(FOR, 1, 0, to_nano(5));
        register(AGAINST, 0, 1, to_nano(10));
        register(AGAINST, 1, 1, to_nano(10));

        stake(FOR_DELEGATED, 0, to_nano(10));
        delegate(FOR_DELEGATED);
        stake(AGAINST_DELEGATED, 1, to_nano(3));
        delegate(AGAINST_DELEGATED);

        stake(FOR_DELEGATED_WRONG, 0, to_nano(10));
        delegate(FOR_DELEGATED_WRONG);
        stake(AGAINST_DELEGATED_WRONG, 1, to_nano(3));
        delegate(AGAINST_DELEGATED_WRONG);

        config(1, 100);

        assert_ok!(GovernanceMod::do_add_subnet_custom_proposal(
            origin,
            0,
            vec![b'0'; 64]
        ));

        vote(FOR, 0, true);
        vote(AGAINST, 0, false);

        step_block(100);

        assert_eq!(
            Proposals::<Test>::get(0).unwrap().status,
            ProposalStatus::Accepted {
                block: 100,
                stake_for: 15_000_000_000,
                stake_against: 13_000_000_000,
            }
        );
    });
}

#[test]
fn creates_treasury_transfer_proposal_and_transfers() {
    new_test_ext().execute_with(|| {
        zero_min_burn();

        let origin = get_origin(0.into());
        GovernanceMod::add_transfer_dao_treasury_proposal(
            origin.clone(),
            vec![b'0'; 64],
            to_nano(5),
            0.into(),
        )
        .expect_err("proposal should not be created when treasury does not have enough money");

        add_balance(DaoTreasuryAddress::<Test>::get(), to_nano(10));
        add_balance(0.into(), to_nano(3));
        register(0, 0, 0, to_nano(1));
        config(to_nano(1), 100);

        GovernanceMod::add_transfer_dao_treasury_proposal(
            origin,
            vec![b'0'; 64],
            to_nano(5),
            0.into(),
        )
        .expect("proposal should be created");
        vote(0, 0, true);

        step_block(100);

        assert_eq!(get_balance(DaoTreasuryAddress::<Test>::get()), to_nano(5));
        assert_eq!(get_balance(0.into()), to_nano(7));
    });
}

/// This test, observes the distribution of governance reward logic over time.
#[test]
fn rewards_wont_exceed_treasury() {
    new_test_ext().execute_with(|| {
        zero_min_burn();
        // Fill the governance address with 1 mil so we are not limited by the max allocation
        let amount = to_nano(1_000_000_000);
        let key = DaoTreasuryAddress::<Test>::get();
        add_balance(key, amount);

        let governance_config: GovernanceConfiguration = GlobalGovernanceConfig::<Test>::get();
        let n = 0;
        let allocation = get_reward_allocation::<Test>(&governance_config, n).unwrap();
        assert_eq!(
            FixedI128::<U32>::saturating_from_num(allocation),
            governance_config.max_proposal_reward_treasury_allocation
        );
    });
}

#[test]
fn test_whitelist() {
    new_test_ext().execute_with(|| {
        let key = 0;
        let adding_key = 1;
        let mut params = SubspaceMod::global_params();
        params.curator = key.into();
        assert_ok!(SubspaceMod::set_global_params(params));

        let proposal_cost = GeneralSubnetApplicationCost::<Test>::get();
        let data = "test".as_bytes().to_vec();

        add_balance(key.into(), proposal_cost + 1);
        // first submit an application
        let balance_before = SubspaceMod::get_balance_u64(&key.into());

        assert_ok!(GovernanceMod::add_dao_application(
            get_origin(key.into()),
            adding_key.into(),
            data.clone(),
        ));

        let balance_after = SubspaceMod::get_balance_u64(&key.into());
        assert_eq!(balance_after, balance_before - proposal_cost);

        // Assert that the proposal is initially in the Pending status
        for (_, value) in CuratorApplications::<Test>::iter() {
            assert_eq!(value.status, ApplicationStatus::Pending);
            assert_eq!(value.user_id, adding_key.into());
            assert_eq!(value.data, data);
        }

        // add key to whitelist
        assert_ok!(GovernanceMod::add_to_whitelist(
            get_origin(key.into()),
            adding_key.into(),
            1,
        ));

        let balance_after_accept = SubspaceMod::get_balance_u64(&key.into());

        assert_eq!(balance_after_accept, balance_before);

        // Assert that the proposal is now in the Accepted status
        for (_, value) in CuratorApplications::<Test>::iter() {
            assert_eq!(value.status, ApplicationStatus::Accepted);
            assert_eq!(value.user_id, adding_key.into());
            assert_eq!(value.data, data);
        }

        assert!(GovernanceMod::is_in_legit_whitelist(&adding_key.into()));
    });
}

// ----------------
// Registration
// ----------------

#[test]
fn test_remove_from_whitelist() {
    new_test_ext().execute_with(|| {
        let whitelist_key = U256::from(0);
        let module_key = U256::from(1);
        Curator::<Test>::put(whitelist_key);

        let proposal_cost = Test::get_global_governance_configuration().proposal_cost;
        let data = "test".as_bytes().to_vec();

        // apply
        add_balance(whitelist_key, proposal_cost + 1);
        // first submit an application
        assert_ok!(GovernanceMod::add_dao_application(
            get_origin(whitelist_key),
            module_key,
            data.clone(),
        ));

        // Add the module_key to the whitelist
        assert_ok!(GovernanceMod::add_to_whitelist(
            get_origin(whitelist_key),
            module_key,
            1
        ));
        assert!(GovernanceMod::is_in_legit_whitelist(&module_key));

        // Remove the module_key from the whitelist
        assert_ok!(GovernanceMod::remove_from_whitelist(
            get_origin(whitelist_key),
            module_key
        ));
        assert!(!GovernanceMod::is_in_legit_whitelist(&module_key));
    });
}

#[test]
fn test_invalid_curator() {
    new_test_ext().execute_with(|| {
        let whitelist_key = U256::from(0);
        let invalid_key = U256::from(1);
        let module_key = U256::from(2);
        Curator::<Test>::put(whitelist_key);

        // Try to add to whitelist with an invalid curator key
        assert_noop!(
            GovernanceMod::add_to_whitelist(get_origin(invalid_key), module_key, 1),
            Error::<Test>::NotCurator
        );
        assert!(!GovernanceMod::is_in_legit_whitelist(&module_key));
    });
}