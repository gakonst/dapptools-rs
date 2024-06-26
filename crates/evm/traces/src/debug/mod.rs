mod sources;
use crate::{CallTraceNode, DecodedTraceStep};
use alloy_dyn_abi::{
    parser::{Parameters, Storage},
    DynSolType, DynSolValue, Specifier,
};
use alloy_primitives::U256;
use foundry_common::fmt::format_token;
use foundry_compilers::artifacts::sourcemap::{Jump, SourceElement};
use revm::interpreter::OpCode;
use revm_inspectors::tracing::types::CallTraceStep;
pub use sources::{ArtifactData, ContractSources, SourceData};

#[derive(Clone, Debug)]
pub struct DebugTraceIdentifier {
    /// Source map of contract sources
    contracts_sources: ContractSources,
}

impl DebugTraceIdentifier {
    pub fn new(contracts_sources: ContractSources) -> Self {
        Self { contracts_sources }
    }

    /// Identifies internal function invocations in a given [CallTraceNode].
    ///
    /// Accepts the node itself and identified name of the contract which node corresponds to.
    pub fn identify_node_steps(
        &self,
        node: &CallTraceNode,
        contract_name: &str,
    ) -> Vec<DecodedTraceStep> {
        DebugStepsWalker::new(node, &self.contracts_sources, contract_name).walk()
    }
}

struct DebugStepsWalker<'a> {
    node: &'a CallTraceNode,
    current_step: usize,
    stack: Vec<(String, usize)>,
    identified: Vec<DecodedTraceStep>,
    sources: &'a ContractSources,
    contract_name: &'a str,
}

impl<'a> DebugStepsWalker<'a> {
    pub fn new(
        node: &'a CallTraceNode,
        sources: &'a ContractSources,
        contract_name: &'a str,
    ) -> Self {
        Self {
            node,
            current_step: 0,
            stack: Vec::new(),
            identified: Vec::new(),
            sources,
            contract_name,
        }
    }

    fn current_step(&self) -> &CallTraceStep {
        &self.node.trace.steps[self.current_step]
    }

    fn src_map(&self, step: usize) -> Option<(SourceElement, &SourceData)> {
        self.sources.find_source_mapping(
            self.contract_name,
            self.node.trace.steps[step].pc,
            self.node.trace.kind.is_any_create(),
        )
    }

    fn prev_src_map(&self) -> Option<(SourceElement, &SourceData)> {
        if self.current_step == 0 {
            return None;
        }

        self.src_map(self.current_step - 1)
    }

    fn current_src_map(&self) -> Option<(SourceElement, &SourceData)> {
        self.src_map(self.current_step)
    }

    fn is_same_loc(&self, step: usize, other: usize) -> bool {
        let Some((loc, _)) = self.src_map(step) else {
            return false;
        };
        let Some((other_loc, _)) = self.src_map(other) else {
            return false;
        };

        loc.offset() == other_loc.offset() &&
            loc.length() == other_loc.length() &&
            loc.index() == other_loc.index()
    }

    /// Invoked when current step is a JUMPDEST preceeded by a JUMP marked as [Jump::In].
    fn jump_in(&mut self) {
        // This usually means that this is a jump into the external function which is an
        // entrypoint for the current frame. We don't want to include this to avoid
        // duplicating traces.
        if self.is_same_loc(self.current_step, self.current_step - 1) {
            return;
        }

        let Some((source_element, source)) = self.current_src_map() else {
            return;
        };

        if let Some(name) = parse_function_from_loc(source, &source_element) {
            self.stack.push((name, self.current_step - 1));
        }
    }

    /// Invoked when current step is a JUMPDEST preceeded by a JUMP marked as [Jump::Out].
    fn jump_out(&mut self) {
        let Some((i, _)) = self.stack.iter().enumerate().rfind(|(_, (_, step_idx))| {
            self.is_same_loc(*step_idx, self.current_step) ||
                self.is_same_loc(step_idx + 1, self.current_step - 1)
        }) else {
            return
        };
        // We've found a match, remove all records between start and end, those
        // are considered invalid.
        let (function_name, start_idx) = self.stack.split_off(i).swap_remove(0);

        let gas_used = self.node.trace.steps[start_idx].gas_remaining as i64 -
            self.node.trace.steps[self.current_step].gas_remaining as i64;

        // Try to decode function inputs and outputs from the stack and memory.
        let (inputs, outputs) = self
            .src_map(start_idx + 1)
            .map(|(source_element, source)| {
                let start = source_element.offset() as usize;
                let end = start + source_element.length() as usize;
                let fn_definition = source.source[start..end].replace('\n', "");
                let (inputs, outputs) = parse_types(&fn_definition);

                (
                    inputs.and_then(|t| {
                        try_decode_args_from_step(&t, &self.node.trace.steps[start_idx + 1])
                    }),
                    outputs.and_then(|t| try_decode_args_from_step(&t, self.current_step())),
                )
            })
            .unwrap_or_default();

        self.identified.push(DecodedTraceStep {
            start_step_idx: start_idx,
            end_step_idx: Some(self.current_step),
            inputs,
            outputs,
            function_name,
            gas_used,
        });
    }

