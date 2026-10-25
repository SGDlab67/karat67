//! Account and slot-coverage fetching, modeled on Solana JSON-RPC.
//!
//! Reconciliation reads on-chain state through the [`AccountFetcher`] seam so
//! the diffing logic stays offline-testable: tests drive it with
//! [`MockFetcher`], while the CLI drives it with [`RpcAccountFetcher`] against
//! a real endpoint. Completeness uses the same structs via
//! [`SlotCoverageFetcher`] (`getBlocks`).

use std::cell::Cell;
use std::collections::HashMap;

/// One account's on-chain bytes plus the slot they were observed at.
///
/// `getMultipleAccounts` reports a single context slot for the whole batch
/// rather than a per-account slot, so [`RpcAccountFetcher`] copies the context
/// slot into every account. The field is kept per-account so a future fetcher
/// that can attribute a precise write slot need not change the shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedAccount {
    pub data: Vec<u8>,
    pub slot: u64,
}

/// Outcome of a `getMultipleAccounts`-style batch fetch.
///
/// `accounts` is positional: entry `i` corresponds to the `i`th requested key
/// and is `None` when the account does not exist on chain. `context_slot` is
/// the RPC context slot the batch was read at; reconciliation uses it for
/// slot-lag tolerance against each row's optional indexed write slot.
#[derive(Debug, Clone)]
pub struct FetchResult {
    pub context_slot: u64,
    pub accounts: Vec<Option<FetchedAccount>>,
}

/// Fetches account data by pubkey, batched like `getMultipleAccounts`.
///
/// Keys are base-58 pubkey strings; this seam deliberately avoids a Solana
/// SDK dependency. An `Err` means the fetch itself was unreachable (network,
/// RPC error); a present-but-`None` entry means the account does not exist.
pub trait AccountFetcher {
    /// Fetch each key's account, preserving order. Returns the batch context
    /// slot alongside one `Option<FetchedAccount>` per requested key.
    fn get_multiple_accounts(&self, keys: &[String]) -> anyhow::Result<FetchResult>;

    /// Slot of the most recent transaction that referenced `key`, read from a
    /// node that has seen at least `min_context_slot`.
    ///
    /// Reconciliation uses this as its reference point on a byte mismatch: if
    /// nothing touched the account after the indexed write slot, the indexed
    /// bytes must equal the chain's, and a mismatch is corruption, not lag.
    /// Read-only references and failed transactions count, so the answer is
    /// an upper bound on the last write: it can only turn a verdict into a
    /// skip, never into a false failure.
    ///
    /// `Ok(None)` means unknown (no history on this node, or a fetcher with
    /// no history at all); the caller then falls back to the slot-lag rule.
    fn last_touch_slot(&self, key: &str, min_context_slot: u64) -> anyhow::Result<Option<u64>> {
        let _ = (key, min_context_slot);
        Ok(None)
    }
}

/// Fetches confirmed block slots in a range, like Solana's `getBlocks`.
///
/// Leaders skip slots: those heights never produce a block and are omitted
/// from the result. Completeness treats omitted skip-slots as non-gaps — only
/// slots that `getBlocks` returns are expected to appear in the index.
pub trait SlotCoverageFetcher {
    /// Confirmed block slots in the inclusive range `[start, end]`.
    ///
    /// An `Err` means the fetch itself was unreachable (network, RPC error).
    fn get_blocks(&self, start: u64, end: u64) -> anyhow::Result<Vec<u64>>;

    /// The highest finalized slot, when the fetcher knows it.
    ///
    /// `getBlocks` answers at `finalized`, so above this slot "not in
    /// `getBlocks`" means "not finalized yet", not "never produced". The
    /// coverage checks cap their window here. `Ok(None)` means unknown, and
    /// the window is used as given.
    fn finalized_slot(&self) -> anyhow::Result<Option<u64>> {
        Ok(None)
    }
}

