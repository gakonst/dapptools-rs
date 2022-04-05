//! Aggregated error type for this module

use crate::eth::pool::transactions::PoolTransaction;
use ethers::types::SignatureError;
use forge_node_core::{
    error::RpcError,
    response::{ResponseResult, RpcResponse},
};
use serde::Serialize;
use tracing::{error, trace};

pub(crate) type Result<T> = std::result::Result<T, BlockchainError>;

#[derive(thiserror::Error, Debug)]
pub enum BlockchainError {
    #[error(transparent)]
    Pool(#[from] PoolError),
    #[error("No signer available")]
    NoSignerAvailable,
    #[error("Chain Id not available")]
    ChainIdNotAvailable,
    #[error("Invalid Transaction")]
    InvalidTransaction,
    #[error("Transaction data is empty")]
    EmptyRawTransactionData,
    #[error("Failed to decode signed transaction")]
    FailedToDecodeSignedTransaction,
    #[error(transparent)]
    SignatureError(#[from] SignatureError),
    #[error("Rpc Endpoint not implemented")]
    RpcUnimplemented,
}

/// Errors that can occur in the transaction pool
#[derive(thiserror::Error, Debug)]
pub enum PoolError {
    #[error("Transaction with cyclic dependent transactions")]
    CyclicTransaction,
    #[error("Invalid transaction")]
    InvalidTransaction(),
    #[error("Tx: [{0:?}] already imported")]
    AlreadyImported(Box<PoolTransaction>),
}

/// Helper trait to easily convert results to rpc results
pub(crate) trait ToRpcResponseResult {
    fn to_rpc_result(self) -> ResponseResult;
}

impl<T: Serialize> ToRpcResponseResult for Result<T> {
    fn to_rpc_result(self) -> ResponseResult {
        match self {
            Ok(val) => match serde_json::to_value(val) {
                Ok(success) => ResponseResult::Success(success),
                Err(err) => {
                    error!("Failed serialize rpc response: {:?}", err);
                    ResponseResult::error(RpcError::internal_error())
                }
            },
            Err(err) => match err {
                BlockchainError::Pool(err) => {
                    error!("txpool error: {:?}", err);
                    RpcError::internal_error()
                }
                BlockchainError::NoSignerAvailable => {
                    RpcError::invalid_params("No Signer available")
                }
                BlockchainError::ChainIdNotAvailable => {
                    RpcError::invalid_params("Chain Id not available")
                }
                BlockchainError::InvalidTransaction => {
                    RpcError::invalid_params("Invalid transaction")
                }
                BlockchainError::EmptyRawTransactionData => {
                    RpcError::invalid_params("Empty transaction data")
                }
                BlockchainError::FailedToDecodeSignedTransaction => {
                    RpcError::invalid_params("Failed to decode transaction")
                }
                BlockchainError::SignatureError(err) => RpcError::invalid_params(err.to_string()),
                BlockchainError::RpcUnimplemented => {
                    { RpcError::internal_error_with("Not implemented") }.into()
                }
            }
            .into(),
        }
    }
}
