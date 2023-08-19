use super::{
    assert_invariants,
    filters::{ArtifactFilters, SenderFilters},
    BasicTxDetails, FuzzRunIdentifiedContracts, InvariantContract, InvariantFuzzError,
    InvariantFuzzTestResult, RandomCallGenerator, TargetedContracts,
};
use crate::{
    executor::{
        inspector::Fuzzer, Executor, RawCallResult, StateChangeset, CHEATCODE_ADDRESS,
        HARDHAT_CONSOLE_ADDRESS,
    },
    fuzz::{
        strategies::{
            build_initial_state, collect_created_contracts, collect_state_from_call,
            invariant_strat, override_call_strat, EvmFuzzState,
        },
        FuzzCase, FuzzedCases,
    },
    utils::{get_function, h160_to_b160},
    CALLER,
};
use ethers::{
    abi::{Abi, Address, Detokenize, FixedBytes, Tokenizable, TokenizableItem},
    prelude::U256,
};
use eyre::{ContextCompat, Result};
use foundry_common::contracts::{ContractsByAddress, ContractsByArtifact};
use foundry_config::{FuzzDictionaryConfig, InvariantConfig};
use parking_lot::{Mutex, RwLock};
use proptest::{
    strategy::{BoxedStrategy, Strategy, ValueTree},
    test_runner::{TestCaseError, TestRunner},
};
use revm::{
    primitives::{HashMap, B160},
    DatabaseCommit,
};
use std::{cell::RefCell, collections::BTreeMap, sync::Arc};

/// Alias for (Dictionary for fuzzing, initial contracts to fuzz and an InvariantStrategy).
type InvariantPreparation =
    (EvmFuzzState, FuzzRunIdentifiedContracts, BoxedStrategy<Vec<BasicTxDetails>>);

/// Enriched results of an invariant run check.
///
/// Contains the success condition and call results of the last run
struct RichInvariantResults {
    success: bool,
    call_result: Option<RawCallResult>,
}

impl RichInvariantResults {
    fn new(success: bool, call_result: Option<RawCallResult>) -> Self {
        Self { success, call_result }
    }
}

/// Wrapper around any [`Executor`] implementor which provides fuzzing support using [`proptest`](https://docs.rs/proptest/1.0.0/proptest/).
///
/// After instantiation, calling `fuzz` will proceed to hammer the deployed smart contracts with
/// inputs, until it finds a counterexample sequence. The provided [`TestRunner`] contains all the
/// configuration which can be overridden via [environment variables](https://docs.rs/proptest/1.0.0/proptest/test_runner/struct.Config.html)
pub struct InvariantExecutor<'a> {
    pub executor: Executor,
    /// Proptest runner.
    runner: TestRunner,
    /// The invariant configuration
    config: InvariantConfig,
    /// Contracts deployed with `setUp()`
    setup_contracts: &'a ContractsByAddress,
    /// Contracts that are part of the project but have not been deployed yet. We need the bytecode
    /// to identify them from the stateset changes.
    project_contracts: &'a ContractsByArtifact,
    /// Filters contracts to be fuzzed through their artifact identifiers.
    artifact_filters: ArtifactFilters,
}

impl<'a> InvariantExecutor<'a> {
    /// Instantiates a fuzzed executor EVM given a testrunner
    pub fn new(
        executor: Executor,
        runner: TestRunner,
        config: InvariantConfig,
        setup_contracts: &'a ContractsByAddress,
        project_contracts: &'a ContractsByArtifact,
    ) -> Self {
        Self {
            executor,
            runner,
            config,
            setup_contracts,
            project_contracts,
            artifact_filters: ArtifactFilters::default(),
        }
    }

