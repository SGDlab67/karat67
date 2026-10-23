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

## Inspect: one call before you act on an account

```bash
karat inspect --account Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE \
  --rpc-url https://solana-rpc.publicnode.com
```

```
  OK  Orca Whirlpool Whirlpool at a legal length, layout verified against the current deploy

  type           Orca Whirlpool Whirlpool
  length         653 bytes
  read at slot   460,593,973
  deploy slot    457,152,296  (registry verified at 457,152,296)

    fee_rate                       400
    sqrt_price                     5945506145713508477
    tick_current_index             -22647
    token_mint_a                   So11111111111111111111111111111111111111112
    token_mint_b                   EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v
    ...
```

`inspect` reads an account and returns what it is, what it holds, and one
verdict on whether to trust the decode:

| verdict | meaning |
|---|---|
| `ok` | registered type, legal length, layout verified against the program as deployed now |
| `stale_registry` | the program was redeployed after its layouts were verified: fields are decoded and may be wrong |
| `layout_mismatch` | registered type at a length its rule forbids |
| `unknown_layout` | owned by a covered program under a discriminator the registry does not have: a new account kind, or one an upgrade added |
| `unregistered_program` | owned by a program karat does not cover; nothing is claimed |
| `missing` | no account at that address |

Lookup is keyed on the owner first, then the discriminator, so a struct name
two programs share (`Oracle`, `Position`) is never read with the wrong
program's layout. The deploy check is what keeps `ok` honest over time: each
covered program records the deploy slot its layouts were last verified
against, and any later deploy downgrades the verdict until someone rechecks.
Three of the four covered programs were redeployed between 2026-10-12 and
2026-10-23; the layouts were rechecked against live accounts of the new
deploys before the baseline moved.

Decoded today: Kamino `Obligation`, `Reserve`, `LendingMarket`; Orca
`Whirlpool`, `Position`; Meteora DLMM `LbPair`; Meteora DBC `VirtualPool`
(the fields an agent acts on: mints, vaults, reserves, prices, ticks). Every
offset was confirmed on live accounts: pubkeys resolve to accounts of the
right kind, and the SOL/USDC Whirlpool and DLMM pools decode to the same
price. The other 22 registered types are identified and shape-checked, with
fields not decoded yet. Integers wider than 32 bits come back as decimal
strings so JavaScript readers do not lose precision.

