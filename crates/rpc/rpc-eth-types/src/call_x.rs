//! CallX RPC response types - extended call response with logs and execution details

use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_eth::Log;
use serde::{Deserialize, Serialize};

/// Helper module for serializing u64 as hex string
mod hex_u64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("0x{:x}", value))
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<u64, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let s = s.trim_start_matches("0x");
        u64::from_str_radix(s, 16).map_err(serde::de::Error::custom)
    }
}

/// Extended call result that includes logs and execution status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogOrRevert {
    /// Block number where the call was executed
    #[serde(with = "hex_u64")]
    pub block_number: u64,
    /// Block hash where the call was executed
    pub block_hash: B256,
    /// Flashblock base hash where the call was executed
    pub flashblock_base_hash: Option<B256>,
    /// Flashblock index where the call was executed
    pub flashblock_index: Option<u64>,
    /// Flashblock number where the call was executed
    pub flashblock_number: Option<u64>,
    /// Execution status (1 = success, 0 = failure)
    #[serde(with = "hex_u64")]
    pub status: u64,
    /// Gas used by the call
    #[serde(with = "hex_u64")]
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
            flashblock_base_hash: None,
            flashblock_index: None,
            flashblock_number: None,
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
            flashblock_base_hash: None,
            flashblock_index: None,
            flashblock_number: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_success_preserves_wire_fields() {
        let result = LogOrRevert::new_success(
            0x123,
            B256::ZERO,
            0x5208,
            Vec::new(),
            Bytes::from_static(&[0xab, 0xcd]),
        );
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(
            value,
            json!({
                "blockNumber": "0x123",
                "blockHash": B256::ZERO,
                "flashblockBaseHash": null,
                "flashblockIndex": null,
                "flashblockNumber": null,
                "status": "0x1",
                "usedGas": "0x5208",
                "logs": [],
                "returns": "0xabcd"
            })
        );
        assert_eq!(serde_json::from_value::<LogOrRevert>(value).unwrap(), result);
    }

    #[test]
    fn failure_preserves_empty_returns_and_optional_logs() {
        for logs in [None, Some(Vec::new())] {
            let result = LogOrRevert::new_failure(
                0,
                B256::ZERO,
                0x100,
                logs.clone(),
                Some("execution reverted".to_string()),
            );
            let value = serde_json::to_value(&result).unwrap();
            assert_eq!(value["status"], "0x0");
            assert_eq!(value["returns"], "0x");
            assert_eq!(value["revertError"], "execution reverted");
            assert_eq!(value.get("logs").is_some(), logs.is_some());
            assert_eq!(serde_json::from_value::<LogOrRevert>(value).unwrap(), result);
        }
    }

    #[test]
    fn call_arguments_preserve_defaults_and_camel_case() {
        let defaults: CallXArgs = serde_json::from_value(json!({})).unwrap();
        assert!(!defaults.should_ignore_logs());
        assert!(!defaults.should_eval_gas());
        assert_eq!(serde_json::to_value(defaults).unwrap(), json!({}));

        let args: CallXArgs = serde_json::from_value(json!({
            "ignoreLogs": true,
            "evalGas": true
        }))
        .unwrap();
        assert!(args.should_ignore_logs());
        assert!(args.should_eval_gas());
    }
}
