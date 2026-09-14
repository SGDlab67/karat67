//! Shape check: an account's observed payload must match the program IDL
//! exactly, by discriminator and by byte length.
//!
//! Origin: an indexer ingested zero-length payloads for `Obligation` and
//! `UserMetadata` accounts for 44 hours while every liveness signal stayed
//! green. The chain does not produce empty, truncated, over-long, or
//! unknown-discriminator data for allocated accounts, so this failure class
//! is detectable without any threshold.

use crate::report::CheckResult;

/// IDL-declared shape of one account type: its 8-byte Anchor discriminator
/// and its exact borsh-encoded size (discriminator included).
#[derive(Debug, Clone, Copy)]
pub struct AccountSpec {
    pub account_type: &'static str,
    pub discriminator: [u8; 8],
    pub data_len: usize,
}

/// Kamino Lending account specs, verified against the program's IDL-derived
/// decoders (discriminator and exact borsh-encoded size).
pub const KAMINO_ACCOUNTS: &[AccountSpec] = &[
    AccountSpec {
        account_type: "Obligation",
        discriminator: [168, 206, 141, 106, 88, 76, 172, 167],
        data_len: 3344,
    },
    AccountSpec {
        account_type: "UserMetadata",
        discriminator: [157, 214, 220, 235, 98, 135, 171, 28],
        data_len: 1032,
    },
    AccountSpec {
        account_type: "Reserve",
        discriminator: [43, 242, 204, 202, 26, 247, 59, 127],
        data_len: 8624,
    },
];

/// Check an observed account payload against the Kamino account registry.
///
/// Fails on: empty or too-short data (under 8 bytes), an unknown
/// discriminator, or a length that does not exactly match the matched
/// account type's IDL-declared size (truncated or over-long).
pub fn check_shape(data: &[u8]) -> CheckResult {
    if data.len() < 8 {
        return CheckResult::fail(
            "shape",
            format!(
                "payload too short: {} bytes, need at least 8 for a discriminator",
                data.len()
            ),
        );
    }

    let discriminator: [u8; 8] = data[..8].try_into().expect("checked len >= 8 above");
    let Some(spec) = KAMINO_ACCOUNTS
        .iter()
        .find(|spec| spec.discriminator == discriminator)
    else {
        return CheckResult::fail("shape", format!("unknown discriminator: {discriminator:?}"));
    };

    let name = format!("shape({})", spec.account_type);
    if data.len() == spec.data_len {
        CheckResult::pass(name)
    } else {
        CheckResult::fail(
            name,
            format!("expected {} bytes, got {}", spec.data_len, data.len()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Status;

    const OBLIGATION_DISCRIMINATOR: [u8; 8] = [168, 206, 141, 106, 88, 76, 172, 167];
    const OBLIGATION_LEN: usize = 3344;

    /// Builds a buffer of `total_len` bytes: the discriminator followed by
    /// zeroed filler.
    fn buffer_with_discriminator(discriminator: [u8; 8], total_len: usize) -> Vec<u8> {
        let mut data = vec![0u8; total_len];
        let copy_len = discriminator.len().min(total_len);
        data[..copy_len].copy_from_slice(&discriminator[..copy_len]);
        data
    }

    #[test]
    fn valid_obligation_passes() {
        let data = buffer_with_discriminator(OBLIGATION_DISCRIMINATOR, OBLIGATION_LEN);
        let result = check_shape(&data);
        assert_eq!(result.status, Status::Pass);
        assert_eq!(result.detail, None);
    }

    #[test]
    fn empty_payload_fails() {
        let result = check_shape(&[]);
        assert_eq!(result.status, Status::Fail);
    }

    #[test]
    fn shorter_than_discriminator_fails() {
        let data = buffer_with_discriminator(OBLIGATION_DISCRIMINATOR, 4);
        let result = check_shape(&data);
        assert_eq!(result.status, Status::Fail);
    }

    #[test]
    fn truncated_by_one_byte_fails() {
        let data = buffer_with_discriminator(OBLIGATION_DISCRIMINATOR, OBLIGATION_LEN - 1);
        let result = check_shape(&data);
        assert_eq!(result.status, Status::Fail);
    }

    #[test]
    fn over_long_by_one_byte_fails() {
        let data = buffer_with_discriminator(OBLIGATION_DISCRIMINATOR, OBLIGATION_LEN + 1);
        let result = check_shape(&data);
        assert_eq!(result.status, Status::Fail);
    }

    #[test]
    fn unknown_discriminator_fails() {
        let data = buffer_with_discriminator([0, 1, 2, 3, 4, 5, 6, 7], OBLIGATION_LEN);
        let result = check_shape(&data);
        assert_eq!(result.status, Status::Fail);
    }
}
