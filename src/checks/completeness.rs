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

/// Where a coverage check can stop: `end`, or the finalized slot if lower.
///
/// `getBlocks` answers at `finalized`. Above the finalized slot a block the
/// chain did produce is still absent, so "not in `getBlocks`" stops meaning
/// "never produced": completeness would pass a tail it never saw, and orphans
/// would flag every confirmed-but-not-final slot an indexer wrote.
enum Reach {
    /// Check `[start, checked_end]`; `checked_end < end` means the tail past
    /// the finalized slot was left out.
    Upto {
        checked_end: u64,
        finalized: Option<u64>,
    },
    /// The whole window is above the finalized slot: nothing to check yet.
    NothingFinal { finalized: u64 },
}

fn reach(start: u64, end: u64, fetcher: &dyn SlotCoverageFetcher) -> anyhow::Result<Reach> {
    Ok(match fetcher.finalized_slot()? {
        None => Reach::Upto {
            checked_end: end,
            finalized: None,
        },
        Some(finalized) if finalized < start => Reach::NothingFinal { finalized },
        Some(finalized) => Reach::Upto {
            checked_end: end.min(finalized),
            finalized: Some(finalized),
        },
    })
}

/// `Skipped` for a window, or the part of one, above the finalized slot.
/// `extra` fields join the JSON detail.
fn not_finalized(
    name: String,
    start: u64,
    end: u64,
    finalized: u64,
    extra: serde_json::Value,
) -> CheckResult {
    let mut detail = serde_json::json!({
        "reason": "not finalized",
        "start": start,
        "end": end,
        "finalized_slot": finalized,
    });
    if let (Some(detail), Some(extra)) = (detail.as_object_mut(), extra.as_object()) {
        detail.extend(extra.clone());
    }
    CheckResult::skipped(name, detail.to_string())
}

