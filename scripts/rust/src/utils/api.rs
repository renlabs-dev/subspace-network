use sp_core::H256 as Hash;
use std::fmt;
use substrate_api_client::{
    ac_primitives::{AssetRuntimeConfig, StorageKey},
    rpc::{Request, WsRpcClient},
    Api, Error as ApiClientError, GetStorage,
};

#[allow(dead_code)]
pub type Client = Api<AssetRuntimeConfig, WsRpcClient>;

pub async fn query_map<T: Request>(
    api: &Api<AssetRuntimeConfig, T>,
    at: Option<Hash>,
    module: &'static str,
    storage: &'static str,
) -> Result<Vec<StorageKey>, ApiClientError> {
    let storage_key = api.get_storage_map_key_prefix(module, storage)?;
    let result = api.get_storage_keys_paged(Some(storage_key), u32::MAX, None, at)?;
    Ok(result)
}

pub fn create_client(url: &str) -> Result<Client, Box<dyn std::error::Error>> {
    let client = WsRpcClient::new(url).map_err(|e| {
        Box::new(CustomError(format!("WsRpcClient error: {:?}", e))) as Box<dyn std::error::Error>
    })?;
    let api = Api::<AssetRuntimeConfig, _>::new(client).map_err(|e| {
        Box::new(CustomError(format!("Api creation error: {:?}", e))) as Box<dyn std::error::Error>
    })?;
    Ok(api)
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct CustomError(pub String);

impl fmt::Display for CustomError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for CustomError {}

impl From<ApiClientError> for CustomError {
    fn from(error: ApiClientError) -> Self {
        CustomError(format!("{:?}", error))
    }
}
