//! Field layouts: where the fields agents ask about sit inside an account.
//!
//! Shape says an account is the right length; a layout says what it holds.
//! Each [`TypeLayout`] lists fields by byte offset, discriminator included,
//! for one registered `(program, account_type)`. Only fields confirmed on
//! live mainnet accounts are listed (2026-10-23, the deploys recorded in
//! [`crate::checks::specs::PROGRAMS`]): every pubkey here was resolved and
//! owned by what the field claims (a mint by a token program, a vault by a
//! token program, a config by the same program), and the SOL/USDC Whirlpool
//! and DLMM pairs decoded to the same price. A field nobody has checked stays
//! out, because a wrong offset decodes to a plausible-looking wrong value.
//!
//! Integers wider than 32 bits are emitted as decimal strings: JSON readers
//! in JavaScript lose precision above 2^53, and a balance read wrong is worse
//! than one read as a string.

use serde_json::{Map, Value};

use crate::upgrade::base58_encode;

/// How the bytes at a field's offset are read. All integers little-endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Pubkey,
    U8,
    U16,
    I32,
    U64,
    U128,
}

impl FieldKind {
    fn width(self) -> usize {
        match self {
            FieldKind::Pubkey => 32,
            FieldKind::U8 => 1,
            FieldKind::U16 => 2,
            FieldKind::I32 => 4,
            FieldKind::U64 => 8,
            FieldKind::U128 => 16,
        }
    }
}

/// One field: name, byte offset from the start of the account, and kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    pub name: &'static str,
    pub offset: usize,
    pub kind: FieldKind,
}

const fn field(name: &'static str, offset: usize, kind: FieldKind) -> Field {
    Field { name, offset, kind }
}

/// The decoded fields of one registered account type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeLayout {
    /// [`crate::checks::specs::AccountSpec::program`].
    pub program: &'static str,
    pub account_type: &'static str,
    pub fields: &'static [Field],
}

use FieldKind::{I32, Pubkey, U8, U16, U64, U128};

/// Every layout karat decodes. A registered type missing here is still
/// identified and shape-checked; its fields are just not decoded yet.
pub const LAYOUTS: &[TypeLayout] = &[
    TypeLayout {
        program: "Kamino Lend",
        account_type: "Obligation",
        fields: &[
            field("tag", 8, U64),
            field("last_update_slot", 16, U64),
            field("lending_market", 32, Pubkey),
            field("owner", 64, Pubkey),
        ],
    },
    TypeLayout {
        program: "Kamino Lend",
        account_type: "Reserve",
        fields: &[
            field("version", 8, U64),
            field("last_update_slot", 16, U64),
            field("lending_market", 32, Pubkey),
            field("liquidity_mint", 128, Pubkey),
            field("liquidity_supply_vault", 160, Pubkey),
            field("liquidity_fee_vault", 192, Pubkey),
            field("liquidity_available_amount", 224, U64),
            field("liquidity_borrowed_amount_sf", 232, U128),
            field("liquidity_market_price_sf", 248, U128),
            field("liquidity_mint_decimals", 272, U64),
        ],
    },
    TypeLayout {
        program: "Kamino Lend",
        account_type: "LendingMarket",
        fields: &[
            field("version", 8, U64),
            field("bump_seed", 16, U64),
            field("lending_market_owner", 24, Pubkey),
        ],
    },
    TypeLayout {
        program: "Orca Whirlpool",
        account_type: "Whirlpool",
        fields: &[
            field("whirlpools_config", 8, Pubkey),
            field("tick_spacing", 41, U16),
            field("fee_rate", 45, U16),
            field("protocol_fee_rate", 47, U16),
            field("liquidity", 49, U128),
            field("sqrt_price", 65, U128),
            field("tick_current_index", 81, I32),
            field("token_mint_a", 101, Pubkey),
            field("token_vault_a", 133, Pubkey),
            field("token_mint_b", 181, Pubkey),
            field("token_vault_b", 213, Pubkey),
        ],
    },
    TypeLayout {
        program: "Orca Whirlpool",
        account_type: "Position",
        fields: &[
            field("whirlpool", 8, Pubkey),
            field("position_mint", 40, Pubkey),
            field("liquidity", 72, U128),
            field("tick_lower_index", 88, I32),
            field("tick_upper_index", 92, I32),
        ],
    },
    TypeLayout {
        program: "Meteora DLMM",
        account_type: "LbPair",
        fields: &[
            field("active_id", 76, I32),
            field("bin_step", 80, U16),
            field("status", 82, U8),
            field("token_x_mint", 88, Pubkey),
            field("token_y_mint", 120, Pubkey),
            field("reserve_x", 152, Pubkey),
            field("reserve_y", 184, Pubkey),
        ],
    },
    TypeLayout {
        program: "Meteora DBC",
        account_type: "VirtualPool",
        fields: &[
            field("config", 72, Pubkey),
            field("creator", 104, Pubkey),
            field("base_mint", 136, Pubkey),
            field("base_vault", 168, Pubkey),
            field("quote_vault", 200, Pubkey),
            field("base_reserve", 232, U64),
            field("quote_reserve", 240, U64),
        ],
    },
];