/// Maximum keys one `getMultipleAccounts` call may carry.
///
/// The RPC rejects a longer list
/// (https://solana.com/docs/rpc/http/getmultipleaccounts), so
/// `RpcAccountFetcher::get_multiple_accounts` batches inside the fetcher, the
/// same way `get_blocks` windows at [`MAX_GETBLOCKS_RANGE`].
pub const MAX_GET_MULTIPLE_ACCOUNTS_KEYS: usize = 100;

/// Fetch `keys` in batches of at most `max`, preserving key order.
///
/// Each batch is read at its own context slot. The merged result reports the
/// oldest of them, so a reconcile verdict that holds at the merged slot holds
/// for every account in the batch. `fetch` is the single-batch call: the RPC
/// fetcher passes its raw request, tests pass a mock.
pub fn get_multiple_accounts_in_batches(
    mut fetch: impl FnMut(&[String]) -> anyhow::Result<FetchResult>,
    keys: &[String],
    max: usize,
) -> anyhow::Result<FetchResult> {
    let mut context_slot: Option<u64> = None;
    let mut accounts = Vec::with_capacity(keys.len());
    for batch in keys.chunks(max.max(1)) {
        let result = fetch(batch)?;
        anyhow::ensure!(
            result.accounts.len() == batch.len(),
            "getMultipleAccounts returned {} entries for {} keys",
            result.accounts.len(),
            batch.len()
        );
        context_slot =
            Some(context_slot.map_or(result.context_slot, |slot| slot.min(result.context_slot)));
        accounts.extend(result.accounts);
    }
    Ok(FetchResult {
        context_slot: context_slot.unwrap_or_default(),
        accounts,
    })
}

/// Maximum slot range one `getBlocks` call may cover.
///
/// The RPC rejects a wider window ("Max range allowed is 500,000 slots",
/// https://solana.com/docs/rpc/http/getblocks), so a caller with a wider
/// window must walk it in pieces. `RpcAccountFetcher::get_blocks` does that
/// inside the fetcher, which is where every caller inherits it.
pub const MAX_GETBLOCKS_RANGE: u64 = 500_000;

/// Response size ceiling for one RPC call.
///
/// A 500,000-slot `getBlocks` window is several megabytes of digits, and a
/// program-wide `getProgramAccounts` key list is tens of megabytes (measured
/// 2026-10-12: pump_swap's `Pool` match list is about 50 MB). ureq's default
/// body cap of 10 MB refuses both, so the ceiling is raised once, for every
/// call, in one place.
pub const MAX_RPC_BODY_BYTES: u64 = 256 * 1024 * 1024;

/// Inclusive windows of at most `max` slots covering `[start, end]`.
///
/// Ascending and non-overlapping, and they tile the range exactly: each
/// window starts one slot after the previous one ends, so no produced slot
/// can fall between two windows. Empty when `end < start` or `max == 0`.
pub fn slot_windows(start: u64, end: u64, max: u64) -> Vec<(u64, u64)> {
    let mut windows = Vec::new();
    if end < start || max == 0 {
        return windows;
    }
    let mut window_start = start;
    loop {
        let window_end = window_start.saturating_add(max - 1).min(end);
        windows.push((window_start, window_end));
        if window_end == end {
            return windows;
        }
        // window_end < end <= u64::MAX, so the increment cannot overflow.
        window_start = window_end + 1;
    }
}

/// Fetch produced block slots over `[start, end]` one window at a time,
/// merging the pieces into a single sorted, deduplicated list.
///
/// `fetch` is the single-window call: `RpcAccountFetcher` passes its raw
/// request, tests pass a mock. The walk lives here rather than in an RPC
/// method so the chunking is offline-testable and so the merge step is the
/// same code path for every caller.
pub fn get_blocks_in_windows(
    mut fetch: impl FnMut(u64, u64) -> anyhow::Result<Vec<u64>>,
    start: u64,
    end: u64,
    max: u64,
) -> anyhow::Result<Vec<u64>> {
    let mut slots: Vec<u64> = Vec::new();
    for (window_start, window_end) in slot_windows(start, end, max) {
        slots.extend(fetch(window_start, window_end)?);
    }
    slots.sort_unstable();
    slots.dedup();
    Ok(slots)
}

