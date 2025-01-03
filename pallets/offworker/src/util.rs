use super::*;
use crate::profitability::{get_copier_stake, is_copying_irrational};
#[allow(unused_imports)]
use num_traits::float::Float;
use types::SimulationYumaParams;

pub fn process_consensus_params<T>(
    subnet_id: u16,
    acc_id: T::AccountId,
    consensus_params: Vec<(u64, ConsensusParams<T>)>,
    simulation_result: ConsensusSimulationResult<T>,
) -> (bool, ShouldDecryptResult<T>, bool)
where
    T: pallet_subspace::Config + pallet_subnet_emission::Config + pallet::Config,
{
    let mut result = ShouldDecryptResult::<T> {
        should_decrypt: false,
        simulation_result: simulation_result.clone(),
        delta: I64F64::from_num(0),
    };

    let mut final_should_send = false;
    let mut forced_send = false;

    log::info!("Processing consensus params for subnet {}", subnet_id);
    log::info!(
        "Number of consensus params entries: {}",
        consensus_params.len()
    );

    for (param_block, params) in &consensus_params {
        log::info!("Processing block {}", param_block);
        log::info!("Number of modules in params: {}", params.modules.len());

        let decrypted_weights: Vec<_> = params
            .modules
            .iter()
            .inspect(|(key, params)| {
                log::debug!(
                    "Processing module key: {:?}, encrypted weight length: {}",
                    key.0,
                    params.weight_encrypted.len()
                );
            })
            .filter_map(|(key, params)| {
                let found_param = consensus_params.iter().find_map(|(_, consensus_param)| {
                    consensus_param
                        .modules
                        .iter()
                        .find(|(module_key, _)| module_key.0 == key.0)
                        .map(|(_, module_params)| (module_params.uid, params))
                });

                if found_param.is_none() {
                    log::warn!(
                        "No matching consensus param found for module key: {:?}",
                        key.0
                    );
                }
                found_param
            })
            .filter_map(|(uid, params)| {
                log::debug!(
                    "Attempting decryption for UID: {}, encrypted weight length: {}",
                    uid,
                    params.weight_encrypted.len()
                );

                if params.weight_encrypted.is_empty() {
                    log::info!("Empty encrypted weights for UID: {}", uid);
                    Some((uid, Vec::new(), Vec::new()))
                } else {
                    log::info!("encrypted weights are: {:?}", params.weight_encrypted);
                    match ow_extensions::offworker::decrypt_weight(params.weight_encrypted.clone())
                    {
                        Some((decrypted, key)) => {
                            log::info!(
                                "Successfully decrypted weights for UID: {}, decrypted length: {}",
                                uid,
                                decrypted.len()
                            );
                            Some((uid, decrypted, key))
                        }
                        None => {
                            // V2 TODO:
                            log::warn!(
                                "Failed to decrypt weights for UID: {} on subnet {}",
                                uid,
                                subnet_id
                            );
                            None
                            // if this ever happens, we need to make a zk proof of encryption
                            // correctness (that it was not done correctly)
                            // and then send this proof back to the runtime together with the
                            // decrypted weights
                        }
                    }
                }
            })
            .collect();

        log::info!(
            "Number of successfully decrypted weights: {}",
            decrypted_weights.len()
        );

        let weights_for_should_decrypt: Vec<_> = decrypted_weights
            .iter()
            .cloned()
            .map(|(uid, weights, _)| (uid, weights))
            .collect();

        log::debug!(
            "Preparing should_decrypt check with {} weight entries",
            weights_for_should_decrypt.len()
        );

        let mut should_decrypt_result = should_decrypt_weights::<T>(
            &weights_for_should_decrypt,
            params.clone(),
            subnet_id,
            simulation_result.clone(),
            *param_block,
        );

        log::info!(
            "should_decrypt result: {}, delta: {}",
            should_decrypt_result.should_decrypt,
            should_decrypt_result.delta
        );

        let current_should_send = if !should_decrypt_result.should_decrypt {
            let rotation_should_send =
                pallet_subnet_emission::Pallet::<T>::should_send_rotation_weights(
                    subnet_id, &acc_id,
                );
            log::info!("rotation_should_send: {}", rotation_should_send);
            if rotation_should_send {
                forced_send = true;
                should_decrypt_result.delta = I64F64::from_num(0);
            }
            rotation_should_send
        } else {
            true
        };

        if current_should_send {
            log::info!(
                "Adding decrypted weights for block {} to epochs",
                param_block
            );
            result = should_decrypt_result;
        }

        final_should_send = current_should_send;
    }
    log::info!("should_decrypt: {}", result.should_decrypt);
    (final_should_send, result, forced_send)
}

