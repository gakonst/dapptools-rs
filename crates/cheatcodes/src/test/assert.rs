use std::fmt::Display;

use itertools::Itertools;

use crate::{Cheatcode, Cheatcodes, Error, Result, Vm::*};

#[derive(Debug, thiserror::Error)]
#[error("Assertion failed")]
struct SimpleAssertionError;

#[derive(thiserror::Error, Debug)]
enum ComparisonAssertionError<'a, T> {
    NotEq(&'a T, &'a T),
    Eq(&'a T, &'a T),
    Ge(&'a T, &'a T),
    Gt(&'a T, &'a T),
    Le(&'a T, &'a T),
    Lt(&'a T, &'a T),
}

impl<'a, T: Display> ComparisonAssertionError<'a, T> {
    fn format_for_values(&self) -> String {
        match self {
            Self::NotEq(a, b) => format!("{} == {}", a, b),
            Self::Eq(a, b) => format!("{} != {}", a, b),
            Self::Ge(a, b) => format!("{} < {}", a, b),
            Self::Gt(a, b) => format!("{} <= {}", a, b),
            Self::Le(a, b) => format!("{} > {}", a, b),
            Self::Lt(a, b) => format!("{} >= {}", a, b),
        }
    }
}

impl<'a, T: Display> ComparisonAssertionError<'a, Vec<T>> {
    fn format_for_arrays(&self) -> String {
        match self {
            Self::NotEq(a, b) => {
                format!("[{}] == [{}]", a.iter().join(", "), b.iter().join(", "))
            }
            Self::Eq(a, b) => {
                format!("[{}] != [{}]", a.iter().join(", "), b.iter().join(", "))
            }
            Self::Ge(a, b) => {
                format!("[{}] < [{}]", a.iter().join(", "), b.iter().join(", "))
            }
            Self::Gt(a, b) => {
                format!("[{}] <= [{}]", a.iter().join(", "), b.iter().join(", "))
            }
            Self::Le(a, b) => {
                format!("[{}] > [{}]", a.iter().join(", "), b.iter().join(", "))
            }
            Self::Lt(a, b) => {
                format!("[{}] >= [{}]", a.iter().join(", "), b.iter().join(", "))
            }
        }
    }
}

type ComparisonResult<'a, T> = Result<Vec<u8>, ComparisonAssertionError<'a, T>>;

impl Cheatcode for assertTrue_0Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        Ok(assert_true(self.condition).map_err(|_| "Assertion failed")?)
    }
}

impl Cheatcode for assertTrue_1Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        Ok(assert_true(self.condition).map_err(|_| self.error.to_string())?)
    }
}

