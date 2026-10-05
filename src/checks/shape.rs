//! Shape check: an account's observed payload must match the program IDL
//! exactly, by discriminator and by byte length.
//!
//! Origin: an indexer ingested zero-length payloads for `Obligation` and
//! `UserMetadata` accounts for 158.91 hours while every liveness signal stayed
//! green. The chain does not produce empty, truncated, over-long, or
//! unknown-discriminator data for allocated accounts, so this failure class
//! is detectable without any threshold.
//!
//! Account layouts live in [`crate::checks::specs`] — the single IDL-informed
//! registry. This module never hard-codes per-type sizes or discriminators;
//! adding a type is a new [`AccountSpec`] row, not a new match arm here.

use crate::checks::check::Check;
use crate::checks::specs::{LenRule, find_by_discriminator};
use crate::report::CheckResult;

// Re-export registry types callers historically imported from this module.
pub use crate::checks::specs::{AccountSpec, KAMINO_ACCOUNTS, METEORA_DBC_ACCOUNTS};

/// Check an observed account payload against the account registry.
///
/// Fails on: empty or too-short data (under 8 bytes), an unknown
/// discriminator, or a length the matched account type's rule rejects. For a
/// fixed-layout type that means any length other than the IDL-declared size
/// (truncated or over-long); for a type carrying variable-length fields it
/// means a length below the declared floor, since nothing above the floor is
/// evidence of corruption.
///
/// All type-specific knowledge comes from the registry tables; there is no
/// per-account match arm in this function. The only branch is on the kind of
/// length rule, which is itself registry data.
pub fn check_shape(data: &[u8]) -> CheckResult {
    if data.len() < 8 {
        let what = if data.is_empty() {
            "empty payload"
        } else {
            "payload too short"
        };
        return CheckResult::fail(
            "shape",
            format!(
                "{what}: {} bytes, every allocated account starts with an 8-byte discriminator",
                data.len()
            ),
        );
    }

    let discriminator: [u8; 8] = data[..8].try_into().expect("checked len >= 8 above");
    let Some(spec) = find_by_discriminator(discriminator) else {
        return CheckResult::fail("shape", format!("unknown discriminator: {discriminator:?}"));
    };

    let name = format!("shape({})", spec.account_type);
    match spec.len_rule {
        LenRule::Exact => {
            if data.len() == spec.data_len {
                return CheckResult::pass(name);
            }
            let (what, diff) = if data.len() < spec.data_len {
                (
                    "truncated",
                    format!("{} missing", spec.data_len - data.len()),
                )
            } else {
                ("over-long", format!("{} extra", data.len() - spec.data_len))
            };
            CheckResult::fail(
                name,
                format!(
                    "{what}: expected {} bytes, got {} ({diff})",
                    spec.data_len,
                    data.len()
                ),
            )
        }
        // A variable-length type has no upper bound to check, so the only
        // failure is an underrun. It is worded differently on purpose: the
        // number quoted is a floor, and a reader who sees "expected 148 bytes"
        // would go looking for a fixed layout that does not exist.
        LenRule::AtLeast => {
            if data.len() >= spec.data_len {
                CheckResult::pass(name)
            } else {
                CheckResult::fail(
                    name,
                    format!(
                        "under minimum: expected at least {} bytes, got {} ({} missing)",
                        spec.data_len,
                        data.len(),
                        spec.data_len - data.len()
                    ),
                )
            }
        }
    }
}

/// A shape check over one observed account payload.
///
/// Thin wrapper that lets the shape check participate in the [`Check`] seam
/// alongside reconciliation and completeness. The logic still lives in
/// [`check_shape`]; this only borrows the payload and delegates.
pub struct ShapeCheck<'a> {
    data: &'a [u8],
}

impl<'a> ShapeCheck<'a> {
    /// Build a shape check over `data`, an observed account payload.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }
}

