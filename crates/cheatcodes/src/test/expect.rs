use std::{
    collections::VecDeque,
    fmt::{self, Display},
};

use crate::{Cheatcode, Cheatcodes, CheatsCtxt, Error, Result, Vm::*};
use alloy_primitives::{
    map::{hash_map::Entry, AddressHashMap, HashMap},
    Address, Bytes, LogData as RawLog, U256,
};
use revm::interpreter::{InstructionResult, Interpreter, InterpreterAction, InterpreterResult};

use super::revert_handlers::RevertParameters;
/// Tracks the expected calls per address.
///
/// For each address, we track the expected calls per call data. We track it in such manner
/// so that we don't mix together calldatas that only contain selectors and calldatas that contain
/// selector and arguments (partial and full matches).
///
/// This then allows us to customize the matching behavior for each call data on the
/// `ExpectedCallData` struct and track how many times we've actually seen the call on the second
/// element of the tuple.
pub type ExpectedCallTracker = HashMap<Address, HashMap<Bytes, (ExpectedCallData, u64)>>;

#[derive(Clone, Debug)]
pub struct ExpectedCallData {
    /// The expected value sent in the call
    pub value: Option<U256>,
    /// The expected gas supplied to the call
    pub gas: Option<u64>,
    /// The expected *minimum* gas supplied to the call
    pub min_gas: Option<u64>,
    /// The number of times the call is expected to be made.
    /// If the type of call is `NonCount`, this is the lower bound for the number of calls
    /// that must be seen.
    /// If the type of call is `Count`, this is the exact number of calls that must be seen.
    pub count: u64,
    /// The type of expected call.
    pub call_type: ExpectedCallType,
}

/// The type of expected call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExpectedCallType {
    /// The call is expected to be made at least once.
    NonCount,
    /// The exact number of calls expected.
    Count,
}

/// The type of expected revert.
#[derive(Clone, Debug)]
pub enum ExpectedRevertKind {
    /// Expects revert from the next non-cheatcode call.
    Default,
    /// Expects revert from the next cheatcode call.
    ///
    /// The `pending_processing` flag is used to track whether we have exited
    /// `expectCheatcodeRevert` context or not.
    /// We have to track it to avoid expecting `expectCheatcodeRevert` call to revert itself.
    Cheatcode { pending_processing: bool },
}

#[derive(Clone, Debug)]
pub struct ExpectedRevert {
    /// The expected data returned by the revert, None being any.
    pub reason: Option<Vec<u8>>,
    /// The depth at which the revert is expected.
    pub depth: u64,
    /// The type of expected revert.
    pub kind: ExpectedRevertKind,
    /// If true then only the first 4 bytes of expected data returned by the revert are checked.
    pub partial_match: bool,
    /// Contract expected to revert next call.
    pub reverter: Option<Address>,
    /// Address that reverted the call.
    pub reverted_by: Option<Address>,
    /// Max call depth reached during next call execution.
    pub max_depth: u64,
    /// Number of times this revert is expected.
    pub count: u64,
    /// Actual number of times this revert has been seen.
    pub actual_count: u64,
}

#[derive(Clone, Debug)]
pub struct ExpectedEmit {
    /// The depth at which we expect this emit to have occurred
    pub depth: u64,
    /// The log we expect
    pub log: Option<RawLog>,
    /// The checks to perform:
    /// ```text
    /// ┌───────┬───────┬───────┬───────┬────┐
    /// │topic 0│topic 1│topic 2│topic 3│data│
    /// └───────┴───────┴───────┴───────┴────┘
    /// ```
    pub checks: [bool; 5],
    /// If present, check originating address against this
    pub address: Option<Address>,
    /// If present, relax the requirement that topic 0 must be present. This allows anonymous
    /// events with no indexed topics to be matched.
    pub anonymous: bool,
    /// Whether the log was actually found in the subcalls
    pub found: bool,
    /// Number of times the log is expected to be emitted
    pub count: u64,
}

#[derive(Clone, Debug)]
pub struct ExpectedCreate {
    pub deployer: Address,
    pub bytecode: Bytes,
    pub create_scheme: CreateScheme,
}