/// In-memory [`AccountFetcher`] / [`SlotCoverageFetcher`] with canned data for
/// hermetic tests.
///
/// A normal (non-`cfg(test)`) struct so it can back examples and integration
/// tests too. Unknown keys resolve to `None` (missing on chain); construct
/// with [`MockFetcher::unreachable`] to model a fetch that errors out.
pub struct MockFetcher {
    context_slot: u64,
    accounts: HashMap<String, FetchedAccount>,
    /// Confirmed block slots returned by [`SlotCoverageFetcher::get_blocks`].
    /// Slots outside the requested window are filtered out.
    blocks: Vec<u64>,
    /// `get_blocks` invocations, so a test can prove a short-circuit made no
    /// call at all rather than guessing from the result.
    block_fetches: Cell<usize>,
    /// Last-touch slots returned by [`AccountFetcher::last_touch_slot`].
    touches: HashMap<String, u64>,
    /// Returned by [`SlotCoverageFetcher::finalized_slot`].
    finalized: Option<u64>,
    unreachable: bool,
}

impl MockFetcher {
    /// A reachable fetcher reporting `context_slot`, with no accounts yet.
    pub fn new(context_slot: u64) -> Self {
        Self {
            context_slot,
            accounts: HashMap::new(),
            blocks: Vec::new(),
            block_fetches: Cell::new(0),
            touches: HashMap::new(),
            finalized: None,
            unreachable: false,
        }
    }

    /// A fetcher whose every fetch fails, modeling an unreachable endpoint.
    pub fn unreachable() -> Self {
        Self {
            context_slot: 0,
            accounts: HashMap::new(),
            blocks: Vec::new(),
            block_fetches: Cell::new(0),
            touches: HashMap::new(),
            finalized: None,
            unreachable: true,
        }
    }

    /// How many times [`SlotCoverageFetcher::get_blocks`] was called.
    pub fn block_fetches(&self) -> usize {
        self.block_fetches.get()
    }

    /// Register `key` as present on chain with `data`, observed at `slot`.
    pub fn with_account(mut self, key: impl Into<String>, data: Vec<u8>, slot: u64) -> Self {
        self.accounts
            .insert(key.into(), FetchedAccount { data, slot });
        self
    }

    /// Register confirmed block slots for [`SlotCoverageFetcher::get_blocks`].
    ///
    /// Omit leader skip-slots here the same way real `getBlocks` omits them.
    pub fn with_blocks(mut self, blocks: impl IntoIterator<Item = u64>) -> Self {
        self.blocks.extend(blocks);
        self
    }

    /// Register the slot of the last transaction that referenced `key`.
    pub fn with_last_touch(mut self, key: impl Into<String>, slot: u64) -> Self {
        self.touches.insert(key.into(), slot);
        self
    }

    /// Report `slot` as the highest finalized slot.
    pub fn with_finalized(mut self, slot: u64) -> Self {
        self.finalized = Some(slot);
        self
    }
}

impl AccountFetcher for MockFetcher {
    fn get_multiple_accounts(&self, keys: &[String]) -> anyhow::Result<FetchResult> {
        if self.unreachable {
            anyhow::bail!("mock fetcher is unreachable");
        }
        let accounts = keys
            .iter()
            .map(|key| self.accounts.get(key).cloned())
            .collect();
        Ok(FetchResult {
            context_slot: self.context_slot,
            accounts,
        })
    }

    fn last_touch_slot(&self, key: &str, _min_context_slot: u64) -> anyhow::Result<Option<u64>> {
        if self.unreachable {
            anyhow::bail!("mock fetcher is unreachable");
        }
        Ok(self.touches.get(key).copied())
    }
}

