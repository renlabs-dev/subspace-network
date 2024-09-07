mod utils;
use sp_core::{crypto::AccountId32, Decode, H256 as Hash};
use std::{collections::HashMap, fs::File, io::Write, path::Path};
use structopt::StructOpt;
use substrate_api_client::{ac_primitives::StorageKey, GetChainInfo, GetStorage};
use utils::api::{create_client, query_map, Client, CustomError};
const QUERY_URL: &str = "wss://bittensor-finney.api.onfinality.io/public:443";
const STANDARD_MODULE: &str = "SubtensorModule";

#[derive(StructOpt, Debug)]
#[structopt(name = "snapshot_generator")]
struct Opt {
    #[structopt(short, long)]
    subnet: u32,
    #[structopt(short, long)]
    output: Option<String>,
    #[structopt(short, long, default_value = ".")]
    directory: String,
    #[structopt(long, default_value = "360")]
    tempo: u32,
    #[structopt(long, default_value = "3600000")]
    start_block: u32,
    #[structopt(long, default_value = "24")]
    iter_epochs: u32,
}

async fn get_stake(
    client: &Client,
    subnet: u32,
    block_hash: Option<Hash>,
) -> HashMap<String, u128> {
    let mut stake = HashMap::new();

    let all_uids = client
        .get_storage_map::<_, Vec<(AccountId32, u64)>>(STANDARD_MODULE, "Uids", subnet, block_hash)
        .unwrap()
        .into_iter()
        .flat_map(|vec| vec)
        .collect::<Vec<_>>();

    dbg!(&all_uids);

    for (counter, (account_id, uid)) in all_uids.into_iter().enumerate() {
        let stake_result = client
            .get_storage_map::<_, Vec<(String, u64)>>(
                STANDARD_MODULE,
                "Stake",
                account_id,
                block_hash,
            )
            .unwrap();

        if let Some(stake_vec) = stake_result {
            let total_stake: u128 = stake_vec.iter().map(|(_, amount)| *amount as u128).sum();
            stake.insert(uid.to_string(), total_stake);
        } else {
            stake.insert(uid.to_string(), 0);
        }

        if (counter + 1) % 100 == 0 {
            println!("Processed {} uids", counter + 1);
        }
    }

    dbg!(&stake);
    stake
}

async fn get_last_update(
    client: &Client,
    subnet: u32,
    block_hash: Option<Hash>,
) -> HashMap<String, String> {
    let last_update: Vec<StorageKey> =
        query_map(client, block_hash, STANDARD_MODULE, "LastUpdate").await.unwrap();

    dbg!(&last_update);

    let mut decoded_last_update: HashMap<u16, Vec<u64>> = HashMap::new();

    for storage_key in last_update {
        if let Ok(Some((key, value))) =
            client.get_storage_by_key::<(u16, Vec<u64>)>(storage_key, block_hash)
        {
            decoded_last_update.insert(key, value);
        }
    }

    dbg!(&decoded_last_update);

    let mut sane_last_update = HashMap::new();

    for (uid, value) in decoded_last_update {
        sane_last_update.insert(uid.to_string(), format!("{:?}", value));
    }

    sane_last_update
}

async fn get_validator_permits(
    client: &Client,
    subnet: u32,
    block_hash: Option<Hash>,
) -> HashMap<String, bool> {
    let validator_permits: Vec<StorageKey> =
        query_map(client, block_hash, STANDARD_MODULE, "ValidatorPermit").await.unwrap();

    let mut decoded_validator_permits = HashMap::new();

    for storage_key in validator_permits {
        if let Ok(Some(value)) =
            client.get_storage_by_key::<(u16, Vec<bool>)>(storage_key, block_hash)
        {
            decoded_validator_permits.insert(value.0, value.1);
        }
    }

    let mut sane_validator_permits = HashMap::new();

    for (uid, permits) in decoded_validator_permits {
        if let Some(&permit) = permits.first() {
            sane_validator_permits.insert(uid.to_string(), permit);
        }
    }

    sane_validator_permits
}

