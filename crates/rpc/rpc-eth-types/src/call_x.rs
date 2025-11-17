//! CallX RPC response types - extended call response with logs and execution details

use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_eth::Log;
use serde::{Deserialize, Serialize};

/// Extended call result that includes logs and execution status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogOrRevert {
    /// Block number where the call was executed
    pub block_number: u64,
    /// Block hash where the call was executed
    pub block_hash: B256,
    /// Flashblock index where the call was executed
    pub flashblock_index: Option<u64>,
    /// Execution status (1 = success, 0 = failure)
    pub status: u64,
    /// Gas used by the call
    pub used_gas: u64,
    /// Logs generated during execution
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs: Option<Vec<Log>>,
    /// Return data from the call
    pub returns: Bytes,
    /// Revert error message if call reverted
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revert_error: Option<String>,
}

impl LogOrRevert {
    /// Creates a new successful call result
    pub fn new_success(
        block_number: u64,
        block_hash: B256,
        used_gas: u64,
        logs: Vec<Log>,
        returns: Bytes,
    ) -> Self {
        Self {
            block_number,
            block_hash,
            flashblock_index: None,
            status: 1, // success
            used_gas,
            logs: Some(logs),
            returns,
            revert_error: None,
        }
    }

    /// Creates a new failed call result
    pub fn new_failure(
        block_number: u64,
        block_hash: B256,
        used_gas: u64,
        logs: Option<Vec<Log>>,
        revert_error: Option<String>,
    ) -> Self {
        Self {
            block_number,
            block_hash,
            flashblock_index: None,
            status: 0, // failure
            used_gas,
            logs,
            returns: Bytes::new(),
            revert_error,
        }
    }
}

/// Optional parameters for CallX
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallXArgs {
    /// Whether to ignore logs in the response (for performance optimization)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore_logs: Option<bool>,
    /// Whether to evaluate gas usage
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eval_gas: Option<bool>,
}

impl CallXArgs {
    /// Returns true if logs should be ignored
    pub fn should_ignore_logs(&self) -> bool {
        self.ignore_logs.unwrap_or(false)
    }

    /// Returns true if gas should be evaluated
    pub fn should_eval_gas(&self) -> bool {
        self.eval_gas.unwrap_or(false)
    }
}
