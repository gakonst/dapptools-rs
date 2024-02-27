use std::{collections::HashMap, ops::Range, sync::Arc};

use super::{
    Cheatcodes, CheatsConfig, ChiselState, CoverageCollector, Debugger, Fuzzer, LogCollector,
    StackSnapshotType, TracePrinter, TracingInspector, TracingInspectorConfig,
};
use alloy_primitives::{Address, Bytes, Log, B256, U256};
use foundry_evm_core::{backend::DatabaseExt, debug::DebugArena};
use foundry_evm_coverage::HitMaps;
use foundry_evm_traces::CallTraceArena;
use revm::{
    interpreter::{
        CallInputs, CallOutcome, CreateInputs, CreateOutcome, InstructionResult, Interpreter,
    },
    primitives::{BlockEnv, Env},
    EvmContext, Inspector,
};

#[derive(Clone, Debug, Default)]
#[must_use = "builders do nothing unless you call `build` on them"]
pub struct InspectorStackBuilder {
    /// The block environment.
    ///
    /// Used in the cheatcode handler to overwrite the block environment separately from the
    /// execution block environment.
    pub block: Option<BlockEnv>,
    /// The gas price.
    ///
    /// Used in the cheatcode handler to overwrite the gas price separately from the gas price
    /// in the execution environment.
    pub gas_price: Option<U256>,
    /// The cheatcodes config.
    pub cheatcodes: Option<Arc<CheatsConfig>>,
    /// The fuzzer inspector and its state, if it exists.
    pub fuzzer: Option<Fuzzer>,
    /// Whether to enable tracing.
    pub trace: Option<bool>,
    /// Whether to enable the debugger.
    pub debug: Option<bool>,
    /// Whether logs should be collected.
    pub logs: Option<bool>,
    /// Whether coverage info should be collected.
    pub coverage: Option<bool>,
    /// Whether to print all opcode traces into the console. Useful for debugging the EVM.
    pub print: Option<bool>,
    /// The chisel state inspector.
    pub chisel_state: Option<usize>,
}

impl InspectorStackBuilder {
    /// Create a new inspector stack builder.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the block environment.
    #[inline]
    pub fn block(mut self, block: BlockEnv) -> Self {
        self.block = Some(block);
        self
    }

    /// Set the gas price.
    #[inline]
    pub fn gas_price(mut self, gas_price: U256) -> Self {
        self.gas_price = Some(gas_price);
        self
    }

    /// Enable cheatcodes with the given config.
    #[inline]
    pub fn cheatcodes(mut self, config: Arc<CheatsConfig>) -> Self {
        self.cheatcodes = Some(config);
        self
    }

    /// Set the fuzzer inspector.
    #[inline]
    pub fn fuzzer(mut self, fuzzer: Fuzzer) -> Self {
        self.fuzzer = Some(fuzzer);
        self
    }

    /// Set the Chisel inspector.
    #[inline]
    pub fn chisel_state(mut self, final_pc: usize) -> Self {
        self.chisel_state = Some(final_pc);
        self
    }

    /// Set whether to collect logs.
    #[inline]
    pub fn logs(mut self, yes: bool) -> Self {
        self.logs = Some(yes);
        self
    }

    /// Set whether to collect coverage information.
    #[inline]
    pub fn coverage(mut self, yes: bool) -> Self {
        self.coverage = Some(yes);
        self
    }

    /// Set whether to enable the debugger.
    #[inline]
    pub fn debug(mut self, yes: bool) -> Self {
        self.debug = Some(yes);
        self
    }

    /// Set whether to enable the trace printer.
    #[inline]
    pub fn print(mut self, yes: bool) -> Self {
        self.print = Some(yes);
        self
    }

    /// Set whether to enable the tracer.
    #[inline]
    pub fn trace(mut self, yes: bool) -> Self {
        self.trace = Some(yes);
        self
    }

    /// Builds the stack of inspectors to use when transacting/committing on the EVM.
    ///
    /// See also [`revm::Evm::inspect_ref`] and [`revm::Evm::commit_ref`].
    pub fn build(self) -> InspectorStack {
        let Self {
            block,
            gas_price,
            cheatcodes,
            fuzzer,
            trace,
            debug,
            logs,
            coverage,
            print,
            chisel_state,
        } = self;
        let mut stack = InspectorStack::new();

        // inspectors
        if let Some(config) = cheatcodes {
            stack.set_cheatcodes(Cheatcodes::new(config));
        }
        if let Some(fuzzer) = fuzzer {
            stack.set_fuzzer(fuzzer);
        }
        if let Some(chisel_state) = chisel_state {
            stack.set_chisel(chisel_state);
        }
        stack.collect_coverage(coverage.unwrap_or(false));
        stack.collect_logs(logs.unwrap_or(true));
        stack.enable_debugger(debug.unwrap_or(false));
        stack.print(print.unwrap_or(false));
        stack.tracing(trace.unwrap_or(false));

        // environment, must come after all of the inspectors
        if let Some(block) = block {
            stack.set_block(&block);
        }
        if let Some(gas_price) = gas_price {
            stack.set_gas_price(gas_price);
        }

        stack
    }
}

