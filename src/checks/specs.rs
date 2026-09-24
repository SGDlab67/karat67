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

/// IDL-declared shape of one account type: its 8-byte Anchor discriminator
/// and its exact on-chain size (discriminator included).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountSpec {
    pub account_type: &'static str,
    pub discriminator: [u8; 8],
    pub data_len: usize,
}

/// Kamino Lending account specs — the IDL-informed table shape checks consult.
///
/// Verified against klend `*_SIZE` constants (+ 8 for the Anchor discriminator)
/// and carbon `kamino-lending-decoder` discriminators.
pub const KAMINO_ACCOUNTS: &[AccountSpec] = &[
    AccountSpec {
        account_type: "Obligation",
        discriminator: [168, 206, 141, 106, 88, 76, 172, 167],
        // OBLIGATION_SIZE 3336 + 8
        data_len: 3344,
    },
    AccountSpec {
        account_type: "UserMetadata",
        discriminator: [157, 214, 220, 235, 98, 135, 171, 28],
        // USER_METADATA_SIZE 1024 + 8
        data_len: 1032,
    },
    AccountSpec {
        account_type: "Reserve",
        discriminator: [43, 242, 204, 202, 26, 247, 59, 127],
        // RESERVE_SIZE 8616 + 8
        data_len: 8624,
    },
    AccountSpec {
        account_type: "LendingMarket",
        discriminator: [246, 114, 50, 98, 72, 157, 28, 120],
        // LENDING_MARKET_SIZE 4656 + 8
        data_len: 4664,
    },
];

/// Look up a registered account by its Anchor discriminator.
pub fn find_by_discriminator(discriminator: [u8; 8]) -> Option<&'static AccountSpec> {
    KAMINO_ACCOUNTS
        .iter()
        .find(|spec| spec.discriminator == discriminator)
}

/// Look up a registered account by its IDL type name (CLI / caller path).
pub fn find_by_account_type(account_type: &str) -> Option<&'static AccountSpec> {
    KAMINO_ACCOUNTS
        .iter()
        .find(|spec| spec.account_type == account_type)
}

/// Names of every registered account type, in registry order.
pub fn account_type_names() -> Vec<&'static str> {
    KAMINO_ACCOUNTS
        .iter()
        .map(|spec| spec.account_type)
        .collect()
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
        for spec in KAMINO_ACCOUNTS {
            assert_eq!(
                find_by_discriminator(spec.discriminator).map(|s| s.account_type),
                Some(spec.account_type)
            );
        }
    }
}