/// The layout for `(program, account_type)`, if karat decodes that type.
pub fn find_layout(program: &str, account_type: &str) -> Option<&'static TypeLayout> {
    LAYOUTS
        .iter()
        .find(|layout| layout.program == program && layout.account_type == account_type)
}

/// Decode every field of `layout` from `data`.
///
/// `Err` names the first field that does not fit: callers shape-check first,
/// so this only fires when a layout outruns its own registered length.
pub fn decode(layout: &TypeLayout, data: &[u8]) -> anyhow::Result<Map<String, Value>> {
    let mut fields = Map::new();
    for field in layout.fields {
        let bytes = data
            .get(field.offset..field.offset + field.kind.width())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "field {} at offset {} needs {} bytes, account has {}",
                    field.name,
                    field.offset,
                    field.kind.width(),
                    data.len()
                )
            })?;
        fields.insert(field.name.to_string(), read(field.kind, bytes));
    }
    Ok(fields)
}

fn read(kind: FieldKind, bytes: &[u8]) -> Value {
    let array = |n: usize| -> [u8; 16] {
        let mut buf = [0u8; 16];
        buf[..n].copy_from_slice(&bytes[..n]);
        buf
    };
    match kind {
        FieldKind::Pubkey => Value::String(base58_encode(bytes)),
        FieldKind::U8 => Value::from(bytes[0]),
        FieldKind::U16 => Value::from(u16::from_le_bytes([bytes[0], bytes[1]])),
        FieldKind::I32 => Value::from(i32::from_le_bytes(
            bytes[..4].try_into().expect("width checked"),
        )),
        FieldKind::U64 => Value::String(u128::from_le_bytes(array(8)).to_string()),
        FieldKind::U128 => Value::String(u128::from_le_bytes(array(16)).to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::specs::all_specs;

    #[test]
    fn every_layout_names_a_registered_type_and_fits_inside_it() {
        for layout in LAYOUTS {
            let spec = all_specs()
                .find(|s| s.program == layout.program && s.account_type == layout.account_type)
                .unwrap_or_else(|| panic!("{} is not registered", layout.account_type));
            for field in layout.fields {
                assert!(
                    field.offset >= 8,
                    "{}.{} overlaps the discriminator",
                    layout.account_type,
                    field.name
                );
                let end = field.offset + field.kind.width();
                // For an AtLeast type data_len is the floor, so a field past
                // it could be absent on a legal account.
                assert!(
                    end <= spec.data_len,
                    "{}.{} ends at {end}, past {} bytes",
                    layout.account_type,
                    field.name,
                    spec.data_len
                );
            }
        }
    }

    #[test]
    fn fields_within_a_layout_do_not_overlap() {
        for layout in LAYOUTS {
            let mut spans: Vec<(usize, usize)> = layout
                .fields
                .iter()
                .map(|f| (f.offset, f.offset + f.kind.width()))
                .collect();
            spans.sort_unstable();
            for pair in spans.windows(2) {
                assert!(
                    pair[0].1 <= pair[1].0,
                    "{} fields overlap",
                    layout.account_type
                );
            }
        }
    }

    #[test]
    fn decodes_each_kind_little_endian() {
        let mut data = vec![0u8; 300];
        data[41..43].copy_from_slice(&64u16.to_le_bytes());
        data[81..85].copy_from_slice(&(-22_639i32).to_le_bytes());
        data[65..81].copy_from_slice(&(u128::MAX - 1).to_le_bytes());
        data[101..133].copy_from_slice(&[1u8; 32]);
        let whirlpool = find_layout("Orca Whirlpool", "Whirlpool").unwrap();
        let fields = decode(whirlpool, &data).unwrap();
        assert_eq!(fields["tick_spacing"], 64);
        assert_eq!(fields["tick_current_index"], -22_639);
        assert_eq!(fields["sqrt_price"], (u128::MAX - 1).to_string());
        assert_eq!(fields["token_mint_a"], base58_encode(&[1u8; 32]));
        assert_eq!(fields["liquidity"], "0");
    }

    #[test]
    fn a_short_account_is_an_error_naming_the_field() {
        let reserve = find_layout("Kamino Lend", "Reserve").unwrap();
        let error = decode(reserve, &[0u8; 100]).unwrap_err().to_string();
        assert!(error.contains("liquidity_mint"), "{error}");
    }
}
