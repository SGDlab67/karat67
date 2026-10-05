# Evidence: silent zero-length Obligations in a production index

The README claims green liveness hid empty Kamino payloads for 158.91 hours.
This directory lets anyone reproduce that number in one query, and shows the
failure class that `karat shape` exists to catch.

`zero_length_obligations.parquet` (644 KB, zstd) is a slice of a real
`klend-indexer` run: a Yellowstone gRPC to ClickHouse indexer for Kamino Lend.
It holds every zero-length account update in the window plus the complete
history of one affected account. Account `data` bytes are excluded; the
columns are `pubkey`, `slot`, `write_version`, `data_len`, `lamports`, `kind`,
`ingested_at`.

Window: slots 437,280,282 to 439,271,320. Source run: 932,930 rows over
167,316 distinct slots.

## Reproduce the headline number

```bash
duckdb -c "
SELECT round(date_diff('second', min(ingested_at), max(ingested_at))/3600.0, 2) AS hours,
       count(*) AS rows, count(DISTINCT pubkey) AS pubkeys
FROM read_parquet('zero_length_obligations.parquet') WHERE data_len = 0;"
```

```
hours = 158.91 | rows = 64650 | pubkeys = 4385
```

## Why these are corrupt writes and not account closures

A Solana account that is closed has its lamports reclaimed. Every one of these
64,650 rows has `lamports > 0`, so none of them is a close:

```bash
duckdb -c "
SELECT count(*) FILTER (WHERE lamports = 0) AS closures,
       count(*) FILTER (WHERE lamports > 0) AS still_funded
FROM read_parquet('zero_length_obligations.parquet') WHERE data_len = 0;"
```

```
closures = 0 | still_funded = 64650
```

Stronger still, the dataset contradicts itself. 3,185 of those pubkeys carry a
full 3,344-byte Obligation at other slots while holding the same lamports. An
Obligation cannot be 3,344 bytes at one slot and 0 bytes at another without
losing a lamport:

```bash
duckdb -c "
WITH per_key AS (
  SELECT pubkey,
         count(*) FILTER (WHERE data_len = 0)    AS zero_rows,
         count(*) FILTER (WHERE data_len = 3344) AS obligation_rows
  FROM read_parquet('zero_length_obligations.parquet') GROUP BY pubkey)
SELECT count(*) AS self_contradicting_pubkeys, sum(zero_rows) AS bad_rows
FROM per_key WHERE zero_rows > 0 AND obligation_rows > 0;"
```

```
self_contradicting_pubkeys = 3185 | bad_rows = 57708
```

The remaining 1,200 pubkeys are only ever zero-length in this window. They are
not counted above because this slice cannot tell a corrupt write from an
account the subscription picks up and never decodes.

## One account, start to finish

`BYojGuT56e2TUb8PQwRyT1wL5X5Ekv4kZH1HUQgBu6Zg`, a Kamino Obligation, holding
24,165,120 lamports throughout:

| slot | data_len | lamports |
|---|---|---|
| 437,280,282 | 3,344 | 24,165,120 |
| 437,903,892 | **0** | 24,165,120 |
| 439,264,851 | 3,344 | 24,165,120 |

```bash
duckdb -c "
WITH t AS (
  SELECT slot, data_len, lamports,
         lag(data_len) OVER (ORDER BY slot, write_version) AS prev_len
  FROM read_parquet('zero_length_obligations.parquet')
  WHERE hex(pubkey) = '9CBAB5F9B9F21F08DA5F515653BBE3732287A45B8F18526E120E8767073C127B')
SELECT slot, prev_len AS before, data_len AS after, lamports
FROM t WHERE prev_len IS DISTINCT FROM data_len ORDER BY slot;"
```

The index held an empty Obligation for 1,360,959 slots. Freshness, throughput,
and liveness were green for all of it, because rows kept arriving. Only the
bytes were wrong, and nothing was looking at the bytes.

That is what `shape` looks at: the account's length and Anchor discriminator
against the IDL. Run it over this slice and every `data_len = 0` row fails,
with no RPC and no network.