impl SlotCoverageFetcher for MockFetcher {
    fn get_blocks(&self, start: u64, end: u64) -> anyhow::Result<Vec<u64>> {
        self.block_fetches.set(self.block_fetches.get() + 1);
        if self.unreachable {
            anyhow::bail!("mock fetcher is unreachable");
        }
        let mut slots: Vec<u64> = self
            .blocks
            .iter()
            .copied()
            .filter(|&slot| slot >= start && slot <= end)
            .collect();
        slots.sort_unstable();
        slots.dedup();
        Ok(slots)
    }

    fn finalized_slot(&self) -> anyhow::Result<Option<u64>> {
        if self.unreachable {
            anyhow::bail!("mock fetcher is unreachable");
        }
        Ok(self.finalized)
    }
}

/// [`AccountFetcher`] backed by a real Solana JSON-RPC endpoint.
///
/// Uses a blocking, rustls-based HTTP client (`ureq`) so the build stays
/// toolchain-only: no OpenSSL/`libssl-dev` system dependency in CI or the
/// Cloud Agent environment. The endpoint is configurable, never hardcoded.
#[cfg(feature = "rpc")]
pub struct RpcAccountFetcher {
    endpoint: String,
    agent: ureq::Agent,
    /// Keys per `getMultipleAccounts` call. Starts at the protocol cap and
    /// drops to whatever lower cap the endpoint reports (PublicNode's free
    /// tier allows 10, measured 2026-10-23).
    batch_limit: std::cell::Cell<usize>,
}

/// An account as `getAccountInfo` returns it: who owns it and its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawAccount {
    /// Base58 owning program.
    pub owner: String,
    pub lamports: u64,
    pub data: Vec<u8>,
}

/// The key cap an RPC names when it rejects a `getMultipleAccounts` call,
/// read from messages like "A maximum of 10 accounts is allowed".
pub fn reported_key_limit(message: &str) -> Option<usize> {
    let rest = &message[message.find("maximum of ")? + "maximum of ".len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|&limit| limit > 0)
}