impl Cheatcode for assertEq_0Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertEq_1Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertEq_2Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertEq_3Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertEq_4Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertEq_5Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertEq_6Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertEq_7Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertEq_8Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertEq_9Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertEq_10Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_eq(&hex::encode_prefixed(a), &hex::encode_prefixed(b))
            .map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertEq_11Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_eq(&hex::encode_prefixed(a), &hex::encode_prefixed(b))
            .map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertEq_12Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_eq(&hex::encode_prefixed(a), &hex::encode_prefixed(b))
            .map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertEq_13Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_eq(&hex::encode_prefixed(a), &hex::encode_prefixed(b))
            .map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertEq_14Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertEq_15Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertEq_16Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertEq_17Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertEq_18Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertEq_19Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertEq_20Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertEq_21Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertEq_22Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertEq_23Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertEq_24Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertEq_25Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertEq_26Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        let a = a.iter().map(hex::encode_prefixed).collect::<Vec<_>>();
        let b = b.iter().map(hex::encode_prefixed).collect::<Vec<_>>();
        Ok(assert_eq(&a, &b).map_err(|e| format!("Assertion failed: {}", e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertEq_27Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        let a = a.iter().map(hex::encode_prefixed).collect::<Vec<_>>();
        let b = b.iter().map(hex::encode_prefixed).collect::<Vec<_>>();
        Ok(assert_eq(&a, &b).map_err(|e| format!("{}: {}", error, e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertNotEq_0Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_not_eq(a, b)
            .map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertNotEq_1Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_not_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertNotEq_2Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_not_eq(a, b)
            .map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertNotEq_3Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_not_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertNotEq_4Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_not_eq(a, b)
            .map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertNotEq_5Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_not_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertNotEq_6Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_not_eq(a, b)
            .map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertNotEq_7Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_not_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertNotEq_8Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_not_eq(a, b)
            .map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertNotEq_9Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_not_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertNotEq_10Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_not_eq(a, b)
            .map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertNotEq_11Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_not_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertNotEq_12Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_not_eq(&hex::encode_prefixed(a), &hex::encode_prefixed(b))
            .map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertNotEq_13Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_not_eq(&hex::encode_prefixed(a), &hex::encode_prefixed(b))
            .map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertNotEq_14Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_not_eq(a, b)
            .map_err(|e| format!("Assertion failed: {}", e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertNotEq_15Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_not_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertNotEq_16Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_not_eq(a, b)
            .map_err(|e| format!("Assertion failed: {}", e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertNotEq_17Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_not_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertNotEq_18Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_not_eq(a, b)
            .map_err(|e| format!("Assertion failed: {}", e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertNotEq_19Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_not_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertNotEq_20Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_not_eq(a, b)
            .map_err(|e| format!("Assertion failed: {}", e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertNotEq_21Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_not_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertNotEq_22Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_not_eq(a, b)
            .map_err(|e| format!("Assertion failed: {}", e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertNotEq_23Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_not_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertNotEq_24Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_not_eq(a, b)
            .map_err(|e| format!("Assertion failed: {}", e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertNotEq_25Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_not_eq(a, b).map_err(|e| format!("{}: {}", error, e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertNotEq_26Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        let a = a.iter().map(hex::encode_prefixed).collect::<Vec<_>>();
        let b = b.iter().map(hex::encode_prefixed).collect::<Vec<_>>();
        Ok(assert_not_eq(&a, &b)
            .map_err(|e| format!("Assertion failed: {}", e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertNotEq_27Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        let a = a.iter().map(hex::encode_prefixed).collect::<Vec<_>>();
        let b = b.iter().map(hex::encode_prefixed).collect::<Vec<_>>();
        Ok(assert_not_eq(&a, &b).map_err(|e| format!("{}: {}", error, e.format_for_arrays()))?)
    }
}

impl Cheatcode for assertGt_0Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_gt(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertGt_1Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_gt(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertGt_2Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_gt(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertGt_3Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_gt(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertGe_0Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_ge(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertGe_1Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_ge(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertGe_2Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_ge(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertGe_3Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_ge(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertLt_0Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_lt(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertLt_1Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_lt(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertLt_2Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_lt(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertLt_3Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_lt(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertLe_0Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_le(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertLe_1Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_le(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

impl Cheatcode for assertLe_2Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b } = self;
        Ok(assert_le(a, b).map_err(|e| format!("Assertion failed: {}", e.format_for_values()))?)
    }
}

impl Cheatcode for assertLe_3Call {
    fn apply(&self, _state: &mut Cheatcodes) -> Result {
        let Self { a, b, error } = self;
        Ok(assert_le(a, b).map_err(|e| format!("{}: {}", error, e.format_for_values()))?)
    }
}

fn assert_true(condition: bool) -> Result<Vec<u8>, SimpleAssertionError> {
    if condition {
        Ok(Default::default())
    } else {
        Err(SimpleAssertionError)
    }
}

fn assert_eq<'a, T: PartialEq>(a: &'a T, b: &'a T) -> ComparisonResult<'a, T> {
    if a == b {
        Ok(Default::default())
    } else {
        Err(ComparisonAssertionError::Eq(a, b))
    }
}

fn assert_not_eq<'a, T: PartialEq>(a: &'a T, b: &'a T) -> ComparisonResult<'a, T> {
    if a != b {
        Ok(Default::default())
    } else {
        Err(ComparisonAssertionError::NotEq(a, b))
    }
}

fn assert_gt<'a, T: PartialOrd>(a: &'a T, b: &'a T) -> ComparisonResult<'a, T> {
    if a > b {
        Ok(Default::default())
    } else {
        Err(ComparisonAssertionError::Gt(a, b))
    }
}

fn assert_ge<'a, T: PartialOrd>(a: &'a T, b: &'a T) -> ComparisonResult<'a, T> {
    if a >= b {
        Ok(Default::default())
    } else {
        Err(ComparisonAssertionError::Ge(a, b))
    }
}

fn assert_lt<'a, T: PartialOrd>(a: &'a T, b: &'a T) -> ComparisonResult<'a, T> {
    if a < b {
        Ok(Default::default())
    } else {
        Err(ComparisonAssertionError::Lt(a, b))
    }
}

fn assert_le<'a, T: PartialOrd>(a: &'a T, b: &'a T) -> ComparisonResult<'a, T> {
    if a <= b {
        Ok(Default::default())
    } else {
        Err(ComparisonAssertionError::Le(a, b))
    }
}
