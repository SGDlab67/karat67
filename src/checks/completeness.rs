//! Completeness check: indexed slot coverage against on-chain produced slots.
//!
//! Liveness can stay green while the indexer silently drops slots. Completeness
//! compares the indexed slot set to Solana's `getBlocks` for a window
//! `[start, end]`. Missing = on-chain produced slots minus indexed slots.
//!
//! **Skip-slot behavior:** Solana leaders skip slots. Those heights never
//! produce a block and are omitted from `getBlocks`. An unindexed skip-slot
//! inside the window is **not** a gap — only slots returned by `getBlocks`
//! are expected in the index.
//!
//! The mirror check lives here too. [`check_orphans`] asks the opposite
//! question: not "did I miss a slot the chain produced?" but "did I write a
//! slot the chain never produced?". The two directions share the window and
//! the `getBlocks` call, and deliberately do not share the skip-slot rule.

use std::collections::HashSet;

use crate::fetch::SlotCoverageFetcher;
use crate::report::CheckResult;

/// How many missing slots to list in a Fail detail sample.
const MISSING_SAMPLE_LIMIT: usize = 16;

/// Compare indexed slots to `getBlocks` for the inclusive window `[start, end]`.
///
/// Returns a single [`CheckResult`]:
/// - every produced slot is indexed -> `Pass`
/// - at least one produced slot is absent from `indexed` -> `Fail` with JSON
///   detail (`missing_count`, `missing_sample`, window counts)
/// - the fetch itself was unreachable -> `Skipped`
///
/// Leader skip-slots (in range but omitted by `getBlocks`) are ignored even
/// when absent from `indexed`.
pub fn check_completeness(
    indexed: &[u64],
    start: u64,
    end: u64,
    fetcher: &dyn SlotCoverageFetcher,
) -> CheckResult {
    let name = format!("completeness([{start}..={end}])");

    let produced = match fetcher.get_blocks(start, end) {
        Ok(slots) => slots,
        Err(error) => {
            return CheckResult::skipped(name, format!("fetch unreachable: {error}"));
        }
    };

    let indexed_set: HashSet<u64> = indexed.iter().copied().collect();
    let mut missing: Vec<u64> = produced
        .iter()
        .copied()
        .filter(|slot| !indexed_set.contains(slot))
        .collect();
    missing.sort_unstable();

    if missing.is_empty() {
        return CheckResult::pass(name);
    }

    let missing_sample: Vec<u64> = missing.iter().copied().take(MISSING_SAMPLE_LIMIT).collect();
    let detail = serde_json::json!({
        "start": start,
        "end": end,
        "produced_count": produced.len(),
        "indexed_count": indexed_set.len(),
        "missing_count": missing.len(),
        "missing_sample": missing_sample,
    });
    CheckResult::fail(name, detail.to_string())
}

