//! Reconciliation check: sampled indexed accounts diffed against the chain.
//!
//! Shape catches malformed rows; reconciliation catches *wrong* rows that are
//! individually well-shaped. It samples indexed accounts, fetches the same
//! keys via `getMultipleAccounts`, and byte-diffs indexed values against
//! on-chain values, emitting the same [`CheckResult`] contract as shape.
//!
//! When indexed bytes disagree with the chain, an optional indexed write slot
//! enables slot-lag tolerance: a mismatch within [`DEFAULT_MAX_SLOT_LAG`] (or
//! a caller-supplied `max_slot_lag`) is reported as `Skipped` with a JSON
//! reason, never as `Pass`. Beyond the tolerance — or with no write slot —
//! mismatches remain `Fail`.

use crate::fetch::{AccountFetcher, FetchResult, FetchedAccount};
use crate::report::CheckResult;

/// Default maximum slot lag tolerated on a byte mismatch before failing.
///
/// About 32 slots ≈ 13 seconds on Solana mainnet. Override via the CLI
/// `--max-slot-lag` flag or the `max_slot_lag` argument to [`reconcile`].
pub const DEFAULT_MAX_SLOT_LAG: u64 = 32;

/// One sampled indexed account: pubkey, indexed bytes, optional write slot.
pub type SampleRow = (String, Vec<u8>, Option<u64>);

/// Reconcile an indexed sample against on-chain state.
///
/// `sample` is an ordered list of `(pubkey, indexed_bytes, indexed_slot)`.
/// `indexed_slot` is the slot at which the indexer wrote the row; when
/// absent, any byte mismatch is a hard `Fail`. Returns one [`CheckResult`]
/// per sampled account, in order:
/// - bytes equal -> `Pass`
/// - account absent on chain (`None`) -> `Fail`
/// - bytes differ, no indexed slot -> `Fail`
/// - bytes differ, `delta == 0` -> `Fail`
/// - bytes differ, `0 < delta <= max_slot_lag` -> `Skipped` (slot lag)
/// - bytes differ, `delta > max_slot_lag` -> `Fail`
/// - the fetch itself was unreachable -> `Skipped` for every account
///
/// `delta` is `context_slot.saturating_sub(indexed_slot)`.
pub fn reconcile(
    sample: &[SampleRow],
    fetcher: &dyn AccountFetcher,
    max_slot_lag: u64,
) -> Vec<CheckResult> {
    let keys: Vec<String> = sample.iter().map(|(key, _, _)| key.clone()).collect();

    match fetcher.get_multiple_accounts(&keys) {
        Err(error) => sample
            .iter()
            .map(|(key, _, _)| {
                CheckResult::skipped(
                    format!("reconcile({key})"),
                    format!("fetch unreachable: {error}"),
                )
            })
            .collect(),
        Ok(FetchResult {
            context_slot,
            accounts,
        }) => sample
            .iter()
            .zip(accounts)
            .map(|((key, indexed, indexed_slot), onchain)| {
                reconcile_one(
                    key,
                    indexed,
                    *indexed_slot,
                    onchain.as_ref(),
                    context_slot,
                    max_slot_lag,
                )
            })
            .collect(),
    }
}

/// Diff one indexed account against its on-chain counterpart.
fn reconcile_one(
    key: &str,
    indexed: &[u8],
    indexed_slot: Option<u64>,
    onchain: Option<&FetchedAccount>,
    context_slot: u64,
    max_slot_lag: u64,
) -> CheckResult {
    let name = format!("reconcile({key})");

    let Some(onchain) = onchain else {
        return CheckResult::fail(
            name,
            format!("on-chain account missing at context slot {context_slot}"),
        );
    };

    if indexed == onchain.data.as_slice() {
        return CheckResult::pass(name);
    }

    let first_diff_offset = first_mismatch(indexed, &onchain.data);

    let Some(write_slot) = indexed_slot else {
        let detail = serde_json::json!({
            "account": key,
            "indexed_len": indexed.len(),
            "onchain_len": onchain.data.len(),
            "first_diff_offset": first_diff_offset,
            "context_slot": context_slot,
        });
        return CheckResult::fail(name, detail.to_string());
    };

    let delta = context_slot.saturating_sub(write_slot);

    if delta == 0 || delta > max_slot_lag {
        let detail = serde_json::json!({
            "account": key,
            "indexed_len": indexed.len(),
            "onchain_len": onchain.data.len(),
            "first_diff_offset": first_diff_offset,
            "context_slot": context_slot,
            "indexed_slot": write_slot,
        });
        return CheckResult::fail(name, detail.to_string());
    }

    // 0 < delta <= max_slot_lag: lag-tolerated mismatch — Skipped, never Pass.
    let detail = serde_json::json!({
        "reason": "slot lag",
        "account": key,
        "indexed_slot": write_slot,
        "context_slot": context_slot,
        "lag": delta,
        "first_diff_offset": first_diff_offset,
    });
    CheckResult::skipped(name, detail.to_string())
}

