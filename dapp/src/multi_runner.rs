use crate::{runner::TestResult, ContractRunner};
use evm_adapters::Evm;

use ethers::{
    abi::Abi,
    prelude::ArtifactOutput,
    solc::{Artifact, Project},
    types::{Address, U256},
};

use proptest::test_runner::TestRunner;
use regex::Regex;

use eyre::{Context, Result};
use std::{
    collections::{BTreeMap, HashMap},
    marker::PhantomData,
};

/// Builder used for instantiating the multi-contract runner
#[derive(Debug, Default)]
pub struct MultiContractRunnerBuilder {
    /// The fuzzer to be used for running fuzz tests
    pub fuzzer: Option<TestRunner>,
    /// The address which will be used to deploy the initial contracts
    pub deployer: Address,
    /// The initial balance for each one of the deployed smart contracts
    pub initial_balance: U256,
}

impl MultiContractRunnerBuilder {
    /// Given an EVM, proceeds to return a runner which is able to execute all tests
    /// against that evm
    pub fn build<A, E, S>(
        self,
        project: Project<A>,
        mut evm: E,
    ) -> Result<MultiContractRunner<E, S>>
    where
        // TODO: Can we remove the static? It's due to the `into_artifacts()` call below
        A: ArtifactOutput + 'static,
        E: Evm<S>,
    {
        let output = project.compile()?;
        if output.is_unchanged() {
            println!("no files changed, compilation skippped.");
        } else if output.has_compiler_errors() {
            // return the diagnostics error back to the user.
            eyre::bail!(output.to_string())
        } else {
            println!("success.");
        }

        let deployer = self.deployer;
        let initial_balance = self.initial_balance;

        // This is just the contracts compiled, but we need to merge this with the read cached
        // artifacts
        let contracts = output.into_artifacts();
        let contracts: BTreeMap<String, (Abi, Address, Vec<String>)> = contracts
            .map(|(fname, contract)| {
                let (abi, bytecode) = contract.into_inner();
                (fname, abi.unwrap(), bytecode.unwrap())
            })
            // Only take contracts with empty constructors.
            .filter(|(_, abi, _)| {
                abi.constructor.as_ref().map(|c| c.inputs.is_empty()).unwrap_or(true)
            })
            // Only take contracts which contain a `test` function
            .filter(|(_, abi, _)| abi.functions().any(|func| func.name.starts_with("test")))
            // deploy the contracts
            .map(|(name, abi, bytecode)| {
                let span = tracing::trace_span!("deploying", ?name);
                let _enter = span.enter();

                let (addr, _, _, logs) = evm
                    .deploy(deployer, bytecode, 0.into())
                    .wrap_err(format!("could not deploy {}", name))?;

                evm.set_balance(addr, initial_balance);
                Ok((name, (abi, addr, logs)))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;

        Ok(MultiContractRunner { contracts, evm, state: PhantomData, fuzzer: self.fuzzer })
    }

    pub fn deployer(mut self, deployer: Address) -> Self {
        self.deployer = deployer;
        self
    }

    pub fn initial_balance(mut self, initial_balance: U256) -> Self {
        self.initial_balance = initial_balance;
        self
    }

    pub fn fuzzer(mut self, fuzzer: TestRunner) -> Self {
        self.fuzzer = Some(fuzzer);
        self
    }
}

/// A multi contract runner receives a set of contracts deployed in an EVM instance and proceeds
/// to run all test functions in these contracts.
pub struct MultiContractRunner<E, S> {
    /// Mapping of contract name to compiled bytecode, deployed address and logs emitted during
    /// deployment
    contracts: BTreeMap<String, (Abi, Address, Vec<String>)>,
    /// The EVM instance used in the test runner
    evm: E,
    /// The fuzzer which will be used to run parametric tests (w/ non-0 solidity args)
    fuzzer: Option<TestRunner>,
    /// Market type for the EVM state being used
    state: PhantomData<S>,
}

impl<E, S> MultiContractRunner<E, S>
where
    E: Evm<S>,
    S: Clone,
{
    pub fn test(&mut self, pattern: Regex) -> Result<HashMap<String, HashMap<String, TestResult>>> {
        // TODO: Convert to iterator, ideally parallel one?
        let contracts = std::mem::take(&mut self.contracts);
        let results = contracts
            .iter()
            .map(|(name, (abi, address, logs))| {
                let result = self.run_tests(name, abi, *address, logs, &pattern)?;
                Ok((name.clone(), result))
            })
            .filter_map(|x: Result<_>| x.ok())
            .filter_map(|(name, res)| if res.is_empty() { None } else { Some((name, res)) })
            .collect::<HashMap<_, _>>();

        self.contracts = contracts;

        Ok(results)
    }

    // The _name field is unused because we only want it for tracing
    #[tracing::instrument(
        name = "contract",
        skip_all,
        err,
        fields(name = %_name)
    )]
    fn run_tests(
        &mut self,
        _name: &str,
        contract: &Abi,
        address: Address,
        init_logs: &[String],
        pattern: &Regex,
    ) -> Result<HashMap<String, TestResult>> {
        let mut runner = ContractRunner::new(&mut self.evm, contract, address, init_logs);
        runner.run_tests(pattern, self.fuzzer.as_mut())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_multi_runner<S: Clone, E: Evm<S>>(evm: E) {
        let mut runner =
            MultiContractRunnerBuilder::default().contracts("./GreetTest.sol").build(evm).unwrap();

        let results = runner.test(Regex::new(".*").unwrap()).unwrap();

        // 2 contracts
        assert_eq!(results.len(), 2);

        // 3 tests on greeter 1 on gm
        assert_eq!(results["GreeterTest"].len(), 3);
        assert_eq!(results["GmTest"].len(), 1);
        for (_, res) in results {
            assert!(res.iter().all(|(_, result)| result.success));
        }

        let only_gm = runner.test(Regex::new("testGm.*").unwrap()).unwrap();
        assert_eq!(only_gm.len(), 1);
        assert_eq!(only_gm["GmTest"].len(), 1);
    }

    fn test_ds_test_fail<S: Clone, E: Evm<S>>(evm: E) {
        let mut runner =
            MultiContractRunnerBuilder::default().contracts("./../FooTest.sol").build(evm).unwrap();
        let results = runner.test(Regex::new(".*").unwrap()).unwrap();
        let test = results.get("FooTest").unwrap().get("testFailX").unwrap();
        assert!(test.success);
    }

    mod sputnik {
        use super::*;
        use evm::Config;
        use evm_adapters::sputnik::{
            helpers::{new_backend, new_vicinity},
            Executor,
        };

        #[test]
        fn test_sputnik_debug_logs() {
            let config = Config::istanbul();
            let gas_limit = 12_500_000;
            let env = new_vicinity();
            let backend = new_backend(&env, Default::default());
            let evm = Executor::new_with_cheatcodes(backend, gas_limit, &config, false);

            let mut runner = MultiContractRunnerBuilder::default()
                .contracts("./testdata/DebugLogsTest.sol")
                .libraries(&["../evm-adapters/testdata".to_owned()])
                .build(evm)
                .unwrap();

            let results = runner.test(Regex::new(".*").unwrap()).unwrap();
            let reasons = results["DebugLogsTest"]
                .iter()
                .map(|(name, res)| (name, res.logs.clone()))
                .collect::<HashMap<_, _>>();
            assert_eq!(
                reasons[&"test1".to_owned()],
                vec!["constructor".to_owned(), "setUp".to_owned(), "one".to_owned()]
            );
            assert_eq!(
                reasons[&"test2".to_owned()],
                vec!["constructor".to_owned(), "setUp".to_owned(), "two".to_owned()]
            );
        }

        #[test]
        fn test_sputnik_multi_runner() {
            let config = Config::istanbul();
            let gas_limit = 12_500_000;
            let env = new_vicinity();
            let backend = new_backend(&env, Default::default());
            let evm = Executor::new(gas_limit, &config, &backend);
            test_multi_runner(evm);
        }

        #[test]
        fn test_sputnik_ds_test_fail() {
            let config = Config::istanbul();
            let gas_limit = 12_500_000;
            let env = new_vicinity();
            let backend = new_backend(&env, Default::default());
            let evm = Executor::new(gas_limit, &config, &backend);
            test_ds_test_fail(evm);
        }
    }

    // TODO: Add EvmOdin tests once we get the Mocked Host working
}