    /// Fuzzes any deployed contract and checks any broken invariant at `invariant_address`.
    pub fn invariant_fuzz(
        &mut self,
        invariant_contract: InvariantContract,
    ) -> Result<InvariantFuzzTestResult> {
        let (fuzz_state, targeted_contracts, strat) = self.prepare_fuzzing(&invariant_contract)?;

        // Stores the consumed gas and calldata of every successful fuzz call.
        let fuzz_cases: RefCell<Vec<FuzzedCases>> = RefCell::new(Default::default());

        // Stores data related to reverts or failed assertions of the test.
        let failures = RefCell::new(InvariantFailures::new());

        let last_call_results = RefCell::new(
            assert_invariants(
                &invariant_contract,
                &self.executor,
                &[],
                &mut failures.borrow_mut(),
                self.config.shrink_sequence,
            )
            .ok(),
        );
        let last_run_calldata: RefCell<Vec<BasicTxDetails>> = RefCell::new(vec![]);
        // Make sure invariants are sound even before starting to fuzz
        if last_call_results.borrow().is_none() {
            fuzz_cases.borrow_mut().push(FuzzedCases::new(vec![]));
        }

        // The strategy only comes with the first `input`. We fill the rest of the `inputs`
        // until the desired `depth` so we can use the evolving fuzz dictionary
        // during the run. We need another proptest runner to query for random
        // values.
        let branch_runner = RefCell::new(self.runner.clone());
        let _ = self.runner.run(&strat, |mut inputs| {
            // Scenarios where we want to fail as soon as possible.
            if self.config.fail_on_revert && failures.borrow().reverts == 1 {
                return Err(TestCaseError::fail("Revert occurred."))
            }

            // Before each run, we must reset the backend state.
            let mut executor = self.executor.clone();

            // Used for stat reports (eg. gas usage).
            let mut fuzz_runs = Vec::with_capacity(self.config.depth as usize);

            // Created contracts during a run.
            let mut created_contracts = vec![];

            for current_run in 0..self.config.depth {
                let (sender, (address, calldata)) =
                    inputs.last().expect("to have the next randomly generated input.");

                // Executes the call from the randomly generated sequence.
                let call_result = executor
                    .call_raw(*sender, *address, calldata.0.clone(), U256::zero())
                    .expect("could not make raw evm call");

                // Collect data for fuzzing from the state changeset.
                let mut state_changeset =
                    call_result.state_changeset.to_owned().expect("no changesets");

                collect_data(
                    &mut state_changeset,
                    sender,
                    &call_result,
                    fuzz_state.clone(),
                    &self.config.dictionary,
                );

                if let Err(error) = collect_created_contracts(
                    &state_changeset,
                    self.project_contracts,
                    self.setup_contracts,
                    &self.artifact_filters,
                    targeted_contracts.clone(),
                    &mut created_contracts,
                ) {
                    warn!(target: "forge::test", "{error}");
                }

                // Commit changes to the database.
                executor.backend.commit(state_changeset.clone());

                fuzz_runs.push(FuzzCase {
                    calldata: calldata.clone(),
                    gas: call_result.gas_used,
                    stipend: call_result.stipend,
                });

                let RichInvariantResults { success: can_continue, call_result: call_results } =
                    can_continue(
                        &invariant_contract,
                        call_result,
                        &executor,
                        &inputs,
                        &mut failures.borrow_mut(),
                        &targeted_contracts,
                        state_changeset,
                        self.config.fail_on_revert,
                        self.config.shrink_sequence,
                    );

                if !can_continue || current_run == self.config.depth - 1 {
                    *last_run_calldata.borrow_mut() = inputs.clone();
                }

                if !can_continue {
                    break
                }

                *last_call_results.borrow_mut() = call_results;

                // Generates the next call from the run using the recently updated
                // dictionary.
                inputs.extend(
                    strat
                        .new_tree(&mut branch_runner.borrow_mut())
                        .map_err(|_| TestCaseError::Fail("Could not generate case".into()))?
                        .current(),
                );
            }

            // We clear all the targeted contracts created during this run.
            if !created_contracts.is_empty() {
                let mut writable_targeted = targeted_contracts.lock();
                for addr in created_contracts.iter() {
                    writable_targeted.remove(addr);
                }
            }

            fuzz_cases.borrow_mut().push(FuzzedCases::new(fuzz_runs));

            Ok(())
        });

        trace!(target: "forge::test::invariant::dictionary", "{:?}", fuzz_state.read().values().iter().map(hex::encode).collect::<Vec<_>>());

        let (reverts, error) = failures.into_inner().into_inner();

        Ok(InvariantFuzzTestResult {
            error,
            cases: fuzz_cases.into_inner(),
            reverts,
            last_run_inputs: last_run_calldata.take(),
        })
    }

