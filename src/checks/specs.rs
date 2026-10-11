//! IDL-informed account registry: the single source of truth for shape checks.
//!
//! A new account type is one [`AccountSpec`] entry — discriminator and
//! exact on-chain byte length — not a new branch in [`crate::checks::shape`].
//! A new program is a new table of the same shape (or an addition to this
//! one). The [`crate::checks::check::Check`] trait stays the uniform seam;
//! the data here is what makes "a new program costs zero check code" true.
//!
//! # Verification
//!
//! Sizes come from Kamino Lend's `*_SIZE` constants (struct body only);
//! `data_len` is that size plus the 8-byte Anchor discriminator. Discriminators
//! match the carbon `kamino-lending-decoder` account decode arms (and
//! `sha256("account:<Name>")[0..8]`).
//!
//! Meteora Dynamic Bonding Curve (DBC), program
//! `dbcij3LWUppWqq96dh6gJWwBifmcGfLSB5D4DuSMaqN` (the same address on mainnet
//! and devnet), was measured rather than read off an IDL: a full
//! `getProgramAccounts` sweep of the program on mainnet-beta returned
//! 2,406,183 live accounts, grouped by their first 8 bytes and by length. Each
//! struct name below was then confirmed independently, by computing
//! `sha256("account:<StructName>")[..8]` and matching it against the observed
//! discriminator, so a name here is a verified claim and not a guess at what a
//! byte prefix might mean. The per-type account counts are recorded with each
//! entry because they are the evidence: they say how much live data each rule
//! is actually asserted over.
//!
//! # Why length alone cannot name a type
//!
//! The sweep found `VirtualPool` at 424 bytes and `TransferHookPool` (named
//! later via `sha256("account:TransferHookPool")[..8]`) also at 424 bytes.
//! Two different account types, byte-identical in length. A length-only check
//! would therefore accept one type's bytes as the other's and silently
//! validate a payload it has never seen a layout for, which is exactly the
//! "green while wrong" failure this crate exists to catch. The discriminator
//! is what names a type; the length is only a corroborating constraint on the
//! named type. That is why every rule here is a pair.
//!
//! # Coverage is not completeness
//!
//! Registering every named type seen in a snapshot still does not license an
//! "accounts for all" claim: DBC can mint new account kinds, and some IDL
//! types (`ClaimFeeOperator`, legacy `Config`, `LockEscrow`) had zero live
//! accounts on the recount that named `TransferHookPool` /
//! `ConfigWithTransferHook`. Absent rows stay absent until a live account
//! exists or an Exact zero-count row is added deliberately. A future reader
//! adding a type still needs the name confirmed the same way (matching
//! `sha256("account:<StructName>")[..8]`), not inferred from the size.
//!
//! # Name reuse across programs
//!
//! An Anchor discriminator depends only on the struct name, so two programs
//! that both define `Oracle` (Meteora DLMM at 3,232 bytes, Orca Whirlpool at
//! 254) share one discriminator. Lookup here takes bytes alone, without the
//! owning program, so a reused name can only be registered for one program.
//! `Oracle`, `Operator` and `TokenBadge` are registered at most once for that
//! reason, and an unregistered program that reuses a registered name (say a
//! `Position` from a program other than Whirlpool) is judged against the
//! registered program's rule. Across Carbon's decoders, 106 of 263 account
//! types share a discriminator with another program
//! (`evidence/carbon-discriminators.md`).
// ponytail: discriminator-only lookup. Key on (owner, discriminator) once a
// caller always has the owner; `KaratDecoder` does, the CLI does only via RPC.

/// How an account type's `data_len` constrains an observed payload.
///
/// Anchor accounts split into two kinds and conflating them breaks the check
/// in one direction or the other. A fixed-layout account has exactly one legal
/// length, so anything else is corruption and must fail. An account carrying
/// variable-length fields (strings, vectors) has a legal length *range*, so
/// demanding an exact match would fail healthy accounts and train a reader to
/// ignore the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LenRule {
    /// `data_len` is the only legal length: shorter is truncated, longer is
    /// over-long, both are failures.
    Exact,
    /// `data_len` is a floor, not a target. Anything at or above it passes;
    /// only an underrun fails, because a payload below the floor cannot even
    /// hold the type's fixed fields.
    AtLeast,
}

