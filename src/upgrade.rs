//! Program upgrades: the slot a program's code last changed at, and shape
//! samples of the accounts it owns.
//!
//! A decoder goes stale silently when a program is upgraded: the account
//! layout changes at a slot, and every decoder built before that slot
//! describes the old layout (Carbon #541). Carbon can run versioned decoders
//! through `SlotRangeFilter`, but only if the caller already knows the upgrade
//! slot. `karat sentinel` reads that slot off the chain: the Program account
//! names its ProgramData account, and the ProgramData account's first 45
//! bytes are a bincode `UpgradeableLoaderState::ProgramData` header carrying
//! the deploy slot and the upgrade authority.
//!
//! Only the header is read (`dataSlice`), so a sentinel run never downloads a
//! program's machine code.
//!
//! The sample half exists because "the decoder is stale" is a guess until the
//! live lengths say so: `--sample N` lists the accounts matching a
//! discriminator, fetches a spread of them in full, shape-checks each, and
//! reports the length distribution it saw.

use std::collections::BTreeMap;

/// Bincode variant tag for `UpgradeableLoaderState::ProgramData`.
const PROGRAM_DATA_TAG: u32 = 3;

/// `getMultipleAccounts` keys per call: the RPC rejects more than 100.
pub const MAX_ACCOUNT_BATCH: usize = 100;

/// What a ProgramData account says about the program it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramUpgrade {
    /// Slot of the most recent deploy or upgrade. The program's code changed
    /// at this slot, so its account layouts may have changed with it.
    pub deploy_slot: u64,
    /// Base58 upgrade authority, or `None` for an immutable program.
    pub upgrade_authority: Option<String>,
}

/// Decode the front of a ProgramData account.
///
/// The layout is bincode `UpgradeableLoaderState::ProgramData { slot: u64,
/// upgrade_authority_address: Option<Pubkey> }`: a 4-byte little-endian
/// variant tag, then the slot, then a one-byte option tag and 32 bytes of
/// pubkey when an authority is present. The `Some` case is 45 bytes; a
/// program with no authority returns 13, since `dataSlice` returns what
/// exists.
pub fn parse_program_data_prefix(bytes: &[u8]) -> anyhow::Result<ProgramUpgrade> {
    if bytes.len() < 12 {
        anyhow::bail!(
            "ProgramData prefix is {} bytes, need at least 12 for the tag and slot",
            bytes.len()
        );
    }
    let tag = u32::from_le_bytes(bytes[0..4].try_into().expect("4 bytes"));
    if tag != PROGRAM_DATA_TAG {
        anyhow::bail!("not a ProgramData account: variant tag {tag}, expected {PROGRAM_DATA_TAG}");
    }
    let deploy_slot = u64::from_le_bytes(bytes[4..12].try_into().expect("8 bytes"));
    let upgrade_authority = match bytes.get(12) {
        None | Some(0) => None,
        Some(1) => {
            let key = bytes.get(13..45).ok_or_else(|| {
                anyhow::anyhow!(
                    "ProgramData prefix is {} bytes, need 45 for the upgrade authority",
                    bytes.len()
                )
            })?;
            Some(base58_encode(key))
        }
        Some(other) => anyhow::bail!("option tag {other} is not 0 or 1"),
    };
    Ok(ProgramUpgrade {
        deploy_slot,
        upgrade_authority,
    })
}

/// Base58 (Bitcoin alphabet) encoding.
///
/// Encoding only: both callers already hold the bytes (an upgrade authority
/// read from ProgramData, and a discriminator used as a `getProgramAccounts`
/// memcmp filter, which the RPC takes base58-encoded). A full base58 crate is
/// not worth a dependency for that, and `getAccountInfo` `jsonParsed` covers
/// the decode side where a base58 address has to be read.
pub fn base58_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

    // Long division of the big-endian number by 58, keeping remainders as
    // little-endian base58 digits.
    let mut digits: Vec<u8> = Vec::new();
    for &byte in bytes {
        let mut carry = byte as u32;
        for digit in digits.iter_mut() {
            carry += (*digit as u32) << 8;
            *digit = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }

    // Each leading zero byte is one leading '1'.
    let mut out = String::new();
    for &byte in bytes {
        if byte != 0 {
            break;
        }
        out.push('1');
    }
    for &digit in digits.iter().rev() {
        out.push(ALPHABET[digit as usize] as char);
    }
    out
}

/// Read a program's upgrade state from the chain.
///
/// Two calls: the Program account through `jsonParsed`, whose `programData`
/// field arrives as a base58 string, then the ProgramData account's first 45
/// bytes. `Ok(None)` means the program is not upgradeable (there is no
/// ProgramData account), which is not an error: a program that cannot be
/// upgraded cannot go stale at a slot.
#[cfg(feature = "rpc")]
pub fn read_program_upgrade(
    fetcher: &crate::fetch::RpcAccountFetcher,
    program: &str,
) -> anyhow::Result<Option<ProgramUpgrade>> {
    let info = fetcher
        .get_parsed_account_info(program)?
        .ok_or_else(|| anyhow::anyhow!("program account {program} does not exist"))?;
    let Some(program_data) = info
        .get("data")
        .and_then(|data| data.get("parsed"))
        .and_then(|parsed| parsed.get("info"))
        .and_then(|info| info.get("programData"))
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(None);
    };
    let bytes = fetcher
        .get_account_bytes(program_data, 0, 45)?
        .ok_or_else(|| anyhow::anyhow!("ProgramData account {program_data} does not exist"))?;
    Ok(Some(parse_program_data_prefix(&bytes)?))
}

