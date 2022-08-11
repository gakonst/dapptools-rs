//! Tests for invariants

use crate::{config::*, test_helpers::filter::Filter};
use std::collections::BTreeMap;

#[test]
fn test_invariant() {
    let mut runner = runner();

    let results = runner
        .test(
            &Filter::new(".*", ".*", ".*fuzz/invariant/(target|targetAbi|common)"),
            None,
            TEST_OPTS,
        )
        .unwrap();

    assert_multiple(
        &results,
        BTreeMap::from([
            (
                "fuzz/invariant/common/InvariantInnerContract.t.sol:InvariantInnerContract",
                vec![("invariantHideJesus", false, Some("jesus betrayed.".into()), None, None)],
            ),
            (
                "fuzz/invariant/common/InvariantReentrancy.t.sol:InvariantReentrancy",
                vec![("invariantNotStolen", true, None, None, None)],
            ),
            (
                "fuzz/invariant/common/InvariantTest1.t.sol:InvariantTest",
                vec![("invariant_neverFalse", false, Some("false.".into()), None, None)],
            ),
            (
                "fuzz/invariant/target/ExcludeContracts.t.sol:ExcludeContracts",
                vec![("invariantTrueWorld", true, None, None, None)],
            ),
            (
                "fuzz/invariant/target/TargetContracts.t.sol:TargetContracts",
                vec![("invariantTrueWorld", true, None, None, None)],
            ),
            (
                "fuzz/invariant/target/TargetSenders.t.sol:TargetSenders",
                vec![("invariantTrueWorld", false, Some("false world.".into()), None, None)],
            ),
            (
                "fuzz/invariant/target/TargetSelectors.t.sol:TargetSelectors",
                vec![("invariantTrueWorld", true, None, None, None)],
            ),
            (
                "fuzz/invariant/targetAbi/ExcludeArtifacts.t.sol:ExcludeArtifacts",
                vec![("invariantShouldPass", true, None, None, None)],
            ),
            (
                "fuzz/invariant/targetAbi/TargetArtifacts.t.sol:TargetArtifacts",
                vec![
                    ("invariantShouldPass", true, None, None, None),
                    ("invariantShouldFail", false, Some("false world.".into()), None, None),
                ],
            ),
            (
                "fuzz/invariant/targetAbi/TargetArtifactSelectors.t.sol:TargetArtifactSelectors",
                vec![("invariantShouldPass", true, None, None, None)],
            ),
            (
                "fuzz/invariant/targetAbi/TargetArtifactSelectors2.t.sol:TargetArtifactSelectors2",
                vec![("invariantShouldFail", false, Some("its false.".into()), None, None)],
            ),
        ]),
    );
}

#[test]
fn test_invariant_override() {
    let mut runner = runner();

    let mut opts = TEST_OPTS;
    opts.invariant_call_override = true;
    runner.test_options = opts;

    let results = runner
        .test(
            &Filter::new(".*", ".*", ".*fuzz/invariant/common/InvariantReentrancy.t.sol"),
            None,
            opts,
        )
        .unwrap();

    assert_multiple(
        &results,
        BTreeMap::from([(
            "fuzz/invariant/common/InvariantReentrancy.t.sol:InvariantReentrancy",
            vec![("invariantNotStolen", false, Some("stolen.".into()), None, None)],
        )]),
    );
}

#[test]
fn test_invariant_storage() {
    let mut runner = runner();

    let mut opts = TEST_OPTS;
    opts.invariant_depth = 200;
    runner.test_options = opts;

    let results = runner
        .test(
            &Filter::new(".*", ".*", ".*fuzz/invariant/storage/InvariantStorageTest.t.sol"),
            None,
            opts,
        )
        .unwrap();

    assert_multiple(
        &results,
        BTreeMap::from([(
            "fuzz/invariant/storage/InvariantStorageTest.t.sol:InvariantStorageTest",
            vec![
                ("invariantChangeAddress", false, Some("changedAddr".into()), None, None),
                ("invariantChangeString", false, Some("changedStr".into()), None, None),
                ("invariantChangeUint", false, Some("changedUint".into()), None, None),
                ("invariantPush", false, Some("pushUint".into()), None, None),
            ],
        )]),
    );
}