/// IDL-declared shape of one account type: its 8-byte Anchor discriminator
/// and its on-chain size (discriminator included), read as exactly or at
/// least that many bytes according to `len_rule`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountSpec {
    /// Owning program, for output that would otherwise have to guess. The
    /// registry spans more than one program, so a printer cannot name it from
    /// context.
    pub program: &'static str,
    pub account_type: &'static str,
    pub discriminator: [u8; 8],
    /// Exact on-chain length when `len_rule` is [`LenRule::Exact`], the
    /// minimum legal length when it is [`LenRule::AtLeast`].
    pub data_len: usize,
    pub len_rule: LenRule,
}

/// Kamino Lending account specs — the IDL-informed table shape checks consult.
///
/// Verified against klend `*_SIZE` constants (+ 8 for the Anchor discriminator)
/// and carbon `kamino-lending-decoder` discriminators.
pub const KAMINO_ACCOUNTS: &[AccountSpec] = &[
    AccountSpec {
        program: "Kamino Lend",
        account_type: "Obligation",
        discriminator: [168, 206, 141, 106, 88, 76, 172, 167],
        // OBLIGATION_SIZE 3336 + 8
        data_len: 3344,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Kamino Lend",
        account_type: "UserMetadata",
        discriminator: [157, 214, 220, 235, 98, 135, 171, 28],
        // USER_METADATA_SIZE 1024 + 8
        data_len: 1032,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Kamino Lend",
        account_type: "Reserve",
        discriminator: [43, 242, 204, 202, 26, 247, 59, 127],
        // RESERVE_SIZE 8616 + 8
        data_len: 8624,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Kamino Lend",
        account_type: "LendingMarket",
        discriminator: [246, 114, 50, 98, 72, 157, 28, 120],
        // LENDING_MARKET_SIZE 4656 + 8
        data_len: 4664,
        len_rule: LenRule::Exact,
    },
];

