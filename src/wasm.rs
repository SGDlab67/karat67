//! wasm-bindgen exports for the in-browser gate.
//!
//! A browser cannot open sockets, so `RpcAccountFetcher` is compiled out
//! here (see the `rpc` feature). JavaScript performs `getMultipleAccounts`
//! against an endpoint the operator supplies and passes the resulting bytes
//! in; this module only decides.
//!
//! The decision itself is not reimplemented. [`check_account_json`] calls
//! [`crate::gate::check_account`], the same entry point the CLI, the Carbon
//! processor hook, and the MCP tool use, so the page cannot reach a verdict
//! the CLI would not.

use wasm_bindgen::prelude::*;

use crate::checks::reconcile::DEFAULT_MAX_SLOT_LAG;
use crate::checks::shape::check_shape;
use crate::checks::specs::{KAMINO_ACCOUNTS, LenRule, METEORA_DBC_ACCOUNTS};
use crate::fetch::MockFetcher;
use crate::gate::check_account;

/// Slot-lag tolerance the CLI uses when `--max-slot-lag` is not given.
#[wasm_bindgen]
pub fn default_max_slot_lag() -> u64 {
    DEFAULT_MAX_SLOT_LAG
}

/// Shape-check `indexed` alone and return `Vec<CheckResult>` as a JSON string.
///
/// For a payload with no corresponding chain account, such as a constructed
/// case, reconcile is not merely Skipped: there is nothing it could read. This
/// reports the one check that genuinely ran rather than padding the result.
#[wasm_bindgen]
pub fn shape_json(indexed: &[u8]) -> String {
    let results = vec![check_shape(indexed)];
    serde_json::to_string(&results).expect("CheckResult is infallibly serializable")
}

/// Gate one account and return `Vec<CheckResult>` as a JSON string.
///
/// `indexed` is the payload the indexer wrote, passed through unmodified: no
/// discriminator is rebuilt and no padding to an IDL length happens.
///
/// `context_slot` carries whether a fetch occurred at all. `None` means the
/// operator supplied no endpoint, so shape runs alone and reconcile comes back
/// Skipped, exactly as the CLI reports it without `KARAT_RPC_URL`. `Some` means
/// a fetch returned, and then `chain` distinguishes an account that exists
/// (`Some(bytes)`) from one absent on chain (`None`).
///
/// Shape failing short-circuits: the chain bytes are never consulted.
#[wasm_bindgen]
pub fn check_account_json(
    account: &str,
    indexed: &[u8],
    indexed_slot: Option<u64>,
    chain: Option<Box<[u8]>>,
    context_slot: Option<u64>,
    max_slot_lag: Option<u64>,
) -> String {
    let max_slot_lag = max_slot_lag.unwrap_or(DEFAULT_MAX_SLOT_LAG);

    // Named for what it holds here: real chain bytes the browser just fetched.
    // MockFetcher is the in-memory AccountFetcher impl; it fakes the transport,
    // never the data.
    let fetched = context_slot.map(|slot| {
        let fetcher = MockFetcher::new(slot);
        match chain {
            Some(bytes) => fetcher.with_account(account, bytes.into_vec(), slot),
            // Absent on chain: an unregistered key resolves to None.
            None => fetcher,
        }
    });

    let results = check_account(
        account,
        indexed,
        indexed_slot,
        max_slot_lag,
        fetched.as_ref().map(|f| f as _),
    );

    serde_json::to_string(&results).expect("CheckResult is infallibly serializable")
}

/// The registered account specs, as JSON, so the page can label a payload and
/// draw its declared length without keeping a second copy of the registry.
///
/// A hand-maintained JavaScript mirror of
/// [`crate::checks::specs`] drifts the moment a program is added, and a stale
/// mirror shows a visitor a length the gate never checked against. Serving the
/// compiled registry makes that drift impossible.
#[wasm_bindgen]
pub fn specs_json() -> String {
    let specs: Vec<_> = KAMINO_ACCOUNTS
        .iter()
        .chain(METEORA_DBC_ACCOUNTS)
        .map(|spec| {
            serde_json::json!({
                "program": spec.program,
                "type": spec.account_type,
                "disc": spec.discriminator,
                "len": spec.data_len,
                // "exact" means data_len is the only legal length; "atLeast"
                // means it is a floor, so the page must not draw it as a target.
                "lenRule": match spec.len_rule {
                    LenRule::Exact => "exact",
                    LenRule::AtLeast => "atLeast",
                },
            })
        })
        .collect();
    serde_json::to_string(&specs).expect("serde_json::Value is infallibly serializable")
}
