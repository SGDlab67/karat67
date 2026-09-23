# karat67

**Verify that Solana indexer data is correct, not just flowing.**

An indexer can pass every freshness and throughput check while writing empty
or malformed accounts, and every dashboard, risk engine, and AI agent built
on that data inherits the error. `karat67` is an open-source Rust crate and
CLI that checks indexed data against the chain.

## Checks

1. **Shape** - account data length and discriminator against the program IDL.
   Catches the failure class where rows arrive empty while every liveness
   signal stays green.
2. **Reconciliation** - sample N indexed accounts, fetch them via
   `getMultipleAccounts`, and byte-diff indexed values against on-chain
   values. Catches wrong-but-well-shaped rows that the shape check passes.
   Slot-lag tolerance is threaded through (the RPC context slot rides along
   on every result) but not yet applied: a mismatch is always a failure for
   now, never silently a pass.
3. **Completeness** (planned) - slot coverage compared against `getBlocks`,
   not a contiguous slot range, because Solana leaders skip slots.

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
karat reconcile \
  --rpc-url https://api.mainnet-beta.solana.com \
  --account So11111111111111111111111111111111111111112 \
  --indexed-base64 "$INDEXED_BASE64"
```

On a terminal, `shape` prints a readable report; piped (or with `--json`) it
prints JSON. `reconcile` prints one JSON result per account. Both exit
non-zero unless every check passed, so they can be wired into pipeline
checks and CI directly. On a byte mismatch, `reconcile` reports the
offending account, the indexed vs on-chain lengths, and the first differing
byte offset.

The reconciliation client uses a lightweight, rustls-based HTTP client
(`ureq`) instead of the full `solana-client` stack, so builds stay
toolchain-only with no OpenSSL system dependency.

## Origin

A Kamino Lend indexer ingested empty payloads for `Obligation` and
`UserMetadata` accounts for 158.91 hours while every liveness signal stayed
green: checkpoints advanced, freshness was low, rows kept arriving, the
containers were up. Nothing in the monitoring suite asked whether the data
had the right shape. `karat67` exists so the next pipeline does not learn
this lesson the same way.

## Roadmap

- IDL-driven check trait: a new program costs zero code.
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
