//! cast call subcommand
use crate::opts::{
    cast::{parse_block_id, parse_name_or_address},
    EthereumOpts,
};

use clap::Parser;
use ethers::types::{BlockId, NameOrAddress};
use eyre::Result;
use foundry_config::{
    figment::{
        self,
        value::{Dict, Map, Value},
        Metadata, Profile,
    },
    impl_figment_convert_cast, Config,
};

use serde::Serialize;

impl_figment_convert_cast!(CallArgs);

#[derive(Debug, Clone, Parser, Serialize)]
pub struct CallArgs {
    #[clap(help = "the address you want to query", parse(try_from_str = parse_name_or_address))]
    #[serde(skip)]
    pub address: NameOrAddress,
    #[serde(skip)]
    pub sig: String,
    #[serde(skip)]
    pub args: Vec<String>,
    #[clap(long, short, help = "the block you want to query, can also be earliest/latest/pending", parse(try_from_str = parse_block_id))]
    #[serde(skip)]
    pub block: Option<BlockId>,
    #[clap(flatten)]
    #[serde(flatten)]
    pub eth: EthereumOpts,
}

impl figment::Provider for CallArgs {
    fn metadata(&self) -> Metadata {
        Metadata::named("Call args provider")
    }

    fn data(&self) -> Result<Map<Profile, Dict>, figment::Error> {
        let value = Value::serialize(self)?;
        let mut dict = value.into_dict().unwrap();

        let rpc_url = self.eth.rpc_url().map_err(|err| err.to_string())?;
        if rpc_url != "http://localhost:8545" {
            dict.insert("eth_rpc_url".to_string(), Value::from(rpc_url.to_string()));
        }

        if let Some(from) = self.eth.from {
            dict.insert("sender".to_string(), Value::from(format!("{:?}", from)));
        }

        if let Some(etherscan_api_key) = &self.eth.etherscan_key {
            dict.insert(
                "etherscan_api_key".to_string(),
                Value::from(etherscan_api_key.to_string()),
            );
        }

        Ok(Map::from([(Config::selected_profile(), dict)]))
    }
}
