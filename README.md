# karat67

**Verify that Solana indexer data is correct, not just flowing.**

An indexer can pass every freshness and throughput check while writing empty
or malformed accounts, and every dashboard, risk engine, and AI agent built
on that data inherits the error. `karat67` is an open-source Rust crate and
CLI that checks indexed data against the chain.

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

Freshness and liveness are out of scope by design: every indexer already
has them.

## CLI

```bash
cargo install --path .

# Shape: check an observed payload against the Kamino account registry.
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

# Completeness: compare indexed slots to getBlocks for [start, end].
# Repeatable --slot and/or a --slots CSV. Skip-slots omitted by getBlocks
# are not reported as missing.
karat completeness \
  --rpc-url https://api.mainnet-beta.solana.com \
  --start 250000000 \
  --end 250000100 \
  --slots 250000000,250000001,250000003 \
  --slot 250000004
```

On a terminal, `shape` and `completeness` print a readable report; piped (or
with `--json`) they print JSON. `reconcile` prints one JSON result per
account. All three exit non-zero unless every check is `Pass` — `Fail` and
`Skipped` (including slot-lag and unreachable fetch) both fail the process,
so they can be wired into pipeline checks and CI directly. On a byte
mismatch, `reconcile` reports the offending account, the indexed vs on-chain
lengths, and the first differing byte offset; lag-tolerated rows add
`"reason":"slot lag"`. On a completeness gap, Fail detail lists
`missing_count` and a `missing_sample` of produced-but-unindexed slots.

The RPC client uses a lightweight, rustls-based HTTP client (`ureq`) instead
of the full `solana-client` stack, so builds stay toolchain-only with no
OpenSSL system dependency.

## Origin

A Kamino Lend indexer ingested empty payloads for `Obligation` and
`UserMetadata` accounts for 158.91 hours while every liveness signal stayed
green: checkpoints advanced, freshness was low, rows kept arriving, the
containers were up. Nothing in the monitoring suite asked whether the data
had the right shape. `karat67` exists so the next pipeline does not learn
this lesson the same way.

## Roadmap

- IDL-driven account specs (done for Kamino Lend): new account types are
  `AccountSpec` data; the `Check` trait is the uniform seam. Broader
  multi-program IDL ingest still to come.
- Carbon integration: integrity checks as a processor, results emitted
  through the existing metrics layer.
- MCP wrapper so agents can ask whether the indexer behind their data is
  passing integrity checks before acting on it.

Designed to plug into [Carbon](https://github.com/sevenlabs-hq/carbon)
pipelines without rewriting the indexer.

## Status

Early development, pre-release. API unstable. Built in the open, solo, for
the Colosseum Crypto World's Fair 2026.

## License

MIT