    fn process(&mut self) {
        // We are only interested in JUMPs.
        if self.current_step().op != OpCode::JUMP && self.current_step().op != OpCode::JUMPDEST {
            return;
        }

        let Some((prev_source_element, _)) = self.prev_src_map() else {
            return;
        };

        match prev_source_element.jump() {
            Jump::In => self.jump_in(),
            Jump::Out => self.jump_out(),
            _ => {}
        };
    }

    fn step(&mut self) {
        self.process();
        self.current_step += 1;
    }

    pub fn walk(mut self) -> Vec<DecodedTraceStep> {
        while self.current_step < self.node.trace.steps.len() {
            self.step();
        }

        self.identified.sort_by_key(|i| i.start_step_idx);

        self.identified
    }
}

/// Tries to parse the function name from the source code and detect the contract name which
/// contains the given function.
///
/// Returns string in the format `Contract::function`.
fn parse_function_from_loc(source: &SourceData, loc: &SourceElement) -> Option<String> {
    let start = loc.offset() as usize;
    let end = start + loc.length() as usize;
    let source_part = &source.source[start..end];
    if !source_part.starts_with("function") {
        return None;
    }
    let function_name = source_part.split_once("function")?.1.split('(').next()?.trim();
    let contract_name = source.find_contract_name(start, end)?;

    Some(format!("{contract_name}::{function_name}"))
}

/// Parses function input and output types into [Parameters].
fn parse_types(source: &str) -> (Option<Parameters<'_>>, Option<Parameters<'_>>) {
    let inputs = source.find('(').and_then(|params_start| {
        let params_end = params_start + source[params_start..].find(')')?;
        Parameters::parse(&source[params_start..params_end + 1]).ok()
    });
    let outputs = source.find("returns").and_then(|returns_start| {
        let return_params_start = returns_start + source[returns_start..].find('(')?;
        let return_params_end = return_params_start + source[return_params_start..].find(')')?;
        Parameters::parse(&source[return_params_start..return_params_end + 1]).ok()
    });

    (inputs, outputs)
}

/// Given [Parameters] and [CallTraceStep], tries to decode parameters by using stack and memory.
fn try_decode_args_from_step(args: &Parameters<'_>, step: &CallTraceStep) -> Option<Vec<String>> {
    let params = &args.params;

    if params.is_empty() {
        return Some(vec![]);
    }

    let types = params.iter().map(|p| p.resolve().ok().map(|t| (t, p.storage))).collect::<Vec<_>>();

    let stack = step.stack.as_ref()?;

    if stack.len() < types.len() {
        return None;
    }

    let inputs = &stack[stack.len() - types.len()..];

    let decoded = inputs
        .iter()
        .zip(types.iter())
        .map(|(input, type_and_storage)| {
            type_and_storage
                .as_ref()
                .and_then(|(type_, storage)| {
                    match (type_, storage) {
                        // HACK: alloy parser treates user-defined types as uint8: https://github.com/alloy-rs/core/pull/386
                        //
                        // filter out `uint8` params which are marked as storage or memory as this
                        // is not possible in Solidity and means that type is user-defined
                        (DynSolType::Uint(8), Some(Storage::Memory | Storage::Storage)) => None,
                        (_, Some(Storage::Memory)) => {
                            decode_from_memory(type_, step.memory.as_bytes(), input.to::<usize>())
                        }
                        // Read other types from stack
                        _ => type_.abi_decode(&input.to_be_bytes::<32>()).ok(),
                    }
                })
                .as_ref()
                .map(format_token)
                .unwrap_or_else(|| "<unknown>".to_string())
        })
        .collect();

    Some(decoded)
}

/// Decodes given [DynSolType] from memory.
fn decode_from_memory(ty: &DynSolType, memory: &[u8], location: usize) -> Option<DynSolValue> {
    let first_word = &memory[location..location + 32];

    match ty {
        // For `string` and `bytes` layout is a word with length followed by the data
        DynSolType::String | DynSolType::Bytes => {
            let length = U256::from_be_bytes::<32>(first_word.try_into().unwrap()).to::<usize>();
            let data = &memory[location + 32..location + 32 + length];

            match ty {
                DynSolType::Bytes => Some(DynSolValue::Bytes(data.to_vec())),
                DynSolType::String => {
                    Some(DynSolValue::String(String::from_utf8_lossy(data).to_string()))
                }
                _ => unreachable!(),
            }
        }
        // Dynamic arrays are encoded as a word with length followed by words with elements
        // Fixed arrays are encoded as words with elements
        DynSolType::Array(inner) | DynSolType::FixedArray(inner, _) => {
            let (length, start) = match ty {
                DynSolType::FixedArray(_, length) => (*length, location),
                DynSolType::Array(_) => (
                    U256::from_be_bytes::<32>(first_word.try_into().unwrap()).to::<usize>(),
                    location + 32,
                ),
                _ => unreachable!(),
            };
            let mut decoded = Vec::with_capacity(length);

            for i in 0..length {
                let offset = start + i * 32;
                let location = match inner.as_ref() {
                    // Arrays of variable length types are arrays of pointers to the values
                    DynSolType::String | DynSolType::Bytes | DynSolType::Array(_) => {
                        U256::from_be_bytes::<32>(memory[offset..offset + 32].try_into().unwrap())
                            .to()
                    }
                    _ => offset,
                };

                decoded.push(decode_from_memory(inner, memory, location)?);
            }

            Some(DynSolValue::Array(decoded))
        }
        _ => ty.abi_decode(first_word).ok(),
    }
}