    /// Prepares certain structures to execute the invariant tests:
    /// * Fuzz dictionary
    /// * Targeted contracts
    /// * Invariant Strategy
    fn prepare_fuzzing(
        &mut self,
        invariant_contract: &InvariantContract,
    ) -> eyre::Result<InvariantPreparation> {
        // Finds out the chosen deployed contracts and/or senders.
        self.select_contract_artifacts(invariant_contract.address, invariant_contract.abi)?;
        let (targeted_senders, targeted_contracts) =
            self.select_contracts_and_senders(invariant_contract.address, invariant_contract.abi)?;

        if targeted_contracts.is_empty() {
            eyre::bail!("No contracts to fuzz.");
        }

        // Stores fuzz state for use with [fuzz_calldata_from_state].
        let fuzz_state: EvmFuzzState =
            build_initial_state(self.executor.backend.mem_db(), &self.config.dictionary);

        // During execution, any newly created contract is added here and used through the rest of
        // the fuzz run.
        let targeted_contracts: FuzzRunIdentifiedContracts =
            Arc::new(Mutex::new(targeted_contracts));

        // Creates the invariant strategy.
        let strat = invariant_strat(
            fuzz_state.clone(),
            targeted_senders,
            targeted_contracts.clone(),
            self.config.dictionary.dictionary_weight,
        )
        .no_shrink()
        .boxed();

        // Allows `override_call_strat` to use the address given by the Fuzzer inspector during
        // EVM execution.
        let mut call_generator = None;
        if self.config.call_override {
            let target_contract_ref = Arc::new(RwLock::new(Address::zero()));

            call_generator = Some(RandomCallGenerator::new(
                invariant_contract.address,
                self.runner.clone(),
                override_call_strat(
                    fuzz_state.clone(),
                    targeted_contracts.clone(),
                    target_contract_ref.clone(),
                ),
                target_contract_ref,
            ));
        }

        self.executor.inspector.fuzzer =
            Some(Fuzzer { call_generator, fuzz_state: fuzz_state.clone(), collect: true });

        Ok((fuzz_state, targeted_contracts, strat))
    }

    /// Fills the `InvariantExecutor` with the artifact identifier filters (in `path:name` string
    /// format). They will be used to filter contracts after the `setUp`, and more importantly,
    /// during the runs.
    ///
    /// Also excludes any contract without any mutable functions.
    ///
    /// Priority:
    ///
    /// targetArtifactSelectors > excludeArtifacts > targetArtifacts
    pub fn select_contract_artifacts(
        &mut self,
        invariant_address: Address,
        abi: &Abi,
    ) -> eyre::Result<()> {
        // targetArtifactSelectors -> (string, bytes4[])[].
        let targeted_abi = self
            .get_list::<(String, Vec<FixedBytes>)>(
                invariant_address,
                abi,
                "targetArtifactSelectors",
            )
            .into_iter()
            .map(|(contract, functions)| (contract, functions))
            .collect::<BTreeMap<_, _>>();

        // Insert them into the executor `targeted_abi`.
        for (contract, selectors) in targeted_abi {
            let identifier = self.validate_selected_contract(contract, &selectors)?;

            self.artifact_filters.targeted.entry(identifier).or_default().extend(selectors);
        }

        // targetArtifacts -> string[]
        // excludeArtifacts -> string[].
        let [selected_abi, excluded_abi] = ["targetArtifacts", "excludeArtifacts"]
            .map(|method| self.get_list::<String>(invariant_address, abi, method));

        // Insert `excludeArtifacts` into the executor `excluded_abi`.
        for contract in excluded_abi {
            let identifier = self.validate_selected_contract(contract, &[])?;

            if !self.artifact_filters.excluded.contains(&identifier) {
                self.artifact_filters.excluded.push(identifier);
            }
        }

        // Exclude any artifact without mutable functions.
        for (artifact, (abi, _)) in self.project_contracts.iter() {
            if abi
                .functions()
                .filter(|func| {
                    !matches!(
                        func.state_mutability,
                        ethers::abi::StateMutability::Pure | ethers::abi::StateMutability::View
                    )
                })
                .count() ==
                0 &&
                !self.artifact_filters.excluded.contains(&artifact.identifier())
            {
                self.artifact_filters.excluded.push(artifact.identifier());
            }
        }

        // Insert `targetArtifacts` into the executor `targeted_abi`, if they have not been seen
        // before.
        for contract in selected_abi {
            let identifier = self.validate_selected_contract(contract, &[])?;

            if !self.artifact_filters.targeted.contains_key(&identifier) &&
                !self.artifact_filters.excluded.contains(&identifier)
            {
                self.artifact_filters.targeted.insert(identifier, vec![]);
            }
        }

        Ok(())
    }