/// Helper macro to call the same method on multiple inspectors without resorting to dynamic
/// dispatch.
#[macro_export]
macro_rules! call_inspectors {
    ([$($inspector:expr),+ $(,)?], |$id:ident $(,)?| $call:expr $(,)?) => {{$(
        if let Some($id) = $inspector {
            $call
        }
    )+}}
}

/// The collected results of [`InspectorStack`].
pub struct InspectorData {
    pub logs: Vec<Log>,
    pub labels: HashMap<Address, String>,
    pub traces: Option<CallTraceArena>,
    pub debug: Option<DebugArena>,
    pub coverage: Option<HitMaps>,
    pub cheatcodes: Option<Cheatcodes>,
    pub chisel_state: Option<(Vec<U256>,, Vec<u8>, InstructionResult)>,
}

/// An inspector that calls multiple inspectors in sequence.
///
/// If a call to an inspector returns a value other than [InstructionResult::Continue] (or
/// equivalent) the remaining inspectors are not called.
#[derive(Clone, Debug, Default)]
pub struct InspectorStack {
    pub cheatcodes: Option<Cheatcodes>,
    pub chisel_state: Option<ChiselState>,
    pub coverage: Option<CoverageCollector>,
    pub debugger: Option<Debugger>,
    pub fuzzer: Option<Fuzzer>,
    pub log_collector: Option<LogCollector>,
    pub printer: Option<TracePrinter>,
    pub tracer: Option<TracingInspector>,
}

impl InspectorStack {
    /// Creates a new inspector stack.
    ///
    /// Note that the stack is empty by default, and you must add inspectors to it.
    /// This is done by calling the `set_*` methods on the stack directly, or by building the stack
    /// with [`InspectorStack`].
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set variables from an environment for the relevant inspectors.
    #[inline]
    pub fn set_env(&mut self, env: &Env) {
        self.set_block(&env.block);
        self.set_gas_price(env.tx.gas_price);
    }

    /// Sets the block for the relevant inspectors.
    #[inline]
    pub fn set_block(&mut self, block: &BlockEnv) {
        if let Some(cheatcodes) = &mut self.cheatcodes {
            cheatcodes.block = Some(block.clone());
        }
    }

    /// Sets the gas price for the relevant inspectors.
    #[inline]
    pub fn set_gas_price(&mut self, gas_price: U256) {
        if let Some(cheatcodes) = &mut self.cheatcodes {
            cheatcodes.gas_price = Some(gas_price);
        }
    }

    /// Set the cheatcodes inspector.
    #[inline]
    pub fn set_cheatcodes(&mut self, cheatcodes: Cheatcodes) {
        self.cheatcodes = Some(cheatcodes);
    }

    /// Set the fuzzer inspector.
    #[inline]
    pub fn set_fuzzer(&mut self, fuzzer: Fuzzer) {
        self.fuzzer = Some(fuzzer);
    }

    /// Set the Chisel inspector.
    #[inline]
    pub fn set_chisel(&mut self, final_pc: usize) {
        self.chisel_state = Some(ChiselState::new(final_pc));
    }

    /// Set whether to enable the coverage collector.
    #[inline]
    pub fn collect_coverage(&mut self, yes: bool) {
        self.coverage = yes.then(Default::default);
    }

    /// Set whether to enable the debugger.
    #[inline]
    pub fn enable_debugger(&mut self, yes: bool) {
        self.debugger = yes.then(Default::default);
    }

    /// Set whether to enable the log collector.
    #[inline]
    pub fn collect_logs(&mut self, yes: bool) {
        self.log_collector = yes.then(Default::default);
    }

    /// Set whether to enable the trace printer.
    #[inline]
    pub fn print(&mut self, yes: bool) {
        self.printer = yes.then(Default::default);
    }

    /// Set whether to enable the tracer.
    #[inline]
    pub fn tracing(&mut self, yes: bool) {
        self.tracer = yes.then(|| {
            TracingInspector::new(TracingInspectorConfig {
                record_steps: false,
                record_memory_snapshots: false,
                record_stack_snapshots: StackSnapshotType::None,
                record_state_diff: false,
                exclude_precompile_calls: false,
                record_call_return_data: true,
                record_logs: true,
            })
        });
    }

    /// Collects all the data gathered during inspection into a single struct.
    #[inline]
    pub fn collect(self) -> InspectorData {
        InspectorData {
            logs: self.log_collector.map(|logs| logs.logs).unwrap_or_default(),
            labels: self
                .cheatcodes
                .as_ref()
                .map(|cheatcodes| {
                    cheatcodes.labels.clone().into_iter().map(|l| (l.0, l.1)).collect()
                })
                .unwrap_or_default(),
            traces: self.tracer.map(|tracer| tracer.get_traces().clone()),
            debug: self.debugger.map(|debugger| debugger.arena),
            coverage: self.coverage.map(|coverage| coverage.maps),
            cheatcodes: self.cheatcodes,
            chisel_state: self.chisel_state.and_then(|state| state.state),
        }
    }
}