impl Check for ShapeCheck<'_> {
    fn name(&self) -> String {
        // Mirror the `check` field `check_shape` will produce so callers can
        // predict the identifier before running: name by matched account
        // type when the discriminator is known, else the bare `shape`.
        if self.data.len() >= 8 {
            let discriminator: [u8; 8] = self.data[..8].try_into().expect("checked len >= 8");
            if let Some(spec) = find_by_discriminator(discriminator) {
                return format!("shape({})", spec.account_type);
            }
        }
        "shape".to_string()
    }

    fn run(&self) -> CheckResult {
        check_shape(self.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::specs::find_by_account_type;
    use crate::report::Status;

    const OBLIGATION_DISCRIMINATOR: [u8; 8] = [168, 206, 141, 106, 88, 76, 172, 167];
    const OBLIGATION_LEN: usize = 3344;

    const VIRTUAL_POOL_DISCRIMINATOR: [u8; 8] = [213, 224, 5, 209, 98, 69, 119, 92];
    const VIRTUAL_POOL_LEN: usize = 424;
    /// Same on-chain length as `VirtualPool`; formerly the "unresolved 424"
    /// impostor, now registered as `TransferHookPool`.
    const TRANSFER_HOOK_POOL_DISCRIMINATOR: [u8; 8] = [237, 219, 184, 23, 42, 189, 169, 35];
    const PARTNER_METADATA_DISCRIMINATOR: [u8; 8] = [68, 68, 130, 19, 16, 209, 98, 156];
    const PARTNER_METADATA_FLOOR: usize = 148;
    const VIRTUAL_POOL_METADATA_DISCRIMINATOR: [u8; 8] = [217, 37, 82, 250, 43, 47, 228, 254];
    const VIRTUAL_POOL_METADATA_FLOOR: usize = 148;
    const OPERATOR_DISCRIMINATOR: [u8; 8] = [219, 31, 188, 145, 69, 139, 204, 117];
    const OPERATOR_LEN: usize = 72;
    const CONFIG_WITH_TRANSFER_HOOK_DISCRIMINATOR: [u8; 8] = [40, 220, 194, 251, 41, 199, 123, 253];
    const CONFIG_WITH_TRANSFER_HOOK_LEN: usize = 1128;

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
        assert_eq!(
            result.detail.as_deref(),
            Some("truncated: expected 3344 bytes, got 3343 (1 missing)")
        );
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

    #[test]
    fn check_trait_run_matches_check_shape() {
        let data = buffer_with_discriminator(OBLIGATION_DISCRIMINATOR, OBLIGATION_LEN);
        let via_trait = ShapeCheck::new(&data).run();
        let via_fn = check_shape(&data);
        assert_eq!(via_trait.status, via_fn.status);
        assert_eq!(via_trait.check, via_fn.check);
    }

    #[test]
    fn check_trait_name_matches_result_for_known_type() {
        let data = buffer_with_discriminator(OBLIGATION_DISCRIMINATOR, OBLIGATION_LEN);
        let check = ShapeCheck::new(&data);
        assert_eq!(check.name(), "shape(Obligation)");
        assert_eq!(check.name(), check.run().check);
    }

    #[test]
    fn check_trait_name_falls_back_for_unknown_discriminator() {
        let data = buffer_with_discriminator([0, 1, 2, 3, 4, 5, 6, 7], OBLIGATION_LEN);
        assert_eq!(ShapeCheck::new(&data).name(), "shape");
    }

    /// Lock: every registry entry — including types added as data only —
    /// is accepted by `check_shape` with no per-type logic in that function.
    #[test]
    fn adding_spec_is_data_only() {
        assert!(
            KAMINO_ACCOUNTS.len() >= 4,
            "registry must include LendingMarket as a fourth data-only entry"
        );
        assert!(
            find_by_account_type("LendingMarket").is_some(),
            "LendingMarket must be discoverable by name from the registry alone"
        );

        for spec in KAMINO_ACCOUNTS {
            let data = buffer_with_discriminator(spec.discriminator, spec.data_len);
            let result = check_shape(&data);
            assert_eq!(
                result.status,
                Status::Pass,
                "{} at {} bytes should pass via registry lookup only",
                spec.account_type,
                spec.data_len
            );
            assert_eq!(result.check, format!("shape({})", spec.account_type));
            assert_eq!(
                ShapeCheck::new(&data).name(),
                format!("shape({})", spec.account_type)
            );
        }
    }

    /// Lock: Kamino rules stay exact. A variable-length kind now exists in
    /// the registry, and it must not have loosened the fixed-layout types.
    #[test]
    fn kamino_specs_still_reject_over_long_payloads() {
        for spec in KAMINO_ACCOUNTS {
            let data = buffer_with_discriminator(spec.discriminator, spec.data_len + 1);
            let result = check_shape(&data);
            assert_eq!(
                result.status,
                Status::Fail,
                "{} must still fail one byte over",
                spec.account_type
            );
        }
    }

    /// Every DBC type added as data only is accepted at its observed length
    /// through the same registry lookup, with no DBC branch in check_shape.
    #[test]
    fn dbc_specs_pass_at_their_registered_length() {
        assert_eq!(METEORA_DBC_ACCOUNTS.len(), 10);
        for spec in METEORA_DBC_ACCOUNTS {
            let data = buffer_with_discriminator(spec.discriminator, spec.data_len);
            let result = check_shape(&data);
            assert_eq!(
                result.status,
                Status::Pass,
                "{} at {} bytes should pass via registry lookup only",
                spec.account_type,
                spec.data_len
            );
            assert_eq!(result.check, format!("shape({})", spec.account_type));
            assert_eq!(
                ShapeCheck::new(&data).name(),
                format!("shape({})", spec.account_type)
            );
        }
    }

    #[test]
    fn dbc_virtual_pool_truncated_by_one_byte_fails() {
        let data = buffer_with_discriminator(VIRTUAL_POOL_DISCRIMINATOR, VIRTUAL_POOL_LEN - 1);
        let result = check_shape(&data);
        assert_eq!(result.status, Status::Fail);
        assert_eq!(
            result.detail.as_deref(),
            Some("truncated: expected 424 bytes, got 423 (1 missing)")
        );
    }

    #[test]
    fn dbc_pool_config_truncated_by_one_byte_fails() {
        let spec = find_by_account_type("PoolConfig").expect("PoolConfig in registry");
        let data = buffer_with_discriminator(spec.discriminator, spec.data_len - 1);
        let result = check_shape(&data);
        assert_eq!(result.status, Status::Fail);
        assert_eq!(
            result.detail.as_deref(),
            Some("truncated: expected 1048 bytes, got 1047 (1 missing)")
        );
    }

    /// The headline finding, as a test: 424 bytes is shared by `VirtualPool`
    /// and `TransferHookPool`, so a length-only check would mis-name one as
    /// the other. The discriminator is what decides.
    #[test]
    fn same_424_length_does_not_make_transfer_hook_pool_a_virtual_pool() {
        let virtual_pool = buffer_with_discriminator(VIRTUAL_POOL_DISCRIMINATOR, VIRTUAL_POOL_LEN);
        let result = check_shape(&virtual_pool);
        assert_eq!(result.status, Status::Pass);
        assert_eq!(result.check, "shape(VirtualPool)");

        let impostor =
            buffer_with_discriminator(TRANSFER_HOOK_POOL_DISCRIMINATOR, VIRTUAL_POOL_LEN);
        assert_eq!(impostor.len(), virtual_pool.len());
        let result = check_shape(&impostor);
        assert_eq!(result.status, Status::Pass);
        assert_eq!(result.check, "shape(TransferHookPool)");
        assert_ne!(result.check, "shape(VirtualPool)");
    }

    /// `PartnerMetadata` carries variable-length strings: the three originally
    /// documented lengths (and other lengths above the floor) are healthy.
    #[test]
    fn partner_metadata_passes_at_every_observed_length() {
        for len in [148, 152, 249, 221, 252] {
            let data = buffer_with_discriminator(PARTNER_METADATA_DISCRIMINATOR, len);
            let result = check_shape(&data);
            assert_eq!(
                result.status,
                Status::Pass,
                "PartnerMetadata at {len} bytes was observed on mainnet and must pass"
            );
            assert_eq!(result.check, "shape(PartnerMetadata)");
        }
    }

    #[test]
    fn partner_metadata_below_floor_fails_with_minimum_wording() {
        let data =
            buffer_with_discriminator(PARTNER_METADATA_DISCRIMINATOR, PARTNER_METADATA_FLOOR - 1);
        let result = check_shape(&data);
        assert_eq!(result.status, Status::Fail);
        assert_eq!(
            result.detail.as_deref(),
            Some("under minimum: expected at least 148 bytes, got 147 (1 missing)")
        );
    }

    /// The variable-length failure must not be mistakable for a fixed-length
    /// one: a reader chasing "expected 148 bytes" would look for a layout
    /// that does not exist.
    #[test]
    fn variable_underrun_detail_reads_differently_from_fixed_mismatch() {
        let variable =
            buffer_with_discriminator(PARTNER_METADATA_DISCRIMINATOR, PARTNER_METADATA_FLOOR - 1);
        let variable_detail = check_shape(&variable).detail.expect("fail detail");
        let fixed = buffer_with_discriminator(VIRTUAL_POOL_DISCRIMINATOR, VIRTUAL_POOL_LEN - 1);
        let fixed_detail = check_shape(&fixed).detail.expect("fail detail");

        assert!(variable_detail.starts_with("under minimum: expected at least "));
        assert!(fixed_detail.starts_with("truncated: expected "));
        assert!(!variable_detail.contains("truncated"));
    }

    /// No upper bound exists for a variable-length type, so a long payload is
    /// not evidence of corruption the way it is for a fixed layout.
    #[test]
    fn partner_metadata_has_no_upper_bound() {
        let data = buffer_with_discriminator(PARTNER_METADATA_DISCRIMINATOR, 4096);
        assert_eq!(check_shape(&data).status, Status::Pass);
    }

    #[test]
    fn virtual_pool_metadata_passes_at_and_above_floor() {
        for len in [
            VIRTUAL_POOL_METADATA_FLOOR,
            168,
            393,
            VIRTUAL_POOL_METADATA_FLOOR + 200,
        ] {
            let data = buffer_with_discriminator(VIRTUAL_POOL_METADATA_DISCRIMINATOR, len);
            let result = check_shape(&data);
            assert_eq!(
                result.status,
                Status::Pass,
                "VirtualPoolMetadata at {len} bytes must pass AtLeast"
            );
            assert_eq!(result.check, "shape(VirtualPoolMetadata)");
        }
    }

    #[test]
    fn virtual_pool_metadata_below_floor_fails() {
        let data = buffer_with_discriminator(
            VIRTUAL_POOL_METADATA_DISCRIMINATOR,
            VIRTUAL_POOL_METADATA_FLOOR - 1,
        );
        let result = check_shape(&data);
        assert_eq!(result.status, Status::Fail);
        assert_eq!(
            result.detail.as_deref(),
            Some("under minimum: expected at least 148 bytes, got 147 (1 missing)")
        );
    }

    #[test]
    fn operator_exact_72_passes_and_rejects_neighbors() {
        let ok = buffer_with_discriminator(OPERATOR_DISCRIMINATOR, OPERATOR_LEN);
        assert_eq!(check_shape(&ok).status, Status::Pass);
        assert_eq!(check_shape(&ok).check, "shape(Operator)");

        let short = buffer_with_discriminator(OPERATOR_DISCRIMINATOR, OPERATOR_LEN - 1);
        assert_eq!(check_shape(&short).status, Status::Fail);
        let long = buffer_with_discriminator(OPERATOR_DISCRIMINATOR, OPERATOR_LEN + 1);
        assert_eq!(check_shape(&long).status, Status::Fail);
    }

    #[test]
    fn transfer_hook_pool_exact_424_passes_and_rejects_neighbors() {
        let ok = buffer_with_discriminator(TRANSFER_HOOK_POOL_DISCRIMINATOR, VIRTUAL_POOL_LEN);
        assert_eq!(check_shape(&ok).status, Status::Pass);
        assert_eq!(check_shape(&ok).check, "shape(TransferHookPool)");

        let short =
            buffer_with_discriminator(TRANSFER_HOOK_POOL_DISCRIMINATOR, VIRTUAL_POOL_LEN - 1);
        assert_eq!(check_shape(&short).status, Status::Fail);
        let long =
            buffer_with_discriminator(TRANSFER_HOOK_POOL_DISCRIMINATOR, VIRTUAL_POOL_LEN + 1);
        assert_eq!(check_shape(&long).status, Status::Fail);
    }

    #[test]
    fn config_with_transfer_hook_exact_1128_passes_and_rejects_neighbors() {
        let ok = buffer_with_discriminator(
            CONFIG_WITH_TRANSFER_HOOK_DISCRIMINATOR,
            CONFIG_WITH_TRANSFER_HOOK_LEN,
        );
        assert_eq!(check_shape(&ok).status, Status::Pass);
        assert_eq!(check_shape(&ok).check, "shape(ConfigWithTransferHook)");

        let short = buffer_with_discriminator(
            CONFIG_WITH_TRANSFER_HOOK_DISCRIMINATOR,
            CONFIG_WITH_TRANSFER_HOOK_LEN - 1,
        );
        assert_eq!(check_shape(&short).status, Status::Fail);
        let long = buffer_with_discriminator(
            CONFIG_WITH_TRANSFER_HOOK_DISCRIMINATOR,
            CONFIG_WITH_TRANSFER_HOOK_LEN + 1,
        );
        assert_eq!(check_shape(&long).status, Status::Fail);
    }
}