#[cfg(feature = "rpc")]
impl RpcAccountFetcher {
    /// Build a fetcher targeting `endpoint` (a Solana JSON-RPC URL).
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            // Non-2xx replies are read, not thrown: providers put the reason
            // (a key cap, a rate limit) in a JSON body on a 403 or 429.
            agent: ureq::Agent::config_builder()
                .http_status_as_error(false)
                .build()
                .new_agent(),
            batch_limit: std::cell::Cell::new(MAX_GET_MULTIPLE_ACCOUNTS_KEYS),
        }
    }

    /// One account with its owner, read at `confirmed`, and the slot it was
    /// read at. `None` means the account does not exist.
    pub fn get_account(&self, key: &str) -> anyhow::Result<(u64, Option<RawAccount>)> {
        use base64::Engine as _;

        let result = self.rpc_call(
            "getAccountInfo",
            serde_json::json!([key, { "encoding": "base64", "commitment": "confirmed" }]),
        )?;
        let context_slot = result
            .get("context")
            .and_then(|context| context.get("slot"))
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("RPC response missing `result.context.slot`"))?;
        let Some(value) = result.get("value").filter(|value| !value.is_null()) else {
            return Ok((context_slot, None));
        };
        let owner = value
            .get("owner")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("account entry missing `owner`"))?
            .to_string();
        let lamports = value
            .get("lamports")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("account entry missing `lamports`"))?;
        let encoded = value
            .get("data")
            .and_then(|data| data.get(0))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("account `data` is not [base64, \"base64\"]"))?;
        let data = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|e| anyhow::anyhow!("decoding base64 account data failed: {e}"))?;
        Ok((
            context_slot,
            Some(RawAccount {
                owner,
                lamports,
                data,
            }),
        ))
    }

    /// One JSON-RPC call, returning the `result` value.
    ///
    /// Every request in this file goes through here, so the two failure
    /// shapes (an RPC `error` object, a missing `result`) are handled once.
    fn rpc_call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        let mut response = self
            .agent
            .post(&self.endpoint)
            .send_json(&request)
            .map_err(|e| anyhow::anyhow!("{method} request failed: {e}"))?;
        let status = response.status();
        let body: serde_json::Value = response
            .body_mut()
            .with_config()
            .limit(MAX_RPC_BODY_BYTES)
            .read_json()
            .map_err(|e| {
                if status.is_success() {
                    anyhow::anyhow!("reading {method} response failed: {e}")
                } else {
                    anyhow::anyhow!("{method} returned HTTP {status}")
                }
            })?;

        if let Some(error) = body.get("error") {
            anyhow::bail!("RPC returned an error: {error}");
        }
        body.get("result")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("RPC response missing `result`"))
    }

    /// `getAccountInfo` with `jsonParsed`, for accounts whose parsed form is
    /// the point. An upgradeable program's `programData` address comes back
    /// as a base58 string here, so reading it needs no base58 decoder.
    ///
    /// `Ok(None)` means the account does not exist.
    pub fn get_parsed_account_info(&self, key: &str) -> anyhow::Result<Option<serde_json::Value>> {
        let result = self.rpc_call(
            "getAccountInfo",
            serde_json::json!([key, { "encoding": "jsonParsed" }]),
        )?;
        Ok(result.get("value").filter(|v| !v.is_null()).cloned())
    }

    /// `getAccountInfo` for `[offset, offset + length)` of an account's data,
    /// base64 decoded.
    ///
    /// The slice is what keeps a ProgramData read from downloading the
    /// program's code: only the bincode header is requested.
    ///
    /// `Ok(None)` means the account does not exist.
    pub fn get_account_bytes(
        &self,
        key: &str,
        offset: u64,
        length: u64,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        use base64::Engine as _;

        let result = self.rpc_call(
            "getAccountInfo",
            serde_json::json!([
                key,
                {
                    "encoding": "base64",
                    "dataSlice": { "offset": offset, "length": length },
                }
            ]),
        )?;
        let Some(value) = result.get("value").filter(|v| !v.is_null()) else {
            return Ok(None);
        };
        let encoded = value
            .get("data")
            .and_then(|data| data.get(0))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("account `data` is not [base64, \"base64\"]"))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|e| anyhow::anyhow!("decoding base64 account data failed: {e}"))?;
        Ok(Some(bytes))
    }

    /// Keys of every account `program` owns whose first 8 bytes equal the
    /// base58-encoded `discriminator`, with no account data returned
    /// (`dataSlice` of length 0).
    ///
    /// This is the `getProgramAccounts` shape a survey wants: it lists what
    /// matches without paying for the payloads, which the caller then samples
    /// through [`AccountFetcher::get_multiple_accounts`].
    pub fn get_program_account_keys(
        &self,
        program: &str,
        discriminator_base58: &str,
    ) -> anyhow::Result<Vec<String>> {
        let result = self.rpc_call(
            "getProgramAccounts",
            serde_json::json!([
                program,
                {
                    "encoding": "base64",
                    "filters": [
                        { "memcmp": { "offset": 0, "bytes": discriminator_base58 } }
                    ],
                    "dataSlice": { "offset": 0, "length": 0 },
                }
            ]),
        )?;
        let entries = result
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("getProgramAccounts result is not an array"))?;
        entries
            .iter()
            .map(|entry| {
                entry
                    .get("pubkey")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| anyhow::anyhow!("getProgramAccounts entry missing `pubkey`"))
            })
            .collect()
    }
}

#[cfg(feature = "rpc")]
impl AccountFetcher for RpcAccountFetcher {
    fn get_multiple_accounts(&self, keys: &[String]) -> anyhow::Result<FetchResult> {
        loop {
            let limit = self.batch_limit.get();
            match get_multiple_accounts_in_batches(
                |batch| get_multiple_accounts_once(self, batch),
                keys,
                limit,
            ) {
                Ok(result) => return Ok(result),
                // The endpoint named a lower cap: remember it and redo the
                // fetch in batches it accepts. Strictly lower, so this ends.
                Err(error) => match reported_key_limit(&error.to_string()) {
                    Some(lower) if lower < limit => self.batch_limit.set(lower),
                    _ => return Err(error),
                },
            }
        }
    }

