//! Result types shared by every check.

use serde::{Deserialize, Serialize};

/// Outcome of a single check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Pass,
    Fail,
    /// The check could not run (missing input, unreachable RPC, ...).
    Skipped,
}

/// Result of running one check against one target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    /// Stable identifier, e.g. `shape(UserMetadata)`.
    pub check: String,
    pub status: Status,
    /// Human-readable explanation. Populated for `Fail` and `Skipped`.
    pub detail: Option<String>,
}

impl CheckResult {
    pub fn pass(check: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            status: Status::Pass,
            detail: None,
        }
    }

    pub fn fail(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            status: Status::Fail,
            detail: Some(detail.into()),
        }
    }

    pub fn skipped(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            status: Status::Skipped,
            detail: Some(detail.into()),
        }
    }
}
