//! Smoke test for the real Solana JSON-RPC `AccountFetcher`.
//!
//! Ignored by default and gated on `KARAT_RPC_URL` so `cargo test --all` in CI
//! never touches the network. Run explicitly against a real endpoint with:
//!
//! ```bash
//! KARAT_RPC_URL=https://api.mainnet-beta.solana.com \
//!   cargo test --test rpc_smoke -- --ignored
//! ```

use karat67::fetch::{AccountFetcher, RpcAccountFetcher};

/// Wrapped SOL mint: a well-known mainnet account that always exists.
const WRAPPED_SOL_MINT: &str = "So11111111111111111111111111111111111111112";

#[test]
#[ignore = "requires KARAT_RPC_URL and network access"]
fn fetches_known_account_from_real_rpc() {
    let Ok(rpc_url) = std::env::var("KARAT_RPC_URL") else {
        eprintln!("KARAT_RPC_URL not set; skipping real-RPC smoke test");
        return;
    };

    let fetcher = RpcAccountFetcher::new(rpc_url);
    let result = fetcher
        .get_multiple_accounts(&[WRAPPED_SOL_MINT.to_string()])
        .expect("getMultipleAccounts should succeed against a live endpoint");

    assert!(
        result.context_slot > 0,
        "expected a positive context slot, got {}",
        result.context_slot
    );
    let account = result.accounts[0]
        .as_ref()
        .expect("wrapped SOL mint must exist on chain");
    assert!(
        !account.data.is_empty(),
        "wrapped SOL mint account data should not be empty"
    );
}