    fn last_touch_slot(&self, key: &str, min_context_slot: u64) -> anyhow::Result<Option<u64>> {
        // minContextSlot keeps a load-balanced endpoint from answering off a
        // node older than the account read: a stale node would report an old
        // last touch and turn lag into a false corruption verdict.
        let result = self.rpc_call(
            "getSignaturesForAddress",
            serde_json::json!([key, {
                "limit": 1,
                "commitment": "confirmed",
                "minContextSlot": min_context_slot,
            }]),
        )?;
        let entries = result
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("getSignaturesForAddress result is not an array"))?;
        match entries.first() {
            None => Ok(None),
            Some(entry) => entry
                .get("slot")
                .and_then(serde_json::Value::as_u64)
                .map(Some)
                .ok_or_else(|| anyhow::anyhow!("signature entry missing `slot`: {entry}")),
        }
    }
}

/// One raw `getMultipleAccounts` call for at most
/// [`MAX_GET_MULTIPLE_ACCOUNTS_KEYS`] keys.
///
/// Read at `confirmed`: the RPC default is `finalized`, about 32 slots behind
/// the tip, and an indexer streaming at `processed` or `confirmed` would sit
/// ahead of every read.
#[cfg(feature = "rpc")]
fn get_multiple_accounts_once(
    fetcher: &RpcAccountFetcher,
    keys: &[String],
) -> anyhow::Result<FetchResult> {
    let result = fetcher.rpc_call(
        "getMultipleAccounts",
        serde_json::json!([keys, { "encoding": "base64", "commitment": "confirmed" }]),
    )?;
    let context_slot = result
        .get("context")
        .and_then(|context| context.get("slot"))
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("RPC response missing `result.context.slot`"))?;
    let values = result
        .get("value")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("RPC response missing `result.value` array"))?;

    let accounts = values
        .iter()
        .map(|value| decode_account(value, context_slot))
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(FetchResult {
        context_slot,
        accounts,
    })
}

#[cfg(feature = "rpc")]
impl SlotCoverageFetcher for RpcAccountFetcher {
    fn get_blocks(&self, start: u64, end: u64) -> anyhow::Result<Vec<u64>> {
        if end < start {
            anyhow::bail!("getBlocks end slot {end} is before start slot {start}");
        }
        // Wide windows are chunked here, so no caller has to know the RPC's
        // 500,000-slot cap. The 158.91 h outage in karat67's incident log is
        // about 1.4M slots: three windows at this size.
        get_blocks_in_windows(
            |window_start, window_end| get_blocks_once(self, window_start, window_end),
            start,
            end,
            MAX_GETBLOCKS_RANGE,
        )
    }

    fn finalized_slot(&self) -> anyhow::Result<Option<u64>> {
        let result = self.rpc_call(
            "getSlot",
            serde_json::json!([{ "commitment": "finalized" }]),
        )?;
        result
            .as_u64()
            .map(Some)
            .ok_or_else(|| anyhow::anyhow!("getSlot result is not a u64: {result}"))
    }
}

/// One raw `getBlocks` call, with no windowing.
///
/// [`RpcAccountFetcher::get_blocks`] is the entry point; it walks the
/// 500,000-slot chunking around this single-window request.
#[cfg(feature = "rpc")]
fn get_blocks_once(fetcher: &RpcAccountFetcher, start: u64, end: u64) -> anyhow::Result<Vec<u64>> {
    let result = fetcher.rpc_call("getBlocks", serde_json::json!([start, end]))?;
    let values = result
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("RPC response missing `result` array"))?;

    values
        .iter()
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("getBlocks entry is not a u64: {value}"))
        })
        .collect()
}

