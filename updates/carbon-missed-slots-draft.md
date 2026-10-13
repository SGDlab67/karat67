# Upstream draft: `DatasourceDisconnection.missed_slots` off by one

Draft written 2026-10-12 while building `karat gaps` (Milestone 1 of
`updates/agent-brief-real-issues.md`). Not filed. Filing is the user's call.

Target: `sevenlabs-hq/carbon`, `datasources/yellowstone-grpc-datasource/src/lib.rs`
(the same arithmetic is repeated in `rpc-block-subscribe-datasource`).

Related: Carbon #476 (silent data loss after reconnect), fixed by PR #471 which
introduced `DatasourceDisconnection` in carbon-core 2.0.0.

## What the field claims and what it computes

carbon-core 2.0.0, `crates/core/src/datasource.rs`:

```rust
/// Number of slots missed during downtime.
pub missed_slots: u64,
```

`datasources/yellowstone-grpc-datasource/src/lib.rs`:

```rust
let missed = slot.saturating_sub(last_slot);

let disconnection = DatasourceDisconnection {
    source: "yellowstone-grpc".to_string(),
    disconnect_time,
    last_slot_before_disconnect: last_slot,
    first_slot_after_reconnect: slot,
    missed_slots: missed,
};
```

`last_slot` is the slot of the last message processed before the drop; `slot`
is the slot of the first message processed after the reconnect. Both boundary
slots were processed, so the interval that can contain missed blocks is open at
both ends: `(last_slot, slot)`.

`slot - last_slot` is one more than the number of heights in that open
interval, and it is a height count, not a count of missed blocks.

## The failing test

Extract the arithmetic into a named function and pin the boundary:

```rust
/// Heights strictly between the last message before a drop and the first
/// message after the reconnect. Both ends were processed, so a pair of
/// adjacent slots means nothing was missed.
fn slots_missed(last_slot: Slot, first_slot: Slot) -> u64 {
    first_slot.saturating_sub(last_slot + 1)
    // or, over the open interval: first_slot - last_slot - 1 with saturating
    // arithmetic on both ends.
}

#[test]
fn adjacent_slots_report_nothing_missed() {
    // The stream processed slot 100, dropped, and its next message was slot
    // 101. No height in between produced a block.
    assert_eq!(slots_missed(100, 101), 0);
}

#[test]
fn a_one_slot_outage_reports_one_slot() {
    // 100 processed, 102 next: height 101 is the only candidate.
    assert_eq!(slots_missed(100, 102), 1);
}
```

Against the code as merged, the first test fails: `missed` is `1`.

## The four suspected weaknesses, confirmed against the source

Read 2026-10-12 from `sevenlabs-hq/carbon` at `a64db3c` (v2.0.0):

1. **Counts leader skip slots. Confirmed. Errs over-fetch.** `missed_slots` is
   a height delta, so every leader skip inside the window is counted as a
   missed slot. A slot that never produced a block cannot have been missed.
   This is the same skip-slot rule `karat completeness` and `karat orphans`
   already encode, in opposite directions.
2. **Off by one against its own doc comment. Confirmed. Errs over-fetch by
   one.** With `last=100`, `first=101`, `missed_slots` reports 1 while the
   interval contains nothing. See the failing test above.
3. **`first_slot_after_reconnect` comes from the first update message, not the
   reconnect. Confirmed. Errs over-fetch, unboundedly on account-only
   subscriptions.** The slot is taken from the first `Account`, `Transaction`
   or `Block` update after reconnecting (`if first_message_after_reconnect`
   ... `match &msg.update_oneof`). On a subscription whose account changes
   rarely, that message can arrive long after the socket is healthy, so the
   window is stretched to cover time the stream was actually up and
   processing. The count then includes slots the indexer did see.
4. **The event can be dropped silently. Confirmed. Errs under-fetch.**
   `let _ = tx.try_send(disconnection);` discards both the send result and
   the `TrySendError`, so a full notifier channel loses the event with no log
   and no retry, and the two `take()` calls that build the event have already
   cleared the datasource's own disconnect state, so that event is lost rather
   than re-emitted. The sibling `rpc-block-subscribe-datasource` does the same
   arithmetic but does log a failed `try_send`, so this one is the odd case
   out.

Over-fetch and under-fetch in the same field is the real problem: a consumer
cannot correct `missed_slots` in either direction. It is a rough signal, not a
work list.

## Smallest upstream change

1. Fix the arithmetic and keep the failing test above: `first - last - 1`,
   saturating.
2. Re-document the field for what it is: heights between the two observed
   slots, not a verified count of missed blocks, and not adjusted for leader
   skips.
3. Log or count the dropped `try_send` instead of `let _ =`.

Anything beyond that (a real list) needs a `getBlocks` call, which is a chain
read the datasource does not do and should not start doing. That is what
karat67 is for.

## What karat67 does instead, today

`karat gaps --last-before <SLOT> --first-after <SLOT>` takes the same two
fields and returns the exact backfill list: every block `getBlocks` produced in
the open interval, sorted, with leader skip slots absent by construction. The
Carbon adapter is one call:

```rust
let gaps = karat67::carbon::gaps_for_disconnection(&event, &fetcher)?;
```

`examples/carbon-kamino` wires it to a real `YellowstoneGrpcGeyserClient`
`disconnect_notifier` channel.

For the same window, `karat gaps` prints Carbon's number next to the real one,
so the over-count is visible per event rather than argued in a review.
