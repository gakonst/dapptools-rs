//! Cast is a Swiss Army knife for interacting with Ethereum applications from the command line.

#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use alloy_consensus::TxEnvelope;
use alloy_dyn_abi::{DynSolType, DynSolValue, FunctionExt};
use alloy_json_abi::Function;
use alloy_network::AnyNetwork;
use alloy_primitives::{
    hex,
    utils::{keccak256, ParseUnits, Unit},
    Address, Keccak256, TxHash, TxKind, B256, I256, U256,
};
use alloy_provider::{
    network::eip2718::{Decodable2718, Encodable2718},
    PendingTransactionBuilder, Provider,
};
use alloy_rlp::Decodable;
use alloy_rpc_types::{BlockId, BlockNumberOrTag, Filter, TransactionRequest};
use alloy_serde::WithOtherFields;
use alloy_sol_types::sol;
use base::{Base, NumberWithBase, ToBase};
use chrono::DateTime;
use eyre::{Context, ContextCompat, OptionExt, Result};
use foundry_block_explorers::Client;
use foundry_common::{
    abi::{encode_function_args, get_func},
    compile::etherscan_project,
    fmt::*,
    fs, get_pretty_tx_receipt_attr, shell, TransactionReceiptWithRevertReason,
};
use foundry_compilers::flatten::Flattener;
use foundry_config::Chain;
use futures::{future::Either, FutureExt, StreamExt};
use rayon::prelude::*;
use revm::primitives::Eof;
use std::{
    borrow::Cow,
    fmt::Write,
    io,
    path::PathBuf,
    str::FromStr,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::signal::ctrl_c;
use utils::decode_instructions;

use foundry_common::abi::encode_function_args_packed;
pub use foundry_evm::*;

pub mod base;
pub mod errors;
mod rlp_converter;

use rlp_converter::Item;

#[macro_use]
extern crate foundry_common;

// TODO: CastContract with common contract initializers? Same for CastProviders?

sol! {
    #[sol(rpc)]
    interface IERC20 {
        #[derive(Debug)]
        function balanceOf(address owner) external view returns (uint256);
    }
}

pub struct Cast<P> {
    provider: P,
}

impl<P: Provider<AnyNetwork>> Cast<P> {
    /// Creates a new Cast instance from the provided client
    ///
    /// # Example
    ///
    /// ```
    /// use alloy_provider::{network::AnyNetwork, ProviderBuilder, RootProvider};
    /// use cast::Cast;
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider =
    ///     ProviderBuilder::<_, _, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;
    /// let cast = Cast::new(provider);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(provider: P) -> Self {
        Self { provider }
    }

    /// Makes a read-only call to the specified address
    ///
    /// # Example
    ///
    /// ```
    /// use alloy_primitives::{Address, U256, Bytes};
    /// use alloy_rpc_types::{TransactionRequest};
    /// use alloy_serde::WithOtherFields;
    /// use cast::Cast;
    /// use alloy_provider::{RootProvider, ProviderBuilder, network::AnyNetwork};
    /// use std::str::FromStr;
    /// use alloy_sol_types::{sol, SolCall};
    ///
    /// sol!(
    ///     function greeting(uint256 i) public returns (string);
    /// );
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let alloy_provider = ProviderBuilder::<_,_, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;;
    /// let to = Address::from_str("0xB3C95ff08316fb2F2e3E52Ee82F8e7b605Aa1304")?;
    /// let greeting = greetingCall { i: U256::from(5) }.abi_encode();
    /// let bytes = Bytes::from_iter(greeting.iter());
    /// let tx = TransactionRequest::default().to(to).input(bytes.into());
    /// let tx = WithOtherFields::new(tx);
    /// let cast = Cast::new(alloy_provider);
    /// let data = cast.call(&tx, None, None).await?;
    /// println!("{}", data);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn call(
        &self,
        req: &WithOtherFields<TransactionRequest>,
        func: Option<&Function>,
        block: Option<BlockId>,
    ) -> Result<String> {
        let res = self.provider.call(req).block(block.unwrap_or_default()).await?;

        let mut decoded = vec![];

        if let Some(func) = func {
            // decode args into tokens
            decoded = match func.abi_decode_output(res.as_ref(), false) {
                Ok(decoded) => decoded,
                Err(err) => {
                    // ensure the address is a contract
                    if res.is_empty() {
                        // check that the recipient is a contract that can be called
                        if let Some(TxKind::Call(addr)) = req.to {
                            if let Ok(code) = self
                                .provider
                                .get_code_at(addr)
                                .block_id(block.unwrap_or_default())
                                .await
                            {
                                if code.is_empty() {
                                    eyre::bail!("contract {addr:?} does not have any code")
                                }
                            }
                        } else if Some(TxKind::Create) == req.to {
                            eyre::bail!("tx req is a contract deployment");
                        } else {
                            eyre::bail!("recipient is None");
                        }
                    }
                    return Err(err).wrap_err(
                        "could not decode output; did you specify the wrong function return data type?"
                    );
                }
            };
        }

        // handle case when return type is not specified
        Ok(if decoded.is_empty() {
            res.to_string()
        } else if shell::is_json() {
            let tokens = decoded.iter().map(format_token_raw).collect::<Vec<_>>();
            serde_json::to_string_pretty(&tokens).unwrap()
        } else {
            // seth compatible user-friendly return type conversions
            decoded.iter().map(format_token).collect::<Vec<_>>().join("\n")
        })
    }

    /// Generates an access list for the specified transaction
    ///
    /// # Example
    ///
    /// ```
    /// use cast::{Cast};
    /// use alloy_primitives::{Address, U256, Bytes};
    /// use alloy_rpc_types::{TransactionRequest};
    /// use alloy_serde::WithOtherFields;
    /// use alloy_provider::{RootProvider, ProviderBuilder, network::AnyNetwork};
    /// use std::str::FromStr;
    /// use alloy_sol_types::{sol, SolCall};
    ///
    /// sol!(
    ///     function greeting(uint256 i) public returns (string);
    /// );
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider = ProviderBuilder::<_,_, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;;
    /// let to = Address::from_str("0xB3C95ff08316fb2F2e3E52Ee82F8e7b605Aa1304")?;
    /// let greeting = greetingCall { i: U256::from(5) }.abi_encode();
    /// let bytes = Bytes::from_iter(greeting.iter());
    /// let tx = TransactionRequest::default().to(to).input(bytes.into());
    /// let tx = WithOtherFields::new(tx);
    /// let cast = Cast::new(&provider);
    /// let access_list = cast.access_list(&tx, None).await?;
    /// println!("{}", access_list);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn access_list(
        &self,
        req: &WithOtherFields<TransactionRequest>,
        block: Option<BlockId>,
    ) -> Result<String> {
        let access_list =
            self.provider.create_access_list(req).block_id(block.unwrap_or_default()).await?;
        let res = if shell::is_json() {
            serde_json::to_string(&access_list)?
        } else {
            let mut s =
                vec![format!("gas used: {}", access_list.gas_used), "access list:".to_string()];
            for al in access_list.access_list.0 {
                s.push(format!("- address: {}", &al.address.to_checksum(None)));
                if !al.storage_keys.is_empty() {
                    s.push("  keys:".to_string());
                    for key in al.storage_keys {
                        s.push(format!("    {key:?}"));
                    }
                }
            }
            s.join("\n")
        };

        Ok(res)
    }

    pub async fn balance(&self, who: Address, block: Option<BlockId>) -> Result<U256> {
        Ok(self.provider.get_balance(who).block_id(block.unwrap_or_default()).await?)
    }

    /// Sends a transaction to the specified address
    ///
    /// # Example
    ///
    /// ```
    /// use cast::{Cast};
    /// use alloy_primitives::{Address, U256, Bytes};
    /// use alloy_serde::WithOtherFields;
    /// use alloy_rpc_types::{TransactionRequest};
    /// use alloy_provider::{RootProvider, ProviderBuilder, network::AnyNetwork};
    /// use std::str::FromStr;
    /// use alloy_sol_types::{sol, SolCall};
    ///
    /// sol!(
    ///     function greet(string greeting) public;
    /// );
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider = ProviderBuilder::<_,_, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;;
    /// let from = Address::from_str("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045")?;
    /// let to = Address::from_str("0xB3C95ff08316fb2F2e3E52Ee82F8e7b605Aa1304")?;
    /// let greeting = greetCall { greeting: "hello".to_string() }.abi_encode();
    /// let bytes = Bytes::from_iter(greeting.iter());
    /// let gas = U256::from_str("200000").unwrap();
    /// let value = U256::from_str("1").unwrap();
    /// let nonce = U256::from_str("1").unwrap();
    /// let tx = TransactionRequest::default().to(to).input(bytes.into()).from(from);
    /// let tx = WithOtherFields::new(tx);
    /// let cast = Cast::new(provider);
    /// let data = cast.send(tx).await?;
    /// println!("{:#?}", data);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn send(
        &self,
        tx: WithOtherFields<TransactionRequest>,
    ) -> Result<PendingTransactionBuilder<AnyNetwork>> {
        let res = self.provider.send_transaction(tx).await?;

        Ok(res)
    }

    /// Publishes a raw transaction to the network
    ///
    /// # Example
    ///
    /// ```
    /// use alloy_provider::{network::AnyNetwork, ProviderBuilder, RootProvider};
    /// use cast::Cast;
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider =
    ///     ProviderBuilder::<_, _, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;
    /// let cast = Cast::new(provider);
    /// let res = cast.publish("0x1234".to_string()).await?;
    /// println!("{:?}", res);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn publish(
        &self,
        mut raw_tx: String,
    ) -> Result<PendingTransactionBuilder<AnyNetwork>> {
        raw_tx = match raw_tx.strip_prefix("0x") {
            Some(s) => s.to_string(),
            None => raw_tx,
        };
        let tx = hex::decode(raw_tx)?;
        let res = self.provider.send_raw_transaction(&tx).await?;

        Ok(res)
    }

    /// # Example
    ///
    /// ```
    /// use alloy_provider::{network::AnyNetwork, ProviderBuilder, RootProvider};
    /// use cast::Cast;
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider =
    ///     ProviderBuilder::<_, _, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;
    /// let cast = Cast::new(provider);
    /// let block = cast.block(5, true, None).await?;
    /// println!("{}", block);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn block<B: Into<BlockId>>(
        &self,
        block: B,
        full: bool,
        field: Option<String>,
    ) -> Result<String> {
        let block = block.into();
        if let Some(ref field) = field {
            if field == "transactions" && !full {
                eyre::bail!("use --full to view transactions")
            }
        }

        let block = self
            .provider
            .get_block(block, full.into())
            .await?
            .ok_or_else(|| eyre::eyre!("block {:?} not found", block))?;

        let block = if let Some(ref field) = field {
            get_pretty_block_attr(&block, field)
                .unwrap_or_else(|| format!("{field} is not a valid block field"))
        } else if shell::is_json() {
            serde_json::to_value(&block).unwrap().to_string()
        } else {
            block.pretty()
        };

        Ok(block)
    }

    async fn block_field_as_num<B: Into<BlockId>>(&self, block: B, field: String) -> Result<U256> {
        let block = block.into();
        let block_field = Self::block(
            self,
            block,
            false,
            // Select only select field
            Some(field),
        )
        .await?;

        let ret = if block_field.starts_with("0x") {
            U256::from_str_radix(strip_0x(&block_field), 16).expect("Unable to convert hex to U256")
        } else {
            U256::from_str_radix(&block_field, 10).expect("Unable to convert decimal to U256")
        };
        Ok(ret)
    }

    pub async fn base_fee<B: Into<BlockId>>(&self, block: B) -> Result<U256> {
        Self::block_field_as_num(self, block, String::from("baseFeePerGas")).await
    }

    pub async fn age<B: Into<BlockId>>(&self, block: B) -> Result<String> {
        let timestamp_str =
            Self::block_field_as_num(self, block, String::from("timestamp")).await?.to_string();
        let datetime = DateTime::from_timestamp(timestamp_str.parse::<i64>().unwrap(), 0).unwrap();
        Ok(datetime.format("%a %b %e %H:%M:%S %Y").to_string())
    }

    pub async fn timestamp<B: Into<BlockId>>(&self, block: B) -> Result<U256> {
        Self::block_field_as_num(self, block, "timestamp".to_string()).await
    }

    pub async fn chain(&self) -> Result<&str> {
        let genesis_hash = Self::block(
            self,
            0,
            false,
            // Select only block hash
            Some(String::from("hash")),
        )
        .await?;

        Ok(match &genesis_hash[..] {
            "0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3" => {
                match &(Self::block(self, 1920000, false, Some("hash".to_string())).await?)[..] {
                    "0x94365e3a8c0b35089c1d1195081fe7489b528a84b22199c916180db8b28ade7f" => {
                        "etclive"
                    }
                    _ => "ethlive",
                }
            }
            "0xa3c565fc15c7478862d50ccd6561e3c06b24cc509bf388941c25ea985ce32cb9" => "kovan",
            "0x41941023680923e0fe4d74a34bdac8141f2540e3ae90623718e47d66d1ca4a2d" => "ropsten",
            "0x7ca38a1916c42007829c55e69d3e9a73265554b586a499015373241b8a3fa48b" => {
                "optimism-mainnet"
            }
            "0xc1fc15cd51159b1f1e5cbc4b82e85c1447ddfa33c52cf1d98d14fba0d6354be1" => {
                "optimism-goerli"
            }
            "0x02adc9b449ff5f2467b8c674ece7ff9b21319d76c4ad62a67a70d552655927e5" => {
                "optimism-kovan"
            }
            "0x521982bd54239dc71269eefb58601762cc15cfb2978e0becb46af7962ed6bfaa" => "fraxtal",
            "0x910f5c4084b63fd860d0c2f9a04615115a5a991254700b39ba072290dbd77489" => {
                "fraxtal-testnet"
            }
            "0x7ee576b35482195fc49205cec9af72ce14f003b9ae69f6ba0faef4514be8b442" => {
                "arbitrum-mainnet"
            }
            "0x0cd786a2425d16f152c658316c423e6ce1181e15c3295826d7c9904cba9ce303" => "morden",
            "0x6341fd3daf94b748c72ced5a5b26028f2474f5f00d824504e4fa37a75767e177" => "rinkeby",
            "0xbf7e331f7f7c1dd2e05159666b3bf8bc7a8a3a9eb1d518969eab529dd9b88c1a" => "goerli",
            "0x14c2283285a88fe5fce9bf5c573ab03d6616695d717b12a127188bcacfc743c4" => "kotti",
            "0xa9c28ce2141b56c474f1dc504bee9b01eb1bd7d1a507580d5519d4437a97de1b" => "polygon-pos",
            "0x7202b2b53c5a0836e773e319d18922cc756dd67432f9a1f65352b61f4406c697" => {
                "polygon-pos-amoy-testnet"
            }
            "0x81005434635456a16f74ff7023fbe0bf423abbc8a8deb093ffff455c0ad3b741" => "polygon-zkevm",
            "0x676c1a76a6c5855a32bdf7c61977a0d1510088a4eeac1330466453b3d08b60b9" => {
                "polygon-zkevm-cardona-testnet"
            }
            "0x4f1dd23188aab3a76b463e4af801b52b1248ef073c648cbdc4c9333d3da79756" => "gnosis",
            "0xada44fd8d2ecab8b08f256af07ad3e777f17fb434f8f8e678b312f576212ba9a" => "chiado",
            "0x6d3c66c5357ec91d5c43af47e234a939b22557cbb552dc45bebbceeed90fbe34" => "bsctest",
            "0x0d21840abff46b96c84b2ac9e10e4f5cdaeb5693cb665db62a2f3b02d2d57b5b" => "bsc",
            "0x31ced5b9beb7f8782b014660da0cb18cc409f121f408186886e1ca3e8eeca96b" => {
                match &(Self::block(self, 1, false, Some(String::from("hash"))).await?)[..] {
                    "0x738639479dc82d199365626f90caa82f7eafcfe9ed354b456fb3d294597ceb53" => {
                        "avalanche-fuji"
                    }
                    _ => "avalanche",
                }
            }
            "0x23a2658170ba70d014ba0d0d2709f8fbfe2fa660cd868c5f282f991eecbe38ee" => "ink",
            "0xe5fd5cf0be56af58ad5751b401410d6b7a09d830fa459789746a3d0dd1c79834" => "ink-sepolia",
            _ => "unknown",
        })
    }

    pub async fn chain_id(&self) -> Result<u64> {
        Ok(self.provider.get_chain_id().await?)
    }

    pub async fn block_number(&self) -> Result<u64> {
        Ok(self.provider.get_block_number().await?)
    }

    pub async fn gas_price(&self) -> Result<u128> {
        Ok(self.provider.get_gas_price().await?)
    }

    /// # Example
    ///
    /// ```
    /// use alloy_primitives::Address;
    /// use alloy_provider::{network::AnyNetwork, ProviderBuilder, RootProvider};
    /// use cast::Cast;
    /// use std::str::FromStr;
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider =
    ///     ProviderBuilder::<_, _, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;
    /// let cast = Cast::new(provider);
    /// let addr = Address::from_str("0x7eD52863829AB99354F3a0503A622e82AcD5F7d3")?;
    /// let nonce = cast.nonce(addr, None).await?;
    /// println!("{}", nonce);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn nonce(&self, who: Address, block: Option<BlockId>) -> Result<u64> {
        Ok(self.provider.get_transaction_count(who).block_id(block.unwrap_or_default()).await?)
    }

    /// #Example
    ///
    /// ```
    /// use alloy_primitives::{Address, FixedBytes};
    /// use alloy_provider::{network::AnyNetwork, ProviderBuilder, RootProvider};
    /// use cast::Cast;
    /// use std::str::FromStr;
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider =
    ///     ProviderBuilder::<_, _, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;
    /// let cast = Cast::new(provider);
    /// let addr = Address::from_str("0x7eD52863829AB99354F3a0503A622e82AcD5F7d3")?;
    /// let slots = vec![FixedBytes::from_str("0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421")?];
    /// let codehash = cast.codehash(addr, slots, None).await?;
    /// println!("{}", codehash);
    /// # Ok(())
    /// # }
    pub async fn codehash(
        &self,
        who: Address,
        slots: Vec<B256>,
        block: Option<BlockId>,
    ) -> Result<String> {
        Ok(self
            .provider
            .get_proof(who, slots)
            .block_id(block.unwrap_or_default())
            .await?
            .code_hash
            .to_string())
    }

    /// #Example
    ///
    /// ```
    /// use alloy_primitives::{Address, FixedBytes};
    /// use alloy_provider::{network::AnyNetwork, ProviderBuilder, RootProvider};
    /// use cast::Cast;
    /// use std::str::FromStr;
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider =
    ///     ProviderBuilder::<_, _, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;
    /// let cast = Cast::new(provider);
    /// let addr = Address::from_str("0x7eD52863829AB99354F3a0503A622e82AcD5F7d3")?;
    /// let slots = vec![FixedBytes::from_str("0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421")?];
    /// let storage_root = cast.storage_root(addr, slots, None).await?;
    /// println!("{}", storage_root);
    /// # Ok(())
    /// # }
    pub async fn storage_root(
        &self,
        who: Address,
        slots: Vec<B256>,
        block: Option<BlockId>,
    ) -> Result<String> {
        Ok(self
            .provider
            .get_proof(who, slots)
            .block_id(block.unwrap_or_default())
            .await?
            .storage_hash
            .to_string())
    }

    /// # Example
    ///
    /// ```
    /// use alloy_primitives::Address;
    /// use alloy_provider::{network::AnyNetwork, ProviderBuilder, RootProvider};
    /// use cast::Cast;
    /// use std::str::FromStr;
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider =
    ///     ProviderBuilder::<_, _, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;
    /// let cast = Cast::new(provider);
    /// let addr = Address::from_str("0x7eD52863829AB99354F3a0503A622e82AcD5F7d3")?;
    /// let implementation = cast.implementation(addr, false, None).await?;
    /// println!("{}", implementation);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn implementation(
        &self,
        who: Address,
        is_beacon: bool,
        block: Option<BlockId>,
    ) -> Result<String> {
        let slot = match is_beacon {
            true => {
                // Use the beacon slot : bytes32(uint256(keccak256('eip1967.proxy.beacon')) - 1)
                B256::from_str(
                    "0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50",
                )?
            }
            false => {
                // Use the implementation slot :
                // bytes32(uint256(keccak256('eip1967.proxy.implementation')) - 1)
                B256::from_str(
                    "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc",
                )?
            }
        };

        let value = self
            .provider
            .get_storage_at(who, slot.into())
            .block_id(block.unwrap_or_default())
            .await?;
        let addr = Address::from_word(value.into());
        Ok(format!("{addr:?}"))
    }

    /// # Example
    ///
    /// ```
    /// use alloy_primitives::Address;
    /// use alloy_provider::{network::AnyNetwork, ProviderBuilder, RootProvider};
    /// use cast::Cast;
    /// use std::str::FromStr;
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider =
    ///     ProviderBuilder::<_, _, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;
    /// let cast = Cast::new(provider);
    /// let addr = Address::from_str("0x7eD52863829AB99354F3a0503A622e82AcD5F7d3")?;
    /// let admin = cast.admin(addr, None).await?;
    /// println!("{}", admin);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn admin(&self, who: Address, block: Option<BlockId>) -> Result<String> {
        let slot =
            B256::from_str("0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103")?;
        let value = self
            .provider
            .get_storage_at(who, slot.into())
            .block_id(block.unwrap_or_default())
            .await?;
        let addr = Address::from_word(value.into());
        Ok(format!("{addr:?}"))
    }

    /// # Example
    ///
    /// ```
    /// use alloy_primitives::{Address, U256};
    /// use alloy_provider::{network::AnyNetwork, ProviderBuilder, RootProvider};
    /// use cast::Cast;
    /// use std::str::FromStr;
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider =
    ///     ProviderBuilder::<_, _, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;
    /// let cast = Cast::new(provider);
    /// let addr = Address::from_str("7eD52863829AB99354F3a0503A622e82AcD5F7d3")?;
    /// let computed_address = cast.compute_address(addr, None).await?;
    /// println!("Computed address for address {addr}: {computed_address}");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn compute_address(&self, address: Address, nonce: Option<u64>) -> Result<Address> {
        let unpacked = if let Some(n) = nonce { n } else { self.nonce(address, None).await? };
        Ok(address.create(unpacked))
    }

    /// # Example
    ///
    /// ```
    /// use alloy_primitives::Address;
    /// use alloy_provider::{network::AnyNetwork, ProviderBuilder, RootProvider};
    /// use cast::Cast;
    /// use std::str::FromStr;
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider =
    ///     ProviderBuilder::<_, _, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;
    /// let cast = Cast::new(provider);
    /// let addr = Address::from_str("0x00000000219ab540356cbb839cbe05303d7705fa")?;
    /// let code = cast.code(addr, None, false).await?;
    /// println!("{}", code);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn code(
        &self,
        who: Address,
        block: Option<BlockId>,
        disassemble: bool,
    ) -> Result<String> {
        if disassemble {
            let code =
                self.provider.get_code_at(who).block_id(block.unwrap_or_default()).await?.to_vec();
            SimpleCast::disassemble(&code)
        } else {
            Ok(format!(
                "{}",
                self.provider.get_code_at(who).block_id(block.unwrap_or_default()).await?
            ))
        }
    }

    /// Example
    ///
    /// ```
    /// use alloy_primitives::Address;
    /// use alloy_provider::{network::AnyNetwork, ProviderBuilder, RootProvider};
    /// use cast::Cast;
    /// use std::str::FromStr;
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider =
    ///     ProviderBuilder::<_, _, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;
    /// let cast = Cast::new(provider);
    /// let addr = Address::from_str("0x00000000219ab540356cbb839cbe05303d7705fa")?;
    /// let codesize = cast.codesize(addr, None).await?;
    /// println!("{}", codesize);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn codesize(&self, who: Address, block: Option<BlockId>) -> Result<String> {
        let code =
            self.provider.get_code_at(who).block_id(block.unwrap_or_default()).await?.to_vec();
        Ok(format!("{}", code.len()))
    }

    /// # Example
    ///
    /// ```
    /// use alloy_provider::{network::AnyNetwork, ProviderBuilder, RootProvider};
    /// use cast::Cast;
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider =
    ///     ProviderBuilder::<_, _, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;
    /// let cast = Cast::new(provider);
    /// let tx_hash = "0xf8d1713ea15a81482958fb7ddf884baee8d3bcc478c5f2f604e008dc788ee4fc";
    /// let tx = cast.transaction(tx_hash.to_string(), None, false).await?;
    /// println!("{}", tx);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn transaction(
        &self,
        tx_hash: String,
        field: Option<String>,
        raw: bool,
    ) -> Result<String> {
        let tx_hash = TxHash::from_str(&tx_hash).wrap_err("invalid tx hash")?;
        let tx = self
            .provider
            .get_transaction_by_hash(tx_hash)
            .await?
            .ok_or_else(|| eyre::eyre!("tx not found: {:?}", tx_hash))?;

        Ok(if raw {
            format!("0x{}", hex::encode(tx.inner.inner.encoded_2718()))
        } else if let Some(field) = field {
            get_pretty_tx_attr(&tx.inner, field.as_str())
                .ok_or_else(|| eyre::eyre!("invalid tx field: {}", field.to_string()))?
        } else if shell::is_json() {
            // to_value first to sort json object keys
            serde_json::to_value(&tx)?.to_string()
        } else {
            tx.pretty()
        })
    }

    /// # Example
    ///
    /// ```
    /// use alloy_provider::{network::AnyNetwork, ProviderBuilder, RootProvider};
    /// use cast::Cast;
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider =
    ///     ProviderBuilder::<_, _, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;
    /// let cast = Cast::new(provider);
    /// let tx_hash = "0xf8d1713ea15a81482958fb7ddf884baee8d3bcc478c5f2f604e008dc788ee4fc";
    /// let receipt = cast.receipt(tx_hash.to_string(), None, 1, None, false).await?;
    /// println!("{}", receipt);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn receipt(
        &self,
        tx_hash: String,
        field: Option<String>,
        confs: u64,
        timeout: Option<u64>,
        cast_async: bool,
    ) -> Result<String> {
        let tx_hash = TxHash::from_str(&tx_hash).wrap_err("invalid tx hash")?;

        let mut receipt: TransactionReceiptWithRevertReason =
            match self.provider.get_transaction_receipt(tx_hash).await? {
                Some(r) => r,
                None => {
                    // if the async flag is provided, immediately exit if no tx is found, otherwise
                    // try to poll for it
                    if cast_async {
                        eyre::bail!("tx not found: {:?}", tx_hash)
                    } else {
                        PendingTransactionBuilder::new(self.provider.root().clone(), tx_hash)
                            .with_required_confirmations(confs)
                            .with_timeout(timeout.map(Duration::from_secs))
                            .get_receipt()
                            .await?
                    }
                }
            }
            .into();

        // Allow to fail silently
        let _ = receipt.update_revert_reason(&self.provider).await;

        Ok(if let Some(ref field) = field {
            get_pretty_tx_receipt_attr(&receipt, field)
                .ok_or_else(|| eyre::eyre!("invalid receipt field: {}", field))?
        } else if shell::is_json() {
            // to_value first to sort json object keys
            serde_json::to_value(&receipt)?.to_string()
        } else {
            receipt.pretty()
        })
    }

    /// Perform a raw JSON-RPC request
    ///
    /// # Example
    ///
    /// ```
    /// use alloy_provider::{network::AnyNetwork, ProviderBuilder, RootProvider};
    /// use cast::Cast;
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider =
    ///     ProviderBuilder::<_, _, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;
    /// let cast = Cast::new(provider);
    /// let result = cast
    ///     .rpc("eth_getBalance", &["0xc94770007dda54cF92009BFF0dE90c06F603a09f", "latest"])
    ///     .await?;
    /// println!("{}", result);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn rpc<V>(&self, method: &str, params: V) -> Result<String>
    where
        V: alloy_json_rpc::RpcSend,
    {
        let res = self
            .provider
            .raw_request::<V, serde_json::Value>(Cow::Owned(method.to_string()), params)
            .await?;
        Ok(serde_json::to_string(&res)?)
    }

    /// Returns the slot
    ///
    /// # Example
    ///
    /// ```
    /// use alloy_primitives::{Address, B256};
    /// use alloy_provider::{network::AnyNetwork, ProviderBuilder, RootProvider};
    /// use cast::Cast;
    /// use std::str::FromStr;
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider =
    ///     ProviderBuilder::<_, _, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;
    /// let cast = Cast::new(provider);
    /// let addr = Address::from_str("0x00000000006c3852cbEf3e08E8dF289169EdE581")?;
    /// let slot = B256::ZERO;
    /// let storage = cast.storage(addr, slot, None).await?;
    /// println!("{}", storage);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn storage(
        &self,
        from: Address,
        slot: B256,
        block: Option<BlockId>,
    ) -> Result<String> {
        Ok(format!(
            "{:?}",
            B256::from(
                self.provider
                    .get_storage_at(from, slot.into())
                    .block_id(block.unwrap_or_default())
                    .await?
            )
        ))
    }

    pub async fn filter_logs(&self, filter: Filter) -> Result<String> {
        let logs = self.provider.get_logs(&filter).await?;

        let res = if shell::is_json() {
            serde_json::to_string(&logs)?
        } else {
            let mut s = vec![];
            for log in logs {
                let pretty = log
                    .pretty()
                    .replacen('\n', "- ", 1) // Remove empty first line
                    .replace('\n', "\n  "); // Indent
                s.push(pretty);
            }
            s.join("\n")
        };
        Ok(res)
    }

    /// Converts a block identifier into a block number.
    ///
    /// If the block identifier is a block number, then this function returns the block number. If
    /// the block identifier is a block hash, then this function returns the block number of
    /// that block hash. If the block identifier is `None`, then this function returns `None`.
    ///
    /// # Example
    ///
    /// ```
    /// use alloy_primitives::fixed_bytes;
    /// use alloy_provider::{network::AnyNetwork, ProviderBuilder, RootProvider};
    /// use alloy_rpc_types::{BlockId, BlockNumberOrTag};
    /// use cast::Cast;
    /// use std::{convert::TryFrom, str::FromStr};
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider =
    ///     ProviderBuilder::<_, _, AnyNetwork>::default().on_builtin("http://localhost:8545").await?;
    /// let cast = Cast::new(provider);
    ///
    /// let block_number = cast.convert_block_number(Some(BlockId::number(5))).await?;
    /// assert_eq!(block_number, Some(BlockNumberOrTag::Number(5)));
    ///
    /// let block_number = cast
    ///     .convert_block_number(Some(BlockId::hash(fixed_bytes!(
    ///         "0000000000000000000000000000000000000000000000000000000000001234"
    ///     ))))
    ///     .await?;
    /// assert_eq!(block_number, Some(BlockNumberOrTag::Number(4660)));
    ///
    /// let block_number = cast.convert_block_number(None).await?;
    /// assert_eq!(block_number, None);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn convert_block_number(
        &self,
        block: Option<BlockId>,
    ) -> Result<Option<BlockNumberOrTag>, eyre::Error> {
        match block {
            Some(block) => match block {
                BlockId::Number(block_number) => Ok(Some(block_number)),
                BlockId::Hash(hash) => {
                    let block =
                        self.provider.get_block_by_hash(hash.block_hash, false.into()).await?;
                    Ok(block.map(|block| block.header.number).map(BlockNumberOrTag::from))
                }
            },
            None => Ok(None),
        }
    }

    /// Sets up a subscription to the given filter and writes the logs to the given output.
    ///
    /// # Example
    ///
    /// ```
    /// use alloy_primitives::Address;
    /// use alloy_provider::{network::AnyNetwork, ProviderBuilder, RootProvider};
    /// use alloy_rpc_types::Filter;
    /// use alloy_transport::BoxTransport;
    /// use cast::Cast;
    /// use std::{io, str::FromStr};
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let provider =
    ///     ProviderBuilder::<_, _, AnyNetwork>::default().on_builtin("wss://localhost:8545").await?;
    /// let cast = Cast::new(provider);
    ///
    /// let filter =
    ///     Filter::new().address(Address::from_str("0x00000000006c3852cbEf3e08E8dF289169EdE581")?);
    /// let mut output = io::stdout();
    /// cast.subscribe(filter, &mut output).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn subscribe(&self, filter: Filter, output: &mut dyn io::Write) -> Result<()> {
        // Initialize the subscription stream for logs
        let mut subscription = self.provider.subscribe_logs(&filter).await?.into_stream();

        // Check if a to_block is specified, if so, subscribe to blocks
        let mut block_subscription = if filter.get_to_block().is_some() {
            Some(self.provider.subscribe_blocks().await?.into_stream())
        } else {
            None
        };

        let format_json = shell::is_json();
        let to_block_number = filter.get_to_block();

        // If output should be JSON, start with an opening bracket
        if format_json {
            write!(output, "[")?;
        }

        let mut first = true;

        loop {
            tokio::select! {
                // If block subscription is present, listen to it to avoid blocking indefinitely past the desired to_block
                block = if let Some(bs) = &mut block_subscription {
                    Either::Left(bs.next().fuse())
                } else {
                    Either::Right(futures::future::pending())
                } => {
                    if let (Some(block), Some(to_block)) = (block, to_block_number) {
                        if block.number  > to_block {
                            break;
                        }
                    }
                },
                // Process incoming log
                log = subscription.next() => {
                    if format_json {
                        if !first {
                            write!(output, ",")?;
                        }
                        first = false;
                        let log_str = serde_json::to_string(&log).unwrap();
                        write!(output, "{log_str}")?;
                    } else {
                        let log_str = log.pretty()
                            .replacen('\n', "- ", 1)  // Remove empty first line
                            .replace('\n', "\n  ");  // Indent
                        writeln!(output, "{log_str}")?;
                    }
                },
                // Break on cancel signal, to allow for closing JSON bracket
                _ = ctrl_c() => {
                    break;
                },
                else => break,
            }
        }

        // If output was JSON, end with a closing bracket
        if format_json {
            write!(output, "]")?;
        }

        Ok(())
    }

    pub async fn erc20_balance(
        &self,
        token: Address,
        owner: Address,
        block: Option<BlockId>,
    ) -> Result<U256> {
        Ok(IERC20::new(token, &self.provider)
            .balanceOf(owner)
            .block(block.unwrap_or_default())
            .call()
            .await?
            ._0)
    }
}