#[derive(Clone, Debug)]
pub enum CreateScheme {
    Create,
    Create2,
}

impl Display for CreateScheme {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Create => write!(f, "CREATE"),
            Self::Create2 => write!(f, "CREATE2"),
        }
    }
}

impl CreateScheme {
    pub fn eq(&self, create_scheme: revm::primitives::CreateScheme) -> bool {
        matches!(
            (self, create_scheme),
            (Self::Create, revm::primitives::CreateScheme::Create) |
                (Self::Create2, revm::primitives::CreateScheme::Create2 { .. })
        )
    }
}

impl Cheatcode for expectCall_0Call {
    fn apply(&self, state: &mut Cheatcodes) -> Result {
        let Self { callee, data } = self;
        expect_call(state, callee, data, None, None, None, 1, ExpectedCallType::NonCount)
    }
}

impl Cheatcode for expectCall_1Call {
    fn apply(&self, state: &mut Cheatcodes) -> Result {
        let Self { callee, data, count } = self;
        expect_call(state, callee, data, None, None, None, *count, ExpectedCallType::Count)
    }
}

impl Cheatcode for expectCall_2Call {
    fn apply(&self, state: &mut Cheatcodes) -> Result {
        let Self { callee, msgValue, data } = self;
        expect_call(state, callee, data, Some(msgValue), None, None, 1, ExpectedCallType::NonCount)
    }
}

impl Cheatcode for expectCall_3Call {
    fn apply(&self, state: &mut Cheatcodes) -> Result {
        let Self { callee, msgValue, data, count } = self;
        expect_call(
            state,
            callee,
            data,
            Some(msgValue),
            None,
            None,
            *count,
            ExpectedCallType::Count,
        )
    }
}

impl Cheatcode for expectCall_4Call {
    fn apply(&self, state: &mut Cheatcodes) -> Result {
        let Self { callee, msgValue, gas, data } = self;
        expect_call(
            state,
            callee,
            data,
            Some(msgValue),
            Some(*gas),
            None,
            1,
            ExpectedCallType::NonCount,
        )
    }
}

impl Cheatcode for expectCall_5Call {
    fn apply(&self, state: &mut Cheatcodes) -> Result {
        let Self { callee, msgValue, gas, data, count } = self;
        expect_call(
            state,
            callee,
            data,
            Some(msgValue),
            Some(*gas),
            None,
            *count,
            ExpectedCallType::Count,
        )
    }
}

impl Cheatcode for expectCallMinGas_0Call {
    fn apply(&self, state: &mut Cheatcodes) -> Result {
        let Self { callee, msgValue, minGas, data } = self;
        expect_call(
            state,
            callee,
            data,
            Some(msgValue),
            None,
            Some(*minGas),
            1,
            ExpectedCallType::NonCount,
        )
    }
}

impl Cheatcode for expectCallMinGas_1Call {
    fn apply(&self, state: &mut Cheatcodes) -> Result {
        let Self { callee, msgValue, minGas, data, count } = self;
        expect_call(
            state,
            callee,
            data,
            Some(msgValue),
            None,
            Some(*minGas),
            *count,
            ExpectedCallType::Count,
        )
    }
}

impl Cheatcode for expectEmit_0Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { checkTopic1, checkTopic2, checkTopic3, checkData } = *self;
        expect_emit(
            ccx.state,
            ccx.ecx.journaled_state.depth(),
            [true, checkTopic1, checkTopic2, checkTopic3, checkData],
            None,
            false,
            1,
        )
    }
}

impl Cheatcode for expectEmit_1Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { checkTopic1, checkTopic2, checkTopic3, checkData, emitter } = *self;
        expect_emit(
            ccx.state,
            ccx.ecx.journaled_state.depth(),
            [true, checkTopic1, checkTopic2, checkTopic3, checkData],
            Some(emitter),
            false,
            1,
        )
    }
}

impl Cheatcode for expectEmit_2Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self {} = self;
        expect_emit(ccx.state, ccx.ecx.journaled_state.depth(), [true; 5], None, false, 1)
    }
}