/// Meteora Dynamic Bonding Curve account specs.
///
/// Lengths and discriminators are the observed on-chain values from the
/// mainnet-beta `getProgramAccounts` sweep described in the module docs; the
/// comment on each row is that type's live account count at sweep time.
pub const METEORA_DBC_ACCOUNTS: &[AccountSpec] = &[
    AccountSpec {
        program: "Meteora DBC",
        account_type: "VirtualPool",
        discriminator: [213, 224, 5, 209, 98, 69, 119, 92],
        // 1,738,650 accounts. 424 bytes is NOT unique to this type: see
        // TransferHookPool below.
        data_len: 424,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Meteora DBC",
        account_type: "PoolConfig",
        discriminator: [26, 108, 14, 123, 116, 230, 129, 43],
        // 534,996 accounts
        data_len: 1048,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Meteora DBC",
        account_type: "MeteoraDammV2Metadata",
        discriminator: [104, 221, 219, 203, 10, 142, 250, 163],
        // 81,948 accounts
        data_len: 230,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Meteora DBC",
        account_type: "MeteoraDammMigrationMetadata",
        discriminator: [17, 155, 141, 215, 207, 4, 133, 156],
        // 42,953 accounts
        data_len: 280,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Meteora DBC",
        account_type: "TokenBadge",
        discriminator: [116, 219, 204, 229, 249, 116, 255, 150],
        // 2,376 accounts
        data_len: 168,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Meteora DBC",
        account_type: "PartnerMetadata",
        discriminator: [68, 68, 130, 19, 16, 209, 98, 156],
        // Variable length: this type carries variable-length strings. The
        // original sweep quoted 148 / 152 / 249 (66 + 30 + 33 accounts) as
        // examples; a later recount saw 100+ distinct lengths, all ≥ 148, and
        // zero below the floor. An exact rule would fail most of that healthy
        // population. Those three lengths are illustrations, not an exhaustive
        // set.
        //
        // 148 is the floor because it is the smallest length ever observed,
        // not because an IDL says the fixed fields total 148. That provenance
        // is the weakness: a legitimate account with shorter strings than
        // anything in the sweep would be reported as an underrun. Tighten this
        // only with a real layout computation, and widen it only against new
        // observed evidence.
        data_len: 148,
        len_rule: LenRule::AtLeast,
    },
    AccountSpec {
        program: "Meteora DBC",
        account_type: "VirtualPoolMetadata",
        discriminator: [217, 37, 82, 250, 43, 47, 228, 254],
        // Variable length (same string pattern as PartnerMetadata). Disc is
        // sha256("account:VirtualPoolMetadata")[..8]; present in Meteora DBC
        // IDL 0.1.2–0.1.6 and carbon meteora-dbc-decoder. Floor 148 is the
        // fixed-field layout (8 disc + 32 + 96 + 3×4), not the live minimum —
        // the recount that registered this row saw min 168 / max 393 across
        // 160 accounts. Same honesty caveat as PartnerMetadata: a legitimate
        // account below anything observed would still pass at ≥ 148, and
        // anything below 148 cannot hold the fixed fields.
        data_len: 148,
        len_rule: LenRule::AtLeast,
    },
    AccountSpec {
        program: "Meteora DBC",
        account_type: "Operator",
        discriminator: [219, 31, 188, 145, 69, 139, 204, 117],
        // 1 account at sweep recount. Exact 72 = 8 disc + 32 + 16 + 16 from
        // Meteora layout / carbon decoder. Disc is sha256("account:Operator")[..8].
        data_len: 72,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Meteora DBC",
        account_type: "TransferHookPool",
        discriminator: [237, 219, 184, 23, 42, 189, 169, 35],
        // ~2,523 accounts in the original sweep (counted then as a known
        // unknown). Exact 424 = INIT_SPACE 416 + 8. Disc is
        // sha256("account:TransferHookPool")[..8]. Shares length with
        // VirtualPool — the collision that makes length-only checks unsafe.
        data_len: 424,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Meteora DBC",
        account_type: "ConfigWithTransferHook",
        discriminator: [40, 220, 194, 251, 41, 199, 123, 253],
        // ~1,961 accounts in the original sweep (counted then as a known
        // unknown). Exact 1128 = INIT_SPACE 1120 + 8. Disc is
        // sha256("account:ConfigWithTransferHook")[..8].
        data_len: 1128,
        len_rule: LenRule::Exact,
    },
];

/// Meteora DLMM account specs, program
/// `LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo`.
///
/// Names and discriminators come from the carbon `meteora-dlmm-decoder` and
/// match `sha256("account:<Name>")[..8]`. Lengths were measured on mainnet-beta
/// on 2026-10-11: "swept" means a `getProgramAccounts` scan of every account
/// with that discriminator, "one pool" means every account of that type
/// belonging to pool `2Dmu9a9qWcgRS5U5pyUkvtyFrkmFq4hFnK4aLLNQXsCJ`.
pub const METEORA_DLMM_ACCOUNTS: &[AccountSpec] = &[
    AccountSpec {
        program: "Meteora DLMM",
        account_type: "LbPair",
        discriminator: [33, 11, 49, 98, 181, 101, 177, 13],
        // Swept: 163,843 accounts, all 904 bytes.
        data_len: 904,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Meteora DLMM",
        account_type: "BinArray",
        discriminator: [92, 142, 92, 220, 5, 148, 70, 181],
        // Fixed layout (`bins: [Bin; 70]`), too many accounts to sweep on a
        // public endpoint: 48 from 30 recent transactions plus 7 in one pool,
        // all 10,136 bytes.
        data_len: 10136,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Meteora DLMM",
        account_type: "PositionV2",
        discriminator: [117, 176, 212, 199, 245, 180, 133, 182],
        // Variable length: 8,120 bytes covers 70 bins, and an extended position
        // adds 112 bytes per extra bin. One pool: 13 of 17 at 8,120, the rest
        // at 9,240 / 16,072 / 17,528 / 27,720, each 8,120 + k * 112. The
        // floor is the fixed layout (8 + 8,112 after repr(C) padding), not the
        // smallest length observed.
        data_len: 8120,
        len_rule: LenRule::AtLeast,
    },
    AccountSpec {
        program: "Meteora DLMM",
        account_type: "BinArrayBitmapExtension",
        discriminator: [80, 111, 124, 113, 55, 237, 18, 5],
        // Swept: 6,946 accounts, all 1,576 bytes.
        data_len: 1576,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Meteora DLMM",
        account_type: "PresetParameter2",
        discriminator: [171, 236, 148, 115, 162, 113, 222, 174],
        // Swept: 392 accounts, all 192 bytes.
        data_len: 192,
        len_rule: LenRule::Exact,
    },
];