    /// Makes sure that the contract exists in the project. If so, it returns its artifact
    /// identifier.
    fn validate_selected_contract(
        &mut self,
        contract: String,
        selectors: &[FixedBytes],
    ) -> eyre::Result<String> {
        if let Some((artifact, (abi, _))) =
            self.project_contracts.find_by_name_or_identifier(&contract)?
        {
            // Check that the selectors really exist for this contract.
            for selector in selectors {
                abi.functions()
                    .find(|func| func.short_signature().as_slice() == selector.as_slice())
                    .wrap_err(format!("{contract} does not have the selector {selector:?}"))?;
            }

            return Ok(artifact.identifier())
        }
        eyre::bail!("{contract} not found in the project. Allowed format: `contract_name` or `contract_path:contract_name`.");
    }

    /// Selects senders and contracts based on the contract methods `targetSenders() -> address[]`,
    /// `targetContracts() -> address[]` and `excludeContracts() -> address[]`.
    pub fn select_contracts_and_senders(
        &self,
        invariant_address: Address,
        abi: &Abi,
    ) -> eyre::Result<(SenderFilters, TargetedContracts)> {
        let [targeted_senders, excluded_senders, selected, excluded] =
            ["targetSenders", "excludeSenders", "targetContracts", "excludeContracts"]
                .map(|method| self.get_list::<Address>(invariant_address, abi, method));

        let mut contracts: TargetedContracts = self
            .setup_contracts
            .clone()
            .into_iter()
            .filter(|(addr, (identifier, _))| {
                *addr != invariant_address &&
                    *addr != CHEATCODE_ADDRESS &&
                    *addr != HARDHAT_CONSOLE_ADDRESS &&
                    (selected.is_empty() || selected.contains(addr)) &&
                    (self.artifact_filters.targeted.is_empty() ||
                        self.artifact_filters.targeted.contains_key(identifier)) &&
                    (excluded.is_empty() || !excluded.contains(addr)) &&
                    (self.artifact_filters.excluded.is_empty() ||
                        !self.artifact_filters.excluded.contains(identifier))
            })
            .map(|(addr, (identifier, abi))| (addr, (identifier, abi, vec![])))
            .collect();

        self.select_selectors(invariant_address, abi, &mut contracts)?;

        Ok((SenderFilters::new(targeted_senders, excluded_senders), contracts))
    }

    /// Selects the functions to fuzz based on the contract method `targetSelectors()` and
    /// `targetArtifactSelectors()`.
    pub fn select_selectors(
        &self,
        address: Address,
        abi: &Abi,
        targeted_contracts: &mut TargetedContracts,
    ) -> eyre::Result<()> {
        // `targetArtifactSelectors() -> (string, bytes4[])[]`.
        let some_abi_selectors = self
            .artifact_filters
            .targeted
            .iter()
            .filter(|(_, selectors)| !selectors.is_empty())
            .collect::<BTreeMap<_, _>>();

        for (address, (identifier, _)) in self.setup_contracts.iter() {
            if let Some(selectors) = some_abi_selectors.get(identifier) {
                self.add_address_with_functions(
                    *address,
                    (*selectors).clone(),
                    targeted_contracts,
                )?;
            }
        }

        // `targetSelectors() -> (address, bytes4[])[]`.
        let selectors =
            self.get_list::<(Address, Vec<FixedBytes>)>(address, abi, "targetSelectors");

        for (address, bytes4_array) in selectors.into_iter() {
            self.add_address_with_functions(address, bytes4_array, targeted_contracts)?;
        }
        Ok(())
    }

    /// Adds the address and fuzzable functions to `TargetedContracts`.
    fn add_address_with_functions(
        &self,
        address: Address,
        bytes4_array: Vec<Vec<u8>>,
        targeted_contracts: &mut TargetedContracts,
    ) -> eyre::Result<()> {
        if let Some((name, abi, address_selectors)) = targeted_contracts.get_mut(&address) {
            // The contract is already part of our filter, and all we do is specify that we're
            // only looking at specific functions coming from `bytes4_array`.
            for selector in bytes4_array {
                address_selectors.push(get_function(name, &selector, abi)?);
            }
        } else {
            let (name, abi) = self.setup_contracts.get(&address).ok_or_else(|| {
                eyre::eyre!(
                    "[targetSelectors] address does not have an associated contract: {address}"
                )
            })?;

            let functions = bytes4_array
                .into_iter()
                .map(|selector| get_function(name, &selector, abi))
                .collect::<Result<Vec<_>, _>>()?;

            targeted_contracts.insert(address, (name.to_string(), abi.clone(), functions));
        }
        Ok(())
    }