Pass `--indexed-base64` (and `--indexed-slot`) to reconcile your stored copy
in the same call. Exit 0 only when the verdict is `ok` and the copy matches.
The same call is the `inspect_account` MCP tool.

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
   With `--indexed-slot`, a mismatch first asks the chain when the account
   was last touched (`getSignaturesForAddress`, limit 1, pinned with
   `minContextSlot` to the read). Untouched since the indexed slot means the
   bytes cannot have legitimately changed: `Fail` at any lag, with
   `last_touch_slot` in the detail. Touched after it means the indexer is
   behind on that account: `Skipped` with `"reason":"written after indexed
   slot"`, at any lag. Read-only references count as touches, so the answer
   errs toward skipping, never toward a false `Fail`. Only when the node has
   no history for the account does the slot-lag window decide. A chain read
   older than the indexed slot (a lagging node behind a load balancer) is
   `Skipped` with `"reason":"rpc behind index"`, never `Fail`. Accounts are
   read at `confirmed`, 100 keys per `getMultipleAccounts` call, or fewer
   when the endpoint names a lower cap (PublicNode's free tier allows 10).
3. **Completeness** - indexed slot set compared against Solana `getBlocks`
   for a window, not a contiguous slot range. Leaders skip slots; heights
   omitted by `getBlocks` are not gaps. Missing = produced slots minus
   indexed. Catches dropped slots that look healthy under green liveness.
4. **Orphans** - the mirror of completeness: indexed slots inside the window
   that `getBlocks` never produced. Catches state written for a slot that was
   skipped or forked away, which is what indexing at `processed` risks.

Completeness and orphans stop at the finalized slot (`getSlot`, finalized),
because `getBlocks` answers at `finalized`: above it, absent means "not final
yet", not "never produced". If the window runs past it, a clean result is
`Skipped` with `"reason":"not finalized"` rather than `Pass`, and a gap or
orphan below it is still `Fail`. On mainnet in October 2026 the finalized slot
read within one slot of `processed` on two providers, so this rarely bites
there; it matters on lagging nodes and slower clusters.

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

# Sentinel: the slot a program's code last changed at, and the Carbon
# boundary that follows from it. Two reads: the Program account names its
# ProgramData account, and 45 bytes of that decode as the header (deploy
# slot + upgrade authority). No state file, machine code never downloaded.
# --since <SLOT> exits 1 when the program changed after that slot.
karat sentinel \
  --program pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA \
  --rpc-url https://api.mainnet-beta.solana.com

# Same read, plus a live shape sample: list the accounts carrying a
# discriminator, fetch a strided sample of them in full, shape-check each,
# and print the live length distribution. --account-type picks a registered
# type; --discriminator takes raw hex for a type the registry does not
# know yet. A registered type whose live accounts fail shape exits 1.
karat sentinel \
  --program KLend2g3cP87fffoy8q1mQqGKjrxjC8boSyAYavgmjD \
  --sample 20 --account-type Obligation \
  --rpc-url https://api.mainnet-beta.solana.com
```

On a terminal, `shape`, `account`, `completeness`, `orphans`, `gaps` and
`sentinel` print a readable report; piped (or with `--json`) they print JSON, one object per
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

`sentinel` follows the normal exit rule with one extra gate: exit 1 when the
read fails, when `--since` finds the program changed after that slot, or
when a sampled registered type fails shape.

The RPC client uses a lightweight, rustls-based HTTP client (`ureq`) instead
of the full `solana-client` stack, so builds stay toolchain-only with no
OpenSSL system dependency.

## Upgrades and stale decoders

A program upgrade changes account layouts without changing the discriminator,
and every decoder written before that slot is silently wrong about the data.
Carbon can run versioned decoders behind a `SlotRangeFilter`, but only if the
caller already knows the upgrade slot. `karat sentinel` reads it off the
chain: two RPC calls, no state file.

```
$ karat sentinel --program pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA

  karat67  sentinel · pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA

  deploy slot   454,596,406
  authority     7gZufwwAo17y5kg8FMyJy2phgpvv9RSdzWtdXiWHjFr8

  carbon boundary, paste into a versioned decoder set:
    SlotRangeFilter::from(454596406, None)   the current layout, from this slot on
    SlotRangeFilter::to(454596406, None)     the layout before it
```

The Program account arrives through `jsonParsed` and names its ProgramData
account; the first 45 bytes of that account (`dataSlice`) decode as the
`UpgradeableLoaderState::ProgramData` header, which carries the deploy slot
and the upgrade authority. The program's machine code is never downloaded.

`--sample N` answers the second half of the question, "is the decoder
actually stale?": it lists the accounts carrying a discriminator through
`getProgramAccounts` (a memcmp filter and a zero-byte `dataSlice`, so no
account data comes back), fetches a strided sample of N of them in full,
shape-checks each against the registry, and reports the live length
distribution. The sample is strided because the first N accounts come back
in node storage order and are not representative. A registered type whose
live accounts fail shape exits non-zero, and an unregistered discriminator
cannot pass a shape check at all, so measuring an unknown type reports the
distribution and still exits non-zero. A large program's key list can be
tens of megabytes (pump_swap's `Pool` is about 1.5M accounts, about 50 MB of
keys), so the RPC client raises its response ceiling for calls like this;
only the sampled accounts are fetched in full.

All five programs karat knows still carry an upgrade authority, read live on
2026-10-12 (by 2026-10-23 Kamino Lend, Meteora DBC and Orca Whirlpool had
been redeployed again, at 458,858,791, 457,502,600 and 457,152,296; see
`PROGRAMS` in `src/checks/specs.rs`):

| program | deploy slot | upgrade authority |
|---|---|---|
| Kamino Lend | 440,486,775 | GzFgdRJXmawPhGeBsyRCDLx4jAKPsvbUqoqitzppkzkW |
| Meteora DLMM | 423,977,638 | JADaUV8kvDpDbJr55wxXJHVaBS3VCj8thZZHjfeuCVLd |
| Meteora DBC | 445,503,633 | JADaUV8kvDpDbJr55wxXJHVaBS3VCj8thZZHjfeuCVLd |
| Orca Whirlpool | 440,170,207 | GwH3Hiv5mACLX3ufTw1pFsrhSPon5tdw252DBs4Rx4PV |
| pump_swap | 454,596,406 | 7gZufwwAo17y5kg8FMyJy2phgpvv9RSdzWtdXiWHjFr8 |

Every one of them is still being upgraded by its team, which is the point:
an integrity check that assumes a frozen layout has an expiry date.

A 200-account sample of pump_swap `Pool` (out of 1,498,885 live) came back
at eight lengths:

```
  211 x7      243 x10     244 x6      245 x12
  271 x1      287 x2      300 x68     301 x94
```

Those lengths are one append-only layout that grew across program upgrades:
211 (the original, ending at `lp_supply`), 243 (plus `coin_creator`), 244,
245 (plus `is_mayhem_mode` and `is_cashback_coin`, which is exactly Carbon's
struct), 271 (plus `virtual_quote_reserves`, `creator_fee_bps` and two flags),
287 (the newest published layout, the pump_amm IDL of 2026-10-07), and
300 / 301 (the 287 layout plus 13 or 14 trailing zero bytes; the SDK
constant `POOL_ACCOUNT_NEW_SIZE` is 300). Counted against the 1.5M live
pools: about 10 percent fail Carbon's decoder entirely (Borsh underflow,
which Carbon reads as "not my account" and drops silently), only 6.8 percent
match its 245-byte struct exactly, and the rest decode as a silent prefix,
losing fields like `virtual_quote_reserves` and `creator_fee_bps`. That is
decoder staleness seen from the outside, with receipts. `Pool` is
deliberately not in the registry yet: six-plus era layouts, and the tail
semantics of the newest two still open. A rule that cannot be defended
against the live distribution is worse than a documented gap. For layouts
that do split by slot and are understood, `AccountSpec.len_eras` carries one
legal length per era and `len_at(slot)` resolves it.

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
`tools/call`) with three tools. `inspect_account` matches `karat inspect`:
`account`, optional `indexed_base64`, `indexed_slot`, `max_slot_lag`, and
`rpc_url` (or `KARAT_RPC_URL`); its text is the inspection JSON, and a
non-`ok` verdict is an answer (`isError` false), not a failed call, so an
agent branches on `verdict` instead of retrying. `check_account` matches `karat account`:
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
- `karat sentinel` (done): the slot a program last changed at, from its
  ProgramData header in two reads, printed as the Carbon `SlotRangeFilter`
  boundary to paste, plus `--sample N` to measure what a program actually
  writes. All five known programs read clean; a 200-account `Pool` sample
  came back at eight live lengths (2026-10-12), which is why no `Pool` row
  is registered yet.
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
- Owner-aware lookup, keyed on `(program, discriminator)` (done for
  `inspect` through `find_owned`; `shape` and `KaratDecoder` still look up by
  discriminator alone). Across Carbon's
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