/// Orca Whirlpool account specs, program
/// `whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc`.
///
/// Same provenance as [`METEORA_DLMM_ACCOUNTS`], from the carbon
/// `orca-whirlpool-decoder`. "One pool" here is Whirlpool
/// `3wijQvPKm6jHQrAkfPpok5o8WjCWPm1DGG17NmeW8q1w`.
pub const ORCA_WHIRLPOOL_ACCOUNTS: &[AccountSpec] = &[
    AccountSpec {
        program: "Orca Whirlpool",
        account_type: "Whirlpool",
        discriminator: [63, 149, 209, 12, 225, 128, 99, 9],
        // Swept: 163,038 accounts, all 653 bytes.
        data_len: 653,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Orca Whirlpool",
        account_type: "Position",
        discriminator: [170, 188, 143, 228, 122, 64, 247, 208],
        // Fixed layout, too many to sweep: 20 in one pool, all 216 bytes.
        // "Position" is a common Anchor name; see the module docs on reuse.
        data_len: 216,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Orca Whirlpool",
        account_type: "TickArray",
        discriminator: [69, 97, 189, 190, 110, 7, 66, 187],
        // Fixed layout (`ticks: [Tick; 88]`): 12 from 30 recent transactions
        // plus 3 in one pool, all 9,988 bytes.
        data_len: 9988,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Orca Whirlpool",
        account_type: "DynamicTickArray",
        discriminator: [17, 216, 246, 142, 225, 199, 218, 56],
        // Variable length: 60 fixed bytes plus 88 ticks of 1 byte each when
        // uninitialized, 113 when initialized. 148 is that empty layout, and
        // every length seen (148, 260, 1,268, 1,380, ... 9,892) is
        // 148 + k * 112.
        data_len: 148,
        len_rule: LenRule::AtLeast,
    },
    AccountSpec {
        program: "Orca Whirlpool",
        account_type: "FeeTier",
        discriminator: [56, 75, 159, 76, 142, 68, 190, 105],
        // Swept: 673 accounts, all 44 bytes.
        data_len: 44,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Orca Whirlpool",
        account_type: "WhirlpoolsConfig",
        discriminator: [157, 20, 49, 224, 217, 87, 193, 254],
        // Swept: 508 accounts, all 108 bytes.
        data_len: 108,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Orca Whirlpool",
        account_type: "WhirlpoolsConfigExtension",
        discriminator: [2, 99, 215, 163, 240, 26, 153, 58],
        // Swept: 197 accounts, all 616 bytes.
        data_len: 616,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Orca Whirlpool",
        account_type: "AdaptiveFeeTier",
        discriminator: [147, 16, 144, 116, 47, 146, 149, 46],
        // Swept: 17 accounts, all 256 bytes.
        data_len: 256,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Orca Whirlpool",
        account_type: "LockConfig",
        discriminator: [106, 47, 238, 159, 124, 12, 160, 192],
        // Swept: 1,706 accounts, all 241 bytes.
        data_len: 241,
        len_rule: LenRule::Exact,
    },
    AccountSpec {
        program: "Orca Whirlpool",
        account_type: "PositionBundle",
        discriminator: [129, 169, 175, 65, 185, 95, 32, 100],
        // Swept: 225 accounts, all 136 bytes.
        data_len: 136,
        len_rule: LenRule::Exact,
    },
];

