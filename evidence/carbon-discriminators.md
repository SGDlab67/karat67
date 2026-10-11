# Account discriminators across Carbon's decoders

`carbon-account-discriminators.csv` lists every account discriminator
declared in Carbon's 63 decoders (`sevenlabs-hq/carbon` at `a64db3c`,
2026-09-10): decoder, program id, account type, discriminator (hex), and
whether it equals `sha256("account:<Name>")[..8]`. Regenerate it with:

```bash
python3 -I evidence/carbon_discriminators.py <carbon checkout> evidence/carbon-account-discriminators.csv
```

It holds names and discriminators only. A row is not a registry entry: the
registry also needs a length rule measured on live accounts, the way
`src/checks/specs.rs` records each one.

## Counts

| | |
|---|---:|
| Decoders | 63 |
| Decoders with account discriminators | 46 |
| Account types | 263 |
| Discriminator equals `sha256("account:<Name>")[..8]` | 257 |
| Discriminators declared by more than one program | 37 |
| Account types behind those 37 | 106 |

Of the 17 decoders without one, 8 declare no account types at all. The other
9 decode accounts without the `discriminator != [...]` check the script reads,
which includes native programs such as System and Token; a discriminator
registry does not cover those. The 6 mismatches are not errors: 5 are
marginfi-v2 `Minimal*` structs that carry another program's discriminator
under a local name (Kamino `Obligation` and `Reserve`, Drift `SpotMarket` and
`User`, a one-byte Solend tag), and one is the native `AddressLookupTable`
with a 4-byte tag.

## Why it matters for the registry

An Anchor discriminator hashes the struct name, not the program. 106 of 263
account types (40%) share their discriminator with a type in another program,
because names like `Pool`, `Position`, `Config` and `Oracle` are common.

Five registered types are among them:

| Registered as | Same discriminator also declared by |
|---|---|
| Kamino Lend `Obligation` | marginfi-v2 |
| Kamino Lend `Reserve` | kamino-vault, marginfi-v2 |
| Meteora DBC `Operator` | meteora-damm-v2, meteora-dlmm |
| Meteora DBC `TokenBadge` | meteora-damm-v2, meteora-dlmm, orca-whirlpool |
| Orca Whirlpool `Position` | meteora-damm-v2, jupiter-perpetuals |

Lookup today takes bytes alone, so a DAMM v2 `Position` is judged against
Whirlpool's 216-byte rule and fails even when healthy. Some rows are the same
account seen twice: marginfi-v2 and kamino-vault declare Kamino's own
`Obligation` and `Reserve` so they can read them, and those rows name
Kamino-owned accounts.
The Meteora and Whirlpool pairs are distinct accounts with distinct layouts,
measured for `Oracle` at 3,232 bytes (DLMM) against 254 (Whirlpool).

Keying the registry on `(owner, discriminator)` removes the ambiguity, and
`KaratDecoder` already holds the owner. That is the next registry change.
