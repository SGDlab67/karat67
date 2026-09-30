# karat67 examples

Every block below is real captured output, run 2026-09-25 against
`api.mainnet-beta.solana.com` (slots around 450,458,xxx). Exit codes are real.
`Fail` and `Skipped` both exit non-zero, so any of these lines drops into CI or
a pipeline step unchanged.

## 1. shape: is this row the right shape at all?

The registry (`src/checks/specs.rs`) holds four Kamino Lend account types.
A new type is one data row, not a new code path.

```
$ karat shape --account-type Obligation    --len 3344   {"check":"shape(Obligation)","status":"Pass"}      exit 0
$ karat shape --account-type UserMetadata  --len 1032   {"check":"shape(UserMetadata)","status":"Pass"}    exit 0
$ karat shape --account-type Reserve       --len 8624   {"check":"shape(Reserve)","status":"Pass"}         exit 0
$ karat shape --account-type LendingMarket --len 4664   {"check":"shape(LendingMarket)","status":"Pass"}   exit 0
```

The failure this crate exists for, one command:

```
$ karat shape --account-type UserMetadata --len 0
{
  "check": "shape",
  "status": "Fail",
  "detail": "empty payload: 0 bytes, every allocated account starts with an 8-byte discriminator"
}
exit 1
```

A truncated row, which every liveness dashboard also reports as healthy:

```
$ karat shape --account-type Obligation --len 3320
{
  "check": "shape(Obligation)",
  "status": "Fail",
  "detail": "truncated: expected 3344 bytes, got 3320 (24 missing)"
}
exit 1
```

An account type nobody registered is an error, not a quiet pass:

```
$ karat shape --account-type Foo --len 100
Error: unknown account type "Foo", expected one of ["Obligation", "UserMetadata", "Reserve", "LendingMarket"]
exit 1
```

## 2. reconcile: are the bytes we stored the bytes the chain has?

Shape passes on well-formed garbage. Reconcile re-reads the account over
`getMultipleAccounts` and byte-diffs it. Account used here is the wrapped SOL
mint, 82 bytes, with byte 40 flipped to simulate a bad write.

Indexed bytes match the chain:

```
$ karat reconcile --account So111...112 --indexed-base64 "$GOOD"
{"check":"reconcile(So11111111111111111111111111111111111111112)","status":"Pass","detail":null}
exit 0
```

One byte off, no write slot supplied, so there is no lag to excuse it. The
detail names the account, both lengths and the first differing offset:

```
$ karat reconcile --account So111...112 --indexed-base64 "$BAD"
{
  "check": "reconcile(So11111111111111111111111111111111111111112)",
  "status": "Fail",
  "detail": "{\"account\":\"So111...112\",\"context_slot\":450458566,\"first_diff_offset\":40,\"indexed_len\":82,\"onchain_len\":82}"
}
exit 1
```

Same mismatch, but the indexer wrote the row 13 slots ago, inside the default
32-slot window. That is `Skipped` with a stated reason, never `Pass`, and it
still exits non-zero:

```
$ karat reconcile --account So111...112 --indexed-base64 "$BAD" --indexed-slot 450458692
{
  "check": "reconcile(So11111111111111111111111111111111111111112)",
  "status": "Skipped",
  "detail": "{\"account\":\"So111...112\",\"context_slot\":450458705,\"first_diff_offset\":40,\"indexed_slot\":450458692,\"lag\":13,\"reason\":\"slot lag\"}"
}
exit 1
```

Two edges worth knowing:

* Lag of exactly 0 is `Fail`, not `Skipped`. Indexer and RPC agree on the slot,
  so lag cannot explain the difference, and the row is simply wrong.
* Lag past the window is `Fail` with `indexed_slot` in the detail, so the report
  says how stale the row was.

```
$ karat reconcile --account So111...112 --indexed-base64 "$BAD" --indexed-slot 450453523
... "status": "Fail", "context_slot":450458583, "indexed_slot":450453523 ...
exit 1
```

## 3. completeness: did we index every slot that actually existed?

Compared against `getBlocks` for the window, not against a contiguous range.

Every produced slot present:

```
$ karat completeness --start 450456749 --end 450456849 --slots "$PRODUCED"
{"check":"completeness([450456749..=450456849])","status":"Pass","detail":null}
exit 0
```

Drop three of them:

```
$ karat completeness --start 450456749 --end 450456849 --slots "$DROPPED"
{
  "check": "completeness([450456749..=450456849])",
  "status": "Fail",
  "detail": "{\"end\":450456849,\"indexed_count\":98,\"missing_count\":3,\"missing_sample\":[450456753,450456757,450456788],\"produced_count\":101,\"start\":450456749}"
}
exit 1
```

Leader-skipped slots are the whole reason this check reads `getBlocks` instead
of iterating the range. Mainnet produced all 101 slots in every window sampled
on 2026-09-25, so the skip case is covered by
`leader_skip_slots_not_indexed_still_pass` in `src/checks/completeness.rs`:
window [100, 104] with 102 absent from `getBlocks` and absent from the index is
`Pass`, because 102 never existed.

## 4. Library and Carbon processor

`examples/integrity_gate.rs` is what a Carbon-style pipeline runs per decoded
account, and it asserts its own expectations, so `cargo run --example
integrity_gate` is also a check.

```
$ cargo run --example integrity_gate
well-formed row                           3344 bytes  Pass  -
empty payload (the 158.91-hour failure)      0 bytes  Fail  empty payload: 0 bytes, every allocated account starts with an 8-byte discriminator
truncated row                             3000 bytes  Fail  truncated: expected 3344 bytes, got 3000 (344 missing)
over-long row                             3360 bytes  Fail  over-long: expected 3344 bytes, got 3360 (16 extra)
foreign discriminator                     3344 bytes  Fail  unknown discriminator: [7, 7, 7, 7, 7, 7, 7, 7]
unregistered account type                 3344 bytes  Fail  unknown account type "SomeOtherProgramAccount"

all cases behaved as documented
```

Two cases only reachable from the library, since the CLI takes `--len` rather
than bytes: a foreign 8-byte discriminator, and an account type absent from the
registry. Both `Fail`. The gate never passes data it cannot describe.

## 5. Wiring into CI

Exit codes are the entire interface, so no wrapper is needed:

```yaml
- name: integrity gate
  env:
    KARAT_RPC_URL: ${{ secrets.KARAT_RPC_URL }}
  run: |
    karat shape --account-type Obligation --len "$OBSERVED_LEN"
    karat reconcile --account "$PK" --indexed-base64 "$B64" --indexed-slot "$SLOT"
    karat completeness --start "$FROM" --end "$TO" --slots "$INDEXED"
```

Any `Fail`, and any `Skipped` (slot lag, unreachable RPC), stops the job.
