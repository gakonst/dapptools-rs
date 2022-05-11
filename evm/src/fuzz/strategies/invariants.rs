use ethers::{
    abi::{Abi, Function, ParamType},
    types::{Address, Bytes},
};
use std::collections::BTreeMap;

use proptest::prelude::*;
pub use proptest::test_runner::Config as FuzzConfig;

use crate::fuzz::strategies::fuzz_param;

pub fn invariant_strat(
    depth: usize,
    senders: Vec<Address>,
    contracts: BTreeMap<Address, (String, Abi)>,
) -> BoxedStrategy<Vec<(Address, (Address, Bytes))>> {
    let iters = 1..depth + 1;
    proptest::collection::vec(gen_call(senders, contracts), iters).boxed()
}

fn gen_call(
    senders: Vec<Address>,
    contracts: BTreeMap<Address, (String, Abi)>,
) -> BoxedStrategy<(Address, (Address, Bytes))> {
    let random_contract = select_random_contract(contracts);
    random_contract
        .prop_flat_map(move |(contract, abi)| {
            let func = select_random_function(abi);
            let senders = senders.clone();
            func.prop_flat_map(move |func| {
                let sender = select_random_sender(senders.clone());
                (sender, fuzz_contract_with_calldata(contract, func))
            })
        })
        .boxed()
}

fn select_random_sender(senders: Vec<Address>) -> impl Strategy<Value = Address> {
    let selectors = any::<prop::sample::Selector>();
    let senders_: Vec<Address> = senders.clone();

    if !senders.is_empty() {
        // todo should we do an union ? 80% selected 15% random + 0x0 address by default
        selectors.prop_map(move |selector| *selector.select(&senders_)).boxed()
    } else {
        let fuzz = fuzz_param(&ParamType::Address);
        fuzz.prop_map(move |selector| {
            // assurance above
            selector.into_address().unwrap()
        })
        .boxed()
    }
}

fn select_random_contract(
    contracts: BTreeMap<Address, (String, Abi)>,
) -> impl Strategy<Value = (Address, Abi)> {
    let selectors = any::<prop::sample::Selector>();
    selectors.prop_map(move |selector| {
        let res = selector.select(&contracts);
        (*res.0, res.1 .1.clone())
    })
}

fn select_random_function(abi: Abi) -> impl Strategy<Value = Function> {
    let selectors = any::<prop::sample::Selector>();
    let possible_funcs: Vec<ethers::abi::Function> = abi
        .functions()
        .filter(|func| {
            !matches!(
                func.state_mutability,
                ethers::abi::StateMutability::Pure | ethers::abi::StateMutability::View
            )
        })
        .cloned()
        .collect();
    selectors.prop_map(move |selector| {
        let func = selector.select(&possible_funcs);
        func.clone()
    })
}

/// Given a function, it returns a proptest strategy which generates valid abi-encoded calldata
/// for that function's input types.
pub fn fuzz_contract_with_calldata(
    contract: Address,
    func: Function,
) -> impl Strategy<Value = (Address, Bytes)> {
    // We need to compose all the strategies generated for each parameter in all
    // possible combinations
    let strats = func.inputs.iter().map(|input| fuzz_param(&input.kind)).collect::<Vec<_>>();

    strats.prop_map(move |tokens| {
        tracing::trace!(input = ?tokens);
        (contract, func.encode_input(&tokens).unwrap().into())
    })
}