impl Cheatcode for expectEmit_3Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { emitter } = *self;
        expect_emit(ccx.state, ccx.ecx.journaled_state.depth(), [true; 5], Some(emitter), false, 1)
    }
}

impl Cheatcode for expectEmit_4Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { checkTopic1, checkTopic2, checkTopic3, checkData, count } = *self;
        expect_emit(
            ccx.state,
            ccx.ecx.journaled_state.depth(),
            [true, checkTopic1, checkTopic2, checkTopic3, checkData],
            None,
            false,
            count,
        )
    }
}

impl Cheatcode for expectEmit_5Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { checkTopic1, checkTopic2, checkTopic3, checkData, emitter, count } = *self;
        expect_emit(
            ccx.state,
            ccx.ecx.journaled_state.depth(),
            [true, checkTopic1, checkTopic2, checkTopic3, checkData],
            Some(emitter),
            false,
            count,
        )
    }
}

impl Cheatcode for expectEmit_6Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { count } = *self;
        expect_emit(ccx.state, ccx.ecx.journaled_state.depth(), [true; 5], None, false, count)
    }
}

impl Cheatcode for expectEmit_7Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { emitter, count } = *self;
        expect_emit(
            ccx.state,
            ccx.ecx.journaled_state.depth(),
            [true; 5],
            Some(emitter),
            false,
            count,
        )
    }
}

impl Cheatcode for expectEmitAnonymous_0Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { checkTopic0, checkTopic1, checkTopic2, checkTopic3, checkData } = *self;
        expect_emit(
            ccx.state,
            ccx.ecx.journaled_state.depth(),
            [checkTopic0, checkTopic1, checkTopic2, checkTopic3, checkData],
            None,
            true,
            1,
        )
    }
}

impl Cheatcode for expectEmitAnonymous_1Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { checkTopic0, checkTopic1, checkTopic2, checkTopic3, checkData, emitter } = *self;
        expect_emit(
            ccx.state,
            ccx.ecx.journaled_state.depth(),
            [checkTopic0, checkTopic1, checkTopic2, checkTopic3, checkData],
            Some(emitter),
            true,
            1,
        )
    }
}

impl Cheatcode for expectEmitAnonymous_2Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self {} = self;
        expect_emit(ccx.state, ccx.ecx.journaled_state.depth(), [true; 5], None, true, 1)
    }
}

impl Cheatcode for expectEmitAnonymous_3Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { emitter } = *self;
        expect_emit(ccx.state, ccx.ecx.journaled_state.depth(), [true; 5], Some(emitter), true, 1)
    }
}

impl Cheatcode for expectCreateCall {
    fn apply(&self, state: &mut Cheatcodes) -> Result {
        let Self { bytecode, deployer } = self;
        let expected_create = ExpectedCreate {
            bytecode: bytecode.clone(),
            deployer: *deployer,
            create_scheme: CreateScheme::Create,
        };
        expect_create(state, expected_create)
    }
}

impl Cheatcode for expectCreate2Call {
    fn apply(&self, state: &mut Cheatcodes) -> Result {
        let Self { bytecode, deployer } = self;
        let expected_create = ExpectedCreate {
            bytecode: bytecode.clone(),
            deployer: *deployer,
            create_scheme: CreateScheme::Create2,
        };
        expect_create(state, expected_create)
    }
}

impl Cheatcode for expectRevert_0Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self {} = self;
        expect_revert(ccx.state, None, ccx.ecx.journaled_state.depth(), false, false, None, 1)
    }
}

impl Cheatcode for expectRevert_1Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { revertData } = self;
        expect_revert(
            ccx.state,
            Some(revertData.as_ref()),
            ccx.ecx.journaled_state.depth(),
            false,
            false,
            None,
            1,
        )
    }
}

impl Cheatcode for expectRevert_2Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { revertData } = self;
        expect_revert(
            ccx.state,
            Some(revertData),
            ccx.ecx.journaled_state.depth(),
            false,
            false,
            None,
            1,
        )
    }
}

impl Cheatcode for expectRevert_3Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { reverter } = self;
        expect_revert(
            ccx.state,
            None,
            ccx.ecx.journaled_state.depth(),
            false,
            false,
            Some(*reverter),
            1,
        )
    }
}