/// Every registered spec across all program tables, in registry order.
pub fn all_specs() -> impl Iterator<Item = &'static AccountSpec> {
    KAMINO_ACCOUNTS
        .iter()
        .chain(METEORA_DBC_ACCOUNTS)
        .chain(METEORA_DLMM_ACCOUNTS)
        .chain(ORCA_WHIRLPOOL_ACCOUNTS)
}

/// Look up a registered account by its Anchor discriminator.
pub fn find_by_discriminator(discriminator: [u8; 8]) -> Option<&'static AccountSpec> {
    all_specs().find(|spec| spec.discriminator == discriminator)
}

/// Look up a registered account by its IDL type name (CLI / caller path).
pub fn find_by_account_type(account_type: &str) -> Option<&'static AccountSpec> {
    all_specs().find(|spec| spec.account_type == account_type)
}

/// Names of every registered account type, in registry order.
pub fn account_type_names() -> Vec<&'static str> {
    all_specs().map(|spec| spec.account_type).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_lending_market_as_data_only_entry() {
        // Lock: LendingMarket was added as an AccountSpec row only — no
        // size/discriminator arms live in check_shape. Presence here is the
        // whole change surface for a new type.
        let spec = find_by_account_type("LendingMarket").expect("LendingMarket in registry");
        assert_eq!(spec.data_len, 4664);
        assert_eq!(spec.discriminator, [246, 114, 50, 98, 72, 157, 28, 120]);
        assert!(KAMINO_ACCOUNTS.len() >= 4);
    }

    #[test]
    fn every_spec_is_findable_by_discriminator() {
        for spec in all_specs() {
            assert_eq!(
                find_by_discriminator(spec.discriminator).map(|s| s.account_type),
                Some(spec.account_type)
            );
        }
    }

    #[test]
    fn every_row_carries_its_own_table_program() {
        // Guards the copy-paste that adds a row to one table with the other
        // table's program name, which would make the CLI header name the wrong
        // program while every other check still passed.
        for spec in KAMINO_ACCOUNTS {
            assert_eq!(spec.program, "Kamino Lend", "{}", spec.account_type);
        }
        for spec in METEORA_DBC_ACCOUNTS {
            assert_eq!(spec.program, "Meteora DBC", "{}", spec.account_type);
        }
        for spec in METEORA_DLMM_ACCOUNTS {
            assert_eq!(spec.program, "Meteora DLMM", "{}", spec.account_type);
        }
        for spec in ORCA_WHIRLPOOL_ACCOUNTS {
            assert_eq!(spec.program, "Orca Whirlpool", "{}", spec.account_type);
        }
    }

    #[test]
    fn oracle_stays_unregistered_while_lookup_ignores_the_owner() {
        // DLMM and Whirlpool both name an account `Oracle`, so both share
        // sha256("account:Oracle")[..8], at 3,232 and 254 bytes. Registering
        // either would fail every healthy account of the other.
        let oracle = [139, 194, 131, 179, 140, 179, 229, 244];
        assert!(find_by_discriminator(oracle).is_none());
    }

    #[test]
    fn discriminators_are_unique_across_programs() {
        // Lock: lookup is by discriminator alone, so a collision between
        // tables would silently resolve one program's account as another's.
        for spec in all_specs() {
            let matches = all_specs()
                .filter(|other| other.discriminator == spec.discriminator)
                .count();
            assert_eq!(
                matches, 1,
                "{} has a colliding discriminator",
                spec.account_type
            );
        }
    }

    #[test]
    fn dbc_specs_are_findable_by_name_and_discriminator() {
        for (name, discriminator, data_len) in [
            ("VirtualPool", [213, 224, 5, 209, 98, 69, 119, 92], 424),
            ("PoolConfig", [26, 108, 14, 123, 116, 230, 129, 43], 1048),
            (
                "MeteoraDammV2Metadata",
                [104, 221, 219, 203, 10, 142, 250, 163],
                230,
            ),
            (
                "MeteoraDammMigrationMetadata",
                [17, 155, 141, 215, 207, 4, 133, 156],
                280,
            ),
            ("TokenBadge", [116, 219, 204, 229, 249, 116, 255, 150], 168),
            ("Operator", [219, 31, 188, 145, 69, 139, 204, 117], 72),
            (
                "TransferHookPool",
                [237, 219, 184, 23, 42, 189, 169, 35],
                424,
            ),
            (
                "ConfigWithTransferHook",
                [40, 220, 194, 251, 41, 199, 123, 253],
                1128,
            ),
        ] {
            let spec = find_by_account_type(name).expect("DBC type in registry");
            assert_eq!(spec.discriminator, discriminator);
            assert_eq!(spec.data_len, data_len);
            assert_eq!(spec.len_rule, LenRule::Exact);
            assert_eq!(
                find_by_discriminator(discriminator).map(|s| s.account_type),
                Some(name)
            );
        }
    }

    #[test]
    fn dbc_424_byte_length_is_shared_by_two_distinct_types() {
        // The headline finding: VirtualPool and TransferHookPool are both
        // 424 bytes, so length cannot identify a type.
        let virtual_pool = find_by_account_type("VirtualPool").expect("VirtualPool in registry");
        let transfer_hook =
            find_by_account_type("TransferHookPool").expect("TransferHookPool in registry");
        assert_eq!(virtual_pool.data_len, 424);
        assert_eq!(transfer_hook.data_len, 424);
        assert_ne!(virtual_pool.discriminator, transfer_hook.discriminator);
        assert_eq!(
            transfer_hook.discriminator,
            [237, 219, 184, 23, 42, 189, 169, 35]
        );
    }

    #[test]
    fn partner_metadata_is_a_minimum_length_rule() {
        let spec = find_by_account_type("PartnerMetadata").expect("PartnerMetadata in registry");
        assert_eq!(spec.len_rule, LenRule::AtLeast);
        // 148 is the smallest length observed on mainnet, used as the floor.
        // 148 / 152 / 249 in the docs are examples; live lengths exceed those.
        assert_eq!(spec.data_len, 148);
    }

    #[test]
    fn virtual_pool_metadata_is_a_minimum_length_rule() {
        let spec =
            find_by_account_type("VirtualPoolMetadata").expect("VirtualPoolMetadata in registry");
        assert_eq!(spec.len_rule, LenRule::AtLeast);
        assert_eq!(spec.data_len, 148);
        assert_eq!(spec.discriminator, [217, 37, 82, 250, 43, 47, 228, 254]);
    }

    #[test]
    fn kamino_specs_all_stay_exact() {
        // Regression lock: adding a variable-length kind must not relax any
        // fixed-layout Kamino rule.
        for spec in KAMINO_ACCOUNTS {
            assert_eq!(
                spec.len_rule,
                LenRule::Exact,
                "{} must keep exact-length checking",
                spec.account_type
            );
        }
    }

    #[test]
    fn account_type_names_span_both_programs() {
        let names = account_type_names();
        assert!(names.contains(&"Obligation"));
        assert!(names.contains(&"VirtualPool"));
        assert_eq!(
            names.len(),
            KAMINO_ACCOUNTS.len()
                + METEORA_DBC_ACCOUNTS.len()
                + METEORA_DLMM_ACCOUNTS.len()
                + ORCA_WHIRLPOOL_ACCOUNTS.len()
        );
    }
}
