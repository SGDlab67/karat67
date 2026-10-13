# karat67

![CI](https://github.com/SGDlab67/karat67/actions/workflows/ci.yml/badge.svg)
![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)

**Verify that Solana indexer data is correct, not just flowing.**

An indexer can pass every freshness and throughput check while writing empty
or malformed accounts, and every dashboard, risk engine, and AI agent built
on that data inherits the error. `karat67` is an open-source Rust crate and
CLI that checks indexed data against the chain.

Try it in the browser, no install:
[sgdlab67.github.io/karat67](https://sgdlab67.github.io/karat67/). Pick a live
Kamino, Meteora or Orca account, damage the stored copy, and watch shape and
reconcile catch it. The page runs this crate compiled to WebAssembly and reads
mainnet through PublicNode's free endpoint, or yours.

## Checks

1. **Shape** - account data length and discriminator against the program IDL.
   Layouts live in a single `AccountSpec` registry (`checks::specs`): a new
   account type is one data row, not a new size/discriminator code path.
   Catches the failure class where rows arrive empty while every liveness
   signal stays green.
2. **Reconciliation** - sample N indexed accounts, fetch them via
   `getMultipleAccounts`, and byte-diff indexed values against on-chain
   values. Catches wrong-but-well-shaped rows that the shape check passes.
   Optional `--indexed-slot` / `--max-slot-lag` (default 32) tolerate recent
   indexer lag: a mismatch within the window is `Skipped` with a JSON reason
   (`"reason":"slot lag"`), never `Pass`. Beyond the window, or with no write
   slot, a mismatch is still `Fail`.
3. **Completeness** - indexed slot set compared against Solana `getBlocks`
   for a window, not a contiguous slot range. Leaders skip slots; heights
   omitted by `getBlocks` are not gaps. Missing = produced slots minus
   indexed. Catches dropped slots that look healthy under green liveness.
4. **Orphans** - the mirror of completeness: indexed slots inside the window
   that `getBlocks` never produced. Catches state written for a slot that was
   skipped or forked away, which is what indexing at `processed` risks.

Freshness and liveness are out of scope by design: every indexer already
has them.

## CLI

```bash
cargo install --path .

# Shape: check an observed payload against the account registry
# (29 types: Kamino Lend, Meteora DBC, Meteora DLMM, Orca Whirlpool).
karat shape --account-type UserMetadata --len 1032

# Reconciliation: diff indexed bytes against on-chain state. Pass the RPC
# endpoint (or set KARAT_RPC_URL) and, per account, its indexed bytes as
# base64. --account and --indexed-base64 are repeatable and paired by order.
# Optional --indexed-slot (same count) enables slot-lag tolerance;
# --max-slot-lag defaults to 32.
karat reconcile \
  --rpc-url https://api.mainnet-beta.solana.com \
  --account So11111111111111111111111111111111111111112 \
  --indexed-base64 "$INDEXED_BASE64" \
  --indexed-slot "$INDEXED_SLOT" \
  --max-slot-lag 32

# One account, real indexed bytes: shape those exact bytes, and only if
# that is Pass reconcile the same account. Same slot-lag rules as reconcile
# (default max lag 32; no --indexed-slot means a mismatch is Fail). A shape
# failure prints that result and does not call RPC. Exit 0 only when every
# result is Pass.
karat account \
  --account So11111111111111111111111111111111111111112 \
  --indexed-base64 "$INDEXED_BASE64" \
  --indexed-slot "$INDEXED_SLOT" \
  --max-slot-lag 32 \
  --rpc-url https://api.mainnet-beta.solana.com

# Completeness: compare indexed slots to getBlocks for [start, end].
# Repeatable --slot and/or a --slots CSV. Skip-slots omitted by getBlocks
# are not reported as missing.
karat completeness \
  --rpc-url https://api.mainnet-beta.solana.com \
  --start 250000000 \
  --end 250000100 \
  --slots 250000000,250000001,250000003 \
  --slot 250000004

# Orphans: same flags as completeness (RPC from --rpc-url or KARAT_RPC_URL),
# asking the reverse question.
karat orphans --start 250000000 --end 250000100 --slots 250000000,250000002

# Gaps: the exact backfill list after a stream reconnect. Takes the two slots
# a disconnecting datasource knows (Carbon's DatasourceDisconnection: last
# slot before the drop, first slot after the reconnect) and lists every block
# the chain produced strictly between them. Skip-slots are absent by
# construction, and the summary prints Carbon's own estimate next to the real
# count. --out writes the full list, one slot per line.
karat gaps \
  --rpc-url https://api.mainnet-beta.solana.com \
  --last-before 250000000 --first-after 250000300 \
  --out gaps.txt
```

On a terminal, `shape`, `account`, `completeness`, `orphans` and `gaps` print a
readable report; piped (or with `--json`) they print JSON, one object per
check. `reconcile` always prints JSON. `account` runs shape on the decoded
bytes and, only when that is Pass, the reconcile check for that account, so a
bad payload never costs an RPC call. All of these exit non-zero unless every
check is `Pass`. `Fail` and `Skipped` (including slot-lag and unreachable
fetch) both fail the process, so they can be wired into pipeline checks and CI
directly. On a byte
mismatch, `reconcile` reports the offending account, the indexed vs on-chain
lengths, and the first differing byte offset; lag-tolerated rows add
`"reason":"slot lag"`. On a completeness gap, Fail detail lists
`missing_count` and a `missing_sample` of produced-but-unindexed slots.

`gaps` is the one exception to the exit rule, and deliberately so: its
deliverable is a list, so it exits 0 whenever the list was computed, empty or
not. Only a fetch that could not be made exits 1. The full list lands in
`--json` (unless `--out` already wrote it) or in the file `--out` names; the
terminal report lists the first 16 slots and points at `--out` for the rest.

The RPC client uses a lightweight, rustls-based HTTP client (`ureq`) instead
of the full `solana-client` stack, so builds stay toolchain-only with no
OpenSSL system dependency.

## Run it in CI

Offline, no RPC, no secrets: copy
[`.github/workflows/karat-check.yml`](.github/workflows/karat-check.yml) into
your repository and point `--account-type` and `--len` at an observed indexer
row. An empty or malformed payload fails the job with exit code 1, so a bad
row stops the pipeline instead of landing in the warehouse.

Reconcile and completeness call a live RPC. Keep those opt-in behind
`KARAT_RPC_URL` rather than in default CI.

## Origin

A Kamino Lend indexer ingested empty payloads for `Obligation` and
`UserMetadata` accounts for 158.91 hours while every liveness signal stayed
green: checkpoints advanced, freshness was low, rows kept arriving, the
containers were up. Nothing in the monitoring suite asked whether the data
had the right shape. `karat67` exists so the next pipeline does not learn
this lesson the same way.


## Carbon integration

With the `carbon` feature, `KaratDecoder` wraps any Carbon 2
`AccountDecoder`. Wiring it into a pipeline changes one line:

```rust
use karat67::carbon::KaratDecoder;

builder.account(KaratDecoder::new(PROGRAM_ID, KaminoLendingDecoder), processor)
```

Carbon reads a decoder's `None` as "not my account" and skips it silently,
so an empty or truncated payload never shows up anywhere. Borsh also decodes
an over-long payload without complaint. The wrapper closes both gaps:

- A registered type at the wrong length is dropped before the processor sees it.
- An account the program owns but its decoder could not decode is counted,
  separately from accounts that belong to other programs.
- A type karat has no spec for passes through and is counted as unchecked,
  never as checked.

The counters (`karat_accounts_checked_total`, `karat_accounts_rejected_total`,
`karat_accounts_unchecked_total`, `karat_decode_failures_total`) register with
Carbon's `MetricsRegistry`, so the pipeline's existing log or Prometheus
exporter reports them. The feature needs Rust 1.96.1 or later (Carbon 2's
minimum) and is off by default.

`examples/carbon-kamino` runs Carbon's own Kamino Lend decoder, with and
without the wrapper, on a live mainnet Obligation and three corrupted copies:

```
$ cargo run --manifest-path examples/carbon-kamino/Cargo.toml
  payload                  Carbon alone   with karat
  live Obligation          decoded        decoded
  empty payload            dropped        dropped
  truncated by 24 bytes    dropped        dropped
  16 bytes too long        decoded        dropped

  karat_accounts_checked_total     1
  karat_accounts_rejected_total    1
  karat_accounts_unchecked_total   0
  karat_decode_failures_total      2
```

Without the feature, [`KaratIntegrityProcessor`](src/carbon.rs) runs the same
checks from any decode path with no Carbon dependency: `check_account` is the
shape check, and `gate_account` is the same gate as `karat account`
(`cargo run --example account_gate`).

## MCP

`karat-mcp` is a stdio JSON-RPC server (`initialize`, `tools/list`,
`tools/call`) with two tools. `check_account` matches `karat account`:
`account`, `indexed_base64`, optional `indexed_slot`, `max_slot_lag` (default
32), and `rpc_url` (or `KARAT_RPC_URL`). Shape runs first; reconcile runs only
after Pass, so a bad payload never calls RPC. `find_gaps` matches `karat gaps`:
`last_before`, `first_after`, and `rpc_url` (or `KARAT_RPC_URL`), returning the
backfill list for the window.

The tool text is JSON `{ "ok": bool, ... }`. `ok` is true only when the call
completed and every result is Pass, the same rule as the CLI exit code.
`isError` is the inverse of `ok`.

Send one JSON object per line, or a `Content-Length` frame. Replies are
`Content-Length` frames.

```bash
cargo run --quiet --bin karat-mcp
```

## Roadmap

- `karat gaps` (done): the exact backfill list for the window a disconnecting
  stream reports. Carbon 2's `DatasourceDisconnection` carries the two
  boundary slots, but its `missed_slots` field is a height count that includes
  leader skip-slots and both observed ends, and lists nothing. `gaps` runs
  `getBlocks` over the open interval and emits the produced blocks, sorted and
  deduped, which is what a replayer can consume (Carbon adapter:
  `karat67::carbon::gaps_for_disconnection`). Measured live on mainnet: a
  19,999-slot window produced 19,987 blocks with 12 leader skips, where
  Carbon's formula reports 20,000; a 600,001-slot window (599,549 blocks) is
  two `getBlocks` calls, because the fetcher chunks at the RPC's 500,000-slot
  cap, so the 158.91 h outage (about 1.4M slots) would be three. The
  off-by-one is drafted upstream in `updates/carbon-missed-slots-draft.md`.
- IDL-driven account specs (done for Kamino Lend, Meteora DBC, Meteora DLMM
  and Orca Whirlpool, 29 types): new account types are `AccountSpec` data and
  the `Check` trait is the uniform seam. Each program after the first cost one
  spec table and zero check code, which was the claim; it also surfaced three
  things worth recording. Length alone cannot name a type (`VirtualPool` and
  `TransferHookPool` are both 424 bytes). Not every Anchor account has one
  legal length (DLMM positions grow 112 bytes per extra bin), which is why
  `AccountSpec` carries a `LenRule`. And a discriminator names a struct, not a
  program: DLMM and Whirlpool both define `Oracle` at different lengths, so
  lookup has to key on the owner before either can be registered. Coverage of
  a snapshot is not an "accounts for all" claim.
- Owner-aware lookup, keyed on `(program, discriminator)`. Across Carbon's
  decoders, 106 of 263 account types share a discriminator with another
  program ([evidence](evidence/carbon-discriminators.md)), and 5 registered
  types are among them.
- Generated specs: `evidence/carbon-account-discriminators.csv` already holds
  every discriminator in Carbon's 63 decoders; lengths still need measuring on
  live accounts before a row joins the registry.
- Broader agent workflows on top of the shipped `check_account` MCP tool.

Designed to plug into [Carbon](https://github.com/sevenlabs-hq/carbon)
pipelines without rewriting the indexer.

## Status

Early development, pre-release. API unstable. Built in the open, solo, for
the Colosseum Crypto World's Fair 2026.

## Demo

```bash
bash demo/worlds_fair.sh
```

Fetches a live Kamino Obligation and a fresh slot window from mainnet, checks
every beat off screen first, then steps through four checks on Enter: an
empty payload, the live bytes, the live bytes with one byte flipped, and a
slot window with three slots dropped. See [`demo/README.md`](demo/README.md).

## License

MIT