/// Decode one `getMultipleAccounts` value entry into a [`FetchedAccount`].
///
/// `null` means the account does not exist (`None`); otherwise the `data`
/// field is `[base64_string, "base64"]` per the requested encoding.
#[cfg(feature = "rpc")]
fn decode_account(
    value: &serde_json::Value,
    context_slot: u64,
) -> anyhow::Result<Option<FetchedAccount>> {
    use base64::Engine as _;

    if value.is_null() {
        return Ok(None);
    }

    let data_field = value
        .get("data")
        .ok_or_else(|| anyhow::anyhow!("account entry missing `data`"))?;
    let encoded = data_field
        .get(0)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("account `data` is not [base64, \"base64\"]"))?;
    let data = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| anyhow::anyhow!("decoding base64 account data failed: {e}"))?;

    Ok(Some(FetchedAccount {
        data,
        slot: context_slot,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn reads_the_key_cap_an_endpoint_reports() {
        let publicnode = r#"RPC returned an error: {"code":-32602,"message":"A maximum of 10 accounts is allowed. No limit on a dedicated node"}"#;
        assert_eq!(reported_key_limit(publicnode), Some(10));
        assert_eq!(
            reported_key_limit("Too many inputs provided; max 100"),
            None
        );
        assert_eq!(reported_key_limit("A maximum of 0 accounts"), None);
    }

    #[test]
    fn batches_keys_at_the_rpc_cap_and_keeps_order() {
        let keys: Vec<String> = (0..250).map(|i| format!("k{i}")).collect();
        let mut sizes = Vec::new();
        // Each batch answers at a different context slot; the merged result
        // must report the oldest.
        let result = get_multiple_accounts_in_batches(
            |batch| {
                sizes.push(batch.len());
                Ok(FetchResult {
                    context_slot: 1000 - sizes.len() as u64,
                    accounts: batch
                        .iter()
                        .map(|key| {
                            Some(FetchedAccount {
                                data: key.as_bytes().to_vec(),
                                slot: 0,
                            })
                        })
                        .collect(),
                })
            },
            &keys,
            MAX_GET_MULTIPLE_ACCOUNTS_KEYS,
        )
        .expect("reachable");
        assert_eq!(sizes, vec![100, 100, 50]);
        assert_eq!(result.context_slot, 997);
        assert_eq!(result.accounts.len(), 250);
        assert_eq!(result.accounts[249].as_ref().unwrap().data, b"k249");
    }

    #[test]
    fn a_failing_batch_fails_the_whole_fetch() {
        let keys: Vec<String> = (0..150).map(|i| format!("k{i}")).collect();
        let mut calls = 0;
        let result = get_multiple_accounts_in_batches(
            |batch| {
                calls += 1;
                if calls == 2 {
                    anyhow::bail!("rpc down");
                }
                Ok(FetchResult {
                    context_slot: 1,
                    accounts: vec![None; batch.len()],
                })
            },
            &keys,
            MAX_GET_MULTIPLE_ACCOUNTS_KEYS,
        );
        assert!(result.is_err());
    }

    #[test]
    fn a_short_batch_reply_is_an_error_not_a_misaligned_result() {
        let keys: Vec<String> = (0..3).map(|i| format!("k{i}")).collect();
        let result = get_multiple_accounts_in_batches(
            |_| {
                Ok(FetchResult {
                    context_slot: 1,
                    accounts: vec![None],
                })
            },
            &keys,
            MAX_GET_MULTIPLE_ACCOUNTS_KEYS,
        );
        assert!(result.is_err());
    }

    #[test]
    fn mock_returns_registered_account() {
        let fetcher = MockFetcher::new(100).with_account("key1", vec![1, 2, 3], 99);
        let result = fetcher
            .get_multiple_accounts(&["key1".to_string()])
            .expect("reachable");
        assert_eq!(result.context_slot, 100);
        assert_eq!(
            result.accounts[0],
            Some(FetchedAccount {
                data: vec![1, 2, 3],
                slot: 99
            })
        );
    }

    #[test]
    fn mock_returns_none_for_unknown_key() {
        let fetcher = MockFetcher::new(100);
        let result = fetcher
            .get_multiple_accounts(&["missing".to_string()])
            .expect("reachable");
        assert_eq!(result.accounts[0], None);
    }

    #[test]
    fn mock_unreachable_errors() {
        let fetcher = MockFetcher::unreachable();
        assert!(
            fetcher
                .get_multiple_accounts(&["key1".to_string()])
                .is_err()
        );
    }

    #[test]
    fn mock_preserves_key_order() {
        let fetcher = MockFetcher::new(7)
            .with_account("a", vec![0], 1)
            .with_account("c", vec![2], 3);
        let keys = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let result = fetcher.get_multiple_accounts(&keys).expect("reachable");
        assert!(result.accounts[0].is_some());
        assert!(result.accounts[1].is_none());
        assert!(result.accounts[2].is_some());
    }

    #[test]
    fn mock_get_blocks_filters_to_window() {
        let fetcher = MockFetcher::new(0).with_blocks([98, 100, 101, 105, 110]);
        let blocks = fetcher.get_blocks(100, 105).expect("reachable");
        assert_eq!(blocks, vec![100, 101, 105]);
    }

    #[test]
    fn mock_get_blocks_unreachable_errors() {
        let fetcher = MockFetcher::unreachable();
        assert!(fetcher.get_blocks(1, 10).is_err());
    }

    #[test]
    fn slot_windows_keep_a_range_that_fits_in_one_call() {
        assert_eq!(
            slot_windows(100, 104, MAX_GETBLOCKS_RANGE),
            vec![(100, 104)]
        );
    }

    #[test]
    fn slot_windows_split_an_outage_sized_range_into_three() {
        // 1.4M slots, the size of the 158.91 h incident: three windows at
        // the RPC's 500,000-slot cap, tiling exactly.
        let windows = slot_windows(0, 1_400_000, MAX_GETBLOCKS_RANGE);
        assert_eq!(
            windows,
            vec![(0, 499_999), (500_000, 999_999), (1_000_000, 1_400_000)]
        );
    }

    #[test]
    fn slot_windows_of_an_inverted_range_are_empty() {
        assert!(slot_windows(10, 9, MAX_GETBLOCKS_RANGE).is_empty());
    }

    #[test]
    fn chunked_get_blocks_merges_sorted_and_deduplicated() {
        let mock = MockFetcher::new(0).with_blocks([5, 1, 3, 3, 7]);
        let slots =
            get_blocks_in_windows(|s, e| mock.get_blocks(s, e), 0, 9, 100).expect("reachable");
        assert_eq!(slots, vec![1, 3, 5, 7]);
    }

    proptest! {
        /// Chunking a wide window must return exactly what one call would.
        #[test]
        fn chunked_equals_unchunked(
            raw in prop::collection::vec(0u64..3_000_000, 0..200),
            start in 0u64..1_000_000,
            span in 0u64..2_000_000,
            max in 1u64..1000,
        ) {
            let end = start + span;
            let mock = MockFetcher::new(0).with_blocks(raw);
            let unchunked = mock.get_blocks(start, end).expect("reachable");
            let chunked = get_blocks_in_windows(|s, e| mock.get_blocks(s, e), start, end, max)
                .expect("reachable");
            prop_assert_eq!(chunked, unchunked);
        }

        /// Windows cover the range exactly: ascending, contiguous, each at
        /// most `max` slots, starting at `start` and ending at `end`.
        #[test]
        fn windows_tile_the_range(
            start in 0u64..1_000_000,
            span in 0u64..2_000_000,
            max in 1u64..1000,
        ) {
            let end = start + span;
            let windows = slot_windows(start, end, max);
            prop_assert!(!windows.is_empty());
            prop_assert_eq!(windows[0].0, start);
            prop_assert_eq!(windows.last().expect("non-empty").1, end);
            for (i, (window_start, window_end)) in windows.iter().enumerate() {
                prop_assert!(window_start <= window_end);
                prop_assert!(window_end - window_start < max);
                if i > 0 {
                    prop_assert_eq!(*window_start, windows[i - 1].1 + 1);
                }
            }
        }
    }
}
