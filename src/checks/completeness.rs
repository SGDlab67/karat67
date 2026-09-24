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
}