/// One observed account length in a sample: how many accounts had it, and how
/// many of those passed the shape check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LengthStat {
    pub accounts: usize,
    pub passed: usize,
}

/// What a sample of a program's accounts of one type looked like.
#[derive(Debug, Clone)]
pub struct SampleReport {
    /// Accounts matching the discriminator, before sampling.
    pub matched: usize,
    /// Accounts fetched in full and shape-checked.
    pub fetched: usize,
    /// Observed length -> counts, ascending by length.
    pub lengths: BTreeMap<usize, LengthStat>,
}

impl SampleReport {
    /// How many fetched accounts failed the shape check.
    ///
    /// A registered type whose live accounts fail shape is the finding this
    /// command exists for: the registry rule no longer matches the chain.
    pub fn failed(&self) -> usize {
        self.lengths
            .values()
            .map(|stat| stat.accounts - stat.passed)
            .sum()
    }
}

/// Sample `sample` of a program's accounts that carry `discriminator`.
///
/// Lists the matching pubkeys with `getProgramAccounts` (memcmp on the
/// discriminator, `dataSlice` 0), takes up to `sample` of them, fetches those
/// in full through `getMultipleAccounts` in batches of
/// [`MAX_ACCOUNT_BATCH`], and reports each observed length with its shape
/// result.
///
/// The sample is strided across the match list rather than the first N, which
/// come back in the node's storage order and are not representative of a
/// length distribution.
#[cfg(feature = "rpc")]
pub fn sample_accounts(
    fetcher: &crate::fetch::RpcAccountFetcher,
    program: &str,
    discriminator: [u8; 8],
    sample: usize,
) -> anyhow::Result<SampleReport> {
    use crate::checks::shape::check_shape;
    use crate::fetch::AccountFetcher;
    use crate::report::Status;

    let keys = fetcher.get_program_account_keys(program, &base58_encode(&discriminator))?;
    let matched = keys.len();
    let stride = if sample == 0 || matched <= sample {
        1
    } else {
        matched / sample
    };
    let chosen: Vec<String> = keys.into_iter().step_by(stride).take(sample).collect();

    let mut lengths: BTreeMap<usize, LengthStat> = BTreeMap::new();
    let mut fetched = 0usize;
    for batch in chosen.chunks(MAX_ACCOUNT_BATCH) {
        let result = fetcher.get_multiple_accounts(batch)?;
        for account in result.accounts.iter().flatten() {
            fetched += 1;
            let shape = check_shape(&account.data);
            let stat = lengths.entry(account.data.len()).or_insert(LengthStat {
                accounts: 0,
                passed: 0,
            });
            stat.accounts += 1;
            if shape.status == Status::Pass {
                stat.passed += 1;
            }
        }
    }

    Ok(SampleReport {
        matched,
        fetched,
        lengths,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A live Kamino Lend ProgramData prefix, captured 2026-10-12 at slot
    /// 456,291,107 from `9uSbGW1y9H5Av6H5TKxQ1wnFApSq2t3oEpfF2YfjDQGA` with
    /// `dataSlice {offset: 0, length: 45}`.
    const KAMINO_PROGRAM_DATA: &str = "03000000774b411a0000000001ed8a032cf6b12c5867549ba97e1901a1b246c4cf164acdcbbfad014da703e6a7";

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
            .collect()
    }

    #[test]
    fn decodes_a_live_program_data_prefix() {
        let upgrade = parse_program_data_prefix(&hex(KAMINO_PROGRAM_DATA)).expect("decodes");
        assert_eq!(upgrade.deploy_slot, 0x1a414b77);
        assert_eq!(
            upgrade.upgrade_authority.as_deref(),
            Some("GzFgdRJXmawPhGeBsyRCDLx4jAKPsvbUqoqitzppkzkW")
        );
    }

    #[test]
    fn a_prefix_without_an_authority_has_none() {
        // Same header, option tag 0: 13 bytes, the immutable-program case.
        let mut bytes = hex(KAMINO_PROGRAM_DATA);
        bytes.truncate(13);
        bytes[12] = 0;
        let upgrade = parse_program_data_prefix(&bytes).expect("decodes");
        assert_eq!(upgrade.deploy_slot, 0x1a414b77);
        assert_eq!(upgrade.upgrade_authority, None);
    }

    #[test]
    fn rejects_something_that_is_not_program_data() {
        // A Buffer account (tag 1) is not a ProgramData account.
        let mut bytes = hex(KAMINO_PROGRAM_DATA);
        bytes[0] = 1;
        assert!(parse_program_data_prefix(&bytes).is_err());
        assert!(parse_program_data_prefix(&[3, 0, 0, 0]).is_err());
    }

    #[test]
    fn base58_matches_the_chain() {
        // The ProgramData address, from its own bytes: the RPC reported this
        // exact string for the account when it was read as `jsonParsed`.
        assert_eq!(
            base58_encode(&hex(
                "844ca2136eed01488526671b80dd2102c5180aa332a09cbe67b9ce813e2d002b"
            )),
            "9uSbGW1y9H5Av6H5TKxQ1wnFApSq2t3oEpfF2YfjDQGA"
        );
        // The system program: 32 leading zero bytes, 32 leading '1's.
        assert_eq!(base58_encode(&[0u8; 32]), "1".repeat(32));
        // pump_swap's Pool discriminator, as a memcmp filter needs it.
        assert_eq!(
            base58_encode(&[241, 154, 109, 4, 17, 177, 109, 188]),
            "hQrXeCntzbV"
        );
        assert_eq!(base58_encode(&[]), "");
    }
}