impl Cheatcode for expectRevert_4Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { revertData, reverter } = self;
        expect_revert(
            ccx.state,
            Some(revertData.as_ref()),
            ccx.ecx.journaled_state.depth(),
            false,
            false,
            Some(*reverter),
            1,
        )
    }
}

impl Cheatcode for expectRevert_5Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { revertData, reverter } = self;
        expect_revert(
            ccx.state,
            Some(revertData),
            ccx.ecx.journaled_state.depth(),
            false,
            false,
            Some(*reverter),
            1,
        )
    }
}

impl Cheatcode for expectRevert_6Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { count } = self;
        expect_revert(ccx.state, None, ccx.ecx.journaled_state.depth(), false, false, None, *count)
    }
}

impl Cheatcode for expectRevert_7Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { revertData, count } = self;
        expect_revert(
            ccx.state,
            Some(revertData.as_ref()),
            ccx.ecx.journaled_state.depth(),
            false,
            false,
            None,
            *count,
        )
    }
}

impl Cheatcode for expectRevert_8Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { revertData, count } = self;
        expect_revert(
            ccx.state,
            Some(revertData),
            ccx.ecx.journaled_state.depth(),
            false,
            false,
            None,
            *count,
        )
    }
}

impl Cheatcode for expectRevert_9Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { reverter, count } = self;
        expect_revert(
            ccx.state,
            None,
            ccx.ecx.journaled_state.depth(),
            false,
            false,
            Some(*reverter),
            *count,
        )
    }
}

impl Cheatcode for expectRevert_10Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { revertData, reverter, count } = self;
        expect_revert(
            ccx.state,
            Some(revertData.as_ref()),
            ccx.ecx.journaled_state.depth(),
            false,
            false,
            Some(*reverter),
            *count,
        )
    }
}

impl Cheatcode for expectRevert_11Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { revertData, reverter, count } = self;
        expect_revert(
            ccx.state,
            Some(revertData),
            ccx.ecx.journaled_state.depth(),
            false,
            false,
            Some(*reverter),
            *count,
        )
    }
}

impl Cheatcode for expectPartialRevert_0Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { revertData } = self;
        expect_revert(
            ccx.state,
            Some(revertData.as_ref()),
            ccx.ecx.journaled_state.depth(),
            false,
            true,
            None,
            1,
        )
    }
}

impl Cheatcode for expectPartialRevert_1Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { revertData, reverter } = self;
        expect_revert(
            ccx.state,
            Some(revertData.as_ref()),
            ccx.ecx.journaled_state.depth(),
            false,
            true,
            Some(*reverter),
            1,
        )
    }
}

impl Cheatcode for _expectCheatcodeRevert_0Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        expect_revert(ccx.state, None, ccx.ecx.journaled_state.depth(), true, false, None, 1)
    }
}

impl Cheatcode for _expectCheatcodeRevert_1Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { revertData } = self;
        expect_revert(
            ccx.state,
            Some(revertData.as_ref()),
            ccx.ecx.journaled_state.depth(),
            true,
            false,
            None,
            1,
        )
    }
}

impl Cheatcode for _expectCheatcodeRevert_2Call {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { revertData } = self;
        expect_revert(
            ccx.state,
            Some(revertData),
            ccx.ecx.journaled_state.depth(),
            true,
            false,
            None,
            1,
        )
    }
}

impl Cheatcode for expectSafeMemoryCall {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { min, max } = *self;
        expect_safe_memory(ccx.state, min, max, ccx.ecx.journaled_state.depth())
    }
}

impl Cheatcode for stopExpectSafeMemoryCall {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self {} = self;
        ccx.state.allowed_mem_writes.remove(&ccx.ecx.journaled_state.depth());
        Ok(Default::default())
    }
}

impl Cheatcode for expectSafeMemoryCallCall {
    fn apply_stateful(&self, ccx: &mut CheatsCtxt) -> Result {
        let Self { min, max } = *self;
        expect_safe_memory(ccx.state, min, max, ccx.ecx.journaled_state.depth() + 1)
    }
}

