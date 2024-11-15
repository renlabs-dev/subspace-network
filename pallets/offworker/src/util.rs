use super::*;
use crate::profitability::{get_copier_stake, is_copying_irrational};
use types::SimulationYumaParams;

pub fn process_consensus_params<T>(
    subnet_id: u16,
    consensus_params: Vec<(u64, ConsensusParams<T>)>,
    mut simulation_result: ConsensusSimulationResult<T>,
) -> (Vec<BlockWeights>, ShouldDecryptResult<T>)
where
    T: pallet_subspace::Config + pallet_subnet_emission::Config + pallet::Config,
{
    let mut epochs = Vec::new();
    let mut result = ShouldDecryptResult::<T> {
        should_decrypt: false,
        simulation_result: simulation_result.clone(),
        delta: I64F64::from_num(0),
    };

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
                    log::warn!("Empty encrypted weights for UID: {}", uid);
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
                            // TODO:
                            log::error!("Failed to decrypt weights for UID: {}", uid);
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

        log::info!("decrypted weights are: {:?}", decrypted_weights);

        let weights_for_should_decrypt: Vec<_> = decrypted_weights
            .iter()
            .cloned()
            .map(|(uid, weights, _)| (uid, weights))
            .collect();

        log::debug!(
            "Preparing should_decrypt check with {} weight entries",
            weights_for_should_decrypt.len()
        );

        let should_decrypt_result = should_decrypt_weights::<T>(
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

        simulation_result = should_decrypt_result.simulation_result.clone();

        if should_decrypt_result.should_decrypt {
            log::info!(
                "Adding decrypted weights for block {} to epochs",
                param_block
            );
            epochs.push((*param_block, decrypted_weights));
            result = should_decrypt_result;
        }
    }

    log::info!(
        "Final processing result: {} epochs processed, should_decrypt: {}",
        epochs.len(),
        result.should_decrypt
    );

    (epochs, result)
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
    let SimulationYumaParams {
        uid: copier_uid,
        params: simulation_yuma_params,
        decrypted_weights_map,
    } = compute_simulation_yuma_params::<T>(
        decrypted_weights,
        latest_runtime_yuma_params,
        subnet_id,
    );

    // Run consensus simulation with error handling
    let simulation_yuma_output = match YumaEpoch::<T>::new(subnet_id, simulation_yuma_params)
        .run(decrypted_weights_map.into_iter().collect::<Vec<_>>())
    {
        Ok(output) => output,
        // 3. TODO: this is the scenario that occours when the weights are empty usually. the
        //    consenus will return an error broken weights. The offchain worker has to keep working,
        //    even if it has empty weights, so probably do an early check, before running this
        //    consensus, to make sure the consensus is supplied with weights. And if it has empty
        //    weights, just skip the yuma, and make sure that it will send these empty weights, back
        //    to the runtime, with the potential correct weights that it might encounter in the
        //    future.
        Err(e) => {
            log::error!("Failed to run consensus simulation: {:?}", e);
            return ShouldDecryptResult::default(); // this just has to send the weights back to
                                                   // runtime, we immidiately declare the copying as
                                                   // irrational and let all weights to be returned
                                                   // and handeled by the runtime
        }
    };

    // Get delegation fee (this is not a Result type)
    let delegation_fee = MinFees::<T>::get().stake_delegation_fee;

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
) -> SimulationYumaParams<T> {
    let copier_uid: u16 = runtime_yuma_params.modules.len() as u16;

    // This **has** to be ontained from the runtime storage
    let consensus_weights = Consensus::<T>::get(subnet_id);
    let copier_weights: Vec<(u16, u16)> = consensus_weights
        .into_iter()
        .enumerate()
        .map(|(index, value)| (index as u16, value))
        .collect();

    runtime_yuma_params = add_copier_to_yuma_params(copier_uid, runtime_yuma_params, subnet_id);

    let mut onchain_weights: BTreeMap<u16, Vec<(u16, u16)>> =
        Weights::<T>::iter_prefix(subnet_id).collect();

    onchain_weights.extend(
        decrypted_weights
            .iter()
            .cloned()
            .chain(sp_std::iter::once((copier_uid, copier_weights))), /* HONZA TODO figure out
                                                                       * this validator key */
    );

    SimulationYumaParams {
        uid: copier_uid,
        params: runtime_yuma_params,
        decrypted_weights_map: onchain_weights,
    }
}

/// This will mutate ConsensusParams with copier information, ready for simulation
/// This function should run
pub fn add_copier_to_yuma_params<T: Config>(
    copier_uid: u16,
    mut runtime_yuma_params: ConsensusParams<T>,
    subnet_id: u16,
) -> ConsensusParams<T> {
    // It is fine to get the permits from the consensus params
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
        bonds: Vec::new(),
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

    runtime_yuma_params
}
