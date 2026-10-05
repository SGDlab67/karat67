# Residual 647: unexplained DBC accounts after the mainnet sweep

This note classifies the **647** accounts left after the Meteora Dynamic
Bonding Curve (DBC) `getProgramAccounts` attribution recorded in
`src/checks/specs.rs`. It exists so the project can **refuse** any
"accounts for all" claim with a number, not a slogan.

Program: `dbcij3LWUppWqq96dh6gJWwBifmcGfLSB5D4DuSMaqN` (mainnet-beta).

## Sweep arithmetic (as documented)

| Bucket | Accounts |
|---|---:|
| Swept (live GPA) | 2,406,183 |
| Attributed (registered Exact + three PartnerMetadata lengths + two known-unregistered discs) | 2,405,536 |
| **Residual unexplained** | **647** |

Attributed breakdown from the comments in `specs.rs`:

| Spec / bucket | Len rule | Length(s) | Count |
|---|---|---:|---:|
| VirtualPool | Exact | 424 | 1,738,650 |
| PoolConfig | Exact | 1,048 | 534,996 |
| MeteoraDammV2Metadata | Exact | 230 | 81,948 |
| MeteoraDammMigrationMetadata | Exact | 280 | 42,953 |
| TokenBadge | Exact | 168 | 2,376 |
| PartnerMetadata (documented lengths only) | AtLeast ≥ 148 | 148 / 152 / 249 | 66 + 30 + 33 = **129** |
| Known-unregistered disc `[237, 219, …]` | — | 424 | 2,523 |
| Known-unregistered disc `[40, 220, …]` | — | 1,128 | 1,961 |
| **Sum attributed** | | | **2,405,536** |

The residual is everything else: live accounts whose `(discriminator, length)`
pair was not one of those rows.

## Classification of the 647

Raw per-account residual dumps from the original sweep were **not** checked
into `evidence/`. The classification below is a **live recount** (2026-10-05,
public mainnet RPC) of every discriminator that could absorb the hole, then
reconciled against the original attribution rules.

Reconstructed residual (live):

| Rank | Bucket | Live count | Share of 647 | Collides with known Exact? | Covered by existing AtLeast? |
|---:|---|---:|---:|---|---|
| 1 | `PartnerMetadata` at lengths **other than** 148 / 152 / 249 | **485** | ~75% | No (variable-length type) | **Yes** — same disc, all observed ≥ 148 |
| 2 | `VirtualPoolMetadata` (not in karat registry) | **160** | ~25% | No | No — unknown discriminator today |
| 3 | `Operator` (not in karat registry) | **1** | ~0.2% | Length 72 is unique | No — unknown discriminator today |
| | **Sum** | **646** | | | |

Off-by-one vs the documented 647 (646 live) is timing drift between the
original sweep and this recount (DBC grew: VirtualPool alone is now
1,738,745). No other residual-sized population appeared.

### 1. PartnerMetadata length variants (~485) — attribution hole, not a shape hole

The registry already carries `PartnerMetadata` as `LenRule::AtLeast` with
floor 148. The sweep docs only enumerated three observed lengths (129
accounts). Live, that discriminator has **614** accounts across **113**
distinct lengths; **zero** are below the floor.

| PartnerMetadata slice | Live count |
|---|---:|
| Documented lengths 148 / 152 / 249 | 129 |
| Other lengths ≥ 148 | 485 |
| Lengths &lt; 148 | 0 |
| **Total** | **614** |

Top residual PartnerMetadata lengths (live):

| Length | Count |
|---:|---:|
| 221 | 22 |
| 217 | 18 |
| 220 | 17 |
| 245 | 15 |
| 247 | 14 |
| 252 | 13 |
| 222 | 12 |
| 224 | 12 |
| 248 | 12 |
| 251 | 12 |

These accounts **already pass** `check_shape` under the current AtLeast rule.
They inflated the "unexplained" total only because attribution counted three
literal length buckets, not "every account carrying this discriminator."

### 2. VirtualPoolMetadata (160) — missing registry row, IDL-verified name

Discriminator `sha256("account:VirtualPoolMetadata")[..8]` =
`[217, 37, 82, 250, 43, 47, 228, 254]`. Present in Meteora DBC IDL releases
0.1.2–0.1.6 and in `carbon` `meteora-dbc-decoder`. Layout is variable-length
strings (same pattern as PartnerMetadata). Live min length **168**, max
**393**, 160 accounts, many distinct sizes.