/// Compare indexed slots to `getBlocks` for the inclusive window `[start, end]`.
///
/// Returns a single [`CheckResult`]:
/// - every produced slot is indexed -> `Pass`
/// - at least one produced slot is absent from `indexed` -> `Fail` with JSON
///   detail (`missing_count`, `missing_sample`, window counts)
/// - no gap up to the finalized slot, but the window runs past it ->
///   `Skipped` (`"reason":"not finalized"`, with `checked_end`)
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

    let (checked_end, finalized) = match reach(start, end, fetcher) {
        Ok(Reach::Upto {
            checked_end,
            finalized,
        }) => (checked_end, finalized),
        Ok(Reach::NothingFinal { finalized }) => {
            return not_finalized(name, start, end, finalized, serde_json::json!({}));
        }
        Err(error) => {
            return CheckResult::skipped(name, format!("fetch unreachable: {error}"));
        }
    };

    let produced = match fetcher.get_blocks(start, checked_end) {
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
        return match finalized {
            // Clean up to the finalized slot, but the tail is unchecked: a
            // Pass would vouch for blocks getBlocks has not reported yet.
            Some(finalized) if checked_end < end => not_finalized(
                name,
                start,
                end,
                finalized,
                serde_json::json!({ "checked_end": checked_end }),
            ),
            _ => CheckResult::pass(name),
        };
    }

    let missing_sample: Vec<u64> = missing.iter().copied().take(MISSING_SAMPLE_LIMIT).collect();
    let detail = serde_json::json!({
        "start": start,
        "end": end,
        "produced_count": produced.len(),
        "indexed_count": indexed_set.len(),
        "missing_count": missing.len(),
        "missing_sample": missing_sample,
        "checked_end": checked_end,
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
/// - none up to the finalized slot, but some indexed slots sit above it ->
///   `Skipped` (`"reason":"not finalized"`, with `unfinalized_count`)
/// - the fetch itself was unreachable -> `Skipped`
pub fn check_orphans(
    indexed: &[u64],
    start: u64,
    end: u64,
    fetcher: &dyn SlotCoverageFetcher,
) -> CheckResult {
    let name = format!("orphans([{start}..={end}])");

    let (checked_end, finalized) = match reach(start, end, fetcher) {
        Ok(Reach::Upto {
            checked_end,
            finalized,
        }) => (checked_end, finalized),
        Ok(Reach::NothingFinal { finalized }) => {
            return not_finalized(name, start, end, finalized, serde_json::json!({}));
        }
        Err(error) => {
            return CheckResult::skipped(name, format!("fetch unreachable: {error}"));
        }
    };

    let produced = match fetcher.get_blocks(start, checked_end) {
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
        .filter(|&slot| slot <= checked_end && !produced_set.contains(&slot))
        .collect();
    orphans.sort_unstable();
    let unfinalized_count = in_window.iter().filter(|&&slot| slot > checked_end).count();

    if orphans.is_empty() {
        return match finalized {
            // Indexed slots above the finalized slot can still be orphaned;
            // they are neither cleared nor flagged until they finalize.
            Some(finalized) if unfinalized_count > 0 => not_finalized(
                name,
                start,
                end,
                finalized,
                serde_json::json!({ "unfinalized_count": unfinalized_count }),
            ),
            _ => CheckResult::pass(name),
        };
    }

    let orphan_sample: Vec<u64> = orphans.iter().copied().take(MISSING_SAMPLE_LIMIT).collect();
    let detail = serde_json::json!({
        "start": start,
        "end": end,
        "produced_count": produced_set.len(),
        "indexed_count": in_window.len(),
        "orphan_count": orphans.len(),
        "orphan_sample": orphan_sample,
        "unfinalized_count": unfinalized_count,
    });
    CheckResult::fail(name, detail.to_string())
}

/// Produced blocks in the closed range `[last_before, first_after]`: the
/// exact backfill list after a stream reconnect.
///
/// A streaming datasource that drops its connection knows two slots: the last
/// one it processed before the drop and the first one it saw after the
/// reconnect. Whatever the chain produced from one to the other is what the
/// indexer may have missed, in the form it can be replayed.
///
/// The range is closed at both ends because, on account and transaction
/// streams, both boundary slots can be partly delivered: the drop can land
/// between two transactions of `last_before`, and the new stream can start
/// partway through `first_after`. Re-fetching a boundary block is a dedup at
/// worst; skipping one loses whatever the stream did not deliver.
///
/// - `first_after < last_before` is an empty range: empty list, no RPC call.
/// - An unreachable fetch is an `Err`, never an empty list: "nothing was
///   missed" and "could not check" must not read the same.
/// - Leader skip-slots are not gaps. They never produced a block, so
///   `getBlocks` omits them and they cannot appear here.
pub fn gap_slots(
    last_before: u64,
    first_after: u64,
    fetcher: &dyn SlotCoverageFetcher,
) -> anyhow::Result<Vec<u64>> {
    if first_after < last_before {
        return Ok(Vec::new());
    }
    let mut produced = fetcher.get_blocks(last_before, first_after)?;
    produced.sort_unstable();
    produced.dedup();
    Ok(produced)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::MockFetcher;
    use crate::report::Status;
    use proptest::prelude::*;
    use std::collections::BTreeSet;

    fn reason(result: &CheckResult) -> serde_json::Value {
        let detail: serde_json::Value =
            serde_json::from_str(result.detail.as_ref().expect("detail present"))
                .expect("detail is JSON");
        detail["reason"].clone()
    }

    #[test]
    fn orphans_does_not_flag_indexed_slots_above_the_finalized_slot() {
        // 103 and 104 were produced but are not final, so getBlocks omits
        // them. An indexer at confirmed wrote both: that is not an orphan.
        let fetcher = MockFetcher::new(0)
            .with_blocks([100, 101])
            .with_finalized(102);
        let result = check_orphans(&[100, 101, 103, 104], 100, 104, &fetcher);
        assert_eq!(result.status, Status::Skipped);
        assert_eq!(reason(&result), "not finalized");
        assert!(
            result
                .detail
                .as_ref()
                .unwrap()
                .contains("\"unfinalized_count\":2")
        );
    }

    #[test]
    fn orphans_below_the_finalized_slot_still_fail() {
        let fetcher = MockFetcher::new(0)
            .with_blocks([100, 101])
            .with_finalized(102);
        let result = check_orphans(&[100, 102, 104], 100, 104, &fetcher);
        assert_eq!(result.status, Status::Fail);
        assert!(
            result
                .detail
                .as_ref()
                .unwrap()
                .contains("\"orphan_count\":1")
        );
    }

    #[test]
    fn orphans_window_fully_finalized_passes() {
        let fetcher = MockFetcher::new(0)
            .with_blocks([100, 101, 103])
            .with_finalized(500);
        let result = check_orphans(&[100, 101, 103], 100, 104, &fetcher);
        assert_eq!(result.status, Status::Pass);
    }

    #[test]
    fn window_entirely_above_finalized_makes_no_getblocks_call() {
        let fetcher = MockFetcher::new(0).with_finalized(99);
        let orphans = check_orphans(&[100], 100, 104, &fetcher);
        let completeness = check_completeness(&[100], 100, 104, &fetcher);
        assert_eq!(orphans.status, Status::Skipped);
        assert_eq!(completeness.status, Status::Skipped);
        assert_eq!(reason(&completeness), "not finalized");
        assert_eq!(fetcher.block_fetches(), 0);
    }

    #[test]
    fn completeness_never_passes_a_tail_it_could_not_see() {
        let fetcher = MockFetcher::new(0)
            .with_blocks([100, 101])
            .with_finalized(101);
        let result = check_completeness(&[100, 101], 100, 104, &fetcher);
        assert_eq!(result.status, Status::Skipped);
        assert!(
            result
                .detail
                .as_ref()
                .unwrap()
                .contains("\"checked_end\":101")
        );
    }

    #[test]
    fn completeness_gap_below_the_finalized_slot_fails() {
        let fetcher = MockFetcher::new(0)
            .with_blocks([100, 101])
            .with_finalized(101);
        let result = check_completeness(&[100], 100, 104, &fetcher);
        assert_eq!(result.status, Status::Fail);
    }

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

    #[test]
    fn adjacent_slots_backfill_both_boundary_blocks() {
        // Carbon reports 101 - 100 = 1 missed slot, but the drop can land
        // inside 100 and the new stream can start inside 101.
        let fetcher = MockFetcher::new(0).with_blocks([99, 100, 101, 102]);
        let gaps = gap_slots(100, 101, &fetcher).expect("reachable");
        assert_eq!(gaps, vec![100, 101]);
    }

    #[test]
    fn a_reversed_window_is_empty_and_makes_no_fetch() {
        let fetcher = MockFetcher::new(0).with_blocks([100, 101, 102]);
        let gaps = gap_slots(101, 100, &fetcher).expect("reachable");
        assert!(gaps.is_empty());
        // The short circuit happens before the fetcher: no call, not a call
        // that happened to match nothing.
        assert_eq!(fetcher.block_fetches(), 0);
    }

    #[test]
    fn skip_slots_are_not_gaps() {
        // The reconnect window [99, 106] covers heights 99..=106, but the
        // leader skipped 102, so getBlocks never produced it. Only produced
        // blocks belong in the backfill list.
        let fetcher = MockFetcher::new(0).with_blocks([99, 100, 101, 103, 104, 105, 106]);
        let gaps = gap_slots(99, 106, &fetcher).expect("reachable");
        assert_eq!(gaps, vec![99, 100, 101, 103, 104, 105, 106]);
        assert_eq!(fetcher.block_fetches(), 1);
    }

    #[test]
    fn a_window_wider_than_one_getblocks_call_keeps_every_produced_block() {
        // Two chunks' worth of window (the fetcher chunks at 500,000 slots);
        // the backfill list must be complete, sorted and unique regardless.
        let boundary = crate::fetch::MAX_GETBLOCKS_RANGE;
        let fetcher = MockFetcher::new(0).with_blocks([boundary - 1, boundary, boundary + 1]);
        let gaps = gap_slots(boundary - 2, boundary + 3, &fetcher).expect("reachable");
        assert_eq!(gaps, vec![boundary - 1, boundary, boundary + 1]);
    }

    #[test]
    fn unreachable_fetch_is_an_error_not_an_empty_list() {
        let fetcher = MockFetcher::unreachable();
        let result = gap_slots(100, 200, &fetcher);
        assert!(
            result.is_err(),
            "an unreachable fetch must not read as no gaps"
        );
    }

    proptest! {
        /// The list is exactly the produced blocks inside the closed range,
        /// in slot order, with duplicates collapsed.
        #[test]
        fn gaps_are_the_produced_blocks_in_the_closed_range(
            last_before in 0u64..1_000_000,
            span in 0u64..64,
            produced in prop::collection::vec(0u64..1_100_000, 0..300),
        ) {
            let first_after = last_before + span;
            let fetcher = MockFetcher::new(0).with_blocks(produced.iter().copied());
            let got = gap_slots(last_before, first_after, &fetcher).expect("reachable");
            let expected: Vec<u64> = produced
                .iter()
                .copied()
                .collect::<BTreeSet<u64>>()
                .into_iter()
                .filter(|slot| *slot >= last_before && *slot <= first_after)
                .collect();
            prop_assert_eq!(got, expected);
        }
    }
}