async fn get_registration_blocks(
    client: &Client,
    subnet: u32,
    block_hash: Option<Hash>,
) -> HashMap<String, String> {
    let registration_blocks = client
        .get_storage_double_map::<u16, u16, u64>(
            STANDARD_MODULE,
            "BlockAtRegistration",
            subnet as u16,
            u16::MAX,
            block_hash,
        )
        .unwrap();

    dbg!(&registration_blocks);

    let sane_registration_blocks: HashMap<String, String> = registration_blocks
        .into_iter()
        .enumerate()
        .map(|(index, value)| (index.to_string(), value.to_string()))
        .collect();

    sane_registration_blocks
}
async fn get_weights(
    client: &Client,
    subnet: u32,
    block_hash: Option<Hash>,
) -> HashMap<String, HashMap<String, Vec<(u32, u32)>>> {
    let mut weights: HashMap<String, HashMap<String, Vec<(u32, u32)>>> = HashMap::new();

    if let Some(subnet_weights) = client
        .get_storage_map::<_, Vec<(u32, u32)>>(STANDARD_MODULE, "Weights", subnet, block_hash)
        .unwrap()
    {
        let subnet_weights_map: HashMap<String, Vec<(u32, u32)>> = subnet_weights
            .into_iter()
            .enumerate()
            .map(|(index, (uid, w))| (uid.to_string(), vec![(index as u32, w)]))
            .collect();

        weights.insert(subnet.to_string(), subnet_weights_map);
    }

    weights
}

async fn get_epoch_data(
    client: &Client,
    block_hash: Option<Hash>,
    later_block_hash: Option<Hash>,
    subnet: u32,
) -> Result<
    (
        HashMap<String, HashMap<String, Vec<(u32, u32)>>>,
        HashMap<String, String>,
        HashMap<String, String>,
        HashMap<String, bool>,
    ),
    Box<dyn std::error::Error>,
> {
    let weights = get_weights(client, subnet, block_hash).await;
    let last_update = get_last_update(client, subnet, block_hash).await;
    let registration_blocks = get_registration_blocks(client, subnet, block_hash).await;
    let validator_permits = get_validator_permits(client, subnet, later_block_hash).await;

    Ok((weights, last_update, registration_blocks, validator_permits))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opt = Opt::from_args();

    let subnet = opt.subnet;
    let tempo = opt.tempo;
    let start_block = opt.start_block;
    let iter_epochs = opt.iter_epochs;

    let output = opt.output.unwrap_or_else(|| format!("sn{}_weights_stake.json", subnet));
    let output_path = Path::new(&opt.directory).join(output);

    println!("Starting snapshot generation...");
    let client = create_client(QUERY_URL)?;
    println!("Connected to {}", QUERY_URL);

    let mut data: HashMap<String, serde_json::Value> = HashMap::new();

    println!("Getting initial stake...");
    let start_block_hash = client.get_block_hash(Some(start_block)).unwrap().unwrap();
    data.insert(
        "stake".to_string(),
        serde_json::to_value(get_stake(&client, subnet, Some(start_block_hash)).await)
            .expect("Failed to serialize stake data"),
    );
    dbg!("got stake");
    let mut weights = HashMap::new();
    let mut last_update = HashMap::new();
    let mut registration_blocks = HashMap::new();
    let mut validator_permits = HashMap::new();

    for i in 0..iter_epochs {
        let block_number = start_block + (i * tempo);
        let block_hash = client
            .get_block_hash(Some(block_number))
            .map_err(CustomError::from)?
            .expect("Block hash should exist");

        let later_block_hash = client
            .get_block_hash(Some(block_number + 1))
            .map_err(CustomError::from)?
            .expect("Later block hash should exist");

        let (epoch_weights, epoch_last_update, epoch_registration_blocks, epoch_validator_permits) =
            get_epoch_data(&client, Some(block_hash), Some(later_block_hash), subnet).await?;

        weights.insert(block_number.to_string(), epoch_weights);
        last_update.insert(block_number.to_string(), epoch_last_update);
        registration_blocks.insert(block_number.to_string(), epoch_registration_blocks);
        validator_permits.insert(block_number.to_string(), epoch_validator_permits);

        println!("Collected data for block {}", block_number);
    }

    data.insert("weights".to_string(), serde_json::to_value(weights)?);
    data.insert(
        "last_update".to_string(),
        serde_json::to_value(last_update)?,
    );
    data.insert(
        "registration_blocks".to_string(),
        serde_json::to_value(registration_blocks)?,
    );
    data.insert(
        "validator_permits".to_string(),
        serde_json::to_value(validator_permits)?,
    );

    println!("Writing snapshot to {}", output_path.display());
    std::fs::create_dir_all(&opt.directory)?;
    let mut file = File::create(output_path)?;
    file.write_all(serde_json::to_string_pretty(&data)?.as_bytes())?;

    println!("Snapshot generation complete");

    Ok(())
}