/// Returns
#[must_use]
pub fn should_decrypt_weights<T: Config>(
    decrypted_weights: &[(u16, Vec<(u16, u16)>)],
    latest_runtime_yuma_params: ConsensusParams<T>,
    subnet_id: u16,
    mut simulation_result: ConsensusSimulationResult<T>,
    block_number: u64,
) -> ShouldDecryptResult<T> {
    let simulation_params = match compute_simulation_yuma_params::<T>(
        decrypted_weights,
        latest_runtime_yuma_params,
        subnet_id,
    ) {
        Some(params) => params,
        None => {
            return ShouldDecryptResult {
                should_decrypt: true,
                ..Default::default()
            }
        }
    };

    let SimulationYumaParams {
        uid: copier_uid,
        params: simulation_yuma_params,
        decrypted_weights_map,
    } = simulation_params;

    let decrypted_weights = decrypted_weights_map.into_iter().collect::<Vec<_>>();
    if decrypted_weights.is_empty() {
        log::info!("subnet {subnet_id} does not have any decrypted weights");
        return ShouldDecryptResult::<T>::default();
    }

    log::info!("simulation yuma params for subnet {subnet_id} are {simulation_yuma_params:?}");

    // Run consensus simulation with error handling
    let simulation_yuma_output =
        match YumaEpoch::<T>::new(subnet_id, simulation_yuma_params).run(decrypted_weights) {
            Ok(output) => {
                // Save the copier bonds for future use
                save_copier_bonds::<T>(subnet_id, &output);
                output
            }
            Err(e) => {
                log::error!("Failed to run consensus simulation: {:?}", e);
                return ShouldDecryptResult::default();
            }
        };

    // Get delegation fee (this is not a Result type)
    let delegation_fee = MinFees::<T>::get().stake_delegation_fee;

    log::info!("simulation yuma output for subnet {subnet_id} is {simulation_yuma_output:?}");

    // Update simulation result
    simulation_result.update(simulation_yuma_output, copier_uid, delegation_fee);

    // Check if copying is irrational (this returns a tuple, not a Result)
    let (is_irrational, delta) =
        is_copying_irrational::<T>(simulation_result.clone(), block_number);

    // Log results
    if is_irrational {
        log::info!("Copying is leftist");
    } else {
        log::info!("Copying is right-wing");
    }
    log::info!("Delta: {}", delta);

    ShouldDecryptResult {
        should_decrypt: is_irrational,
        delta: delta.abs(),
        simulation_result,
    }
}

/// Appends copier information to simulated consensus ConsensusParams
/// Overwrites onchain decrypted weights with the offchain workers' decrypted weights
pub fn compute_simulation_yuma_params<T: Config>(
    decrypted_weights: &[(u16, Vec<(u16, u16)>)],
    mut runtime_yuma_params: ConsensusParams<T>,
    subnet_id: u16,
) -> Option<SimulationYumaParams<T>> {
    let copier_uid: u16 = runtime_yuma_params.modules.len() as u16;

    // This **has** to be obtained from the runtime storage. So it sees the publicly known consensus
    // on decrypted weights
    let consensus_weights = Consensus::<T>::get(subnet_id);

    // If the consensus is empty or just all zeros, return None
    if consensus_weights.iter().all(|x| *x == 0) || consensus_weights.is_empty() {
        log::warn!("Consensus is empty for subnet {}", subnet_id);
        return None;
    }

    let copier_weights: Vec<(u16, u16)> = consensus_weights
        .into_iter()
        .enumerate()
        .map(|(index, value)| (index as u16, value))
        .collect();

    log::info!(
        "copier weights on subnet {} are: {:?}",
        subnet_id,
        copier_weights
    );

    runtime_yuma_params = add_copier_to_yuma_params(copier_uid, runtime_yuma_params, subnet_id);

    let mut onchain_weights: BTreeMap<u16, Vec<(u16, u16)>> =
        Weights::<T>::iter_prefix(subnet_id).collect();

    onchain_weights.extend(
        decrypted_weights
            .iter()
            .cloned()
            .chain(sp_std::iter::once((copier_uid, copier_weights))),
    );

    log::info!(
        "final weights for subnet {} are: {:?}",
        subnet_id,
        onchain_weights
    );

    Some(SimulationYumaParams {
        uid: copier_uid,
        params: runtime_yuma_params,
        decrypted_weights_map: onchain_weights,
    })
}

