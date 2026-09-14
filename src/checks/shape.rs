//! Shape check: an account's observed data length must match one of the
//! layouts the program IDL declares for it.
//!
//! Origin: a request-level data slice made an indexer ingest zero-length
//! payloads for two account types for 44 hours while every liveness signal
//! stayed green. The chain does not produce zero-length data for allocated
//! accounts, so this failure class is detectable without any threshold.

use crate::report::CheckResult;

/// Expected account data lengths (bytes) for one account type, taken from
/// the program IDL.
#[derive(Debug, Clone)]
pub struct Layouts {
    pub account_type: String,
    pub data_lens: Vec<usize>,
}

/// Check an observed account payload length against the declared layouts.
///
/// `observed_len == 0` models an empty payload, the incident's failure shape.
pub fn check_shape(layouts: &Layouts, observed_len: usize) -> CheckResult {
    let name = format!("shape({})", layouts.account_type);
    if observed_len == 0 {
        return CheckResult::fail(
            name,
            "empty payload: the chain does not produce zero-length data for allocated accounts",
        );
    }
    if layouts.data_lens.contains(&observed_len) {
        CheckResult::pass(name)
    } else {
        CheckResult::fail(
            name,
            format!(
                "data length {observed_len} is not declared by the IDL (expected one of {:?})",
                layouts.data_lens
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Status;

    fn user_metadata() -> Layouts {
        Layouts {
            account_type: "UserMetadata".to_string(),
            data_lens: vec![1032],
        }
    }

    #[test]
    fn declared_length_passes() {
        let result = check_shape(&user_metadata(), 1032);
        assert_eq!(result.status, Status::Pass);
        assert_eq!(result.detail, None);
    }

    #[test]
    fn empty_payload_fails() {
        let result = check_shape(&user_metadata(), 0);
        assert_eq!(result.status, Status::Fail);
    }

    #[test]
    fn undeclared_length_fails() {
        let result = check_shape(&user_metadata(), 4664);
        assert_eq!(result.status, Status::Fail);
    }
}
