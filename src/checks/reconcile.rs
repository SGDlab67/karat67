//! Reconciliation check: sampled indexed accounts diffed against the chain.
//!
//! Shape catches malformed rows; reconciliation catches *wrong* rows that are
//! individually well-shaped. It samples indexed accounts, fetches the same
//! keys via `getMultipleAccounts`, and byte-diffs indexed values against
//! on-chain values, emitting the same [`CheckResult`] contract as shape.
//!
//! Slot lag is threaded through but not yet tolerated: the context slot rides
//! along on every result and [`reconcile_one`] marks the spot where Thursday's
//! slot-lag tolerance plugs in. Mismatches are never reported as `Pass` here.

use crate::fetch::{AccountFetcher, FetchResult, FetchedAccount};
use crate::report::CheckResult;

/// Reconcile an indexed sample against on-chain state.
///
/// `sample` is an ordered list of `(pubkey, indexed_bytes)`. Returns one
/// [`CheckResult`] per sampled account, in order:
/// - bytes equal -> `Pass`
/// - account absent on chain (`None`) -> `Fail`
/// - bytes differ -> `Fail` with a first-mismatch JSON detail
/// - the fetch itself was unreachable -> `Skipped` for every account
pub fn reconcile(sample: &[(String, Vec<u8>)], fetcher: &dyn AccountFetcher) -> Vec<CheckResult> {
    let keys: Vec<String> = sample.iter().map(|(key, _)| key.clone()).collect();

    match fetcher.get_multiple_accounts(&keys) {
        Err(error) => sample
            .iter()
            .map(|(key, _)| {
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
            .map(|((key, indexed), onchain)| {
                reconcile_one(key, indexed, onchain.as_ref(), context_slot)
            })
            .collect(),
    }
}

/// Diff one indexed account against its on-chain counterpart.
fn reconcile_one(
    key: &str,
    indexed: &[u8],
    onchain: Option<&FetchedAccount>,
    context_slot: u64,
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

    // Thursday (slot-lag tolerance) plugs in here: on a byte mismatch, compare
    // `onchain.slot` / `context_slot` against the slot the indexer wrote at and
    // tolerate differences within a slot-lag threshold instead of failing.
    // Until then a mismatch is always a `Fail` — never silently a `Pass`.
    let detail = serde_json::json!({
        "account": key,
        "indexed_len": indexed.len(),
        "onchain_len": onchain.data.len(),
        "first_diff_offset": first_mismatch(indexed, &onchain.data),
        "context_slot": context_slot,
    });
    CheckResult::fail(name, detail.to_string())
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

    fn sample(entries: &[(&str, Vec<u8>)]) -> Vec<(String, Vec<u8>)> {
        entries
            .iter()
            .map(|(key, data)| (key.to_string(), data.clone()))
            .collect()
    }

    #[test]
    fn matching_account_passes() {
        let fetcher = MockFetcher::new(500).with_account("acct1", vec![1, 2, 3, 4], 499);
        let results = reconcile(&sample(&[("acct1", vec![1, 2, 3, 4])]), &fetcher);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, Status::Pass);
        assert_eq!(results[0].check, "reconcile(acct1)");
        assert_eq!(results[0].detail, None);
    }

    #[test]
    fn byte_mismatch_fails_with_first_mismatch_detail() {
        let fetcher = MockFetcher::new(500).with_account("acct1", vec![1, 2, 9, 4], 499);
        let results = reconcile(&sample(&[("acct1", vec![1, 2, 3, 4])]), &fetcher);
        assert_eq!(results[0].status, Status::Fail);

        let detail: serde_json::Value =
            serde_json::from_str(results[0].detail.as_ref().expect("detail present"))
                .expect("detail is JSON");
        assert_eq!(detail["account"], "acct1");
        assert_eq!(detail["indexed_len"], 4);
        assert_eq!(detail["onchain_len"], 4);
        assert_eq!(detail["first_diff_offset"], 2);
        assert_eq!(detail["context_slot"], 500);
    }

    #[test]
    fn length_mismatch_reports_offset_at_shorter_end() {
        let fetcher = MockFetcher::new(500).with_account("acct1", vec![1, 2, 3], 499);
        let results = reconcile(&sample(&[("acct1", vec![1, 2, 3, 4, 5])]), &fetcher);
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
        let results = reconcile(&sample(&[("acct1", vec![1, 2, 3])]), &fetcher);
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
            &sample(&[("acct1", vec![1, 2, 3]), ("acct2", vec![4, 5, 6])]),
            &fetcher,
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
            .with_account("diff", vec![7, 0, 7], 999);
        let results = reconcile(
            &sample(&[
                ("ok", vec![7, 7, 7]),
                ("diff", vec![7, 7, 7]),
                ("gone", vec![1]),
            ]),
            &fetcher,
        );
        assert_eq!(results[0].status, Status::Pass);
        assert_eq!(results[1].status, Status::Fail);
        assert_eq!(results[2].status, Status::Fail);
    }
}
