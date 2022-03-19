/// ABIs used internally in the executor
pub mod abi;
pub use abi::{
    patch_hardhat_console_selector, HardhatConsoleCalls, CHEATCODE_ADDRESS, CONSOLE_ABI,
    HARDHAT_CONSOLE_ABI, HARDHAT_CONSOLE_ADDRESS,
};

/// Executor configuration
pub mod opts;

/// Executor inspectors
pub mod inspector;

/// Forking provider
pub mod fork;

/// Executor builder
pub mod builder;
pub use builder::{ExecutorBuilder, Fork};

/// Executor EVM spec identifiers
pub use revm::SpecId;

/// Executor database trait
pub use revm::db::DatabaseRef;

use self::inspector::InspectorStackConfig;
use crate::{debug::DebugArena, trace::CallTraceArena, CALLER};
use bytes::Bytes;
use ethers::{
    abi::{Abi, Detokenize, RawLog, Tokenize},
    prelude::{decode_function_data, encode_function_data, Address, U256},
};
use eyre::Result;
use foundry_utils::IntoFunction;
use hashbrown::HashMap;
use inspector::InspectorStack;
use revm::{
    db::{CacheDB, DatabaseCommit, EmptyDB},
    return_ok, Account, CreateScheme, Env, Return, TransactOut, TransactTo, TxEnv, EVM,
};
use std::collections::BTreeMap;

/// A mapping of addresses to their changed state.
pub type StateChangeset = HashMap<Address, Account>;

