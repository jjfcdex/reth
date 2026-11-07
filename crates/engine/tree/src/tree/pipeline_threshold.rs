//! Startup-only Base mainnet backfill threshold selection.

use super::MIN_BLOCKS_FOR_PIPELINE_RUN;
use std::ffi::{OsStr, OsString};

const BASE_MAINNET_CHAIN_ID: u64 = 8453;
const BASE_DEFAULT_THRESHOLD: u64 = 1200;
const BASE_THRESHOLD_ENV: &str = "BASE_PIPELINE_RUN_THRESHOLD";

/// Read the override once when creating the handler, never during block processing.
pub(super) fn from_env(chain_id: u64) -> u64 {
    from_reader(chain_id, || std::env::var_os(BASE_THRESHOLD_ENV))
}

pub(super) fn from_reader(chain_id: u64, read: impl FnOnce() -> Option<OsString>) -> u64 {
    if chain_id != BASE_MAINNET_CHAIN_ID {
        return MIN_BLOCKS_FOR_PIPELINE_RUN
    }

    let value = read();
    let (threshold, invalid) = resolve(chain_id, value.as_deref());
    if invalid {
        tracing::warn!(
            target: "engine::tree",
            env = BASE_THRESHOLD_ENV,
            fallback = BASE_DEFAULT_THRESHOLD,
            "Invalid Base pipeline threshold; expected a positive u64, using default"
        );
    }
    tracing::info!(
        target: "engine::tree",
        chain_id,
        pipeline_run_threshold = threshold,
        env = BASE_THRESHOLD_ENV,
        "Configured Base pipeline run threshold"
    );
    threshold
}

/// Returns the effective threshold and whether an override was rejected.
fn resolve(chain_id: u64, value: Option<&OsStr>) -> (u64, bool) {
    if chain_id != BASE_MAINNET_CHAIN_ID {
        return (MIN_BLOCKS_FOR_PIPELINE_RUN, false)
    }
    let Some(value) = value else { return (BASE_DEFAULT_THRESHOLD, false) };
    let parsed = value.to_str().and_then(|value| {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return None
        }
        value.parse::<u64>().ok().filter(|threshold| *threshold > 0)
    });
    parsed.map_or((BASE_DEFAULT_THRESHOLD, true), |threshold| (threshold, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_pipeline_threshold_non_base_does_not_read_env() {
        for chain_id in [1, 10, 130, 84532] {
            assert_eq!(from_reader(chain_id, || panic!("must not read Base env")), 32);
        }
        let reads = std::cell::Cell::new(0);
        assert_eq!(
            from_reader(8453, || {
                reads.set(reads.get() + 1);
                None
            }),
            1200
        );
        assert_eq!(reads.get(), 1);
        assert_eq!(from_reader(8453, || Some("320".into())), 320);
    }

    #[test]
    fn base_pipeline_threshold_defaults_and_overrides() {
        assert_eq!(resolve(8453, None), (1200, false));
        for value in ["32", "320", "1200", "18446744073709551615"] {
            assert_eq!(resolve(8453, Some(OsStr::new(value))), (value.parse().unwrap(), false));
        }
        for chain_id in [1, 10, 130, 84532] {
            assert_eq!(resolve(chain_id, None), (32, false));
            assert_eq!(resolve(chain_id, Some(OsStr::new("320"))), (32, false));
        }
    }

    #[test]
    fn base_pipeline_threshold_invalid_values_fall_back() {
        for value in ["", "0", "-1", "+32", " 32", "32 ", "abc", "18446744073709551616"] {
            assert_eq!(resolve(8453, Some(OsStr::new(value))), (1200, true));
        }
    }

    #[cfg(unix)]
    #[test]
    fn base_pipeline_threshold_non_utf8_falls_back() {
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(resolve(8453, Some(OsStr::from_bytes(&[0xff]))), (1200, true));
    }
}
