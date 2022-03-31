extern crate core;

// Macros useful for testing.
mod macros;

// Utilities for making it easier to handle tests.
pub mod util;
pub mod stdin;

pub use util::{TestCommand, TestProject};

pub use ethers_solc;