#[derive(thiserror::Error, Debug)]
pub enum EvmError {
    /// Error which occurred during execution of a transaction
    #[error("Execution reverted: {reason} (gas: {gas})")]
    Execution {
        reverted: bool,
        reason: String,
        gas: u64,
        stipend: u64,
        logs: Vec<RawLog>,
        traces: Option<CallTraceArena>,
        debug: Option<DebugArena>,
        labels: BTreeMap<Address, String>,
        state_changeset: StateChangeset,
    },
    /// Error which occurred during ABI encoding/decoding
    #[error(transparent)]
    AbiError(#[from] ethers::contract::AbiError),
    /// Any other error.
    #[error(transparent)]
    Eyre(#[from] eyre::Error),
}

/// The result of a deployment.
#[derive(Debug)]
pub struct DeployResult {
    /// The address of the deployed contract
    pub address: Address,
    /// The gas cost of the deployment
    pub gas: u64,
    /// The logs emitted during the deployment
    pub logs: Vec<RawLog>,
    /// The traces of the deployment
    pub traces: Option<CallTraceArena>,
    /// The debug nodes of the call
    pub debug: Option<DebugArena>,
}

/// The result of a call.
#[derive(Debug)]
pub struct CallResult<D: Detokenize> {
    /// Whether the call reverted or not
    pub reverted: bool,
    /// The decoded result of the call
    pub result: D,
    /// The gas used for the call
    pub gas: u64,
    /// The initial gas stipend for the transaction
    pub stipend: u64,
    /// The logs emitted during the call
    pub logs: Vec<RawLog>,
    /// The labels assigned to addresses during the call
    pub labels: BTreeMap<Address, String>,
    /// The traces of the call
    pub traces: Option<CallTraceArena>,
    /// The debug nodes of the call
    pub debug: Option<DebugArena>,
    /// The changeset of the state.
    ///
    /// This is only present if the changed state was not committed to the database (i.e. if you
    /// used `call` and `call_raw` not `call_committing` or `call_raw_committing`).
    pub state_changeset: StateChangeset,
}

/// The result of a raw call.
#[derive(Debug)]
pub struct RawCallResult {
    /// The status of the call
    status: Return,
    /// Whether the call reverted or not
    pub reverted: bool,
    /// The raw result of the call
    pub result: Bytes,
    /// The gas used for the call
    pub gas: u64,
    /// The initial gas stipend for the transaction
    pub stipend: u64,
    /// The logs emitted during the call
    pub logs: Vec<RawLog>,
    /// The labels assigned to addresses during the call
    pub labels: BTreeMap<Address, String>,
    /// The traces of the call
    pub traces: Option<CallTraceArena>,
    /// The debug nodes of the call
    pub debug: Option<DebugArena>,
    /// The changeset of the state.
    ///
    /// This is only present if the changed state was not committed to the database (i.e. if you
    /// used `call` and `call_raw` not `call_committing` or `call_raw_committing`).
    pub state_changeset: StateChangeset,
}

impl Default for RawCallResult {
    fn default() -> Self {
        Self {
            status: Return::Continue,
            reverted: false,
            result: Bytes::new(),
            gas: 0,
            stipend: 0,
            logs: Vec::new(),
            labels: BTreeMap::new(),
            traces: None,
            debug: None,
            state_changeset: StateChangeset::new(),
        }
    }
}

pub struct Executor<DB: DatabaseRef> {
    // Note: We do not store an EVM here, since we are really
    // only interested in the database. REVM's `EVM` is a thin
    // wrapper around spawning a new EVM on every call anyway,
    // so the performance difference should be negligible.
    //
    // Also, if we stored the VM here we would still need to
    // take `&mut self` when we are not committing to the database, since
    // we need to set `evm.env`.
    db: CacheDB<DB>,
    env: Env,
    inspector_config: InspectorStackConfig,
}

impl<DB> Executor<DB>
where
    DB: DatabaseRef,
{
    pub fn new(inner_db: DB, env: Env, inspector_config: InspectorStackConfig) -> Self {
        let mut db = CacheDB::new(inner_db);

        // Need to create a non-empty contract on the cheatcodes address so `extcodesize` checks
        // does not fail
        db.insert_cache(
            *CHEATCODE_ADDRESS,
            revm::AccountInfo { code: Some(Bytes::from_static(&[1])), ..Default::default() },
        );

        Executor { db, env, inspector_config }
    }

    /// Set the balance of an account.
    pub fn set_balance(&mut self, address: Address, amount: U256) {
        let mut account = self.db.basic(address);
        account.balance = amount;

        self.db.insert_cache(address, account);
    }

    /// Gets the balance of an account
    pub fn get_balance(&self, address: Address) -> U256 {
        self.db.basic(address).balance
    }

    /// Set the nonce of an account.
    pub fn set_nonce(&mut self, address: Address, nonce: u64) {
        let mut account = self.db.basic(address);
        account.nonce = nonce;

        self.db.insert_cache(address, account);
    }

    /// Calls the `setUp()` function on a contract.
    pub fn setup(&mut self, address: Address) -> std::result::Result<CallResult<()>, EvmError> {
        self.call_committing::<(), _, _>(*CALLER, address, "setUp()", (), 0.into(), None)
    }

    /// Performs a call to an account on the current state of the VM.
    ///
    /// The state after the call is persisted.
    pub fn call_committing<D: Detokenize, T: Tokenize, F: IntoFunction>(
        &mut self,
        from: Address,
        to: Address,
        func: F,
        args: T,
        value: U256,
        abi: Option<&Abi>,
    ) -> std::result::Result<CallResult<D>, EvmError> {
        let func = func.into();
        let calldata = Bytes::from(encode_function_data(&func, args)?.to_vec());
        let RawCallResult {
            result,
            status,
            reverted,
            gas,
            stipend,
            logs,
            labels,
            traces,
            debug,
            state_changeset,
        } = self.call_raw_committing(from, to, calldata, value)?;
        match status {
            return_ok!() => {
                let result = decode_function_data(&func, result, false)?;
                Ok(CallResult {
                    reverted,
                    result,
                    gas,
                    stipend,
                    logs,
                    labels,
                    traces,
                    debug,
                    state_changeset,
                })
            }
            _ => {
                let reason = foundry_utils::decode_revert(result.as_ref(), abi)
                    .unwrap_or_else(|_| format!("{:?}", status));
                Err(EvmError::Execution {
                    reverted,
                    reason,
                    gas,
                    stipend,
                    logs,
                    traces,
                    debug,
                    labels,
                    state_changeset,
                })
            }
        }
    }

    /// Performs a raw call to an account on the current state of the VM.
    ///
    /// The state after the call is persisted.
    pub fn call_raw_committing(
        &mut self,
        from: Address,
        to: Address,
        calldata: Bytes,
        value: U256,
    ) -> Result<RawCallResult> {
        let result = self.call_raw(from, to, calldata, value)?;
        self.db.commit(result.state_changeset.clone());
        Ok(result)
    }

    /// Performs a call to an account on the current state of the VM.
    ///
    /// The state after the call is not persisted.
    pub fn call<D: Detokenize, T: Tokenize, F: IntoFunction>(
        &self,
        from: Address,
        to: Address,
        func: F,
        args: T,
        value: U256,
        abi: Option<&Abi>,
    ) -> std::result::Result<CallResult<D>, EvmError> {
        let func = func.into();
        let calldata = Bytes::from(encode_function_data(&func, args)?.to_vec());
        let RawCallResult {
            result,
            status,
            reverted,
            gas,
            stipend,
            logs,
            labels,
            traces,
            debug,
            state_changeset,
        } = self.call_raw(from, to, calldata, value)?;
        match status {
            return_ok!() => {
                let result = decode_function_data(&func, result, false)?;
                Ok(CallResult {
                    reverted,
                    result,
                    gas,
                    stipend,
                    logs,
                    labels,
                    traces,
                    debug,
                    state_changeset,
                })
            }
            _ => {
                let reason = foundry_utils::decode_revert(result.as_ref(), abi)
                    .unwrap_or_else(|_| format!("{:?}", status));
                Err(EvmError::Execution {
                    reverted,
                    reason,
                    gas,
                    stipend,
                    logs,
                    traces,
                    debug,
                    labels,
                    state_changeset,
                })
            }
        }
    }

    /// Performs a raw call to an account on the current state of the VM.
    ///
    /// The state after the call is not persisted.
    pub fn call_raw(
        &self,
        from: Address,
        to: Address,
        calldata: Bytes,
        value: U256,
    ) -> Result<RawCallResult> {
        let stipend = stipend(&calldata, self.env.cfg.spec_id);

        // Build VM
        let mut evm = EVM::new();
        evm.env = self.build_env(from, TransactTo::Call(to), calldata, value);
        evm.database(&self.db);

        // Run the call
        let mut inspector = self.inspector_config.stack();
        let (status, out, gas, state_changeset, _) = evm.inspect_ref(&mut inspector);
        let result = match out {
            TransactOut::Call(data) => data,
            _ => Bytes::default(),
        };

        let InspectorData { logs, labels, traces, debug } = collect_inspector_states(inspector);
        Ok(RawCallResult {
            status,
            reverted: !matches!(status, return_ok!()),
            result,
            gas,
            stipend,
            logs: logs.to_vec(),
            labels,
            traces,
            debug,
            state_changeset,
        })
    }

    /// Deploys a contract and commits the new state to the underlying database.
    pub fn deploy(&mut self, from: Address, code: Bytes, value: U256) -> Result<DeployResult> {
        let mut evm = EVM::new();
        evm.env = self.build_env(from, TransactTo::Create(CreateScheme::Create), code, value);
        evm.database(&mut self.db);

        let mut inspector = self.inspector_config.stack();
        let (status, out, gas, _) = evm.inspect_commit(&mut inspector);
        let address = match out {
            TransactOut::Create(_, Some(addr)) => addr,
            // TODO: We should have better error handling logic in the test runner
            // regarding deployments in general
            _ => eyre::bail!("deployment failed: {:?}", status),
        };
        let InspectorData { logs, traces, debug, .. } = collect_inspector_states(inspector);

        Ok(DeployResult { address, gas, logs, traces, debug })
    }

    /// Check if a call to a test contract was successful.
    ///
    /// This function checks both the VM status of the call and DSTest's `failed`.
    ///
    /// DSTest will not revert inside its `assertEq`-like functions which allows
    /// to test multiple assertions in 1 test function while also preserving logs.
    ///
    /// Instead it sets `failed` to `true` which we must check.
    pub fn is_success(
        &self,
        address: Address,
        reverted: bool,
        state_changeset: StateChangeset,
        should_fail: bool,
    ) -> bool {
        // Construct a new VM with the state changeset
        let mut db = CacheDB::new(EmptyDB());
        db.insert_cache(address, self.db.basic(address));
        db.commit(state_changeset);
        let executor = Executor::new(db, self.env.clone(), self.inspector_config.clone());

        let mut success = !reverted;
        if success {
            // Check if a DSTest assertion failed
            let call = executor.call::<bool, _, _>(
                Address::zero(),
                address,
                "failed()(bool)",
                (),
                0.into(),
                None,
            );

            if let Ok(CallResult { result: failed, .. }) = call {
                success = !failed;
            }
        }

        should_fail ^ success
    }

    fn build_env(&self, caller: Address, transact_to: TransactTo, data: Bytes, value: U256) -> Env {
        Env {
            cfg: self.env.cfg.clone(),
            block: self.env.block.clone(),
            tx: TxEnv { caller, transact_to, data, value, ..self.env.tx.clone() },
        }
    }
}

struct InspectorData {
    logs: Vec<RawLog>,
    labels: BTreeMap<Address, String>,
    traces: Option<CallTraceArena>,
    debug: Option<DebugArena>,
}

fn collect_inspector_states(stack: InspectorStack) -> InspectorData {
    InspectorData {
        logs: stack.logs.map(|logs| logs.logs).unwrap_or_default(),
        labels: stack.cheatcodes.map(|cheatcodes| cheatcodes.labels).unwrap_or_default(),
        traces: stack.tracer.map(|tracer| tracer.traces),
        debug: stack.debugger.map(|debugger| debugger.arena),
    }
}

/// Calculates the initial gas stipend for a transaction
fn stipend(calldata: &[u8], spec: SpecId) -> u64 {
    let non_zero_data_cost = if SpecId::enabled(spec, SpecId::ISTANBUL) { 16 } else { 68 };
    calldata.iter().fold(21000, |sum, byte| sum + if *byte == 0 { 4 } else { non_zero_data_cost })
}