    /// Gets list of `T` by calling the contract `method_name` function.
    fn get_list<T>(&self, address: Address, abi: &Abi, method_name: &str) -> Vec<T>
    where
        T: Tokenizable + Detokenize + TokenizableItem,
    {
        if let Some(func) = abi.functions().find(|func| func.name == method_name) {
            if let Ok(call_result) = self.executor.call::<Vec<T>, _, _>(
                CALLER,
                address,
                func.clone(),
                (),
                U256::zero(),
                Some(abi),
            ) {
                return call_result.result
            } else {
                warn!(
                    "The function {} was found but there was an error querying its data.",
                    method_name
                );
            }
        };

        Vec::new()
    }
}

/// Collects data from call for fuzzing. However, it first verifies that the sender is not an EOA
/// before inserting it into the dictionary. Otherwise, we flood the dictionary with
/// randomly generated addresses.
fn collect_data(
    state_changeset: &mut HashMap<B160, revm::primitives::Account>,
    sender: &Address,
    call_result: &RawCallResult,
    fuzz_state: EvmFuzzState,
    config: &FuzzDictionaryConfig,
) {
    // Verify it has no code.
    let mut has_code = false;
    if let Some(Some(code)) =
        state_changeset.get(&h160_to_b160(*sender)).map(|account| account.info.code.as_ref())
    {
        has_code = !code.is_empty();
    }

    // We keep the nonce changes to apply later.
    let mut sender_changeset = None;
    if !has_code {
        sender_changeset = state_changeset.remove(&h160_to_b160(*sender));
    }

    collect_state_from_call(&call_result.logs, &*state_changeset, fuzz_state, config);

    // Re-add changes
    if let Some(changed) = sender_changeset {
        state_changeset.insert(h160_to_b160(*sender), changed);
    }
}

/// Verifies that the invariant run execution can continue.
/// Returns the mapping of (Invariant Function Name -> Call Result, Logs, Traces) if invariants were
/// asserted.
#[allow(clippy::too_many_arguments)]
fn can_continue(
    invariant_contract: &InvariantContract,
    call_result: RawCallResult,
    executor: &Executor,
    calldata: &[BasicTxDetails],
    failures: &mut InvariantFailures,
    targeted_contracts: &FuzzRunIdentifiedContracts,
    state_changeset: StateChangeset,
    fail_on_revert: bool,
    shrink_sequence: bool,
) -> RichInvariantResults {
    let mut call_results = None;

    // Detect handler assertion failures first.
    let handlers_failed = targeted_contracts
        .lock()
        .iter()
        .any(|contract| !executor.is_success(*contract.0, false, state_changeset.clone(), false));

    // Assert invariants IFF the call did not revert and the handlers did not fail.
    if !call_result.reverted && !handlers_failed {
        call_results =
            assert_invariants(invariant_contract, executor, calldata, failures, shrink_sequence)
                .ok();
        if call_results.is_none() {
            return RichInvariantResults::new(false, None)
        }
    } else {
        failures.reverts += 1;

        // The user might want to stop all execution if a revert happens to
        // better bound their testing space.
        if fail_on_revert {
            let error = InvariantFuzzError::new(
                invariant_contract,
                None,
                calldata,
                call_result,
                &[],
                shrink_sequence,
            );

            failures.revert_reason = Some(error.revert_reason.clone());

            return RichInvariantResults::new(false, None)
        }
    }
    RichInvariantResults::new(true, call_results)
}

#[derive(Clone, Default)]
/// Stores information about failures and reverts of the invariant tests.
pub struct InvariantFailures {
    /// Total number of reverts.
    pub reverts: usize,
    /// How many different invariants have been broken.
    pub broken_invariants_count: usize,
    /// The latest revert reason of a run.
    pub revert_reason: Option<String>,
    /// Maps a broken invariant to its specific error.
    pub error: Option<InvariantFuzzError>,
}

impl InvariantFailures {
    fn new() -> Self {
        Self::default()
    }

    fn into_inner(self) -> (usize, Option<InvariantFuzzError>) {
        (self.reverts, self.error)
    }
}