pub struct SimpleCast;

impl SimpleCast {
    /// Returns the maximum value of the given integer type
    ///
    /// # Example
    ///
    /// ```
    /// use alloy_primitives::{I256, U256};
    /// use cast::SimpleCast;
    ///
    /// assert_eq!(SimpleCast::max_int("uint256")?, U256::MAX.to_string());
    /// assert_eq!(SimpleCast::max_int("int256")?, I256::MAX.to_string());
    /// assert_eq!(SimpleCast::max_int("int32")?, i32::MAX.to_string());
    /// # Ok::<(), eyre::Report>(())
    /// ```
    pub fn max_int(s: &str) -> Result<String> {
        Self::max_min_int::<true>(s)
    }

    /// Returns the maximum value of the given integer type
    ///
    /// # Example
    ///
    /// ```
    /// use alloy_primitives::{I256, U256};
    /// use cast::SimpleCast;
    ///
    /// assert_eq!(SimpleCast::min_int("uint256")?, "0");
    /// assert_eq!(SimpleCast::min_int("int256")?, I256::MIN.to_string());
    /// assert_eq!(SimpleCast::min_int("int32")?, i32::MIN.to_string());
    /// # Ok::<(), eyre::Report>(())
    /// ```
    pub fn min_int(s: &str) -> Result<String> {
        Self::max_min_int::<false>(s)
    }