/// Byte offset of the first difference between `a` and `b`.
///
/// Only called when the two are known to differ; if one is a strict prefix of
/// the other, the offset is the length of the shorter (where it ran out).
fn first_mismatch(a: &[u8], b: &[u8]) -> usize {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .unwrap_or_else(|| a.len().min(b.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::MockFetcher;
    use crate::report::Status;

    fn sample(entries: &[(&str, Vec<u8>, Option<u64>)]) -> Vec<SampleRow> {
        entries
            .iter()
            .map(|(key, data, slot)| (key.to_string(), data.clone(), *slot))
            .collect()
    }

    #[test]
    fn matching_account_passes() {
        let fetcher = MockFetcher::new(500).with_account("acct1", vec![1, 2, 3, 4], 499);
        let results = reconcile(
            &sample(&[("acct1", vec![1, 2, 3, 4], None)]),
            &fetcher,
            DEFAULT_MAX_SLOT_LAG,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, Status::Pass);
        assert_eq!(results[0].check, "reconcile(acct1)");
        assert_eq!(results[0].detail, None);
    }

    #[test]
    fn equal_bytes_pass_regardless_of_slot_delta() {
        let fetcher = MockFetcher::new(1000).with_account("acct1", vec![1, 2, 3], 999);
        let results = reconcile(
            &sample(&[("acct1", vec![1, 2, 3], Some(900))]),
            &fetcher,
            DEFAULT_MAX_SLOT_LAG,
        );
        assert_eq!(results[0].status, Status::Pass);
        assert_eq!(results[0].detail, None);
    }

    #[test]
    fn byte_mismatch_fails_with_first_mismatch_detail() {
        let fetcher = MockFetcher::new(500).with_account("acct1", vec![1, 2, 9, 4], 499);
        let results = reconcile(
            &sample(&[("acct1", vec![1, 2, 3, 4], None)]),
            &fetcher,
            DEFAULT_MAX_SLOT_LAG,
        );
        assert_eq!(results[0].status, Status::Fail);

        let detail: serde_json::Value =
            serde_json::from_str(results[0].detail.as_ref().expect("detail present"))
                .expect("detail is JSON");
        assert_eq!(detail["account"], "acct1");
        assert_eq!(detail["indexed_len"], 4);
        assert_eq!(detail["onchain_len"], 4);
        assert_eq!(detail["first_diff_offset"], 2);
        assert_eq!(detail["context_slot"], 500);
        assert!(detail.get("indexed_slot").is_none());
    }

    #[test]
    fn mismatch_at_delta_zero_fails_with_indexed_slot() {
        let fetcher = MockFetcher::new(500).with_account("acct1", vec![1, 2, 9, 4], 500);
        let results = reconcile(
            &sample(&[("acct1", vec![1, 2, 3, 4], Some(500))]),
            &fetcher,
            DEFAULT_MAX_SLOT_LAG,
        );
        assert_eq!(results[0].status, Status::Fail);

        let detail: serde_json::Value =
            serde_json::from_str(results[0].detail.as_ref().expect("detail present"))
                .expect("detail is JSON");
        assert_eq!(detail["context_slot"], 500);
        assert_eq!(detail["indexed_slot"], 500);
        assert_eq!(detail["first_diff_offset"], 2);
        assert!(detail.get("reason").is_none());
    }

    #[test]
    fn mismatch_inside_tolerance_is_skipped_with_slot_lag_json() {
        let fetcher = MockFetcher::new(532).with_account("acct1", vec![9, 9, 9], 532);
        let results = reconcile(
            &sample(&[("acct1", vec![1, 2, 3], Some(500))]),
            &fetcher,
            DEFAULT_MAX_SLOT_LAG,
        );
        assert_eq!(results[0].status, Status::Skipped);
        assert_ne!(results[0].status, Status::Pass);

        let detail: serde_json::Value =
            serde_json::from_str(results[0].detail.as_ref().expect("detail present"))
                .expect("detail is JSON");
        assert_eq!(detail["reason"], "slot lag");
        assert_eq!(detail["account"], "acct1");
        assert_eq!(detail["indexed_slot"], 500);
        assert_eq!(detail["context_slot"], 532);
        assert_eq!(detail["lag"], 32);
        assert_eq!(detail["first_diff_offset"], 0);
    }

    #[test]
    fn mismatch_beyond_tolerance_fails() {
        let fetcher = MockFetcher::new(533).with_account("acct1", vec![9, 9, 9], 533);
        let results = reconcile(
            &sample(&[("acct1", vec![1, 2, 3], Some(500))]),
            &fetcher,
            DEFAULT_MAX_SLOT_LAG,
        );
        assert_eq!(results[0].status, Status::Fail);

        let detail: serde_json::Value =
            serde_json::from_str(results[0].detail.as_ref().expect("detail present"))
                .expect("detail is JSON");
        assert_eq!(detail["indexed_slot"], 500);
        assert_eq!(detail["context_slot"], 533);
        assert!(detail.get("reason").is_none());
    }

    #[test]
    fn mismatch_without_indexed_slot_fails() {
        let fetcher = MockFetcher::new(532).with_account("acct1", vec![9, 9, 9], 532);
        let results = reconcile(
            &sample(&[("acct1", vec![1, 2, 3], None)]),
            &fetcher,
            DEFAULT_MAX_SLOT_LAG,
        );
        assert_eq!(results[0].status, Status::Fail);
    }

    #[test]
    fn max_slot_lag_zero_makes_inside_tolerance_fail() {
        let fetcher = MockFetcher::new(510).with_account("acct1", vec![9], 510);
        let results = reconcile(&sample(&[("acct1", vec![1], Some(500))]), &fetcher, 0);
        assert_eq!(results[0].status, Status::Fail);
    }

    #[test]
    fn lag_tolerated_mismatch_is_never_pass() {
        let fetcher = MockFetcher::new(510).with_account("acct1", vec![0], 510);
        let results = reconcile(
            &sample(&[("acct1", vec![1], Some(500))]),
            &fetcher,
            DEFAULT_MAX_SLOT_LAG,
        );
        assert_eq!(results[0].status, Status::Skipped);
        assert_ne!(results[0].status, Status::Pass);
    }

    #[test]
    fn length_mismatch_reports_offset_at_shorter_end() {
        let fetcher = MockFetcher::new(500).with_account("acct1", vec![1, 2, 3], 499);
        let results = reconcile(
            &sample(&[("acct1", vec![1, 2, 3, 4, 5], None)]),
            &fetcher,
            DEFAULT_MAX_SLOT_LAG,
        );
        assert_eq!(results[0].status, Status::Fail);

        let detail: serde_json::Value =
            serde_json::from_str(results[0].detail.as_ref().expect("detail present"))
                .expect("detail is JSON");
        assert_eq!(detail["indexed_len"], 5);
        assert_eq!(detail["onchain_len"], 3);
        assert_eq!(detail["first_diff_offset"], 3);
    }

    #[test]
    fn missing_on_chain_account_fails() {
        let fetcher = MockFetcher::new(500);
        let results = reconcile(
            &sample(&[("acct1", vec![1, 2, 3], None)]),
            &fetcher,
            DEFAULT_MAX_SLOT_LAG,
        );
        assert_eq!(results[0].status, Status::Fail);
        assert!(
            results[0]
                .detail
                .as_ref()
                .expect("detail present")
                .contains("missing")
        );
    }

    #[test]
    fn unreachable_fetch_skips_every_account() {
        let fetcher = MockFetcher::unreachable();
        let results = reconcile(
            &sample(&[
                ("acct1", vec![1, 2, 3], None),
                ("acct2", vec![4, 5, 6], Some(1)),
            ]),
            &fetcher,
            DEFAULT_MAX_SLOT_LAG,
        );
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.status == Status::Skipped));
        assert!(
            results[0]
                .detail
                .as_ref()
                .expect("detail present")
                .contains("unreachable")
        );
    }

    #[test]
    fn mixed_batch_classifies_each_account_independently() {
        let fetcher = MockFetcher::new(1000)
            .with_account("ok", vec![7, 7, 7], 999)
            .with_account("diff", vec![7, 0, 7], 999)
            .with_account("lag", vec![0, 0, 0], 999);
        let results = reconcile(
            &sample(&[
                ("ok", vec![7, 7, 7], None),
                ("diff", vec![7, 7, 7], Some(1000)),
                ("gone", vec![1], None),
                ("lag", vec![1, 1, 1], Some(990)),
            ]),
            &fetcher,
            DEFAULT_MAX_SLOT_LAG,
        );
        assert_eq!(results[0].status, Status::Pass);
        assert_eq!(results[1].status, Status::Fail); // delta 0
        assert_eq!(results[2].status, Status::Fail); // missing
        assert_eq!(results[3].status, Status::Skipped); // lag 10
    }
}
