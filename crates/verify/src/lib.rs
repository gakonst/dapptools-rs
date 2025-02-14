//! Smart contract verification.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

#[macro_use]
extern crate foundry_common;

#[macro_use]
extern crate tracing;

mod etherscan;

pub mod provider;

pub mod bytecode;
pub use bytecode::VerifyBytecodeArgs;

pub mod retry;
pub use retry::RetryArgs;

mod sourcify;

pub mod verify;
pub use verify::{VerifierArgs, VerifyArgs, VerifyCheckArgs};

mod types;

mod utils;

mod install;
pub use install::{install_missing_dependencies, DependencyInstallOpts};
