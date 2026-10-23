//! Inspect: what an account is, what it holds, and whether to trust that.
//!
//! The question agents and indexers ask most is not "is my data valid?" but
//! "what is in this account?". `inspect` answers it with the verdict
//! attached, so a caller never gets fields without knowing whether the
//! layout behind them is confirmed for the program as deployed now:
//!
//! 1. The owner picks the program, so lookup is keyed on
//!    `(program, discriminator)`; a struct name reused across programs can
//!    never be read with another program's layout.
//! 2. The discriminator picks the registered type, and its length rule at the
//!    read slot must hold.
//! 3. The program's current deploy slot is compared with the deploy the
//!    registry was verified against. A later deploy means the code changed
//!    after karat last looked: fields are still decoded, but the verdict says
//!    `stale_registry`, never `ok`.
//!
//! With indexed bytes supplied, the same call reconciles them against the
//! chain, so one call answers both "what is this?" and "is my copy right?".

use serde_json::{Map, Value, json};

use crate::checks::specs::{AccountSpec, LenRule, ProgramSpec, find_owned, find_program};
use crate::fetch::RawAccount;
use crate::layout::{decode, find_layout};
use crate::report::{CheckResult, Status};

/// The one word a caller branches on. Only [`Verdict::Ok`] means "decoded
/// with a layout confirmed for the deploy that wrote it".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Registered type, legal length, layout verified against the current
    /// deploy.
    Ok,
    /// Registered type and legal length, but the program was redeployed
    /// after the registry was verified. Fields are decoded and may be wrong.
    StaleRegistry,
    /// Registered type at a length its rule forbids: truncated, over-long,
    /// or a layout era the registry does not know.
    LayoutMismatch,
    /// Owned by a registered program, under a discriminator the registry
    /// does not have: a new account kind, or one an upgrade introduced.
    UnknownLayout,
    /// Owned by a program the registry does not cover. No claim either way.
    UnregisteredProgram,
    /// No account at this address.
    Missing,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Ok => "ok",
            Verdict::StaleRegistry => "stale_registry",
            Verdict::LayoutMismatch => "layout_mismatch",
            Verdict::UnknownLayout => "unknown_layout",
            Verdict::UnregisteredProgram => "unregistered_program",
            Verdict::Missing => "missing",
        }
    }
}

/// Everything `inspect` learned about one account.
#[derive(Debug, Clone)]
pub struct Inspection {
    pub account: String,
    /// Slot the account was read at.
    pub slot: u64,
    pub verdict: Verdict,
    /// One sentence on why, for a person or a model to read.
    pub reason: String,
    pub owner: Option<String>,
    pub program: Option<&'static ProgramSpec>,
    pub spec: Option<&'static AccountSpec>,
    pub data_len: Option<usize>,
    /// Current deploy slot; `None` for an immutable program or when not read.
    pub program_deploy_slot: Option<u64>,
    /// Decoded fields, when the type has a layout and the length is legal.
    pub fields: Option<Map<String, Value>>,
    /// Reconcile results for indexed bytes, when the caller supplied them.
    pub indexed: Option<Vec<CheckResult>>,
}

impl Inspection {
    /// True only when the verdict is `ok` and every indexed check passed.
    pub fn ok(&self) -> bool {
        self.verdict == Verdict::Ok
            && self
                .indexed
                .as_ref()
                .is_none_or(|results| results.iter().all(|r| r.status == Status::Pass))
    }

    pub fn to_json(&self) -> Value {
        let mut out = json!({
            "ok": self.ok(),
            "verdict": self.verdict.as_str(),
            "reason": self.reason,
            "account": self.account,
            "slot": self.slot,
            "owner": self.owner,
            "program": self.program.map(|p| p.name),
            "account_type": self.spec.map(|s| s.account_type),
            "data_len": self.data_len,
            "program_deploy_slot": self.program_deploy_slot,
            "registry_verified_deploy_slot": self.program.map(|p| p.verified_deploy_slot),
            "fields": self.fields,
        });
        if let Some(spec) = self.spec {
            let (len, rule) = spec.len_at(self.slot);
            out["expected_len"] = json!({
                "len": len,
                "rule": match rule { LenRule::Exact => "exact", LenRule::AtLeast => "at_least" },
            });
        }
        if let Some(indexed) = &self.indexed {
            out["indexed"] = serde_json::to_value(indexed).unwrap_or(Value::Null);
        }
        out
    }
}