`check_shape` today: **Fail** (`unknown discriminator`).

### 3. Operator (1) — missing registry row, IDL-verified name

Discriminator `sha256("account:Operator")[..8]` =
`[219, 31, 188, 145, 69, 139, 204, 117]`. Layout from Meteora source /
carbon decoder: 8 (disc) + 32 + 16 + 16 = **72** bytes Exact. Live: 1 account
at 72.

`check_shape` today: **Fail** (`unknown discriminator`).

### Exact registered types: no length outliers

Live memcmp recount — every registered Exact discriminator is at its single
declared length (no off-size population hiding in the 647):

| Type | Live count | Length | Odd lengths |
|---|---:|---:|---|
| VirtualPool | 1,738,745 | 424 | none |
| PoolConfig | 535,024 | 1,048 | none |
| MeteoraDammV2Metadata | 81,948 | 230 | none |
| MeteoraDammMigrationMetadata | 42,953 | 280 | none |
| TokenBadge | 2,376 | 168 | none |

## Side finding: the two "known-unregistered" discs now have names

These were **already counted in the 2,405,536 attributed** figure (as honest
unknowns), not in the 647. Name confirmation uses the same rule as the
registry docs: `sha256("account:<Name>")[..8]` must match the observed disc.
Sizes match Meteora `INIT_SPACE + 8`.

| Observed disc | Length | Live count | Confirmed name | Source size |
|---|---:|---:|---|---|
| `[237, 219, 184, 23, 42, 189, 169, 35]` | 424 | 2,525 | **TransferHookPool** | `INIT_SPACE` 416 |
| `[40, 220, 194, 251, 41, 199, 123, 253]` | 1,128 | 1,963 | **ConfigWithTransferHook** | `INIT_SPACE` 1,120 |

Both still collide with another type on length alone (`VirtualPool` also 424;
`PoolConfig` is 1,048, not 1,128 — but the 424 collision remains the headline
reason length-only checks are unsafe).

IDL types with **zero** live accounts on this recount (not part of the 647):
`ClaimFeeOperator`, `Config` (legacy), `LockEscrow`.

## What this means for claims

- **Do not** say karat67 accounts for all DBC accounts. Against the original
  sweep, **647 / 2,406,183 ≈ 0.027%** were outside the attributed rows, and
  **4,484** more were intentionally unregistered unknowns.
- Of the 647, ~75% were already handled by PartnerMetadata's AtLeast rule;
  ~25% are real missing types (`VirtualPoolMetadata`, `Operator`).
- After registering the missing / newly named types, coverage of *this*
  snapshot improves — it still does not license an "accounts for all"
  sentence for a live program that can mint new account kinds.

## Recommended next commit (code, separate from this note)

1. **Register `VirtualPoolMetadata`** — disc above, `LenRule::AtLeast`, floor
   **148** from fixed fields (8 + 32 + 96 + 3×4), with the same observed-floor
   caveat style as PartnerMetadata (live min was 168).
2. **Register `Operator`** — Exact **72**, disc above.
3. **Optionally register `TransferHookPool` (Exact 424) and
   `ConfigWithTransferHook` (Exact 1128)** now that names verify — replace the
   "known-unregistered" bullet list with named rows. Keep the 424 collision
   test; rename the impostor fixture to `TransferHookPool`.
4. **Widen PartnerMetadata docs** (not the rule): note 100+ live lengths, all
   ≥ 148; the three quoted lengths were examples, not an exhaustive set.
5. **Leave** `ClaimFeeOperator` / legacy `Config` / `LockEscrow` unregistered
   until a live account exists (or register Exact rows with count 0 in
   comments if you want IDL completeness without implying production load).

Do **not** invent further discriminators. Any new row still needs the
`sha256("account:<Name>")` match (or carbon decoder agreement) before it
lands in `METEORA_DBC_ACCOUNTS`.

## Reproduce (live)

```bash
# PartnerMetadata length histogram (disc [68,68,130,19,16,209,98,156])
# VirtualPoolMetadata count (disc [217,37,82,250,43,47,228,254])
# Operator count (disc [219,31,188,145,69,139,204,117])
# Use getProgramAccounts + memcmp offset 0 + dataSlice length 0; group by account.space.
```

Public `api.mainnet-beta.solana.com` works for filtered GPAs; expect 429 if
you hammer it. Counts drift as the program grows — re-run before quoting.