/// This will mutate ConsensusParams with copier information, ready for simulation
/// This function should run
pub fn add_copier_to_yuma_params<T: Config>(
    copier_uid: u16,
    mut runtime_yuma_params: ConsensusParams<T>,
    subnet_id: u16,
) -> ConsensusParams<T> {
    let copier_bonds = get_copier_bonds::<T>(subnet_id, &runtime_yuma_params);

    // Calculate total active stake
    let total_active_stake: u64 = runtime_yuma_params
        .modules
        .values()
        .filter(|m| m.validator_permit)
        .map(|m| m.stake_original.to_num::<u64>())
        .sum();

    let copier_stake = get_copier_stake::<T>(total_active_stake);
    let current_block = runtime_yuma_params.current_block;

    // Collect original stakes, including the copier's stake
    let mut all_stakes: Vec<I64F64> = runtime_yuma_params
        .modules
        .values()
        .map(|m| m.stake_original)
        .chain(sp_std::iter::once(I64F64::from_num(copier_stake)))
        .collect();

    // Normalize all stakes using the provided function
    inplace_normalize_64(&mut all_stakes);

    // Convert normalized I64F64 stakes to I32F32
    let normalized_stakes = vec_fixed64_to_fixed32(all_stakes.clone());

    let copier_module = ModuleParams::<T::AccountId> {
        uid: copier_uid,
        last_update: current_block,
        block_at_registration: current_block.saturating_sub(1),
        validator_permit: true,
        stake_normalized: *normalized_stakes.last().unwrap_or(&I32F32::from_num(0)),
        stake_original: I64F64::from_num(copier_stake),
        delegated_to: None,
        bonds: copier_bonds,
        weight_encrypted: Vec::new(),
        weight_hash: Vec::new(),
    };

    let seed = (b"copier", subnet_id, copier_uid).using_encoded(BlakeTwo256::hash);
    let copier_account_id = T::AccountId::decode(&mut seed.as_ref())
        .expect("32 bytes should be sufficient for any AccountId");

    let copier_key = ModuleKey(copier_account_id);

    // Update existing modules with new normalized stakes
    runtime_yuma_params.modules.values_mut().zip(normalized_stakes.iter()).for_each(
        |(module, &normalized)| {
            module.stake_normalized = normalized;
        },
    );

    // Insert the copier module
    runtime_yuma_params.modules.insert(copier_key, copier_module);

    // Set the bonds moving average to 0
    // runtime_yuma_params.bonds_moving_average = 0;

    runtime_yuma_params
}

fn get_copier_bonds<T: Config>(
    subnet_id: u16,
    consensus_params: &ConsensusParams<T>,
) -> Vec<(u16, u16)> {
    let storage_key = alloc::format!("copier_bonds:{subnet_id}");
    let storage = StorageValueRef::persistent(storage_key.as_bytes());

    // Try to get bonds from storage
    match storage.get::<Vec<(u16, u16)>>() {
        Ok(Some(bonds)) => bonds,
        _ => {
            // If no bonds in storage, calculate average bonds
            calculate_average_bonds(consensus_params)
        }
    }
}

pub fn save_copier_bonds<T: Config>(subnet_id: u16, consensus_output: &ConsensusOutput<T>) {
    let storage_key = alloc::format!("copier_bonds:{subnet_id}");
    let storage = StorageValueRef::persistent(storage_key.as_bytes());

    if let Some(Some(bonds)) = consensus_output.bonds.last() {
        storage.set(bonds);
    }
}

/// Calculates the weighted average bonds across all active validators.
///
/// # Arguments
///
/// * `consensus_params` - A reference to the consensus parameters containing validator information
///
/// # Returns
///
/// Returns a vector of tuples `(target, value)` where:
/// * `target` - The bond target identifier
/// * `value` - The weighted average bond value
///
/// # Details
///
/// The function:
/// 1. Filters for active validators with validator permits
/// 2. Handles special cases:
///    - Returns empty vector if no active validators
///    - Returns bonds of single validator if only one exists
/// 3. Calculates total stake across all validators
/// 4. For multiple validators:
///    - Computes weighted bond values based on each validator's stake proportion
///    - Aggregates weighted bonds by target
///
/// # Notes
///
/// - Weights are calculated as: validator_stake / total_stake
/// - Bond values are rounded to nearest integer
/// - Handles potential arithmetic overflow using saturating addition
/// - Returns empty vector if total stake is zero
///
/// # Type Parameters
///
/// * `T` - Must implement the `Config` trait
fn calculate_average_bonds<T: Config>(consensus_params: &ConsensusParams<T>) -> Vec<(u16, u16)> {
    let active_validators: Vec<&ModuleParams<T::AccountId>> =
        consensus_params.modules.values().filter(|m| m.validator_permit).collect();

    match active_validators.len() {
        0 => return Vec::new(),
        1 => return active_validators.first().map(|v| v.bonds.clone()).unwrap_or_default(),
        _ => {}
    }

    let total_stake = active_validators.iter().fold(I64F64::from_num(0), |acc, v| {
        acc.checked_add(v.stake_original).unwrap_or(acc)
    });

    if total_stake == I64F64::from_num(0) {
        return Vec::new();
    }

    let bond_sums =
        active_validators
            .iter()
            .fold(BTreeMap::new(), |mut acc: BTreeMap<u16, u16>, validator| {
                let weight = validator
                    .stake_original
                    .checked_div(total_stake)
                    .unwrap_or(I64F64::from_num(0));

                validator.bonds.iter().for_each(|&(target, value)| {
                    let weighted_value = (value as f64 * weight.to_num::<f64>()).round() as u16;
                    acc.entry(target)
                        .and_modify(|e| *e = e.saturating_add(weighted_value))
                        .or_insert(weighted_value);
                });

                acc
            });

    bond_sums.into_iter().collect()
}
