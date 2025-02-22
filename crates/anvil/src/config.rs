use crate::{
    cmd::StateFile,
    eth::{
        backend::{
            db::{Db, SerializableState},
            fork::{ClientFork, ClientForkConfig},
            genesis::GenesisConfig,
            mem::fork_db::ForkedDatabase,
            time::duration_since_unix_epoch,
        },
        fees::{INITIAL_BASE_FEE, INITIAL_GAS_PRICE},
        pool::transactions::{PoolTransaction, TransactionOrder},
    },
    hardfork::{ChainHardfork, OptimismHardfork},
    mem::{self, in_memory_db::MemDb},
    EthereumHardfork, FeeManager, PrecompileFactory,
};
use alloy_consensus::BlockHeader;
use alloy_genesis::Genesis;
use alloy_network::{AnyNetwork, TransactionResponse};
use alloy_primitives::{hex, map::HashMap, utils::Unit, BlockNumber, TxHash, U256};
use alloy_provider::Provider;
use alloy_rpc_types::{Block, BlockNumberOrTag};
use alloy_signer::Signer;
use alloy_signer_local::{
    coins_bip39::{English, Mnemonic},
    MnemonicBuilder, PrivateKeySigner,
};
use alloy_transport::TransportError;
use anvil_server::ServerConfig;
use eyre::{Context, Result};
use foundry_common::{
    provider::{ProviderBuilder, RetryProvider},
    ALCHEMY_FREE_TIER_CUPS, NON_ARCHIVE_NODE_WARNING, REQUEST_TIMEOUT,
};
use foundry_config::Config;
use foundry_evm::{
    backend::{BlockchainDb, BlockchainDbMeta, SharedBackend},
    constants::DEFAULT_CREATE2_DEPLOYER,
    revm::primitives::{BlockEnv, CfgEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, SpecId, TxEnv},
    utils::apply_chain_and_block_specific_env_changes,
};
use itertools::Itertools;
use parking_lot::RwLock;
use rand::thread_rng;
use revm::primitives::BlobExcessGasAndPrice;
use serde_json::{json, Value};
use std::{
    fmt::Write as FmtWrite,
    fs::File,
    io,
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::RwLock as TokioRwLock;
use yansi::Paint;

pub use foundry_common::version::SHORT_VERSION as VERSION_MESSAGE;

/// Default port the rpc will open
pub const NODE_PORT: u16 = 8545;
/// Default chain id of the node
pub const CHAIN_ID: u64 = 31337;
/// The default gas limit for all transactions
pub const DEFAULT_GAS_LIMIT: u128 = 30_000_000;
/// Default mnemonic for dev accounts
pub const DEFAULT_MNEMONIC: &str = "test test test test test test test test test test test junk";

/// The default IPC endpoint
pub const DEFAULT_IPC_ENDPOINT: &str =
    if cfg!(unix) { "/tmp/anvil.ipc" } else { r"\\.\pipe\anvil.ipc" };

const BANNER: &str = r"
                             _   _
                            (_) | |
      __ _   _ __   __   __  _  | |
     / _` | | '_ \  \ \ / / | | | |
    | (_| | | | | |  \ V /  | | | |
     \__,_| |_| |_|   \_/   |_| |_|
";

/// Configurations of the EVM node
#[derive(Clone, Debug)]
pub struct NodeConfig {
    /// Chain ID of the EVM chain
    pub chain_id: Option<u64>,
    /// Default gas limit for all txs
    pub gas_limit: Option<u128>,
    /// If set to `true`, disables the block gas limit
    pub disable_block_gas_limit: bool,
    /// Default gas price for all txs
    pub gas_price: Option<u128>,
    /// Default base fee
    pub base_fee: Option<u64>,
    /// If set to `true`, disables the enforcement of a minimum suggested priority fee
    pub disable_min_priority_fee: bool,
    /// Default blob excess gas and price
    pub blob_excess_gas_and_price: Option<BlobExcessGasAndPrice>,
    /// The hardfork to use
    pub hardfork: Option<ChainHardfork>,
    /// Signer accounts that will be initialised with `genesis_balance` in the genesis block
    pub genesis_accounts: Vec<PrivateKeySigner>,
    /// Native token balance of every genesis account in the genesis block
    pub genesis_balance: U256,
    /// Genesis block timestamp
    pub genesis_timestamp: Option<u64>,
    /// Signer accounts that can sign messages/transactions from the EVM node
    pub signer_accounts: Vec<PrivateKeySigner>,
    /// Configured block time for the EVM chain. Use `None` to mine a new block for every tx
    pub block_time: Option<Duration>,
    /// Disable auto, interval mining mode uns use `MiningMode::None` instead
    pub no_mining: bool,
    /// Enables auto and interval mining mode
    pub mixed_mining: bool,
    /// port to use for the server
    pub port: u16,
    /// maximum number of transactions in a block
    pub max_transactions: usize,
    /// url of the rpc server that should be used for any rpc calls
    pub eth_rpc_url: Option<String>,
    /// pins the block number or transaction hash for the state fork
    pub fork_choice: Option<ForkChoice>,
    /// headers to use with `eth_rpc_url`
    pub fork_headers: Vec<String>,
    /// specifies chain id for cache to skip fetching from remote in offline-start mode
    pub fork_chain_id: Option<U256>,
    /// The generator used to generate the dev accounts
    pub account_generator: Option<AccountGenerator>,
    /// whether to enable tracing
    pub enable_tracing: bool,
    /// Explicitly disables the use of RPC caching.
    pub no_storage_caching: bool,
    /// How to configure the server
    pub server_config: ServerConfig,
    /// The host the server will listen on
    pub host: Vec<IpAddr>,
    /// How transactions are sorted in the mempool
    pub transaction_order: TransactionOrder,
    /// Filename to write anvil output as json
    pub config_out: Option<PathBuf>,
    /// The genesis to use to initialize the node
    pub genesis: Option<Genesis>,
    /// Timeout in for requests sent to remote JSON-RPC server in forking mode
    pub fork_request_timeout: Duration,
    /// Number of request retries for spurious networks
    pub fork_request_retries: u32,
    /// The initial retry backoff
    pub fork_retry_backoff: Duration,
    /// available CUPS
    pub compute_units_per_second: u64,
    /// The ipc path
    pub ipc_path: Option<Option<String>>,
    /// Enable transaction/call steps tracing for debug calls returning geth-style traces
    pub enable_steps_tracing: bool,
    /// Enable printing of `console.log` invocations.
    pub print_logs: bool,
    /// Enable printing of traces.
    pub print_traces: bool,
    /// Enable auto impersonation of accounts on startup
    pub enable_auto_impersonate: bool,
    /// Configure the code size limit
    pub code_size_limit: Option<usize>,
    /// Configures how to remove historic state.
    ///
    /// If set to `Some(num)` keep latest num state in memory only.
    pub prune_history: PruneStateHistoryConfig,
    /// Max number of states cached on disk.
    pub max_persisted_states: Option<usize>,
    /// The file where to load the state from
    pub init_state: Option<SerializableState>,
    /// max number of blocks with transactions in memory
    pub transaction_block_keeper: Option<usize>,
    /// Disable the default CREATE2 deployer
    pub disable_default_create2_deployer: bool,
    /// Enable Optimism deposit transaction
    pub enable_optimism: bool,
    /// Slots in an epoch
    pub slots_in_an_epoch: u64,
    /// The memory limit per EVM execution in bytes.
    pub memory_limit: Option<u64>,
    /// Factory used by `anvil` to extend the EVM's precompiles.
    pub precompile_factory: Option<Arc<dyn PrecompileFactory>>,
    /// Enable Odyssey features.
    pub odyssey: bool,
    /// Do not print log messages.
    pub silent: bool,
    /// The path where states are cached.
    pub cache_path: Option<PathBuf>,
}

impl NodeConfig {
    fn as_string(&self, fork: Option<&ClientFork>) -> String {
        let mut s: String = String::new();
        let _ = write!(s, "\n{}", BANNER.green());
        let _ = write!(s, "\n    {VERSION_MESSAGE}");
        let _ = write!(s, "\n    {}", "https://github.com/foundry-rs/foundry".green());

        let _ = write!(
            s,
            r#"

Available Accounts
==================
"#
        );
        let balance = alloy_primitives::utils::format_ether(self.genesis_balance);
        for (idx, wallet) in self.genesis_accounts.iter().enumerate() {
            write!(s, "\n({idx}) {} ({balance} ETH)", wallet.address()).unwrap();
        }

        let _ = write!(
            s,
            r#"

Private Keys
==================
"#
        );

        for (idx, wallet) in self.genesis_accounts.iter().enumerate() {
            let hex = hex::encode(wallet.credential().to_bytes());
            let _ = write!(s, "\n({idx}) 0x{hex}");
        }

        if let Some(ref gen) = self.account_generator {
            let _ = write!(
                s,
                r#"

Wallet
==================
Mnemonic:          {}
Derivation path:   {}
"#,
                gen.phrase,
                gen.get_derivation_path()
            );
        }

        if let Some(fork) = fork {
            let _ = write!(
                s,
                r#"

Fork
==================
Endpoint:       {}
Block number:   {}
Block hash:     {:?}
Chain ID:       {}
"#,
                fork.eth_rpc_url(),
                fork.block_number(),
                fork.block_hash(),
                fork.chain_id()
            );

            if let Some(tx_hash) = fork.transaction_hash() {
                let _ = writeln!(s, "Transaction hash: {tx_hash}");
            }
        } else {
            let _ = write!(
                s,
                r#"

Chain ID
==================

{}
"#,
                self.get_chain_id().green()
            );
        }

        if (SpecId::from(self.get_hardfork()) as u8) < (SpecId::LONDON as u8) {
            let _ = write!(
                s,
                r#"
Gas Price
==================

{}
"#,
                self.get_gas_price().green()
            );
        } else {
            let _ = write!(
                s,
                r#"
Base Fee
==================

{}
"#,
                self.get_base_fee().green()
            );
        }

        let _ = write!(
            s,
            r#"
Gas Limit
==================

{}
"#,
            {
                if self.disable_block_gas_limit {
                    "Disabled".to_string()
                } else {
                    self.gas_limit.map(|l| l.to_string()).unwrap_or_else(|| {
                        if self.fork_choice.is_some() {
                            "Forked".to_string()
                        } else {
                            DEFAULT_GAS_LIMIT.to_string()
                        }
                    })
                }
            }
            .green()
        );

        let _ = write!(
            s,
            r#"
Genesis Timestamp
==================

{}
"#,
            self.get_genesis_timestamp().green()
        );

        s
    }

    fn as_json(&self, fork: Option<&ClientFork>) -> Value {
        let mut wallet_description = HashMap::new();
        let mut available_accounts = Vec::with_capacity(self.genesis_accounts.len());
        let mut private_keys = Vec::with_capacity(self.genesis_accounts.len());

        for wallet in &self.genesis_accounts {
            available_accounts.push(format!("{:?}", wallet.address()));
            private_keys.push(format!("0x{}", hex::encode(wallet.credential().to_bytes())));
        }

        if let Some(ref gen) = self.account_generator {
            let phrase = gen.get_phrase().to_string();
            let derivation_path = gen.get_derivation_path().to_string();

            wallet_description.insert("derivation_path".to_string(), derivation_path);
            wallet_description.insert("mnemonic".to_string(), phrase);
        };

        let gas_limit = match self.gas_limit {
            // if we have a disabled flag we should max out the limit
            Some(_) | None if self.disable_block_gas_limit => Some(u64::MAX.to_string()),
            Some(limit) => Some(limit.to_string()),
            _ => None,
        };

        if let Some(fork) = fork {
            json!({
              "available_accounts": available_accounts,
              "private_keys": private_keys,
              "endpoint": fork.eth_rpc_url(),
              "block_number": fork.block_number(),
              "block_hash": fork.block_hash(),
              "chain_id": fork.chain_id(),
              "wallet": wallet_description,
              "base_fee": format!("{}", self.get_base_fee()),
              "gas_price": format!("{}", self.get_gas_price()),
              "gas_limit": gas_limit,
            })
        } else {
            json!({
              "available_accounts": available_accounts,
              "private_keys": private_keys,
              "wallet": wallet_description,
              "base_fee": format!("{}", self.get_base_fee()),
              "gas_price": format!("{}", self.get_gas_price()),
              "gas_limit": gas_limit,
              "genesis_timestamp": format!("{}", self.get_genesis_timestamp()),
            })
        }
    }
}

impl NodeConfig {
    /// Returns a new config intended to be used in tests, which does not print and binds to a
    /// random, free port by setting it to `0`
    #[doc(hidden)]
    pub fn test() -> Self {
        Self { enable_tracing: true, port: 0, silent: true, ..Default::default() }
    }

    /// Returns a new config which does not initialize any accounts on node startup.
    pub fn empty_state() -> Self {
        Self {
            genesis_accounts: vec![],
            signer_accounts: vec![],
            disable_default_create2_deployer: true,
            ..Default::default()
        }
    }
}

impl Default for NodeConfig {
    fn default() -> Self {
        // generate some random wallets
        let genesis_accounts = AccountGenerator::new(10).phrase(DEFAULT_MNEMONIC).gen();
        Self {
            chain_id: None,
            gas_limit: None,
            disable_block_gas_limit: false,
            gas_price: None,
            hardfork: None,
            signer_accounts: genesis_accounts.clone(),
            genesis_timestamp: None,
            genesis_accounts,
            // 100ETH default balance
            genesis_balance: Unit::ETHER.wei().saturating_mul(U256::from(100u64)),
            block_time: None,
            no_mining: false,
            mixed_mining: false,
            port: NODE_PORT,
            // TODO make this something dependent on block capacity
            max_transactions: 1_000,
            eth_rpc_url: None,
            fork_choice: None,
            account_generator: None,
            base_fee: None,
            disable_min_priority_fee: false,
            blob_excess_gas_and_price: None,
            enable_tracing: true,
            enable_steps_tracing: false,
            print_logs: true,
            print_traces: false,
            enable_auto_impersonate: false,
            no_storage_caching: false,
            server_config: Default::default(),
            host: vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
            transaction_order: Default::default(),
            config_out: None,
            genesis: None,
            fork_request_timeout: REQUEST_TIMEOUT,
            fork_headers: vec![],
            fork_request_retries: 5,
            fork_retry_backoff: Duration::from_millis(1_000),
            fork_chain_id: None,
            // alchemy max cpus <https://docs.alchemy.com/reference/compute-units#what-are-cups-compute-units-per-second>
            compute_units_per_second: ALCHEMY_FREE_TIER_CUPS,
            ipc_path: None,
            code_size_limit: None,
            prune_history: Default::default(),
            max_persisted_states: None,
            init_state: None,
            transaction_block_keeper: None,
            disable_default_create2_deployer: false,
            enable_optimism: false,
            slots_in_an_epoch: 32,
            memory_limit: None,
            precompile_factory: None,
            odyssey: false,
            silent: false,
            cache_path: None,
        }
    }
}

impl NodeConfig {
    /// Returns the memory limit of the node
    #[must_use]
    pub fn with_memory_limit(mut self, mems_value: Option<u64>) -> Self {
        self.memory_limit = mems_value;
        self
    }
    /// Returns the base fee to use
    pub fn get_base_fee(&self) -> u64 {
        self.base_fee
            .or_else(|| self.genesis.as_ref().and_then(|g| g.base_fee_per_gas.map(|g| g as u64)))
            .unwrap_or(INITIAL_BASE_FEE)
    }

    /// Returns the base fee to use
    pub fn get_gas_price(&self) -> u128 {
        self.gas_price.unwrap_or(INITIAL_GAS_PRICE)
    }

    pub fn get_blob_excess_gas_and_price(&self) -> BlobExcessGasAndPrice {
        if let Some(blob_excess_gas_and_price) = &self.blob_excess_gas_and_price {
            blob_excess_gas_and_price.clone()
        } else if let Some(excess_blob_gas) = self.genesis.as_ref().and_then(|g| g.excess_blob_gas)
        {
            BlobExcessGasAndPrice::new(excess_blob_gas, false)
        } else {
            // If no excess blob gas is configured, default to 0
            BlobExcessGasAndPrice::new(0, false)
        }
    }

    /// Returns the hardfork to use
    pub fn get_hardfork(&self) -> ChainHardfork {
        if self.odyssey {
            return ChainHardfork::Ethereum(EthereumHardfork::PragueEOF);
        }
        if let Some(hardfork) = self.hardfork {
            return hardfork;
        }
        if self.enable_optimism {
            return OptimismHardfork::default().into();
        }
        EthereumHardfork::default().into()
    }

    /// Sets a custom code size limit
    #[must_use]
    pub fn with_code_size_limit(mut self, code_size_limit: Option<usize>) -> Self {
        self.code_size_limit = code_size_limit;
        self
    }
    /// Disables  code size limit
    #[must_use]
    pub fn disable_code_size_limit(mut self, disable_code_size_limit: bool) -> Self {
        if disable_code_size_limit {
            self.code_size_limit = Some(usize::MAX);
        }
        self
    }

    /// Sets the init state if any
    #[must_use]
    pub fn with_init_state(mut self, init_state: Option<SerializableState>) -> Self {
        self.init_state = init_state;
        self
    }

    /// Loads the init state from a file if it exists
    #[must_use]
    pub fn with_init_state_path(mut self, path: impl AsRef<Path>) -> Self {
        self.init_state = StateFile::parse_path(path).ok().and_then(|file| file.state);
        self
    }

    /// Sets the chain ID
    #[must_use]
    pub fn with_chain_id<U: Into<u64>>(mut self, chain_id: Option<U>) -> Self {
        self.set_chain_id(chain_id);
        self
    }

    /// Returns the chain ID to use
    pub fn get_chain_id(&self) -> u64 {
        self.chain_id
            .or_else(|| self.genesis.as_ref().map(|g| g.config.chain_id))
            .unwrap_or(CHAIN_ID)
    }

    /// Sets the chain id and updates all wallets
    pub fn set_chain_id(&mut self, chain_id: Option<impl Into<u64>>) {
        self.chain_id = chain_id.map(Into::into);
        let chain_id = self.get_chain_id();
        self.genesis_accounts.iter_mut().for_each(|wallet| {
            *wallet = wallet.clone().with_chain_id(Some(chain_id));
        });
        self.signer_accounts.iter_mut().for_each(|wallet| {
            *wallet = wallet.clone().with_chain_id(Some(chain_id));
        })
    }

    /// Sets the gas limit
    #[must_use]
    pub fn with_gas_limit(mut self, gas_limit: Option<u128>) -> Self {
        self.gas_limit = gas_limit;
        self
    }

    /// Disable block gas limit check
    ///
    /// If set to `true` block gas limit will not be enforced
    #[must_use]
    pub fn disable_block_gas_limit(mut self, disable_block_gas_limit: bool) -> Self {
        self.disable_block_gas_limit = disable_block_gas_limit;
        self
    }

    /// Sets the gas price
    #[must_use]
    pub fn with_gas_price(mut self, gas_price: Option<u128>) -> Self {
        self.gas_price = gas_price;
        self
    }

    /// Sets prune history status.
    #[must_use]
    pub fn set_pruned_history(mut self, prune_history: Option<Option<usize>>) -> Self {
        self.prune_history = PruneStateHistoryConfig::from_args(prune_history);
        self
    }

    /// Sets max number of states to cache on disk.
    #[must_use]
    pub fn with_max_persisted_states<U: Into<usize>>(
        mut self,
        max_persisted_states: Option<U>,
    ) -> Self {
        self.max_persisted_states = max_persisted_states.map(Into::into);
        self
    }

    /// Sets max number of blocks with transactions to keep in memory
    #[must_use]
    pub fn with_transaction_block_keeper<U: Into<usize>>(
        mut self,
        transaction_block_keeper: Option<U>,
    ) -> Self {
        self.transaction_block_keeper = transaction_block_keeper.map(Into::into);
        self
    }

    /// Sets the base fee
    #[must_use]
    pub fn with_base_fee(mut self, base_fee: Option<u64>) -> Self {
        self.base_fee = base_fee;
        self
    }

    /// Disable the enforcement of a minimum suggested priority fee
    #[must_use]
    pub fn disable_min_priority_fee(mut self, disable_min_priority_fee: bool) -> Self {
        self.disable_min_priority_fee = disable_min_priority_fee;
        self
    }

    /// Sets the init genesis (genesis.json)
    #[must_use]
    pub fn with_genesis(mut self, genesis: Option<Genesis>) -> Self {
        self.genesis = genesis;
        self
    }

    /// Returns the genesis timestamp to use
    pub fn get_genesis_timestamp(&self) -> u64 {
        self.genesis_timestamp
            .or_else(|| self.genesis.as_ref().map(|g| g.timestamp))
            .unwrap_or_else(|| duration_since_unix_epoch().as_secs())
    }

    /// Sets the genesis timestamp
    #[must_use]
    pub fn with_genesis_timestamp<U: Into<u64>>(mut self, timestamp: Option<U>) -> Self {
        if let Some(timestamp) = timestamp {
            self.genesis_timestamp = Some(timestamp.into());
        }
        self
    }

    /// Sets the hardfork
    #[must_use]
    pub fn with_hardfork(mut self, hardfork: Option<ChainHardfork>) -> Self {
        self.hardfork = hardfork;
        self
    }

    /// Sets the genesis accounts
    #[must_use]
    pub fn with_genesis_accounts(mut self, accounts: Vec<PrivateKeySigner>) -> Self {
        self.genesis_accounts = accounts;
        self
    }

    /// Sets the signer accounts
    #[must_use]
    pub fn with_signer_accounts(mut self, accounts: Vec<PrivateKeySigner>) -> Self {
        self.signer_accounts = accounts;
        self
    }

    /// Sets both the genesis accounts and the signer accounts
    /// so that `genesis_accounts == accounts`
    #[must_use]
    pub fn with_account_generator(mut self, generator: AccountGenerator) -> Self {
        let accounts = generator.gen();
        self.account_generator = Some(generator);
        self.with_signer_accounts(accounts.clone()).with_genesis_accounts(accounts)
    }

    /// Sets the balance of the genesis accounts in the genesis block
    #[must_use]
    pub fn with_genesis_balance<U: Into<U256>>(mut self, balance: U) -> Self {
        self.genesis_balance = balance.into();
        self
    }

    /// Sets the block time to automine blocks
    #[must_use]
    pub fn with_blocktime<D: Into<Duration>>(mut self, block_time: Option<D>) -> Self {
        self.block_time = block_time.map(Into::into);
        self
    }

    #[must_use]
    pub fn with_mixed_mining<D: Into<Duration>>(
        mut self,
        mixed_mining: bool,
        block_time: Option<D>,
    ) -> Self {
        self.block_time = block_time.map(Into::into);
        self.mixed_mining = mixed_mining;
        self
    }

    /// If set to `true` auto mining will be disabled
    #[must_use]
    pub fn with_no_mining(mut self, no_mining: bool) -> Self {
        self.no_mining = no_mining;
        self
    }

    /// Sets the slots in an epoch
    #[must_use]
    pub fn with_slots_in_an_epoch(mut self, slots_in_an_epoch: u64) -> Self {
        self.slots_in_an_epoch = slots_in_an_epoch;
        self
    }

    /// Sets the port to use
    #[must_use]
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Sets the ipc path to use
    ///
    /// Note: this is a double Option for
    ///     - `None` -> no ipc
    ///     - `Some(None)` -> use default path
    ///     - `Some(Some(path))` -> use custom path
    #[must_use]
    pub fn with_ipc(mut self, ipc_path: Option<Option<String>>) -> Self {
        self.ipc_path = ipc_path;
        self
    }

    /// Sets the file path to write the Anvil node's config info to.
    #[must_use]
    pub fn set_config_out(mut self, config_out: Option<PathBuf>) -> Self {
        self.config_out = config_out;
        self
    }

    /// Disables storage caching
    #[must_use]
    pub fn no_storage_caching(self) -> Self {
        self.with_storage_caching(true)
    }

    #[must_use]
    pub fn with_storage_caching(mut self, storage_caching: bool) -> Self {
        self.no_storage_caching = storage_caching;
        self
    }

    /// Sets the `eth_rpc_url` to use when forking
    #[must_use]
    pub fn with_eth_rpc_url<U: Into<String>>(mut self, eth_rpc_url: Option<U>) -> Self {
        self.eth_rpc_url = eth_rpc_url.map(Into::into);
        self
    }

    /// Sets the `fork_choice` to use to fork off from based on a block number
    #[must_use]
    pub fn with_fork_block_number<U: Into<u64>>(self, fork_block_number: Option<U>) -> Self {
        self.with_fork_choice(fork_block_number.map(Into::into))
    }

    /// Sets the `fork_choice` to use to fork off from based on a transaction hash
    #[must_use]
    pub fn with_fork_transaction_hash<U: Into<TxHash>>(
        self,
        fork_transaction_hash: Option<U>,
    ) -> Self {
        self.with_fork_choice(fork_transaction_hash.map(Into::into))
    }

    /// Sets the `fork_choice` to use to fork off from
    #[must_use]
    pub fn with_fork_choice<U: Into<ForkChoice>>(mut self, fork_choice: Option<U>) -> Self {
        self.fork_choice = fork_choice.map(Into::into);
        self
    }

    /// Sets the `fork_chain_id` to use to fork off local cache from
    #[must_use]
    pub fn with_fork_chain_id(mut self, fork_chain_id: Option<U256>) -> Self {
        self.fork_chain_id = fork_chain_id;
        self
    }

    /// Sets the `fork_headers` to use with `eth_rpc_url`
    #[must_use]
    pub fn with_fork_headers(mut self, headers: Vec<String>) -> Self {
        self.fork_headers = headers;
        self
    }

    /// Sets the `fork_request_timeout` to use for requests
    #[must_use]
    pub fn fork_request_timeout(mut self, fork_request_timeout: Option<Duration>) -> Self {
        if let Some(fork_request_timeout) = fork_request_timeout {
            self.fork_request_timeout = fork_request_timeout;
        }
        self
    }

    /// Sets the `fork_request_retries` to use for spurious networks
    #[must_use]
    pub fn fork_request_retries(mut self, fork_request_retries: Option<u32>) -> Self {
        if let Some(fork_request_retries) = fork_request_retries {
            self.fork_request_retries = fork_request_retries;
        }
        self
    }

    /// Sets the initial `fork_retry_backoff` for rate limits
    #[must_use]
    pub fn fork_retry_backoff(mut self, fork_retry_backoff: Option<Duration>) -> Self {
        if let Some(fork_retry_backoff) = fork_retry_backoff {
            self.fork_retry_backoff = fork_retry_backoff;
        }
        self
    }

    /// Sets the number of assumed available compute units per second
    ///
    /// See also, <https://docs.alchemy.com/reference/compute-units#what-are-cups-compute-units-per-second>
    #[must_use]
    pub fn fork_compute_units_per_second(mut self, compute_units_per_second: Option<u64>) -> Self {
        if let Some(compute_units_per_second) = compute_units_per_second {
            self.compute_units_per_second = compute_units_per_second;
        }
        self
    }

    /// Sets whether to enable tracing
    #[must_use]
    pub fn with_tracing(mut self, enable_tracing: bool) -> Self {
        self.enable_tracing = enable_tracing;
        self
    }

    /// Sets whether to enable steps tracing
    #[must_use]
    pub fn with_steps_tracing(mut self, enable_steps_tracing: bool) -> Self {
        self.enable_steps_tracing = enable_steps_tracing;
        self
    }

    /// Sets whether to print `console.log` invocations to stdout.
    #[must_use]
    pub fn with_print_logs(mut self, print_logs: bool) -> Self {
        self.print_logs = print_logs;
        self
    }

    /// Sets whether to print traces to stdout.
    #[must_use]
    pub fn with_print_traces(mut self, print_traces: bool) -> Self {
        self.print_traces = print_traces;
        self
    }

    /// Sets whether to enable autoImpersonate
    #[must_use]
    pub fn with_auto_impersonate(mut self, enable_auto_impersonate: bool) -> Self {
        self.enable_auto_impersonate = enable_auto_impersonate;
        self
    }

    #[must_use]
    pub fn with_server_config(mut self, config: ServerConfig) -> Self {
        self.server_config = config;
        self
    }

    /// Sets the host the server will listen on
    #[must_use]
    pub fn with_host(mut self, host: Vec<IpAddr>) -> Self {
        self.host = if host.is_empty() { vec![IpAddr::V4(Ipv4Addr::LOCALHOST)] } else { host };
        self
    }

    #[must_use]
    pub fn with_transaction_order(mut self, transaction_order: TransactionOrder) -> Self {
        self.transaction_order = transaction_order;
        self
    }

    /// Returns the ipc path for the ipc endpoint if any
    pub fn get_ipc_path(&self) -> Option<String> {
        match &self.ipc_path {
            Some(path) => path.clone().or_else(|| Some(DEFAULT_IPC_ENDPOINT.to_string())),
            None => None,
        }
    }

    /// Prints the config info
    pub fn print(&self, fork: Option<&ClientFork>) -> Result<()> {
        if let Some(path) = &self.config_out {
            let file = io::BufWriter::new(
                File::create(path).wrap_err("unable to create anvil config description file")?,
            );
            let value = self.as_json(fork);
            serde_json::to_writer(file, &value).wrap_err("failed writing JSON")?;
        }
        if !self.silent {
            sh_println!("{}", self.as_string(fork))?;
        }
        Ok(())
    }

    /// Returns the path where the cache file should be stored
    ///
    /// See also [ Config::foundry_block_cache_file()]
    pub fn block_cache_path(&self, block: u64) -> Option<PathBuf> {
        if self.no_storage_caching || self.eth_rpc_url.is_none() {
            return None;
        }
        let chain_id = self.get_chain_id();

        Config::foundry_block_cache_file(chain_id, block)
    }

    /// Sets whether to enable optimism support
    #[must_use]
    pub fn with_optimism(mut self, enable_optimism: bool) -> Self {
        self.enable_optimism = enable_optimism;
        self
    }

    /// Sets whether to disable the default create2 deployer
    #[must_use]
    pub fn with_disable_default_create2_deployer(mut self, yes: bool) -> Self {
        self.disable_default_create2_deployer = yes;
        self
    }

    /// Injects precompiles to `anvil`'s EVM.
    #[must_use]
    pub fn with_precompile_factory(mut self, factory: impl PrecompileFactory + 'static) -> Self {
        self.precompile_factory = Some(Arc::new(factory));
        self
    }

    /// Sets whether to enable Odyssey support
    #[must_use]
    pub fn with_odyssey(mut self, odyssey: bool) -> Self {
        self.odyssey = odyssey;
        self
    }

    /// Makes the node silent to not emit anything on stdout
    #[must_use]
    pub fn silent(self) -> Self {
        self.set_silent(true)
    }

    #[must_use]
    pub fn set_silent(mut self, silent: bool) -> Self {
        self.silent = silent;
        self
    }

    /// Sets the path where states are cached
    #[must_use]
    pub fn with_cache_path(mut self, cache_path: Option<PathBuf>) -> Self {
        self.cache_path = cache_path;
        self
    }

    /// Configures everything related to env, backend and database and returns the
    /// [Backend](mem::Backend)
    ///
    /// *Note*: only memory based backend for now
    pub(crate) async fn setup(&mut self) -> Result<mem::Backend> {
        // configure the revm environment

        let mut cfg =
            CfgEnvWithHandlerCfg::new_with_spec_id(CfgEnv::default(), self.get_hardfork().into());
        cfg.chain_id = self.get_chain_id();
        cfg.limit_contract_code_size = self.code_size_limit;
        // EIP-3607 rejects transactions from senders with deployed code.
        // If EIP-3607 is enabled it can cause issues during fuzz/invariant tests if the
        // caller is a contract. So we disable the check by default.
        cfg.disable_eip3607 = true;
        cfg.disable_block_gas_limit = self.disable_block_gas_limit;
        cfg.handler_cfg.is_optimism = self.enable_optimism;

        if let Some(value) = self.memory_limit {
            cfg.memory_limit = value;
        }

        let env = revm::primitives::Env {
            cfg: cfg.cfg_env,
            block: BlockEnv {
                gas_limit: U256::from(self.gas_limit()),
                basefee: U256::from(self.get_base_fee()),
                ..Default::default()
            },
            tx: TxEnv { chain_id: self.get_chain_id().into(), ..Default::default() },
        };
        let mut env = EnvWithHandlerCfg::new(Box::new(env), cfg.handler_cfg);

        let fees = FeeManager::new(
            cfg.handler_cfg.spec_id,
            self.get_base_fee(),
            !self.disable_min_priority_fee,
            self.get_gas_price(),
            self.get_blob_excess_gas_and_price(),
        );

        let (db, fork): (Arc<TokioRwLock<Box<dyn Db>>>, Option<ClientFork>) =
            if let Some(eth_rpc_url) = self.eth_rpc_url.clone() {
                self.setup_fork_db(eth_rpc_url, &mut env, &fees).await?
            } else {
                (Arc::new(TokioRwLock::new(Box::<MemDb>::default())), None)
            };

        // if provided use all settings of `genesis.json`
        if let Some(ref genesis) = self.genesis {
            env.cfg.chain_id = genesis.config.chain_id;
            env.block.timestamp = U256::from(genesis.timestamp);
            if let Some(base_fee) = genesis.base_fee_per_gas {
                env.block.basefee = U256::from(base_fee);
            }
            if let Some(number) = genesis.number {
                env.block.number = U256::from(number);
            }
            env.block.coinbase = genesis.coinbase;
        }

        let genesis = GenesisConfig {
            timestamp: self.get_genesis_timestamp(),
            balance: self.genesis_balance,
            accounts: self.genesis_accounts.iter().map(|acc| acc.address()).collect(),
            genesis_init: self.genesis.clone(),
        };

        // only memory based backend for now
        let backend = mem::Backend::with_genesis(
            db,
            Arc::new(RwLock::new(env)),
            genesis,
            fees,
            Arc::new(RwLock::new(fork)),
            self.enable_steps_tracing,
            self.print_logs,
            self.print_traces,
            self.odyssey,
            self.prune_history,
            self.max_persisted_states,
            self.transaction_block_keeper,
            self.block_time,
            self.cache_path.clone(),
            Arc::new(TokioRwLock::new(self.clone())),
        )
        .await?;

        // Writes the default create2 deployer to the backend,
        // if the option is not disabled and we are not forking.
        if !self.disable_default_create2_deployer && self.eth_rpc_url.is_none() {
            backend
                .set_create2_deployer(DEFAULT_CREATE2_DEPLOYER)
                .await
                .wrap_err("failed to create default create2 deployer")?;
        }

        if let Some(state) = self.init_state.clone() {
            backend.load_state(state).await.wrap_err("failed to load init state")?;
        }

        Ok(backend)
    }

    /// Configures everything related to forking based on the passed `eth_rpc_url`:
    ///  - returning a tuple of a [ForkedDatabase] wrapped in an [Arc] [RwLock](TokioRwLock) and
    ///    [ClientFork] wrapped in an [Option] which can be used in a [Backend](mem::Backend) to
    ///    fork from.
    ///  - modifying some parameters of the passed `env`
    ///  - mutating some members of `self`
    pub async fn setup_fork_db(
        &mut self,
        eth_rpc_url: String,
        env: &mut EnvWithHandlerCfg,
        fees: &FeeManager,
    ) -> Result<(Arc<TokioRwLock<Box<dyn Db>>>, Option<ClientFork>)> {
        let (db, config) = self.setup_fork_db_config(eth_rpc_url, env, fees).await?;
        let db: Arc<TokioRwLock<Box<dyn Db>>> = Arc::new(TokioRwLock::new(Box::new(db)));
        let fork = ClientFork::new(config, Arc::clone(&db));
        Ok((db, Some(fork)))
    }

    /// Configures everything related to forking based on the passed `eth_rpc_url`:
    ///  - returning a tuple of a [ForkedDatabase] and [ClientForkConfig] which can be used to build
    ///    a [ClientFork] to fork from.
    ///  - modifying some parameters of the passed `env`
    ///  - mutating some members of `self`
    pub async fn setup_fork_db_config(
        &mut self,
        eth_rpc_url: String,
        env: &mut EnvWithHandlerCfg,
        fees: &FeeManager,
    ) -> Result<(ForkedDatabase, ClientForkConfig)> {
        // TODO make provider agnostic
        let provider = Arc::new(
            ProviderBuilder::new(&eth_rpc_url)
                .timeout(self.fork_request_timeout)
                .initial_backoff(self.fork_retry_backoff.as_millis() as u64)
                .compute_units_per_second(self.compute_units_per_second)
                .max_retry(self.fork_request_retries)
                .initial_backoff(1000)
                .headers(self.fork_headers.clone())
                .build()
                .wrap_err("failed to establish provider to fork url")?,
        );

        let (fork_block_number, fork_chain_id, force_transactions) = if let Some(fork_choice) =
            &self.fork_choice
        {
            let (fork_block_number, force_transactions) =
                derive_block_and_transactions(fork_choice, &provider).await.wrap_err(
                    "failed to derive fork block number and force transactions from fork choice",
                )?;
            let chain_id = if let Some(chain_id) = self.fork_chain_id {
                Some(chain_id)
            } else if self.hardfork.is_none() {
                // Auto-adjust hardfork if not specified, but only if we're forking mainnet.
                let chain_id =
                    provider.get_chain_id().await.wrap_err("failed to fetch network chain ID")?;
                if alloy_chains::NamedChain::Mainnet == chain_id {
                    let hardfork: EthereumHardfork = fork_block_number.into();
                    env.handler_cfg.spec_id = hardfork.into();
                    self.hardfork = Some(ChainHardfork::Ethereum(hardfork));
                }
                Some(U256::from(chain_id))
            } else {
                None
            };

            (fork_block_number, chain_id, force_transactions)
        } else {
            // pick the last block number but also ensure it's not pending anymore
            let bn = find_latest_fork_block(&provider)
                .await
                .wrap_err("failed to get fork block number")?;
            (bn, None, None)
        };

        let block = provider
            .get_block(BlockNumberOrTag::Number(fork_block_number).into(), false.into())
            .await
            .wrap_err("failed to get fork block")?;

        let block = if let Some(block) = block {
            block
        } else {
            if let Ok(latest_block) = provider.get_block_number().await {
                let mut message = format!(
                    "Failed to get block for block number: {fork_block_number}\n\
latest block number: {latest_block}"
                );
                // If the `eth_getBlockByNumber` call succeeds, but returns null instead of
                // the block, and the block number is less than equal the latest block, then
                // the user is forking from a non-archive node with an older block number.
                if fork_block_number <= latest_block {
                    message.push_str(&format!("\n{NON_ARCHIVE_NODE_WARNING}"));
                }
                eyre::bail!("{message}");
            }
            eyre::bail!("failed to get block for block number: {fork_block_number}")
        };

        let gas_limit = self.fork_gas_limit(&block);
        self.gas_limit = Some(gas_limit);

        env.block = BlockEnv {
            number: U256::from(fork_block_number),
            timestamp: U256::from(block.header.timestamp),
            difficulty: block.header.difficulty,
            // ensures prevrandao is set
            prevrandao: Some(block.header.mix_hash.unwrap_or_default()),
            gas_limit: U256::from(gas_limit),
            // Keep previous `coinbase` and `basefee` value
            coinbase: env.block.coinbase,
            basefee: env.block.basefee,
            ..Default::default()
        };

        // if not set explicitly we use the base fee of the latest block
        if self.base_fee.is_none() {
            if let Some(base_fee) = block.header.base_fee_per_gas {
                self.base_fee = Some(base_fee);
                env.block.basefee = U256::from(base_fee);
                // this is the base fee of the current block, but we need the base fee of
                // the next block
                let next_block_base_fee = fees.get_next_block_base_fee_per_gas(
                    block.header.gas_used as u128,
                    gas_limit,
                    block.header.base_fee_per_gas.unwrap_or_default(),
                );

                // update next base fee
                fees.set_base_fee(next_block_base_fee);
            }
            if let (Some(blob_excess_gas), Some(blob_gas_used)) =
                (block.header.excess_blob_gas, block.header.blob_gas_used)
            {
                env.block.blob_excess_gas_and_price =
                    Some(BlobExcessGasAndPrice::new(blob_excess_gas, false));
                let next_block_blob_excess_gas = fees
                    .get_next_block_blob_excess_gas(blob_excess_gas as u128, blob_gas_used as u128);
                fees.set_blob_excess_gas_and_price(BlobExcessGasAndPrice::new(
                    next_block_blob_excess_gas,
                    false,
                ));
            }
        }

        // use remote gas price
        if self.gas_price.is_none() {
            if let Ok(gas_price) = provider.get_gas_price().await {
                self.gas_price = Some(gas_price);
                fees.set_gas_price(gas_price);
            }
        }

        let block_hash = block.header.hash;

        let chain_id = if let Some(chain_id) = self.chain_id {
            chain_id
        } else {
            let chain_id = if let Some(fork_chain_id) = fork_chain_id {
                fork_chain_id.to()
            } else {
                provider.get_chain_id().await.wrap_err("failed to fetch network chain ID")?
            };

            // need to update the dev signers and env with the chain id
            self.set_chain_id(Some(chain_id));
            env.cfg.chain_id = chain_id;
            env.tx.chain_id = chain_id.into();
            chain_id
        };
        let override_chain_id = self.chain_id;
        // apply changes such as difficulty -> prevrandao and chain specifics for current chain id
        apply_chain_and_block_specific_env_changes::<AnyNetwork>(env, &block);

        let meta = BlockchainDbMeta::new(*env.env.clone(), eth_rpc_url.clone());
        let block_chain_db = if self.fork_chain_id.is_some() {
            BlockchainDb::new_skip_check(meta, self.block_cache_path(fork_block_number))
        } else {
            BlockchainDb::new(meta, self.block_cache_path(fork_block_number))
        };

        // This will spawn the background thread that will use the provider to fetch
        // blockchain data from the other client
        let backend = SharedBackend::spawn_backend_thread(
            Arc::clone(&provider),
            block_chain_db.clone(),
            Some(fork_block_number.into()),
        );

        let config = ClientForkConfig {
            eth_rpc_url,
            block_number: fork_block_number,
            block_hash,
            transaction_hash: self.fork_choice.and_then(|fc| fc.transaction_hash()),
            provider,
            chain_id,
            override_chain_id,
            timestamp: block.header.timestamp,
            base_fee: block.header.base_fee_per_gas.map(|g| g as u128),
            timeout: self.fork_request_timeout,
            retries: self.fork_request_retries,
            backoff: self.fork_retry_backoff,
            compute_units_per_second: self.compute_units_per_second,
            total_difficulty: block.header.total_difficulty.unwrap_or_default(),
            blob_gas_used: block.header.blob_gas_used.map(|g| g as u128),
            blob_excess_gas_and_price: env.block.blob_excess_gas_and_price.clone(),
            force_transactions,
        };

        let mut db = ForkedDatabase::new(backend, block_chain_db);

        // need to insert the forked block's hash
        db.insert_block_hash(U256::from(config.block_number), config.block_hash);

        Ok((db, config))
    }

    /// we only use the gas limit value of the block if it is non-zero and the block gas
    /// limit is enabled, since there are networks where this is not used and is always
    /// `0x0` which would inevitably result in `OutOfGas` errors as soon as the evm is about to record gas, See also <https://github.com/foundry-rs/foundry/issues/3247>
    pub(crate) fn fork_gas_limit<T: TransactionResponse, H: BlockHeader>(
        &self,
        block: &Block<T, H>,
    ) -> u128 {
        if !self.disable_block_gas_limit {
            if let Some(gas_limit) = self.gas_limit {
                return gas_limit;
            } else if block.header.gas_limit() > 0 {
                return block.header.gas_limit() as u128;
            }
        }

        u64::MAX as u128
    }

    /// Returns the gas limit for a non forked anvil instance
    ///
    /// Checks the config for the `disable_block_gas_limit` flag
    pub(crate) fn gas_limit(&self) -> u128 {
        if self.disable_block_gas_limit {
            return u64::MAX as u128;
        }

        self.gas_limit.unwrap_or(DEFAULT_GAS_LIMIT)
    }
}

/// If the fork choice is a block number, simply return it with an empty list of transactions.
/// If the fork choice is a transaction hash, determine the block that the transaction was mined in,
/// and return the block number before the fork block along with all transactions in the fork block
/// that are before (and including) the fork transaction.
async fn derive_block_and_transactions(
    fork_choice: &ForkChoice,
    provider: &Arc<RetryProvider>,
) -> eyre::Result<(BlockNumber, Option<Vec<PoolTransaction>>)> {
    match fork_choice {
        ForkChoice::Block(block_number) => Ok((block_number.to_owned(), None)),
        ForkChoice::Transaction(transaction_hash) => {
            // Determine the block that this transaction was mined in
            let transaction = provider
                .get_transaction_by_hash(transaction_hash.0.into())
                .await?
                .ok_or_else(|| eyre::eyre!("failed to get fork transaction by hash"))?;
            let transaction_block_number = transaction.block_number.unwrap();

            // Get the block pertaining to the fork transaction
            let transaction_block = provider
                .get_block_by_number(
                    transaction_block_number.into(),
                    alloy_rpc_types::BlockTransactionsKind::Full,
                )
                .await?
                .ok_or_else(|| eyre::eyre!("failed to get fork block by number"))?;

            // Filter out transactions that are after the fork transaction
            let filtered_transactions = transaction_block
                .transactions
                .as_transactions()
                .ok_or_else(|| eyre::eyre!("failed to get transactions from full fork block"))?
                .iter()
                .take_while_inclusive(|&transaction| transaction.tx_hash() != transaction_hash.0)
                .collect::<Vec<_>>();

            // Convert the transactions to PoolTransactions
            let force_transactions = filtered_transactions
                .iter()
                .map(|&transaction| PoolTransaction::try_from(transaction.clone()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| eyre::eyre!("Err converting to pool transactions {e}"))?;
            Ok((transaction_block_number.saturating_sub(1), Some(force_transactions)))
        }
    }
}

/// Fork delimiter used to specify which block or transaction to fork from
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForkChoice {
    /// Block number to fork from
    Block(BlockNumber),
    /// Transaction hash to fork from
    Transaction(TxHash),
}

impl ForkChoice {
    /// Returns the block number to fork from
    pub fn block_number(&self) -> Option<BlockNumber> {
        match self {
            Self::Block(block_number) => Some(*block_number),
            Self::Transaction(_) => None,
        }
    }

    /// Returns the transaction hash to fork from
    pub fn transaction_hash(&self) -> Option<TxHash> {
        match self {
            Self::Block(_) => None,
            Self::Transaction(transaction_hash) => Some(*transaction_hash),
        }
    }
}

/// Convert a transaction hash into a ForkChoice
impl From<TxHash> for ForkChoice {
    fn from(tx_hash: TxHash) -> Self {
        Self::Transaction(tx_hash)
    }
}

/// Convert a decimal block number into a ForkChoice
impl From<u64> for ForkChoice {
    fn from(block: u64) -> Self {
        Self::Block(block)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PruneStateHistoryConfig {
    pub enabled: bool,
    pub max_memory_history: Option<usize>,
}

impl PruneStateHistoryConfig {
    /// Returns `true` if writing state history is supported
    pub fn is_state_history_supported(&self) -> bool {
        !self.enabled || self.max_memory_history.is_some()
    }

    /// Returns true if this setting was enabled.
    pub fn is_config_enabled(&self) -> bool {
        self.enabled
    }

    pub fn from_args(val: Option<Option<usize>>) -> Self {
        val.map(|max_memory_history| Self { enabled: true, max_memory_history }).unwrap_or_default()
    }
}

/// Can create dev accounts
#[derive(Clone, Debug)]
pub struct AccountGenerator {
    chain_id: u64,
    amount: usize,
    phrase: String,
    derivation_path: Option<String>,
}

impl AccountGenerator {
    pub fn new(amount: usize) -> Self {
        Self {
            chain_id: CHAIN_ID,
            amount,
            phrase: Mnemonic::<English>::new(&mut thread_rng()).to_phrase(),
            derivation_path: None,
        }
    }

    #[must_use]
    pub fn phrase(mut self, phrase: impl Into<String>) -> Self {
        self.phrase = phrase.into();
        self
    }

    fn get_phrase(&self) -> &str {
        &self.phrase
    }

    #[must_use]
    pub fn chain_id(mut self, chain_id: impl Into<u64>) -> Self {
        self.chain_id = chain_id.into();
        self
    }

    #[must_use]
    pub fn derivation_path(mut self, derivation_path: impl Into<String>) -> Self {
        let mut derivation_path = derivation_path.into();
        if !derivation_path.ends_with('/') {
            derivation_path.push('/');
        }
        self.derivation_path = Some(derivation_path);
        self
    }

    fn get_derivation_path(&self) -> &str {
        self.derivation_path.as_deref().unwrap_or("m/44'/60'/0'/0/")
    }
}

impl AccountGenerator {
    pub fn gen(&self) -> Vec<PrivateKeySigner> {
        let builder = MnemonicBuilder::<English>::default().phrase(self.phrase.as_str());

        // use the derivation path
        let derivation_path = self.get_derivation_path();

        let mut wallets = Vec::with_capacity(self.amount);
        for idx in 0..self.amount {
            let builder =
                builder.clone().derivation_path(format!("{derivation_path}{idx}")).unwrap();
            let wallet = builder.build().unwrap().with_chain_id(Some(self.chain_id));
            wallets.push(wallet)
        }
        wallets
    }
}

/// Returns the path to anvil dir `~/.foundry/anvil`
pub fn anvil_dir() -> Option<PathBuf> {
    Config::foundry_dir().map(|p| p.join("anvil"))
}

/// Returns the root path to anvil's temporary storage `~/.foundry/anvil/`
pub fn anvil_tmp_dir() -> Option<PathBuf> {
    anvil_dir().map(|p| p.join("tmp"))
}

/// Finds the latest appropriate block to fork
///
/// This fetches the "latest" block and checks whether the `Block` is fully populated (`hash` field
/// is present). This prevents edge cases where anvil forks the "latest" block but `eth_getBlockByNumber` still returns a pending block, <https://github.com/foundry-rs/foundry/issues/2036>
async fn find_latest_fork_block<P: Provider<AnyNetwork>>(
    provider: P,
) -> Result<u64, TransportError> {
    let mut num = provider.get_block_number().await?;

    // walk back from the head of the chain, but at most 2 blocks, which should be more than enough
    // leeway
    for _ in 0..2 {
        if let Some(block) = provider.get_block(num.into(), false.into()).await? {
            if !block.header.hash.is_zero() {
                break;
            }
        }
        // block not actually finalized, so we try the block before
        num = num.saturating_sub(1)
    }

    Ok(num)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prune_history() {
        let config = PruneStateHistoryConfig::default();
        assert!(config.is_state_history_supported());
        let config = PruneStateHistoryConfig::from_args(Some(None));
        assert!(!config.is_state_history_supported());
        let config = PruneStateHistoryConfig::from_args(Some(Some(10)));
        assert!(config.is_state_history_supported());
    }
}
