//! Account fetching, modeled on Solana's `getMultipleAccounts` RPC.
//!
//! Reconciliation reads on-chain state through the [`AccountFetcher`] seam so
//! the diffing logic stays offline-testable: tests drive it with
//! [`MockFetcher`], while the CLI drives it with [`RpcAccountFetcher`] against
//! a real endpoint.

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
/// the RPC context slot the batch was read at; reconciliation threads it
/// through so slot-lag tolerance can plug in later.
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
}

/// In-memory [`AccountFetcher`] with canned data for hermetic tests.
///
/// A normal (non-`cfg(test)`) struct so it can back examples and integration
/// tests too. Unknown keys resolve to `None` (missing on chain); construct
/// with [`MockFetcher::unreachable`] to model a fetch that errors out.
pub struct MockFetcher {
    context_slot: u64,
    accounts: HashMap<String, FetchedAccount>,
    unreachable: bool,
}

impl MockFetcher {
    /// A reachable fetcher reporting `context_slot`, with no accounts yet.
    pub fn new(context_slot: u64) -> Self {
        Self {
            context_slot,
            accounts: HashMap::new(),
            unreachable: false,
        }
    }

    /// A fetcher whose every fetch fails, modeling an unreachable endpoint.
    pub fn unreachable() -> Self {
        Self {
            context_slot: 0,
            accounts: HashMap::new(),
            unreachable: true,
        }
    }

    /// Register `key` as present on chain with `data`, observed at `slot`.
    pub fn with_account(mut self, key: impl Into<String>, data: Vec<u8>, slot: u64) -> Self {
        self.accounts
            .insert(key.into(), FetchedAccount { data, slot });
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
}

/// [`AccountFetcher`] backed by a real Solana JSON-RPC endpoint.
///
/// Uses a blocking, rustls-based HTTP client (`ureq`) so the build stays
/// toolchain-only: no OpenSSL/`libssl-dev` system dependency in CI or the
/// Cloud Agent environment. The endpoint is configurable, never hardcoded.
pub struct RpcAccountFetcher {
    endpoint: String,
    agent: ureq::Agent,
}

impl RpcAccountFetcher {
    /// Build a fetcher targeting `endpoint` (a Solana JSON-RPC URL).
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            agent: ureq::Agent::new_with_defaults(),
        }
    }
}

impl AccountFetcher for RpcAccountFetcher {
    fn get_multiple_accounts(&self, keys: &[String]) -> anyhow::Result<FetchResult> {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getMultipleAccounts",
            "params": [keys, { "encoding": "base64" }],
        });

        let mut response = self
            .agent
            .post(&self.endpoint)
            .send_json(&request)
            .map_err(|e| anyhow::anyhow!("getMultipleAccounts request failed: {e}"))?;
        let body: serde_json::Value = response
            .body_mut()
            .read_json()
            .map_err(|e| anyhow::anyhow!("reading getMultipleAccounts response failed: {e}"))?;

        if let Some(error) = body.get("error") {
            anyhow::bail!("RPC returned an error: {error}");
        }

        let result = body
            .get("result")
            .ok_or_else(|| anyhow::anyhow!("RPC response missing `result`"))?;
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
}

/// Decode one `getMultipleAccounts` value entry into a [`FetchedAccount`].
///
/// `null` means the account does not exist (`None`); otherwise the `data`
/// field is `[base64_string, "base64"]` per the requested encoding.
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
}