/// Inspect an account already read from chain.
///
/// `raw` is the account at `slot` (`None` when it does not exist).
/// `deploy_slot_of` returns a program's current deploy slot, `None` for an
/// immutable program; it is called once, and only for a registered owner, so
/// an unregistered account costs no upgrade read.
pub fn inspect_raw(
    account: &str,
    slot: u64,
    raw: Option<&RawAccount>,
    deploy_slot_of: impl FnOnce(&str) -> anyhow::Result<Option<u64>>,
) -> anyhow::Result<Inspection> {
    let mut out = Inspection {
        account: account.to_string(),
        slot,
        verdict: Verdict::Missing,
        reason: format!("no account at {account} at slot {slot}"),
        owner: None,
        program: None,
        spec: None,
        data_len: None,
        program_deploy_slot: None,
        fields: None,
        indexed: None,
    };
    let Some(raw) = raw else {
        return Ok(out);
    };
    out.owner = Some(raw.owner.clone());
    out.data_len = Some(raw.data.len());

    let Some(program) = find_program(&raw.owner) else {
        out.verdict = Verdict::UnregisteredProgram;
        out.reason = format!(
            "owner {} is not a program karat has layouts for; nothing is claimed",
            raw.owner
        );
        return Ok(out);
    };
    out.program = Some(program);

    let spec = raw
        .data
        .get(..8)
        .and_then(|prefix| find_owned(program, prefix.try_into().expect("8 bytes")));
    let Some(spec) = spec else {
        out.verdict = Verdict::UnknownLayout;
        out.reason = match raw.data.get(..8) {
            Some(prefix) => format!(
                "{} owns this account, but discriminator {} is not a registered {} type: a new account kind, or one an upgrade added",
                program.name,
                hex(prefix),
                program.name
            ),
            None => format!(
                "{} owns this account, but it holds {} bytes, too few for a discriminator",
                program.name,
                raw.data.len()
            ),
        };
        return Ok(out);
    };
    out.spec = Some(spec);

    let (len, rule) = spec.len_at(slot);
    let legal = match rule {
        LenRule::Exact => raw.data.len() == len,
        LenRule::AtLeast => raw.data.len() >= len,
    };
    if !legal {
        out.verdict = Verdict::LayoutMismatch;
        out.reason = format!(
            "{} {} is {} bytes; the registry allows {}{}",
            program.name,
            spec.account_type,
            raw.data.len(),
            if rule == LenRule::AtLeast {
                "at least "
            } else {
                "exactly "
            },
            len
        );
        return Ok(out);
    }

    if let Some(layout) = find_layout(program.name, spec.account_type) {
        match decode(layout, &raw.data) {
            Ok(fields) => out.fields = Some(fields),
            Err(error) => {
                out.verdict = Verdict::LayoutMismatch;
                out.reason = error.to_string();
                return Ok(out);
            }
        }
    }

    out.program_deploy_slot = deploy_slot_of(program.id)?;
    match out.program_deploy_slot {
        Some(deploy) if deploy > program.verified_deploy_slot => {
            out.verdict = Verdict::StaleRegistry;
            out.reason = format!(
                "{} was redeployed at slot {deploy}, after the registry was verified against slot {}; the layout may have changed",
                program.name, program.verified_deploy_slot
            );
        }
        _ => {
            out.verdict = Verdict::Ok;
            out.reason = format!(
                "{} {} at a legal length, layout verified against the current deploy{}",
                program.name,
                spec.account_type,
                if out.fields.is_some() {
                    ""
                } else {
                    " (fields not decoded for this type yet)"
                }
            );
        }
    }
    Ok(out)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Read `account` from chain and inspect it; with `indexed`, also reconcile
/// those bytes (and their optional write slot) against the chain.
#[cfg(feature = "rpc")]
pub fn inspect_account(
    fetcher: &crate::fetch::RpcAccountFetcher,
    account: &str,
    indexed: Option<(Vec<u8>, Option<u64>)>,
    max_slot_lag: u64,
) -> anyhow::Result<Inspection> {
    let (slot, raw) = fetcher.get_account(account)?;
    let mut out = inspect_raw(account, slot, raw.as_ref(), |program| {
        Ok(crate::upgrade::read_program_upgrade(fetcher, program)?.map(|u| u.deploy_slot))
    })?;
    if let Some((bytes, indexed_slot)) = indexed {
        let sample = vec![(account.to_string(), bytes, indexed_slot)];
        out.indexed = Some(crate::checks::reconcile::reconcile(
            &sample,
            fetcher,
            max_slot_lag,
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::specs::find_by_account_type;

    const WHIRLPOOL: &str = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc";
    const VERIFIED: u64 = 457_152_296;

    fn whirlpool_account(len: usize) -> RawAccount {
        let spec = find_by_account_type("Whirlpool").unwrap();
        let mut data = vec![0u8; len];
        data[..8].copy_from_slice(&spec.discriminator);
        data[45..47].copy_from_slice(&400u16.to_le_bytes());
        RawAccount {
            owner: WHIRLPOOL.into(),
            lamports: 1,
            data,
        }
    }

    fn never_called(_: &str) -> anyhow::Result<Option<u64>> {
        panic!("deploy slot read for an account that does not need it")
    }

    #[test]
    fn registered_type_at_the_verified_deploy_is_ok_with_fields() {
        let raw = whirlpool_account(653);
        let out = inspect_raw("pool", 1, Some(&raw), |_| Ok(Some(VERIFIED))).unwrap();
        assert_eq!(out.verdict, Verdict::Ok);
        assert!(out.ok());
        assert_eq!(out.fields.as_ref().unwrap()["fee_rate"], 400);
        let json = out.to_json();
        assert_eq!(json["account_type"], "Whirlpool");
        assert_eq!(json["expected_len"]["len"], 653);
    }

    #[test]
    fn a_later_deploy_decodes_but_never_says_ok() {
        let raw = whirlpool_account(653);
        let out = inspect_raw("pool", 1, Some(&raw), |_| Ok(Some(VERIFIED + 1))).unwrap();
        assert_eq!(out.verdict, Verdict::StaleRegistry);
        assert!(!out.ok());
        assert!(out.fields.is_some());
    }

    #[test]
    fn an_immutable_program_cannot_go_stale() {
        let raw = whirlpool_account(653);
        let out = inspect_raw("pool", 1, Some(&raw), |_| Ok(None)).unwrap();
        assert_eq!(out.verdict, Verdict::Ok);
    }

    #[test]
    fn wrong_length_is_a_layout_mismatch_without_fields() {
        let raw = whirlpool_account(600);
        let out = inspect_raw("pool", 1, Some(&raw), never_called).unwrap();
        assert_eq!(out.verdict, Verdict::LayoutMismatch);
        assert!(out.fields.is_none());
        assert!(out.reason.contains("exactly 653"), "{}", out.reason);
    }

    #[test]
    fn another_programs_discriminator_is_unknown_not_misread() {
        // A Kamino Obligation discriminator inside a Whirlpool-owned account
        // must not be read with Kamino's layout.
        let mut raw = whirlpool_account(3344);
        let obligation = find_by_account_type("Obligation").unwrap();
        raw.data[..8].copy_from_slice(&obligation.discriminator);
        let out = inspect_raw("x", 1, Some(&raw), never_called).unwrap();
        assert_eq!(out.verdict, Verdict::UnknownLayout);
        assert!(out.spec.is_none());
    }

    #[test]
    fn unregistered_owner_makes_no_claim_and_no_upgrade_read() {
        let raw = RawAccount {
            owner: "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".into(),
            lamports: 1,
            data: vec![0; 82],
        };
        let out = inspect_raw("mint", 1, Some(&raw), never_called).unwrap();
        assert_eq!(out.verdict, Verdict::UnregisteredProgram);
    }

    #[test]
    fn missing_account_is_missing() {
        let out = inspect_raw("gone", 7, None, never_called).unwrap();
        assert_eq!(out.verdict, Verdict::Missing);
        assert_eq!(out.to_json()["slot"], 7);
    }

    #[test]
    fn indexed_failure_turns_ok_false() {
        let raw = whirlpool_account(653);
        let mut out = inspect_raw("pool", 1, Some(&raw), |_| Ok(Some(VERIFIED))).unwrap();
        out.indexed = Some(vec![CheckResult::fail("reconcile(pool)", "differs")]);
        assert_eq!(out.verdict, Verdict::Ok);
        assert!(!out.ok());
    }
}
