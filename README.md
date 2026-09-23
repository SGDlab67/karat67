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
2. **Reconciliation** (in progress) - sample N indexed accounts, fetch them
   via `getMultipleAccounts`, and diff against indexed values, allowing for
   slot lag.
3. **Completeness** (planned) - slot coverage compared against `getBlocks`,
   not a contiguous slot range, because Solana leaders skip slots.

Freshness and liveness are out of scope by design: every indexer already
has them.

## CLI

```bash
cargo install --path .
karat shape --account-type UserMetadata --len 1032
```

On a terminal the command prints a readable report; piped (or with
`--json`) it prints JSON. It exits non-zero on failure, so it can be wired
into pipeline checks and CI directly.

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