    fn max_min_int<const MAX: bool>(s: &str) -> Result<String> {
        let ty = DynSolType::parse(s).wrap_err("Invalid type, expected `(u)int<bit size>`")?;
        match ty {
            DynSolType::Int(n) => {
                let mask = U256::from(1).wrapping_shl(n - 1);
                let max = (U256::MAX & mask).saturating_sub(U256::from(1));
                if MAX {
                    Ok(max.to_string())
                } else {
                    let min = I256::from_raw(max).wrapping_neg() + I256::MINUS_ONE;
                    Ok(min.to_string())
                }
            }
            DynSolType::Uint(n) => {
                if MAX {
                    let mut max = U256::MAX;
                    if n < 255 {
                        max &= U256::from(1).wrapping_shl(n).wrapping_sub(U256::from(1));
                    }
                    Ok(max.to_string())
                } else {
                    Ok("0".to_string())
                }
            }
            _ => Err(eyre::eyre!("Type is not int/uint: {s}")),
        }
    }

    /// Converts UTF-8 text input to hex
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(Cast::from_utf8("yo"), "0x796f");
    /// assert_eq!(Cast::from_utf8("Hello, World!"), "0x48656c6c6f2c20576f726c6421");
    /// assert_eq!(Cast::from_utf8("TurboDappTools"), "0x547572626f44617070546f6f6c73");
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn from_utf8(s: &str) -> String {
        hex::encode_prefixed(s)
    }