/// Compare indexed slots to `getBlocks` the other way: find slots the index
/// wrote that the chain never produced, for the inclusive window `[start, end]`.
///
/// Solana confirms optimistically and Yellowstone gRPC streams at `processed`.
/// An indexer that writes at `processed` to cut latency will sometimes persist
/// state from a slot that loses its fork. The database then holds rows for a
/// slot that is not on the canonical chain, and nothing today surfaces that:
/// [`check_completeness`] only asks whether something the chain produced is
/// absent from the index, never whether something in the index was never
/// produced.
///
/// Only indexed slots inside `[start, end]` are considered. An indexed slot
/// outside the window says nothing about this window: `getBlocks` was not
/// asked about it, so its absence is ignorance, not evidence.
///
/// **The skip-slot rule does not mirror, and this is the part that surprises
/// people.** For completeness, a slot missing from `getBlocks` is a leader
/// skip-slot and therefore not a gap. For orphans, a slot missing from
/// `getBlocks` that the index nonetheless wrote data for is exactly the bug.
/// Either the leader skipped that height, so there was no block to index, or
/// the slot was forked away. Both readings end the same way: the index holds
/// state for a slot that never landed on the canonical chain. Absent from
/// `getBlocks` plus present in `indexed` is always a finding here, even though
/// absent from `getBlocks` plus absent from `indexed` is fine over there.
///
/// Returns a single [`CheckResult`]:
/// - no in-window indexed slot is unproduced -> `Pass`
/// - at least one is -> `Fail` with JSON detail (`orphan_count`,
///   `orphan_sample`, window counts)
/// - the fetch itself was unreachable -> `Skipped`
pub fn check_orphans(
    indexed: &[u64],
    start: u64,
    end: u64,
    fetcher: &dyn SlotCoverageFetcher,
) -> CheckResult {
    let name = format!("orphans([{start}..={end}])");

    let produced = match fetcher.get_blocks(start, end) {
        Ok(slots) => slots,
        Err(error) => {
            return CheckResult::skipped(name, format!("fetch unreachable: {error}"));
        }
    };

    let produced_set: HashSet<u64> = produced.iter().copied().collect();
    let in_window: HashSet<u64> = indexed
        .iter()
        .copied()
        .filter(|&slot| slot >= start && slot <= end)
        .collect();

    let mut orphans: Vec<u64> = in_window
        .iter()
        .copied()
        .filter(|slot| !produced_set.contains(slot))
        .collect();
    orphans.sort_unstable();

    if orphans.is_empty() {
        return CheckResult::pass(name);
    }

    let orphan_sample: Vec<u64> = orphans.iter().copied().take(MISSING_SAMPLE_LIMIT).collect();
    let detail = serde_json::json!({
        "start": start,
        "end": end,
        "produced_count": produced_set.len(),
        "indexed_count": in_window.len(),
        "orphan_count": orphans.len(),
        "orphan_sample": orphan_sample,
    });
    CheckResult::fail(name, detail.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::MockFetcher;
    use crate::report::Status;

    #[test]
    fn full_coverage_passes() {
        let fetcher = MockFetcher::new(0).with_blocks([100, 101, 103, 104]);
        let result = check_completeness(&[100, 101, 103, 104], 100, 104, &fetcher);
        assert_eq!(result.status, Status::Pass);
        assert_eq!(result.check, "completeness([100..=104])");
        assert_eq!(result.detail, None);
    }

    #[test]
    fn missing_produced_slots_fails_with_detail() {
        // getBlocks produced 100,101,103,104; indexer missed 101 and 104.
        let fetcher = MockFetcher::new(0).with_blocks([100, 101, 103, 104]);
        let result = check_completeness(&[100, 103], 100, 104, &fetcher);
        assert_eq!(result.status, Status::Fail);

        let detail: serde_json::Value =
            serde_json::from_str(result.detail.as_ref().expect("detail present"))
                .expect("detail is JSON");
        assert_eq!(detail["start"], 100);
        assert_eq!(detail["end"], 104);
        assert_eq!(detail["produced_count"], 4);
        assert_eq!(detail["indexed_count"], 2);
        assert_eq!(detail["missing_count"], 2);
        assert_eq!(detail["missing_sample"], serde_json::json!([101, 104]));
    }

    #[test]
    fn leader_skip_slots_not_indexed_still_pass() {
        // Window [100, 104]: leaders skipped 102 (absent from getBlocks).
        // Indexer has every produced slot; 102 is correctly absent → Pass.
        let fetcher = MockFetcher::new(0).with_blocks([100, 101, 103, 104]);
        let result = check_completeness(&[100, 101, 103, 104], 100, 104, &fetcher);
        assert_eq!(result.status, Status::Pass);
        assert_eq!(result.detail, None);

        let produced = fetcher.get_blocks(100, 104).expect("reachable");
        assert!(
            !produced.contains(&102),
            "102 must be a skip-slot in the mock"
        );
        // Explicit: 102 is in range, not in getBlocks, not indexed → still Pass.
        assert!(!result.check.is_empty());
    }

    #[test]
    fn unreachable_fetch_is_skipped() {
        let fetcher = MockFetcher::unreachable();
        let result = check_completeness(&[100, 101], 100, 101, &fetcher);
        assert_eq!(result.status, Status::Skipped);
        assert!(
            result
                .detail
                .as_ref()
                .expect("detail present")
                .contains("unreachable")
        );
    }

    #[test]
    fn missing_sample_is_capped() {
        let produced: Vec<u64> = (0..40).collect();
        let fetcher = MockFetcher::new(0).with_blocks(produced);
        // Index nothing → every produced slot is missing.
        let result = check_completeness(&[], 0, 39, &fetcher);
        assert_eq!(result.status, Status::Fail);

        let detail: serde_json::Value =
            serde_json::from_str(result.detail.as_ref().expect("detail present"))
                .expect("detail is JSON");
        assert_eq!(detail["missing_count"], 40);
        let sample = detail["missing_sample"].as_array().expect("sample array");
        assert_eq!(sample.len(), MISSING_SAMPLE_LIMIT);
        assert_eq!(sample[0], 0);
        assert_eq!(sample[15], 15);
    }

    #[test]
    fn no_orphans_passes() {
        let fetcher = MockFetcher::new(0).with_blocks([100, 101, 103, 104]);
        let result = check_orphans(&[100, 101, 103, 104], 100, 104, &fetcher);
        assert_eq!(result.status, Status::Pass);
        assert_eq!(result.check, "orphans([100..=104])");
        assert_eq!(result.detail, None);
    }

    #[test]
    fn indexed_slot_never_produced_fails_and_names_it() {
        // getBlocks produced 100,101,103,104. The index also wrote 102, which
        // the chain never produced: a skipped or forked-away height.
        let fetcher = MockFetcher::new(0).with_blocks([100, 101, 103, 104]);
        let result = check_orphans(&[100, 101, 102, 103, 104], 100, 104, &fetcher);
        assert_eq!(result.status, Status::Fail);

        let detail: serde_json::Value =
            serde_json::from_str(result.detail.as_ref().expect("detail present"))
                .expect("detail is JSON");
        assert_eq!(detail["start"], 100);
        assert_eq!(detail["end"], 104);
        assert_eq!(detail["produced_count"], 4);
        assert_eq!(detail["indexed_count"], 5);
        assert_eq!(detail["orphan_count"], 1);
        assert_eq!(detail["orphan_sample"], serde_json::json!([102]));
    }

    #[test]
    fn indexed_slots_outside_window_are_ignored() {
        // 98 and 110 are unproduced as far as this window knows, but getBlocks
        // was never asked about them, so they are not evidence of anything.
        let fetcher = MockFetcher::new(0).with_blocks([100, 101]);
        let result = check_orphans(&[98, 100, 101, 110], 100, 101, &fetcher);
        assert_eq!(result.status, Status::Pass);
        assert_eq!(result.detail, None);
    }

    #[test]
    fn orphans_unreachable_fetch_is_skipped() {
        let fetcher = MockFetcher::unreachable();
        let result = check_orphans(&[100, 101], 100, 101, &fetcher);
        assert_eq!(result.status, Status::Skipped);
        assert_eq!(result.check, "orphans([100..=101])");
        assert!(
            result
                .detail
                .as_ref()
                .expect("detail present")
                .contains("unreachable")
        );
    }

    #[test]
    fn produced_but_unindexed_slot_is_not_an_orphan() {
        // The asymmetry: 101 and 104 were produced and never indexed. That is
        // completeness's finding, not this one. check_orphans only reports
        // slots the index wrote, so it passes while completeness fails.
        let fetcher = MockFetcher::new(0).with_blocks([100, 101, 103, 104]);
        let orphans = check_orphans(&[100, 103], 100, 104, &fetcher);
        assert_eq!(orphans.status, Status::Pass);

        let completeness = check_completeness(&[100, 103], 100, 104, &fetcher);
        assert_eq!(completeness.status, Status::Fail);
    }

    #[test]
    fn orphan_sample_is_capped() {
        // getBlocks produced nothing in the window; the index wrote 40 slots.
        let fetcher = MockFetcher::new(0);
        let indexed: Vec<u64> = (0..40).collect();
        let result = check_orphans(&indexed, 0, 39, &fetcher);
        assert_eq!(result.status, Status::Fail);

        let detail: serde_json::Value =
            serde_json::from_str(result.detail.as_ref().expect("detail present"))
                .expect("detail is JSON");
        assert_eq!(detail["orphan_count"], 40);
        let sample = detail["orphan_sample"].as_array().expect("sample array");
        assert_eq!(sample.len(), MISSING_SAMPLE_LIMIT);
        assert_eq!(sample[0], 0);
        assert_eq!(sample[15], 15);
    }
}
