use std::collections::BTreeMap;

use crate::executors::RawCallResult;
use alloy_primitives::{Bytes, Log};
use foundry_common::evm::Breakpoints;
use foundry_evm_coverage::HitMaps;
use foundry_evm_fuzz::FuzzCase;
use foundry_evm_traces::SparsedTraceArena;
use revm::interpreter::InstructionResult;
use std::collections::HashMap;

/// Returned by a single fuzz in the case of a successful run
#[derive(Debug)]
pub struct CaseOutcome {
    /// Data of a single fuzz test case.
    pub case: FuzzCase,
    /// The traces of the call.
    pub traces: Option<SparsedTraceArena>,
    /// The coverage info collected during the call.
    pub coverage: Option<HitMaps>,
    /// Breakpoints char pc map.
    pub breakpoints: Breakpoints,
    /// logs of a single fuzz test case.
    pub logs: Vec<Log>,
    /// Gas snapshots of a single fuzz test case
    pub gas_snapshots: BTreeMap<String, BTreeMap<String, String>>,
    // Deprecated cheatcodes mapped to their replacements.
    pub deprecated_cheatcodes: HashMap<&'static str, Option<&'static str>>,
}

/// Returned by a single fuzz when a counterexample has been discovered
#[derive(Debug)]
pub struct CounterExampleOutcome {
    /// Minimal reproduction test case for failing test.
    pub counterexample: (Bytes, RawCallResult),
    /// The status of the call.
    pub exit_reason: InstructionResult,
    /// Breakpoints char pc map.
    pub breakpoints: Breakpoints,
}

/// Outcome of a single fuzz
#[derive(Debug)]
pub enum FuzzOutcome {
    Case(CaseOutcome),
    CounterExample(CounterExampleOutcome),
}
