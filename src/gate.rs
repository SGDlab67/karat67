//! Account gate: shape the exact indexed bytes, then reconcile only on Pass.
//!
//! The CLI, the Carbon processor hook, and the MCP tool all call
//! [`check_account`] so none of them reimplement the decision. A non-Pass
//! shape result is returned alone — the fetcher is not invoked and no RPC
//! happens.

use crate::checks::reconcile::{SampleRow, reconcile};
use crate::checks::shape::check_shape;
use crate::fetch::AccountFetcher;
use crate::report::{CheckResult, Status};

/// True only when `results` is non-empty and every status is [`Status::Pass`].
///
/// Skipped (slot lag, unreachable RPC, missing fetcher) and Fail are not Pass.
pub fn all_pass(results: &[CheckResult]) -> bool {
    !results.is_empty() && results.iter().all(|result| result.status == Status::Pass)
}

/// Shape-check `indexed`, then reconcile that one account if shape passes.
///
/// `indexed` is the indexed payload itself. This does not rebuild a
/// discriminator or pad to an IDL length. When shape is not Pass, `fetcher`
/// is not called. After Pass, a missing fetcher is a Skipped reconcile result,
/// never a second Pass. Slot-lag rules are [`reconcile`]'s: delta 0 or a
/// missing indexed slot is a hard mismatch Fail; `0 < delta <= max_slot_lag`
/// is Skipped.
pub fn check_account(
    account: &str,
    indexed: &[u8],
    indexed_slot: Option<u64>,
    max_slot_lag: u64,
    fetcher: Option<&dyn AccountFetcher>,
) -> Vec<CheckResult> {
    let shape = check_shape(indexed);
    if shape.status != Status::Pass {
        return vec![shape];
    }

    let Some(fetcher) = fetcher else {
        return vec![
            shape,
            CheckResult::skipped(
                format!("reconcile({account})"),
                "shape passed; rpc_url or KARAT_RPC_URL is required to reconcile",
            ),
        ];
    };

    let sample: Vec<SampleRow> = vec![(account.to_string(), indexed.to_vec(), indexed_slot)];
    let mut results = vec![shape];
    results.extend(reconcile(&sample, fetcher, max_slot_lag));
    results
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::checks::reconcile::DEFAULT_MAX_SLOT_LAG;
    use crate::checks::specs::find_by_account_type;
    use crate::fetch::{AccountFetcher, FetchResult, MockFetcher};
    use crate::report::Status;

    /// Counts `get_multiple_accounts` calls around a [`MockFetcher`].
    struct CountingFetcher {
        inner: MockFetcher,
        calls: Cell<u32>,
    }

    impl CountingFetcher {
        fn new(inner: MockFetcher) -> Self {
            Self {
                inner,
                calls: Cell::new(0),
            }
        }
    }

    impl AccountFetcher for CountingFetcher {
        fn get_multiple_accounts(&self, keys: &[String]) -> anyhow::Result<FetchResult> {
            self.calls.set(self.calls.get() + 1);
            self.inner.get_multiple_accounts(keys)
        }
    }

    fn obligation_bytes() -> Vec<u8> {
        let spec = find_by_account_type("Obligation").expect("Obligation is registered");
        let mut data = spec.discriminator.to_vec();
        data.resize(spec.data_len, 0);
        data
    }

    fn run(bytes: &[u8], slot: Option<u64>, fetcher: &CountingFetcher) -> Vec<CheckResult> {
        check_account("acct", bytes, slot, DEFAULT_MAX_SLOT_LAG, Some(fetcher))
    }

    #[test]
    fn empty_and_garbage_bytes_fail_shape_without_fetch() {
        let fetcher = CountingFetcher::new(MockFetcher::unreachable());

        let empty = run(&[], None, &fetcher);
        assert_eq!(empty.len(), 1);
        assert_eq!(empty[0].status, Status::Fail);
        assert!(empty[0].check.starts_with("shape"));

        let garbage = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 9];
        let garbage_result = run(&garbage, Some(1), &fetcher);
        assert_eq!(garbage_result.len(), 1);
        assert_eq!(garbage_result[0].status, Status::Fail);
        assert!(garbage_result[0].check.starts_with("shape"));

        assert_eq!(fetcher.calls.get(), 0);
        assert!(!all_pass(&empty));
        assert!(!all_pass(&garbage_result));
    }

    #[test]
    fn valid_shape_and_matching_chain_bytes_pass() {
        let data = obligation_bytes();
        let fetcher =
            CountingFetcher::new(MockFetcher::new(50).with_account("acct", data.clone(), 50));
        let results = run(&data, Some(50), &fetcher);

        assert_eq!(fetcher.calls.get(), 1);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| result.status == Status::Pass));
        assert!(all_pass(&results));
        assert!(results[0].check.starts_with("shape"));
        assert_eq!(results[1].check, "reconcile(acct)");
    }

    #[test]
    fn mismatch_inside_lag_is_skipped_and_never_pass() {
        let data = obligation_bytes();
        let mut chain = data.clone();
        chain[8] ^= 0xff;
        // context 110 − indexed 100 = 10, inside the default window of 32.
        let fetcher = CountingFetcher::new(MockFetcher::new(110).with_account("acct", chain, 110));
        let results = run(&data, Some(100), &fetcher);

        assert_eq!(fetcher.calls.get(), 1);
        assert_eq!(results[0].status, Status::Pass);
        let reconcile_result = &results[1];
        assert_eq!(reconcile_result.status, Status::Skipped);
        assert_ne!(reconcile_result.status, Status::Pass);
        assert!(!all_pass(&results));
        let detail = reconcile_result.detail.as_deref().expect("lag detail");
        assert!(detail.contains("slot lag"));
    }

    #[test]
    fn mismatch_at_delta_zero_fails() {
        let data = obligation_bytes();
        let mut chain = data.clone();
        chain[8] ^= 0xff;
        let fetcher = CountingFetcher::new(MockFetcher::new(80).with_account("acct", chain, 80));
        let results = run(&data, Some(80), &fetcher);

        assert_eq!(fetcher.calls.get(), 1);
        assert_eq!(results[1].status, Status::Fail);
        assert!(!all_pass(&results));
        let detail = results[1].detail.as_deref().expect("fail detail");
        assert!(!detail.contains("slot lag"));
    }

    #[test]
    fn unreachable_fetch_after_shape_pass_is_skipped() {
        let data = obligation_bytes();
        let fetcher = CountingFetcher::new(MockFetcher::unreachable());
        let results = run(&data, Some(1), &fetcher);

        assert_eq!(fetcher.calls.get(), 1);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].status, Status::Pass);
        assert_eq!(results[1].status, Status::Skipped);
        assert_ne!(results[1].status, Status::Pass);
        assert!(
            results[1]
                .detail
                .as_deref()
                .expect("skip detail")
                .contains("unreachable")
        );
        assert!(!all_pass(&results));
    }
}