impl RevertParameters for ExpectedRevert {
    fn reverter(&self) -> Option<Address> {
        self.reverter
    }

    fn reason(&self) -> Option<&[u8]> {
        self.reason.as_deref()
    }

    fn partial_match(&self) -> bool {
        self.partial_match
    }
}

/// Handles expected calls specified by the `expectCall` cheatcodes.
///
/// It can handle calls in two ways:
/// - If the cheatcode was used with a `count` argument, it will expect the call to be made exactly
///   `count` times. e.g. `vm.expectCall(address(0xc4f3), abi.encodeWithSelector(0xd34db33f), 4)`
///   will expect the call to address(0xc4f3) with selector `0xd34db33f` to be made exactly 4 times.
///   If the amount of calls is less or more than 4, the test will fail. Note that the `count`
///   argument cannot be overwritten with another `vm.expectCall`. If this is attempted,
///   `expectCall` will revert.
/// - If the cheatcode was used without a `count` argument, it will expect the call to be made at
///   least the amount of times the cheatcode was called. This means that `vm.expectCall` without a
///   count argument can be called many times, but cannot be called with a `count` argument after it
///   was called without one. If the latter happens, `expectCall` will revert. e.g
///   `vm.expectCall(address(0xc4f3), abi.encodeWithSelector(0xd34db33f))` will expect the call to
///   address(0xc4f3) and selector `0xd34db33f` to be made at least once. If the amount of calls is
///   0, the test will fail. If the call is made more than once, the test will pass.
#[allow(clippy::too_many_arguments)] // It is what it is
fn expect_call(
    state: &mut Cheatcodes,
    target: &Address,
    calldata: &Bytes,
    value: Option<&U256>,
    mut gas: Option<u64>,
    mut min_gas: Option<u64>,
    count: u64,
    call_type: ExpectedCallType,
) -> Result {
    let expecteds = state.expected_calls.entry(*target).or_default();

    if let Some(val) = value {
        if *val > U256::ZERO {
            // If the value of the transaction is non-zero, the EVM adds a call stipend of 2300 gas
            // to ensure that the basic fallback function can be called.
            let positive_value_cost_stipend = 2300;
            if let Some(gas) = &mut gas {
                *gas += positive_value_cost_stipend;
            }
            if let Some(min_gas) = &mut min_gas {
                *min_gas += positive_value_cost_stipend;
            }
        }
    }

    match call_type {
        ExpectedCallType::Count => {
            // Get the expected calls for this target.
            // In this case, as we're using counted expectCalls, we should not be able to set them
            // more than once.
            ensure!(
                !expecteds.contains_key(calldata),
                "counted expected calls can only bet set once"
            );
            expecteds.insert(
                calldata.clone(),
                (ExpectedCallData { value: value.copied(), gas, min_gas, count, call_type }, 0),
            );
        }
        ExpectedCallType::NonCount => {
            // Check if the expected calldata exists.
            // If it does, increment the count by one as we expect to see it one more time.
            match expecteds.entry(calldata.clone()) {
                Entry::Occupied(mut entry) => {
                    let (expected, _) = entry.get_mut();
                    // Ensure we're not overwriting a counted expectCall.
                    ensure!(
                        expected.call_type == ExpectedCallType::NonCount,
                        "cannot overwrite a counted expectCall with a non-counted expectCall"
                    );
                    expected.count += 1;
                }
                // If it does not exist, then create it.
                Entry::Vacant(entry) => {
                    entry.insert((
                        ExpectedCallData { value: value.copied(), gas, min_gas, count, call_type },
                        0,
                    ));
                }
            }
        }
    }

    Ok(Default::default())
}

fn expect_emit(
    state: &mut Cheatcodes,
    depth: u64,
    checks: [bool; 5],
    address: Option<Address>,
    anonymous: bool,
    count: u64,
) -> Result {
    let expected_emit =
        ExpectedEmit { depth, checks, address, found: false, log: None, anonymous, count };
    if let Some(found_emit_pos) = state.expected_emits.iter().position(|(emit, _)| emit.found) {
        // The order of emits already found (back of queue) should not be modified, hence push any
        // new emit before first found emit.
        state.expected_emits.insert(found_emit_pos, (expected_emit, Default::default()));
    } else {
        // If no expected emits then push new one at the back of queue.
        state.expected_emits.push_back((expected_emit, Default::default()));
    }

    Ok(Default::default())
}