    /// Converts hex input to UTF-8 text
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(Cast::to_utf8("0x796f")?, "yo");
    /// assert_eq!(Cast::to_utf8("0x48656c6c6f2c20576f726c6421")?, "Hello, World!");
    /// assert_eq!(Cast::to_utf8("0x547572626f44617070546f6f6c73")?, "TurboDappTools");
    /// assert_eq!(Cast::to_utf8("0xe4bda0e5a5bd")?, "你好");
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn to_utf8(s: &str) -> Result<String> {
        let bytes = hex::decode(s)?;
        Ok(String::from_utf8_lossy(bytes.as_ref()).to_string())
    }

    /// Converts hex data into text data
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(Cast::to_ascii("0x796f")?, "yo");
    /// assert_eq!(Cast::to_ascii("48656c6c6f2c20576f726c6421")?, "Hello, World!");
    /// assert_eq!(Cast::to_ascii("0x547572626f44617070546f6f6c73")?, "TurboDappTools");
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn to_ascii(hex: &str) -> Result<String> {
        let bytes = hex::decode(hex)?;
        if !bytes.iter().all(u8::is_ascii) {
            return Err(eyre::eyre!("Invalid ASCII bytes"));
        }
        Ok(String::from_utf8(bytes).unwrap())
    }

    /// Converts fixed point number into specified number of decimals
    /// ```
    /// use alloy_primitives::U256;
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(Cast::from_fixed_point("10", "0")?, "10");
    /// assert_eq!(Cast::from_fixed_point("1.0", "1")?, "10");
    /// assert_eq!(Cast::from_fixed_point("0.10", "2")?, "10");
    /// assert_eq!(Cast::from_fixed_point("0.010", "3")?, "10");
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn from_fixed_point(value: &str, decimals: &str) -> Result<String> {
        // TODO: https://github.com/alloy-rs/core/pull/461
        let units: Unit = if let Ok(x) = decimals.parse() {
            Unit::new(x).ok_or_else(|| eyre::eyre!("invalid unit"))?
        } else {
            decimals.parse()?
        };
        let n = ParseUnits::parse_units(value, units)?;
        Ok(n.to_string())
    }

    /// Converts integers with specified decimals into fixed point numbers
    ///
    /// # Example
    ///
    /// ```
    /// use alloy_primitives::U256;
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(Cast::to_fixed_point("10", "0")?, "10.");
    /// assert_eq!(Cast::to_fixed_point("10", "1")?, "1.0");
    /// assert_eq!(Cast::to_fixed_point("10", "2")?, "0.10");
    /// assert_eq!(Cast::to_fixed_point("10", "3")?, "0.010");
    ///
    /// assert_eq!(Cast::to_fixed_point("-10", "0")?, "-10.");
    /// assert_eq!(Cast::to_fixed_point("-10", "1")?, "-1.0");
    /// assert_eq!(Cast::to_fixed_point("-10", "2")?, "-0.10");
    /// assert_eq!(Cast::to_fixed_point("-10", "3")?, "-0.010");
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn to_fixed_point(value: &str, decimals: &str) -> Result<String> {
        let (sign, mut value, value_len) = {
            let number = NumberWithBase::parse_int(value, None)?;
            let sign = if number.is_nonnegative() { "" } else { "-" };
            let value = format!("{number:#}");
            let value_stripped = value.strip_prefix('-').unwrap_or(&value).to_string();
            let value_len = value_stripped.len();
            (sign, value_stripped, value_len)
        };
        let decimals = NumberWithBase::parse_uint(decimals, None)?.number().to::<usize>();

        let value = if decimals >= value_len {
            // Add "0." and pad with 0s
            format!("0.{value:0>decimals$}")
        } else {
            // Insert decimal at -idx (i.e 1 => decimal idx = -1)
            value.insert(value_len - decimals, '.');
            value
        };

        Ok(format!("{sign}{value}"))
    }

    /// Concatencates hex strings
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(Cast::concat_hex(["0x00", "0x01"]), "0x0001");
    /// assert_eq!(Cast::concat_hex(["1", "2"]), "0x12");
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn concat_hex<T: AsRef<str>>(values: impl IntoIterator<Item = T>) -> String {
        let mut out = String::new();
        for s in values {
            let s = s.as_ref();
            out.push_str(s.strip_prefix("0x").unwrap_or(s))
        }
        format!("0x{out}")
    }