impl<DB: DatabaseExt> Inspector<DB> for InspectorStack {
    fn initialize_interp(&mut self, interp: &mut Interpreter, context: &mut EvmContext<DB>) {
        call_inspectors!(
            [
                &mut self.debugger,
                &mut self.coverage,
                &mut self.tracer,
                &mut self.log_collector,
                &mut self.cheatcodes,
                &mut self.printer
            ],
            |inspector| {
                inspector.initialize_interp(interp, context);
            }
        );
    }

    fn step(&mut self, interp: &mut Interpreter, context: &mut EvmContext<DB>) {
        call_inspectors!(
            [
                &mut self.fuzzer,
                &mut self.debugger,
                &mut self.tracer,
                &mut self.coverage,
                &mut self.log_collector,
                &mut self.cheatcodes,
                &mut self.printer
            ],
            |inspector| {
                inspector.step(interp, context);
            }
        );
    }

    fn step_end(&mut self, interp: &mut Interpreter, context: &mut EvmContext<DB>) {
        call_inspectors!(
            [
                &mut self.debugger,
                &mut self.tracer,
                &mut self.log_collector,
                &mut self.cheatcodes,
                &mut self.printer,
                &mut self.chisel_state
            ],
            |inspector| {
                inspector.step_end(interp, context);
            }
        );
    }

    fn log(&mut self, context: &mut EvmContext<DB>, log: &Log) {
        call_inspectors!(
            [&mut self.tracer, &mut self.log_collector, &mut self.cheatcodes, &mut self.printer],
            |inspector| {
                inspector.log(context, log);
            }
        );
    }

    fn call(
        &mut self,
        context: &mut EvmContext<DB>,
        inputs: &mut CallInputs,
        return_memory_offset: Range<usize>,
    ) -> Option<CallOutcome> {
        call_inspectors!(
            [
                &mut self.fuzzer,
                &mut self.debugger,
                &mut self.tracer,
                &mut self.coverage,
                &mut self.log_collector,
                &mut self.cheatcodes,
                &mut self.printer
            ],
            |inspector| {
                if let Some(outcome) = inspector.call(context, inputs, return_memory_offset) {
                    return Some(outcome);
                }
            }
        );

        None
    }

    #[inline]
    fn call_end(
        &mut self,
        context: &mut EvmContext<DB>,
        inputs: &CallInputs,
        outcome: CallOutcome,
    ) -> CallOutcome {
        call_inspectors!(
            [
                &mut self.fuzzer,
                &mut self.debugger,
                &mut self.tracer,
                &mut self.coverage,
                &mut self.log_collector,
                &mut self.cheatcodes,
                &mut self.printer
            ],
            |inspector| {
                let new_ret = inspector.call_end(context, inputs, outcome.clone());

                // If the inspector returns a different ret or a revert with a non-empty message,
                // we assume it wants to tell us something
                if new_ret != outcome {
                    return new_ret;
                }
            }
        );

        outcome
    }

    #[inline]
    fn create(
        &mut self,
        context: &mut EvmContext<DB>,
        inputs: &mut CreateInputs,
    ) -> Option<CreateOutcome> {
        call_inspectors!(
            [
                &mut self.debugger,
                &mut self.tracer,
                &mut self.coverage,
                &mut self.log_collector,
                &mut self.cheatcodes,
                &mut self.printer
            ],
            |inspector| {
                if let Some(out) = inspector.create(context, inputs) {
                    return Some(out);
                }
            }
        );

        None
    }

    #[inline]
    fn create_end(
        &mut self,
        context: &mut EvmContext<DB>,
        inputs: &CreateInputs,
        outcome: CreateOutcome,
    ) -> CreateOutcome {
        call_inspectors!(
            [
                &mut self.debugger,
                &mut self.tracer,
                &mut self.coverage,
                &mut self.log_collector,
                &mut self.cheatcodes,
                &mut self.printer
            ],
            |inspector| {
                let new_ret = inspector.create_end(context, inputs, outcome.clone());

                // If the inspector returns a different ret or a revert with a non-empty message,
                // we assume it wants to tell us something
                if new_ret != outcome {
                    return new_ret;
                }
            }
        );

        outcome
    }

    fn selfdestruct(&mut self, contract: Address, target: Address, value: U256) {
        call_inspectors!(
            [
                &mut self.debugger,
                &mut self.tracer,
                &mut self.log_collector,
                &mut self.cheatcodes,
                &mut self.printer,
                &mut self.chisel_state
            ],
            |inspector| {
                Inspector::<DB>::selfdestruct(inspector, contract, target, value);
            }
        );
    }
}