pub(crate) fn handle_expect_emit(
    state: &mut Cheatcodes,
    log: &alloy_primitives::Log,
    interpreter: &mut Interpreter,
) {
    // Fill or check the expected emits.
    // We expect for emit checks to be filled as they're declared (from oldest to newest),
    // so we fill them and push them to the back of the queue.
    // If the user has properly filled all the emits, they'll end up in their original order.
    // If not, the queue will not be in the order the events will be intended to be filled,
    // and we'll be able to later detect this and bail.

    // First, we can return early if all events have been matched.
    // This allows a contract to arbitrarily emit more events than expected (additive behavior),
    // as long as all the previous events were matched in the order they were expected to be.
    if state.expected_emits.iter().all(|(expected, _)| expected.found) {
        return
    }

    let should_fill_logs = state.expected_emits.iter().any(|(expected, _)| expected.log.is_none());
    let index_to_fill_or_check = if should_fill_logs {
        // If there's anything to fill, we start with the last event to match in the queue
        // (without taking into account events already matched).
        state
            .expected_emits
            .iter()
            .position(|(emit, _)| emit.found)
            .unwrap_or(state.expected_emits.len())
            .saturating_sub(1)
    } else {
        // Otherwise, if all expected logs are filled, we start to check any unmatched event
        // in the declared order, so we start from the front (like a queue).
        0
    };

    let (mut event_to_fill_or_check, mut count_map) = state
        .expected_emits
        .remove(index_to_fill_or_check)
        .expect("we should have an emit to fill or check");

    let Some(expected) = &event_to_fill_or_check.log else {
        // Unless the caller is trying to match an anonymous event, the first topic must be
        // filled.
        if event_to_fill_or_check.anonymous || !log.topics().is_empty() {
            event_to_fill_or_check.log = Some(log.data.clone());
            // If we only filled the expected log then we put it back at the same position.
            state
                .expected_emits
                .insert(index_to_fill_or_check, (event_to_fill_or_check, count_map));
        } else {
            interpreter.instruction_result = InstructionResult::Revert;
            interpreter.next_action = InterpreterAction::Return {
                result: InterpreterResult {
                    output: Error::encode("use vm.expectEmitAnonymous to match anonymous events"),
                    gas: interpreter.gas,
                    result: InstructionResult::Revert,
                },
            };
        }
        return
    };

    // Increment/set `count` for `log.address` and `log.data`
    match count_map.entry(log.address) {
        Entry::Occupied(mut entry) => {
            // Checks and inserts the log into the map.
            // If the log doesn't pass the checks, it is ignored and `count` is not incremented.
            let log_count_map = entry.get_mut();
            log_count_map.insert(&log.data);
        }
        Entry::Vacant(entry) => {
            let mut log_count_map = LogCountMap::new(&event_to_fill_or_check);

            if log_count_map.satisfies_checks(&log.data) {
                log_count_map.insert(&log.data);

                // Entry is only inserted if it satisfies the checks.
                entry.insert(log_count_map);
            }
        }
    }

    event_to_fill_or_check.found = || -> bool {
        if !checks_topics_and_data(event_to_fill_or_check.checks, expected, log) {
            return false
        }

        // Maybe match source address.
        if event_to_fill_or_check.address.is_some_and(|addr| addr != log.address) {
            return false;
        }

        let expected_count = event_to_fill_or_check.count;

        match event_to_fill_or_check.address {
            Some(emitter) => count_map
                .get(&emitter)
                .is_some_and(|log_map| log_map.count(&log.data) >= expected_count),
            None => count_map
                .values()
                .find(|log_map| log_map.satisfies_checks(&log.data))
                .is_some_and(|map| map.count(&log.data) >= expected_count),
        }
    }();

    // If we found the event, we can push it to the back of the queue
    // and begin expecting the next event.
    if event_to_fill_or_check.found {
        state.expected_emits.push_back((event_to_fill_or_check, count_map));
    } else {
        // We did not match this event, so we need to keep waiting for the right one to
        // appear.
        state.expected_emits.push_front((event_to_fill_or_check, count_map));
    }
}