    /// Converts a number into uint256 hex string with 0x prefix
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(
    ///     Cast::to_uint256("100")?,
    ///     "0x0000000000000000000000000000000000000000000000000000000000000064"
    /// );
    /// assert_eq!(
    ///     Cast::to_uint256("192038293923")?,
    ///     "0x0000000000000000000000000000000000000000000000000000002cb65fd1a3"
    /// );
    /// assert_eq!(
    ///     Cast::to_uint256(
    ///         "115792089237316195423570985008687907853269984665640564039457584007913129639935"
    ///     )?,
    ///     "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
    /// );
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn to_uint256(value: &str) -> Result<String> {
        let n = NumberWithBase::parse_uint(value, None)?;
        Ok(format!("{n:#066x}"))
    }

    /// Converts a number into int256 hex string with 0x prefix
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(
    ///     Cast::to_int256("0")?,
    ///     "0x0000000000000000000000000000000000000000000000000000000000000000"
    /// );
    /// assert_eq!(
    ///     Cast::to_int256("100")?,
    ///     "0x0000000000000000000000000000000000000000000000000000000000000064"
    /// );
    /// assert_eq!(
    ///     Cast::to_int256("-100")?,
    ///     "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff9c"
    /// );
    /// assert_eq!(
    ///     Cast::to_int256("192038293923")?,
    ///     "0x0000000000000000000000000000000000000000000000000000002cb65fd1a3"
    /// );
    /// assert_eq!(
    ///     Cast::to_int256("-192038293923")?,
    ///     "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffd349a02e5d"
    /// );
    /// assert_eq!(
    ///     Cast::to_int256(
    ///         "57896044618658097711785492504343953926634992332820282019728792003956564819967"
    ///     )?,
    ///     "0x7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
    /// );
    /// assert_eq!(
    ///     Cast::to_int256(
    ///         "-57896044618658097711785492504343953926634992332820282019728792003956564819968"
    ///     )?,
    ///     "0x8000000000000000000000000000000000000000000000000000000000000000"
    /// );
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn to_int256(value: &str) -> Result<String> {
        let n = NumberWithBase::parse_int(value, None)?;
        Ok(format!("{n:#066x}"))
    }

    /// Converts an eth amount into a specified unit
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(Cast::to_unit("1 wei", "wei")?, "1");
    /// assert_eq!(Cast::to_unit("1", "wei")?, "1");
    /// assert_eq!(Cast::to_unit("1ether", "wei")?, "1000000000000000000");
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn to_unit(value: &str, unit: &str) -> Result<String> {
        let value = DynSolType::coerce_str(&DynSolType::Uint(256), value)?
            .as_uint()
            .wrap_err("Could not convert to uint")?
            .0;
        let unit = unit.parse().wrap_err("could not parse units")?;
        Ok(Self::format_unit_as_string(value, unit))
    }

    /// Convert a number into a uint with arbitrary decimals.
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// # fn main() -> eyre::Result<()> {
    /// assert_eq!(Cast::parse_units("1.0", 6)?, "1000000"); // USDC (6 decimals)
    /// assert_eq!(Cast::parse_units("2.5", 6)?, "2500000");
    /// assert_eq!(Cast::parse_units("1.0", 12)?, "1000000000000"); // 12 decimals
    /// assert_eq!(Cast::parse_units("1.23", 3)?, "1230"); // 3 decimals
    /// # Ok(())
    /// # }
    /// ```
    pub fn parse_units(value: &str, unit: u8) -> Result<String> {
        let unit = Unit::new(unit).ok_or_else(|| eyre::eyre!("invalid unit"))?;

        Ok(ParseUnits::parse_units(value, unit)?.to_string())
    }

    /// Format a number from smallest unit to decimal with arbitrary decimals.
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// # fn main() -> eyre::Result<()> {
    /// assert_eq!(Cast::format_units("1000000", 6)?, "1"); // USDC (6 decimals)
    /// assert_eq!(Cast::format_units("2500000", 6)?, "2.500000");
    /// assert_eq!(Cast::format_units("1000000000000", 12)?, "1"); // 12 decimals
    /// assert_eq!(Cast::format_units("1230", 3)?, "1.230"); // 3 decimals
    /// # Ok(())
    /// # }
    /// ```
    pub fn format_units(value: &str, unit: u8) -> Result<String> {
        let value = NumberWithBase::parse_int(value, None)?.number();
        let unit = Unit::new(unit).ok_or_else(|| eyre::eyre!("invalid unit"))?;
        Ok(Self::format_unit_as_string(value, unit))
    }

    // Helper function to format units as a string
    fn format_unit_as_string(value: U256, unit: Unit) -> String {
        let mut formatted = ParseUnits::U256(value).format_units(unit);
        // Trim empty fractional part.
        if let Some(dot) = formatted.find('.') {
            let fractional = &formatted[dot + 1..];
            if fractional.chars().all(|c: char| c == '0') {
                formatted = formatted[..dot].to_string();
            }
        }
        formatted
    }

    /// Converts wei into an eth amount
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(Cast::from_wei("1", "gwei")?, "0.000000001");
    /// assert_eq!(Cast::from_wei("12340000005", "gwei")?, "12.340000005");
    /// assert_eq!(Cast::from_wei("10", "ether")?, "0.000000000000000010");
    /// assert_eq!(Cast::from_wei("100", "eth")?, "0.000000000000000100");
    /// assert_eq!(Cast::from_wei("17", "ether")?, "0.000000000000000017");
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn from_wei(value: &str, unit: &str) -> Result<String> {
        let value = NumberWithBase::parse_int(value, None)?.number();
        Ok(ParseUnits::U256(value).format_units(unit.parse()?))
    }

    /// Converts an eth amount into wei
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(Cast::to_wei("100", "gwei")?, "100000000000");
    /// assert_eq!(Cast::to_wei("100", "eth")?, "100000000000000000000");
    /// assert_eq!(Cast::to_wei("1000", "ether")?, "1000000000000000000000");
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn to_wei(value: &str, unit: &str) -> Result<String> {
        let unit = unit.parse().wrap_err("could not parse units")?;
        Ok(ParseUnits::parse_units(value, unit)?.to_string())
    }

    // Decodes RLP encoded data with validation for canonical integer representation
    ///
    /// # Examples
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(Cast::from_rlp("0xc0", false).unwrap(), "[]");
    /// assert_eq!(Cast::from_rlp("0x0f", false).unwrap(), "\"0x0f\"");
    /// assert_eq!(Cast::from_rlp("0x33", false).unwrap(), "\"0x33\"");
    /// assert_eq!(Cast::from_rlp("0xc161", false).unwrap(), "[\"0x61\"]");
    /// assert_eq!(Cast::from_rlp("820002", true).is_err(), true);
    /// assert_eq!(Cast::from_rlp("820002", false).unwrap(), "\"0x0002\"");
    /// assert_eq!(Cast::from_rlp("00", true).is_err(), true);
    /// assert_eq!(Cast::from_rlp("00", false).unwrap(), "\"0x00\"");
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn from_rlp(value: impl AsRef<str>, as_int: bool) -> Result<String> {
        let bytes = hex::decode(value.as_ref()).wrap_err("Could not decode hex")?;

        if as_int {
            return Ok(U256::decode(&mut &bytes[..])?.to_string());
        }

        let item = Item::decode(&mut &bytes[..]).wrap_err("Could not decode rlp")?;

        Ok(item.to_string())
    }

    /// Encodes hex data or list of hex data to hexadecimal rlp
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(Cast::to_rlp("[]").unwrap(), "0xc0".to_string());
    /// assert_eq!(Cast::to_rlp("0x22").unwrap(), "0x22".to_string());
    /// assert_eq!(Cast::to_rlp("[\"0x61\"]",).unwrap(), "0xc161".to_string());
    /// assert_eq!(Cast::to_rlp("[\"0xf1\", \"f2\"]").unwrap(), "0xc481f181f2".to_string());
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn to_rlp(value: &str) -> Result<String> {
        let val = serde_json::from_str(value)
            .unwrap_or_else(|_| serde_json::Value::String(value.to_string()));
        let item = Item::value_to_item(&val)?;
        Ok(format!("0x{}", hex::encode(alloy_rlp::encode(item))))
    }

    /// Converts a number of one base to another
    ///
    /// # Example
    ///
    /// ```
    /// use alloy_primitives::I256;
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(Cast::to_base("100", Some("10"), "16")?, "0x64");
    /// assert_eq!(Cast::to_base("100", Some("10"), "oct")?, "0o144");
    /// assert_eq!(Cast::to_base("100", Some("10"), "binary")?, "0b1100100");
    ///
    /// assert_eq!(Cast::to_base("0xffffffffffffffff", None, "10")?, u64::MAX.to_string());
    /// assert_eq!(
    ///     Cast::to_base("0xffffffffffffffffffffffffffffffff", None, "dec")?,
    ///     u128::MAX.to_string()
    /// );
    /// // U256::MAX overflows as internally it is being parsed as I256
    /// assert_eq!(
    ///     Cast::to_base(
    ///         "0x7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    ///         None,
    ///         "decimal"
    ///     )?,
    ///     I256::MAX.to_string()
    /// );
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn to_base(value: &str, base_in: Option<&str>, base_out: &str) -> Result<String> {
        let base_in = Base::unwrap_or_detect(base_in, value)?;
        let base_out: Base = base_out.parse()?;
        if base_in == base_out {
            return Ok(value.to_string());
        }

        let mut n = NumberWithBase::parse_int(value, Some(&base_in.to_string()))?;
        n.set_base(base_out);

        // Use Debug fmt
        Ok(format!("{n:#?}"))
    }

    /// Converts hexdata into bytes32 value
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// let bytes = Cast::to_bytes32("1234")?;
    /// assert_eq!(bytes, "0x1234000000000000000000000000000000000000000000000000000000000000");
    ///
    /// let bytes = Cast::to_bytes32("0x1234")?;
    /// assert_eq!(bytes, "0x1234000000000000000000000000000000000000000000000000000000000000");
    ///
    /// let err = Cast::to_bytes32("0x123400000000000000000000000000000000000000000000000000000000000011").unwrap_err();
    /// assert_eq!(err.to_string(), "string >32 bytes");
    /// # Ok::<_, eyre::Report>(())
    pub fn to_bytes32(s: &str) -> Result<String> {
        let s = strip_0x(s);
        if s.len() > 64 {
            eyre::bail!("string >32 bytes");
        }

        let padded = format!("{s:0<64}");
        Ok(padded.parse::<B256>()?.to_string())
    }

    /// Encodes string into bytes32 value
    pub fn format_bytes32_string(s: &str) -> Result<String> {
        let str_bytes: &[u8] = s.as_bytes();
        eyre::ensure!(str_bytes.len() <= 32, "bytes32 strings must not exceed 32 bytes in length");

        let mut bytes32: [u8; 32] = [0u8; 32];
        bytes32[..str_bytes.len()].copy_from_slice(str_bytes);
        Ok(hex::encode_prefixed(bytes32))
    }

    /// Decodes string from bytes32 value
    pub fn parse_bytes32_string(s: &str) -> Result<String> {
        let bytes = hex::decode(s)?;
        eyre::ensure!(bytes.len() == 32, "expected 32 byte hex-string");
        let len = bytes.iter().take_while(|x| **x != 0).count();
        Ok(std::str::from_utf8(&bytes[..len])?.into())
    }

    /// Decodes checksummed address from bytes32 value
    pub fn parse_bytes32_address(s: &str) -> Result<String> {
        let s = strip_0x(s);
        if s.len() != 64 {
            eyre::bail!("expected 64 byte hex-string, got {s}");
        }

        let s = if let Some(stripped) = s.strip_prefix("000000000000000000000000") {
            stripped
        } else {
            return Err(eyre::eyre!("Not convertible to address, there are non-zero bytes"));
        };

        let lowercase_address_string = format!("0x{s}");
        let lowercase_address = Address::from_str(&lowercase_address_string)?;

        Ok(lowercase_address.to_checksum(None))
    }

    /// Decodes abi-encoded hex input or output
    ///
    /// When `input=true`, `calldata` string MUST not be prefixed with function selector
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    /// use alloy_primitives::hex;
    ///
    ///     // Passing `input = false` will decode the data as the output type.
    ///     // The input data types and the full function sig are ignored, i.e.
    ///     // you could also pass `balanceOf()(uint256)` and it'd still work.
    ///     let data = "0x0000000000000000000000000000000000000000000000000000000000000001";
    ///     let sig = "balanceOf(address, uint256)(uint256)";
    ///     let decoded = Cast::abi_decode(sig, data, false)?[0].as_uint().unwrap().0.to_string();
    ///     assert_eq!(decoded, "1");
    ///
    ///     // Passing `input = true` will decode the data with the input function signature.
    ///     // We exclude the "prefixed" function selector from the data field (the first 4 bytes).
    ///     let data = "0x0000000000000000000000008dbd1b711dc621e1404633da156fcc779e1c6f3e000000000000000000000000d9f3c9cc99548bf3b44a43e0a2d07399eb918adc000000000000000000000000000000000000000000000000000000000000002a000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000";
    ///     let sig = "safeTransferFrom(address, address, uint256, uint256, bytes)";
    ///     let decoded = Cast::abi_decode(sig, data, true)?;
    ///     let decoded = [
    ///         decoded[0].as_address().unwrap().to_string().to_lowercase(),
    ///         decoded[1].as_address().unwrap().to_string().to_lowercase(),
    ///         decoded[2].as_uint().unwrap().0.to_string(),
    ///         decoded[3].as_uint().unwrap().0.to_string(),
    ///         hex::encode(decoded[4].as_bytes().unwrap())
    ///     ]
    ///     .into_iter()
    ///     .collect::<Vec<_>>();
    ///
    ///     assert_eq!(
    ///         decoded,
    ///         vec!["0x8dbd1b711dc621e1404633da156fcc779e1c6f3e", "0xd9f3c9cc99548bf3b44a43e0a2d07399eb918adc", "42", "1", ""]
    ///     );
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn abi_decode(sig: &str, calldata: &str, input: bool) -> Result<Vec<DynSolValue>> {
        foundry_common::abi::abi_decode_calldata(sig, calldata, input, false)
    }

    /// Decodes calldata-encoded hex input or output
    ///
    /// Similar to `abi_decode`, but `calldata` string MUST be prefixed with function selector
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    /// use alloy_primitives::hex;
    ///
    /// // Passing `input = false` will decode the data as the output type.
    /// // The input data types and the full function sig are ignored, i.e.
    /// // you could also pass `balanceOf()(uint256)` and it'd still work.
    /// let data = "0x0000000000000000000000000000000000000000000000000000000000000001";
    /// let sig = "balanceOf(address, uint256)(uint256)";
    /// let decoded = Cast::calldata_decode(sig, data, false)?[0].as_uint().unwrap().0.to_string();
    /// assert_eq!(decoded, "1");
    ///
    ///     // Passing `input = true` will decode the data with the input function signature.
    ///     let data = "0xf242432a0000000000000000000000008dbd1b711dc621e1404633da156fcc779e1c6f3e000000000000000000000000d9f3c9cc99548bf3b44a43e0a2d07399eb918adc000000000000000000000000000000000000000000000000000000000000002a000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000";
    ///     let sig = "safeTransferFrom(address, address, uint256, uint256, bytes)";
    ///     let decoded = Cast::calldata_decode(sig, data, true)?;
    ///     let decoded = [
    ///         decoded[0].as_address().unwrap().to_string().to_lowercase(),
    ///         decoded[1].as_address().unwrap().to_string().to_lowercase(),
    ///         decoded[2].as_uint().unwrap().0.to_string(),
    ///         decoded[3].as_uint().unwrap().0.to_string(),
    ///         hex::encode(decoded[4].as_bytes().unwrap()),
    ///    ]
    ///    .into_iter()
    ///    .collect::<Vec<_>>();
    ///     assert_eq!(
    ///         decoded,
    ///         vec!["0x8dbd1b711dc621e1404633da156fcc779e1c6f3e", "0xd9f3c9cc99548bf3b44a43e0a2d07399eb918adc", "42", "1", ""]
    ///     );
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn calldata_decode(sig: &str, calldata: &str, input: bool) -> Result<Vec<DynSolValue>> {
        foundry_common::abi::abi_decode_calldata(sig, calldata, input, true)
    }

    /// Performs ABI encoding based off of the function signature. Does not include
    /// the function selector in the result.
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(
    ///     "0x0000000000000000000000000000000000000000000000000000000000000001",
    ///     Cast::abi_encode("f(uint a)", &["1"]).unwrap().as_str()
    /// );
    /// assert_eq!(
    ///     "0x0000000000000000000000000000000000000000000000000000000000000001",
    ///     Cast::abi_encode("constructor(uint a)", &["1"]).unwrap().as_str()
    /// );
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn abi_encode(sig: &str, args: &[impl AsRef<str>]) -> Result<String> {
        let func = get_func(sig)?;
        match encode_function_args(&func, args) {
            Ok(res) => Ok(hex::encode_prefixed(&res[4..])),
            Err(e) => eyre::bail!("Could not ABI encode the function and arguments. Did you pass in the right types?\nError\n{}", e),
        }
    }

    /// Performs packed ABI encoding based off of the function signature or tuple.
    ///
    /// # Examplez
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(
    ///     "0x0000000000000000000000000000000000000000000000000000000000000064000000000000000000000000000000000000000000000000000000000000012c00000000000000c8",
    ///     Cast::abi_encode_packed("(uint128[] a, uint64 b)", &["[100, 300]", "200"]).unwrap().as_str()
    /// );
    ///
    /// assert_eq!(
    ///     "0x8dbd1b711dc621e1404633da156fcc779e1c6f3e68656c6c6f20776f726c64",
    ///     Cast::abi_encode_packed("foo(address a, string b)", &["0x8dbd1b711dc621e1404633da156fcc779e1c6f3e", "hello world"]).unwrap().as_str()
    /// );
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn abi_encode_packed(sig: &str, args: &[impl AsRef<str>]) -> Result<String> {
        // If the signature is a tuple, we need to prefix it to make it a function
        let sig =
            if sig.trim_start().starts_with('(') { format!("foo{sig}") } else { sig.to_string() };

        let func = get_func(sig.as_str())?;
        let encoded = match encode_function_args_packed(&func, args) {
            Ok(res) => hex::encode(res),
            Err(e) => eyre::bail!("Could not ABI encode the function and arguments. Did you pass in the right types?\nError\n{}", e),
        };
        Ok(format!("0x{encoded}"))
    }

    /// Performs ABI encoding to produce the hexadecimal calldata with the given arguments.
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(
    ///     "0xb3de648b0000000000000000000000000000000000000000000000000000000000000001",
    ///     Cast::calldata_encode("f(uint256 a)", &["1"]).unwrap().as_str()
    /// );
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn calldata_encode(sig: impl AsRef<str>, args: &[impl AsRef<str>]) -> Result<String> {
        let func = get_func(sig.as_ref())?;
        let calldata = encode_function_args(&func, args)?;
        Ok(hex::encode_prefixed(calldata))
    }

    /// Returns the slot number for a given mapping key and slot.
    ///
    /// Given `mapping(k => v) m`, for a key `k` the slot number of its associated `v` is
    /// `keccak256(concat(h(k), p))`, where `h` is the padding function for `k`'s type, and `p`
    /// is slot number of the mapping `m`.
    ///
    /// See [the Solidity documentation](https://docs.soliditylang.org/en/latest/internals/layout_in_storage.html#mappings-and-dynamic-arrays)
    /// for more details.
    ///
    /// # Example
    ///
    /// ```
    /// # use cast::SimpleCast as Cast;
    ///
    /// // Value types.
    /// assert_eq!(
    ///     Cast::index("address", "0xD0074F4E6490ae3f888d1d4f7E3E43326bD3f0f5", "2").unwrap().as_str(),
    ///     "0x9525a448a9000053a4d151336329d6563b7e80b24f8e628e95527f218e8ab5fb"
    /// );
    /// assert_eq!(
    ///     Cast::index("uint256", "42", "6").unwrap().as_str(),
    ///     "0xfc808b0f31a1e6b9cf25ff6289feae9b51017b392cc8e25620a94a38dcdafcc1"
    /// );
    ///
    /// // Strings and byte arrays.
    /// assert_eq!(
    ///     Cast::index("string", "hello", "1").unwrap().as_str(),
    ///     "0x8404bb4d805e9ca2bd5dd5c43a107e935c8ec393caa7851b353b3192cd5379ae"
    /// );
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn index(key_type: &str, key: &str, slot_number: &str) -> Result<String> {
        let mut hasher = Keccak256::new();

        let k_ty = DynSolType::parse(key_type).wrap_err("Could not parse type")?;
        let k = k_ty.coerce_str(key).wrap_err("Could not parse value")?;
        match k_ty {
            // For value types, `h` pads the value to 32 bytes in the same way as when storing the
            // value in memory.
            DynSolType::Bool |
            DynSolType::Int(_) |
            DynSolType::Uint(_) |
            DynSolType::FixedBytes(_) |
            DynSolType::Address |
            DynSolType::Function => hasher.update(k.as_word().unwrap()),

            // For strings and byte arrays, `h(k)` is just the unpadded data.
            DynSolType::String | DynSolType::Bytes => hasher.update(k.as_packed_seq().unwrap()),

            DynSolType::Array(..) |
            DynSolType::FixedArray(..) |
            DynSolType::Tuple(..) |
            DynSolType::CustomStruct { .. } => {
                eyre::bail!("Type `{k_ty}` is not supported as a mapping key")
            }
        }

        let p = DynSolType::Uint(256)
            .coerce_str(slot_number)
            .wrap_err("Could not parse slot number")?;
        let p = p.as_word().unwrap();
        hasher.update(p);

        let location = hasher.finalize();
        Ok(location.to_string())
    }

    /// Keccak-256 hashes arbitrary data
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(
    ///     Cast::keccak("foo")?,
    ///     "0x41b1a0649752af1b28b3dc29a1556eee781e4a4c3a1f7f53f90fa834de098c4d"
    /// );
    /// assert_eq!(
    ///     Cast::keccak("123abc")?,
    ///     "0xb1f1c74a1ba56f07a892ea1110a39349d40f66ca01d245e704621033cb7046a4"
    /// );
    /// assert_eq!(
    ///     Cast::keccak("0x12")?,
    ///     "0x5fa2358263196dbbf23d1ca7a509451f7a2f64c15837bfbb81298b1e3e24e4fa"
    /// );
    /// assert_eq!(
    ///     Cast::keccak("12")?,
    ///     "0x7f8b6b088b6d74c2852fc86c796dca07b44eed6fb3daf5e6b59f7c364db14528"
    /// );
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn keccak(data: &str) -> Result<String> {
        // Hex-decode if data starts with 0x.
        let hash =
            if data.starts_with("0x") { keccak256(hex::decode(data)?) } else { keccak256(data) };
        Ok(hash.to_string())
    }

    /// Performs the left shift operation (<<) on a number
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(Cast::left_shift("16", "10", Some("10"), "hex")?, "0x4000");
    /// assert_eq!(Cast::left_shift("255", "16", Some("dec"), "hex")?, "0xff0000");
    /// assert_eq!(Cast::left_shift("0xff", "16", None, "hex")?, "0xff0000");
    /// # Ok::<_, eyre::Report>(())
    /// ```
    pub fn left_shift(
        value: &str,
        bits: &str,
        base_in: Option<&str>,
        base_out: &str,
    ) -> Result<String> {
        let base_out: Base = base_out.parse()?;
        let value = NumberWithBase::parse_uint(value, base_in)?;
        let bits = NumberWithBase::parse_uint(bits, None)?;

        let res = value.number() << bits.number();

        Ok(res.to_base(base_out, true)?)
    }

    /// Performs the right shift operation (>>) on a number
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(Cast::right_shift("0x4000", "10", None, "dec")?, "16");
    /// assert_eq!(Cast::right_shift("16711680", "16", Some("10"), "hex")?, "0xff");
    /// assert_eq!(Cast::right_shift("0xff0000", "16", None, "hex")?, "0xff");
    /// # Ok::<(), eyre::Report>(())
    /// ```
    pub fn right_shift(
        value: &str,
        bits: &str,
        base_in: Option<&str>,
        base_out: &str,
    ) -> Result<String> {
        let base_out: Base = base_out.parse()?;
        let value = NumberWithBase::parse_uint(value, base_in)?;
        let bits = NumberWithBase::parse_uint(bits, None)?;

        let res = value.number().wrapping_shr(bits.number().saturating_to());

        Ok(res.to_base(base_out, true)?)
    }

    /// Fetches source code of verified contracts from etherscan.
    ///
    /// # Example
    ///
    /// ```
    /// # use cast::SimpleCast as Cast;
    /// # use foundry_config::NamedChain;
    /// # async fn foo() -> eyre::Result<()> {
    /// assert_eq!(
    ///     "/*
    ///             - Bytecode Verification performed was compared on second iteration -
    ///             This file is part of the DAO.....",
    ///     Cast::etherscan_source(
    ///         NamedChain::Mainnet.into(),
    ///         "0xBB9bc244D798123fDe783fCc1C72d3Bb8C189413".to_string(),
    ///         Some("<etherscan_api_key>".to_string()),
    ///         None,
    ///         None
    ///     )
    ///     .await
    ///     .unwrap()
    ///     .as_str()
    /// );
    /// # Ok(())
    /// # }
    /// ```
    pub async fn etherscan_source(
        chain: Chain,
        contract_address: String,
        etherscan_api_key: Option<String>,
        explorer_api_url: Option<String>,
        explorer_url: Option<String>,
    ) -> Result<String> {
        let client = explorer_client(chain, etherscan_api_key, explorer_api_url, explorer_url)?;
        let metadata = client.contract_source_code(contract_address.parse()?).await?;
        Ok(metadata.source_code())
    }

    /// Fetches the source code of verified contracts from etherscan and expands the resulting
    /// files to a directory for easy perusal.
    ///
    /// # Example
    ///
    /// ```
    /// # use cast::SimpleCast as Cast;
    /// # use foundry_config::NamedChain;
    /// # use std::path::PathBuf;
    /// # async fn expand() -> eyre::Result<()> {
    /// Cast::expand_etherscan_source_to_directory(
    ///     NamedChain::Mainnet.into(),
    ///     "0xBB9bc244D798123fDe783fCc1C72d3Bb8C189413".to_string(),
    ///     Some("<etherscan_api_key>".to_string()),
    ///     PathBuf::from("output_dir"),
    ///     None,
    ///     None,
    /// )
    /// .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn expand_etherscan_source_to_directory(
        chain: Chain,
        contract_address: String,
        etherscan_api_key: Option<String>,
        output_directory: PathBuf,
        explorer_api_url: Option<String>,
        explorer_url: Option<String>,
    ) -> eyre::Result<()> {
        let client = explorer_client(chain, etherscan_api_key, explorer_api_url, explorer_url)?;
        let meta = client.contract_source_code(contract_address.parse()?).await?;
        let source_tree = meta.source_tree();
        source_tree.write_to(&output_directory)?;
        Ok(())
    }

    /// Fetches the source code of verified contracts from etherscan, flattens it and writes it to
    /// the given path or stdout.
    pub async fn etherscan_source_flatten(
        chain: Chain,
        contract_address: String,
        etherscan_api_key: Option<String>,
        output_path: Option<PathBuf>,
        explorer_api_url: Option<String>,
        explorer_url: Option<String>,
    ) -> Result<()> {
        let client = explorer_client(chain, etherscan_api_key, explorer_api_url, explorer_url)?;
        let metadata = client.contract_source_code(contract_address.parse()?).await?;
        let Some(metadata) = metadata.items.first() else {
            eyre::bail!("Empty contract source code")
        };

        let tmp = tempfile::tempdir()?;
        let project = etherscan_project(metadata, tmp.path())?;
        let target_path = project.find_contract_path(&metadata.contract_name)?;

        let flattened = Flattener::new(project, &target_path)?.flatten();

        if let Some(path) = output_path {
            fs::create_dir_all(path.parent().unwrap())?;
            fs::write(&path, flattened)?;
            sh_println!("Flattened file written at {}", path.display())?
        } else {
            sh_println!("{flattened}")?
        }

        Ok(())
    }

    /// Disassembles hex encoded bytecode into individual / human readable opcodes
    ///
    /// # Example
    ///
    /// ```
    /// use alloy_primitives::hex;
    /// use cast::SimpleCast as Cast;
    ///
    /// # async fn foo() -> eyre::Result<()> {
    /// let bytecode = "0x608060405260043610603f57600035";
    /// let opcodes = Cast::disassemble(&hex::decode(bytecode)?)?;
    /// println!("{}", opcodes);
    /// # Ok(())
    /// # }
    /// ```
    pub fn disassemble(code: &[u8]) -> Result<String> {
        let mut output = String::new();

        for step in decode_instructions(code)? {
            write!(output, "{:08x}: ", step.pc)?;

            if let Some(op) = step.op {
                write!(output, "{op}")?;
            } else {
                write!(output, "INVALID")?;
            }

            if !step.immediate.is_empty() {
                write!(output, " {}", hex::encode_prefixed(step.immediate))?;
            }

            writeln!(output)?;
        }

        Ok(output)
    }

    /// Gets the selector for a given function signature
    /// Optimizes if the `optimize` parameter is set to a number of leading zeroes
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// assert_eq!(Cast::get_selector("foo(address,uint256)", 0)?.0, String::from("0xbd0d639f"));
    /// # Ok::<(), eyre::Error>(())
    /// ```
    pub fn get_selector(signature: &str, optimize: usize) -> Result<(String, String)> {
        if optimize > 4 {
            eyre::bail!("number of leading zeroes must not be greater than 4");
        }
        if optimize == 0 {
            let selector = get_func(signature)?.selector();
            return Ok((selector.to_string(), String::from(signature)));
        }
        let Some((name, params)) = signature.split_once('(') else {
            eyre::bail!("invalid function signature");
        };

        let num_threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        let found = AtomicBool::new(false);

        let result: Option<(u32, String, String)> =
            (0..num_threads).into_par_iter().find_map_any(|i| {
                let nonce_start = i as u32;
                let nonce_step = num_threads as u32;

                let mut nonce = nonce_start;
                while nonce < u32::MAX && !found.load(Ordering::Relaxed) {
                    let input = format!("{name}{nonce}({params}");
                    let hash = keccak256(input.as_bytes());
                    let selector = &hash[..4];

                    if selector.iter().take_while(|&&byte| byte == 0).count() == optimize {
                        found.store(true, Ordering::Relaxed);
                        return Some((nonce, hex::encode_prefixed(selector), input));
                    }

                    nonce += nonce_step;
                }
                None
            });

        match result {
            Some((_nonce, selector, signature)) => Ok((selector, signature)),
            None => eyre::bail!("No selector found"),
        }
    }

    /// Extracts function selectors, arguments and state mutability from bytecode
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// let bytecode = "6080604052348015600e575f80fd5b50600436106026575f3560e01c80632125b65b14602a575b5f80fd5b603a6035366004603c565b505050565b005b5f805f60608486031215604d575f80fd5b833563ffffffff81168114605f575f80fd5b925060208401356001600160a01b03811681146079575f80fd5b915060408401356001600160e01b03811681146093575f80fd5b80915050925092509256";
    /// let functions = Cast::extract_functions(bytecode)?;
    /// assert_eq!(functions, vec![("0x2125b65b".to_string(), "uint32,address,uint224".to_string(), "pure")]);
    /// # Ok::<(), eyre::Report>(())
    /// ```
    pub fn extract_functions(bytecode: &str) -> Result<Vec<(String, String, &str)>> {
        let code = hex::decode(strip_0x(bytecode))?;
        let info = evmole::contract_info(
            evmole::ContractInfoArgs::new(&code)
                .with_selectors()
                .with_arguments()
                .with_state_mutability(),
        );
        Ok(info
            .functions
            .expect("functions extraction was requested")
            .into_iter()
            .map(|f| {
                (
                    hex::encode_prefixed(f.selector),
                    f.arguments
                        .expect("arguments extraction was requested")
                        .into_iter()
                        .map(|t| t.sol_type_name().to_string())
                        .collect::<Vec<String>>()
                        .join(","),
                    f.state_mutability
                        .expect("state_mutability extraction was requested")
                        .as_json_str(),
                )
            })
            .collect())
    }

    /// Decodes a raw EIP2718 transaction payload
    /// Returns details about the typed transaction and ECSDA signature components
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// let tx = "0x02f8f582a86a82058d8459682f008508351050808303fd84948e42f2f4101563bf679975178e880fd87d3efd4e80b884659ac74b00000000000000000000000080f0c1c49891dcfdd40b6e0f960f84e6042bcb6f000000000000000000000000b97ef9ef8734c71904d8002f8b6bc66dd9c48a6e00000000000000000000000000000000000000000000000000000000007ff4e20000000000000000000000000000000000000000000000000000000000000064c001a05d429597befe2835396206781b199122f2e8297327ed4a05483339e7a8b2022aa04c23a7f70fb29dda1b4ee342fb10a625e9b8ddc6a603fb4e170d4f6f37700cb8";
    /// let tx_envelope = Cast::decode_raw_transaction(&tx)?;
    /// # Ok::<(), eyre::Report>(())
    pub fn decode_raw_transaction(tx: &str) -> Result<TxEnvelope> {
        let tx_hex = hex::decode(strip_0x(tx))?;
        let tx = TxEnvelope::decode_2718(&mut tx_hex.as_slice())?;
        Ok(tx)
    }

    /// Decodes EOF container bytes
    /// Pretty prints the decoded EOF container contents
    ///
    /// # Example
    ///
    /// ```
    /// use cast::SimpleCast as Cast;
    ///
    /// let eof = "0xef0001010004020001005604002000008000046080806040526004361015e100035f80fd5f3560e01c63773d45e01415e1ffee6040600319360112e10028600435906024358201809211e100066020918152f3634e487b7160e01b5f52601160045260245ffd5f80fd0000000000000000000000000124189fc71496f8660db5189f296055ed757632";
    /// let decoded = Cast::decode_eof(&eof)?;
    /// println!("{}", decoded);
    /// # Ok::<(), eyre::Report>(())
    pub fn decode_eof(eof: &str) -> Result<String> {
        let eof_hex = hex::decode(eof)?;
        let eof = Eof::decode(eof_hex.into())?;
        Ok(pretty_eof(&eof)?)
    }
}

fn strip_0x(s: &str) -> &str {
    s.strip_prefix("0x").unwrap_or(s)
}

fn explorer_client(
    chain: Chain,
    api_key: Option<String>,
    api_url: Option<String>,
    explorer_url: Option<String>,
) -> Result<Client> {
    let mut builder = Client::builder().with_chain_id(chain);

    let deduced = chain.etherscan_urls();

    let explorer_url = explorer_url
        .or(deduced.map(|d| d.1.to_string()))
        .ok_or_eyre("Please provide the explorer browser URL using `--explorer-url`")?;
    builder = builder.with_url(explorer_url)?;

    let api_url = api_url
        .or(deduced.map(|d| d.0.to_string()))
        .ok_or_eyre("Please provide the explorer API URL using `--explorer-api-url`")?;
    builder = builder.with_api_url(api_url)?;

    if let Some(api_key) = api_key {
        builder = builder.with_api_key(api_key);
    }

    builder.build().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::SimpleCast as Cast;
    use alloy_primitives::hex;
    use alloy_signer::k256::{elliptic_curve::bigint::Integer, sha2::digest::typenum::uint};
    use itertools::Itertools;

    #[test]
    fn simple_selector() {
        assert_eq!("0xc2985578", Cast::get_selector("foo()", 0).unwrap().0.as_str())
    }

    #[test]
    fn selector_with_arg() {
        assert_eq!("0xbd0d639f", Cast::get_selector("foo(address,uint256)", 0).unwrap().0.as_str())
    }

    #[test]
    fn calldata_uint() {
        assert_eq!(
            "0xb3de648b0000000000000000000000000000000000000000000000000000000000000001",
            Cast::calldata_encode("f(uint256 a)", &["1"]).unwrap().as_str()
        );
    }

    // <https://github.com/foundry-rs/foundry/issues/2681>
    #[test]
    fn calldata_array() {
        assert_eq!(
            "0xcde2baba0000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000000",
            Cast::calldata_encode("propose(string[])", &["[\"\"]"]).unwrap().as_str()
        );
    }

    #[test]
    fn calldata_bool() {
        assert_eq!(
            "0x6fae94120000000000000000000000000000000000000000000000000000000000000000",
            Cast::calldata_encode("bar(bool)", &["false"]).unwrap().as_str()
        );
    }

    #[test]
    fn abi_decode() {
        let data = "0x0000000000000000000000000000000000000000000000000000000000000001";
        let sig = "balanceOf(address, uint256)(uint256)";
        assert_eq!(
            "1",
            Cast::abi_decode(sig, data, false).unwrap()[0].as_uint().unwrap().0.to_string()
        );

        let data = "0x0000000000000000000000008dbd1b711dc621e1404633da156fcc779e1c6f3e000000000000000000000000d9f3c9cc99548bf3b44a43e0a2d07399eb918adc000000000000000000000000000000000000000000000000000000000000002a000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000";
        let sig = "safeTransferFrom(address,address,uint256,uint256,bytes)";
        let decoded = Cast::abi_decode(sig, data, true).unwrap();
        let decoded = [
            decoded[0]
                .as_address()
                .unwrap()
                .to_string()
                .strip_prefix("0x")
                .unwrap()
                .to_owned()
                .to_lowercase(),
            decoded[1]
                .as_address()
                .unwrap()
                .to_string()
                .strip_prefix("0x")
                .unwrap()
                .to_owned()
                .to_lowercase(),
            decoded[2].as_uint().unwrap().0.to_string(),
            decoded[3].as_uint().unwrap().0.to_string(),
            hex::encode(decoded[4].as_bytes().unwrap()),
        ]
        .to_vec();
        assert_eq!(
            decoded,
            vec![
                "8dbd1b711dc621e1404633da156fcc779e1c6f3e",
                "d9f3c9cc99548bf3b44a43e0a2d07399eb918adc",
                "42",
                "1",
                ""
            ]
        );
    }

    #[test]
    fn calldata_decode() {
        let data = "0x0000000000000000000000000000000000000000000000000000000000000001";
        let sig = "balanceOf(address, uint256)(uint256)";
        let decoded =
            Cast::calldata_decode(sig, data, false).unwrap()[0].as_uint().unwrap().0.to_string();
        assert_eq!(decoded, "1");

        // Passing `input = true` will decode the data with the input function signature.
        // We exclude the "prefixed" function selector from the data field (the first 4 bytes).
        let data = "0xf242432a0000000000000000000000008dbd1b711dc621e1404633da156fcc779e1c6f3e000000000000000000000000d9f3c9cc99548bf3b44a43e0a2d07399eb918adc000000000000000000000000000000000000000000000000000000000000002a000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000";
        let sig = "safeTransferFrom(address, address, uint256, uint256, bytes)";
        let decoded = Cast::calldata_decode(sig, data, true).unwrap();
        let decoded = [
            decoded[0].as_address().unwrap().to_string().to_lowercase(),
            decoded[1].as_address().unwrap().to_string().to_lowercase(),
            decoded[2].as_uint().unwrap().0.to_string(),
            decoded[3].as_uint().unwrap().0.to_string(),
            hex::encode(decoded[4].as_bytes().unwrap()),
        ]
        .into_iter()
        .collect::<Vec<_>>();
        assert_eq!(
            decoded,
            vec![
                "0x8dbd1b711dc621e1404633da156fcc779e1c6f3e",
                "0xd9f3c9cc99548bf3b44a43e0a2d07399eb918adc",
                "42",
                "1",
                ""
            ]
        );
    }

    #[test]
    fn calldata_decode_array_param() {
        let data = "0xdb5b0ed700000000000000000000000000000000000000000000000000000000000000a0000000000000000000000000000000000000000000000000000000006772bf190000000000000000000000000000000000000000000000000000000000020716000000000000000000000000af9d27ffe4d51ed54ac8eec78f2785d7e11e5ab100000000000000000000000000000000000000000000000000000000000002c0000000000000000000000000000000000000000000000000000000000000000404366a6dc4b2f348a85e0066e46f0cc206fca6512e0ed7f17ca7afb88e9a4c27000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000093922dee6e380c28a50c008ab167b7800bb24c2026cd1b22f1c6fb884ceed7400000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000060f85e59ecad6c1a6be343a945abedb7d5b5bfad7817c4d8cc668da7d391faf700000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000093dfbf04395fbec1f1aed4ad0f9d3ba880ff58a60485df5d33f8f5e0fb73188600000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000aa334a426ea9e21d5f84eb2d4723ca56b92382b9260ab2b6769b7c23d437b6b512322a25cecc954127e60cf91ef056ac1da25f90b73be81c3ff1872fa48d10c7ef1ccb4087bbeedb54b1417a24abbb76f6cd57010a65bb03c7b6602b1eaf0e32c67c54168232d4edc0bfa1b815b2af2a2d0a5c109d675a4f2de684e51df9abb324ab1b19a81bac80f9ce3a45095f3df3a7cf69ef18fc08e94ac3cbc1c7effeacca68e3bfe5d81e26a659b500000000000000000000000000000000000000000000";
        let sig = "sequenceBatchesValidium((bytes32,bytes32,uint64,bytes32)[],uint64,uint64,address,bytes)";
        let decoded = Cast::calldata_decode(sig, data, true).unwrap();
        let decoded = (
            (
                (
                    fixed_bytes_to_hex(
                        decoded[0].as_array().unwrap()[0].as_tuple().unwrap()[0].as_fixed_bytes(),
                    ),
                    fixed_bytes_to_hex(
                        decoded[0].as_array().unwrap()[0].as_tuple().unwrap()[1].as_fixed_bytes(),
                    ),
                    decoded[0].as_array().unwrap()[0].as_tuple().unwrap()[2]
                        .as_uint()
                        .unwrap()
                        .0
                        .to_string(),
                    fixed_bytes_to_hex(
                        decoded[0].as_array().unwrap()[0].as_tuple().unwrap()[3].as_fixed_bytes(),
                    ),
                ),
                (
                    fixed_bytes_to_hex(
                        decoded[0].as_array().unwrap()[1].as_tuple().unwrap()[0].as_fixed_bytes(),
                    ),
                    fixed_bytes_to_hex(
                        decoded[0].as_array().unwrap()[1].as_tuple().unwrap()[1].as_fixed_bytes(),
                    ),
                    decoded[0].as_array().unwrap()[1].as_tuple().unwrap()[2]
                        .as_uint()
                        .unwrap()
                        .0
                        .to_string(),
                    fixed_bytes_to_hex(
                        decoded[0].as_array().unwrap()[1].as_tuple().unwrap()[3].as_fixed_bytes(),
                    ),
                ),
                (
                    fixed_bytes_to_hex(
                        decoded[0].as_array().unwrap()[2].as_tuple().unwrap()[0].as_fixed_bytes(),
                    ),
                    fixed_bytes_to_hex(
                        decoded[0].as_array().unwrap()[2].as_tuple().unwrap()[1].as_fixed_bytes(),
                    ),
                    decoded[0].as_array().unwrap()[2].as_tuple().unwrap()[2]
                        .as_uint()
                        .unwrap()
                        .0
                        .to_string(),
                    fixed_bytes_to_hex(
                        decoded[0].as_array().unwrap()[2].as_tuple().unwrap()[3].as_fixed_bytes(),
                    ),
                ),
                (
                    fixed_bytes_to_hex(
                        decoded[0].as_array().unwrap()[3].as_tuple().unwrap()[0].as_fixed_bytes(),
                    ),
                    fixed_bytes_to_hex(
                        decoded[0].as_array().unwrap()[3].as_tuple().unwrap()[1].as_fixed_bytes(),
                    ),
                    decoded[0].as_array().unwrap()[3].as_tuple().unwrap()[2]
                        .as_uint()
                        .unwrap()
                        .0
                        .to_string(),
                    fixed_bytes_to_hex(
                        decoded[0].as_array().unwrap()[3].as_tuple().unwrap()[3].as_fixed_bytes(),
                    ),
                ),
            ),
            decoded[1].as_uint().unwrap().0.to_string(),
            decoded[2].as_uint().unwrap().0.to_string(),
            decoded[3].as_address().unwrap().to_string().to_lowercase(),
            format!("0x{}", hex::encode(decoded[4].as_bytes().unwrap())),
        );
        assert_eq!(
            decoded,
            (
                (
                  (
                    "0x04366a6dc4b2f348a85e0066e46f0cc206fca6512e0ed7f17ca7afb88e9a4c27".to_string(),
                    "0x0000000000000000000000000000000000000000000000000000000000000000".to_string(),
                    "0".to_string(),
                    "0x0000000000000000000000000000000000000000000000000000000000000000".to_string()
                  ),
                  (
                    "0x093922dee6e380c28a50c008ab167b7800bb24c2026cd1b22f1c6fb884ceed74".to_string(),
                    "0x0000000000000000000000000000000000000000000000000000000000000000".to_string(),
                    "0".to_string(),
                    "0x0000000000000000000000000000000000000000000000000000000000000000".to_string()
                  ),
                  (
                    "0x60f85e59ecad6c1a6be343a945abedb7d5b5bfad7817c4d8cc668da7d391faf7".to_string(),
                    "0x0000000000000000000000000000000000000000000000000000000000000000".to_string(),
                    "0".to_string(),
                    "0x0000000000000000000000000000000000000000000000000000000000000000".to_string()
                  ),
                  (
                    "0x93dfbf04395fbec1f1aed4ad0f9d3ba880ff58a60485df5d33f8f5e0fb731886".to_string(),
                    "0x0000000000000000000000000000000000000000000000000000000000000000".to_string(),
                    "0".to_string(),
                    "0x0000000000000000000000000000000000000000000000000000000000000000".to_string()
                  )
                ),
                "1735573273".to_string(),
                "132886".to_string(),
                "0xAF9d27ffe4d51eD54AC8eEc78f2785D7E11E5ab1".to_string().to_lowercase(),
                "0x334a426ea9e21d5f84eb2d4723ca56b92382b9260ab2b6769b7c23d437b6b512322a25cecc954127e60cf91ef056ac1da25f90b73be81c3ff1872fa48d10c7ef1ccb4087bbeedb54b1417a24abbb76f6cd57010a65bb03c7b6602b1eaf0e32c67c54168232d4edc0bfa1b815b2af2a2d0a5c109d675a4f2de684e51df9abb324ab1b19a81bac80f9ce3a45095f3df3a7cf69ef18fc08e94ac3cbc1c7effeacca68e3bfe5d81e26a659b5".to_string().to_lowercase()
            )
        );
    }

    fn fixed_bytes_to_hex(fixed_bytes: Option<(&[u8], usize)>) -> String {
        let fixed_bytes_as_slice = hex::encode(fixed_bytes.unwrap().0.iter().as_slice());
        let fixed_bytes_hex = format!("0x{}", fixed_bytes_as_slice);
        fixed_bytes_hex
    }

    #[test]
    fn concat_hex() {
        assert_eq!(Cast::concat_hex(["0x00", "0x01"]), "0x0001");
        assert_eq!(Cast::concat_hex(["1", "2"]), "0x12");
    }

    #[test]
    fn from_rlp() {
        let rlp = "0xf8b1a02b5df5f0757397573e8ff34a8b987b21680357de1f6c8d10273aa528a851eaca8080a02838ac1d2d2721ba883169179b48480b2ba4f43d70fcf806956746bd9e83f90380a0e46fff283b0ab96a32a7cc375cecc3ed7b6303a43d64e0a12eceb0bc6bd8754980a01d818c1c414c665a9c9a0e0c0ef1ef87cacb380b8c1f6223cb2a68a4b2d023f5808080a0236e8f61ecde6abfebc6c529441f782f62469d8a2cc47b7aace2c136bd3b1ff08080808080";
        let item = Cast::from_rlp(rlp, false).unwrap();
        assert_eq!(
            item,
            r#"["0x2b5df5f0757397573e8ff34a8b987b21680357de1f6c8d10273aa528a851eaca","0x","0x","0x2838ac1d2d2721ba883169179b48480b2ba4f43d70fcf806956746bd9e83f903","0x","0xe46fff283b0ab96a32a7cc375cecc3ed7b6303a43d64e0a12eceb0bc6bd87549","0x","0x1d818c1c414c665a9c9a0e0c0ef1ef87cacb380b8c1f6223cb2a68a4b2d023f5","0x","0x","0x","0x236e8f61ecde6abfebc6c529441f782f62469d8a2cc47b7aace2c136bd3b1ff0","0x","0x","0x","0x","0x"]"#
        )
    }

    #[test]
    fn disassemble_incomplete_sequence() {
        let incomplete = &hex!("60"); // PUSH1
        let disassembled = Cast::disassemble(incomplete);
        assert!(disassembled.is_err());

        let complete = &hex!("6000"); // PUSH1 0x00
        let disassembled = Cast::disassemble(complete);
        assert!(disassembled.is_ok());

        let incomplete = &hex!("7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"); // PUSH32 with 31 bytes
        let disassembled = Cast::disassemble(incomplete);
        assert!(disassembled.is_err());

        let complete = &hex!("7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"); // PUSH32 with 32 bytes
        let disassembled = Cast::disassemble(complete);
        assert!(disassembled.is_ok());
    }
}