/// Handles expected emits specified by the `expectEmit` cheatcodes.
///
/// The second element of the tuple counts the number of times the log has been emitted by a
/// particular address
pub type ExpectedEmitTracker = VecDeque<(ExpectedEmit, AddressHashMap<LogCountMap>)>;

#[derive(Clone, Debug, Default)]
pub struct LogCountMap {
    checks: [bool; 5],
    expected_log: RawLog,
    map: HashMap<RawLog, u64>,
}

impl LogCountMap {
    /// Instantiates `LogCountMap`.
    fn new(expected_emit: &ExpectedEmit) -> Self {
        Self {
            checks: expected_emit.checks,
            expected_log: expected_emit.log.clone().expect("log should be filled here"),
            map: Default::default(),
        }
    }

    /// Inserts a log into the map and increments the count.
    ///
    /// The log must pass all checks against the expected log for the count to increment.
    ///
    /// Returns true if the log was inserted and count was incremented.
    fn insert(&mut self, log: &RawLog) -> bool {
        // If its already in the map, increment the count without checking.
        if self.map.contains_key(log) {
            self.map.entry(log.clone()).and_modify(|c| *c += 1);

            return true
        }

        if !self.satisfies_checks(log) {
            return false
        }

        self.map.entry(log.clone()).and_modify(|c| *c += 1).or_insert(1);

        true
    }

    /// Checks the incoming raw log against the expected logs topics and data.
    fn satisfies_checks(&self, log: &RawLog) -> bool {
        checks_topics_and_data(self.checks, &self.expected_log, log)
    }

    pub fn count(&self, log: &RawLog) -> u64 {
        if !self.satisfies_checks(log) {
            return 0
        }

        self.count_unchecked()
    }

    pub fn count_unchecked(&self) -> u64 {
        self.map.values().sum()
    }
}

fn expect_create(state: &mut Cheatcodes, expected_create: ExpectedCreate) -> Result {
    state.expected_creates.push(expected_create);

    Ok(Default::default())
}

fn expect_revert(
    state: &mut Cheatcodes,
    reason: Option<&[u8]>,
    depth: u64,
    cheatcode: bool,
    partial_match: bool,
    reverter: Option<Address>,
    count: u64,
) -> Result {
    ensure!(
        state.expected_revert.is_none(),
        "you must call another function prior to expecting a second revert"
    );
    state.expected_revert = Some(ExpectedRevert {
        reason: reason.map(<[_]>::to_vec),
        depth,
        kind: if cheatcode {
            ExpectedRevertKind::Cheatcode { pending_processing: true }
        } else {
            ExpectedRevertKind::Default
        },
        partial_match,
        reverter,
        reverted_by: None,
        max_depth: depth,
        count,
        actual_count: 0,
    });
    Ok(Default::default())
}

fn checks_topics_and_data(checks: [bool; 5], expected: &RawLog, log: &RawLog) -> bool {
    if log.topics().len() != expected.topics().len() {
        return false
    }

    // Check topics.
    if !log
        .topics()
        .iter()
        .enumerate()
        .filter(|(i, _)| checks[*i])
        .all(|(i, topic)| topic == &expected.topics()[i])
    {
        return false
    }

    // Check data
    if checks[4] && expected.data.as_ref() != log.data.as_ref() {
        return false
    }

    true
}

fn expect_safe_memory(state: &mut Cheatcodes, start: u64, end: u64, depth: u64) -> Result {
    ensure!(start < end, "memory range start ({start}) is greater than end ({end})");
    #[allow(clippy::single_range_in_vec_init)] // Wanted behaviour
    let offsets = state.allowed_mem_writes.entry(depth).or_insert_with(|| vec![0..0x60]);
    offsets.push(start..end);
    Ok(Default::default())
}
