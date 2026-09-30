//! Shape first, reconcile only after Pass. Not an indexer.
//! `cargo run --example account_gate`

use karat67::carbon::KaratIntegrityProcessor;
use karat67::checks::reconcile::DEFAULT_MAX_SLOT_LAG;
use karat67::checks::specs::find_by_account_type;
use karat67::fetch::MockFetcher;
use karat67::report::{CheckResult, Status};

fn gate(bytes: &[u8], slot: Option<u64>, fetcher: &MockFetcher) -> Vec<CheckResult> {
    KaratIntegrityProcessor::gate_account("acct", bytes, slot, DEFAULT_MAX_SLOT_LAG, fetcher)
}

fn main() {
    let spec = find_by_account_type("Obligation").expect("registered");
    let mut bytes = spec.discriminator.to_vec();
    bytes.resize(spec.data_len, 0);

    // Unreachable on purpose: empty bytes are a shape Fail and never call fetch.
    let empty = gate(&[], None, &MockFetcher::unreachable());
    assert_eq!((empty.len(), empty[0].status), (1, Status::Fail));
    println!("empty: {}", serde_json::to_string(&empty[0]).unwrap());

    // Mismatch at lag 10, inside the default window of 32: Skipped, never Pass.
    let mut drifted = bytes.clone();
    drifted[8] ^= 1;
    let lagged = gate(
        &bytes,
        Some(100),
        &MockFetcher::new(110).with_account("acct", drifted, 110),
    );
    assert_eq!(lagged[1].status, Status::Skipped);
    assert_ne!(lagged[1].status, Status::Pass);
    println!("lag: {}", serde_json::to_string(&lagged).unwrap());

    let matched = gate(
        &bytes,
        Some(100),
        &MockFetcher::new(100).with_account("acct", bytes.clone(), 100),
    );
    assert!(matched.iter().all(|result| result.status == Status::Pass));
    println!("match: {}", serde_json::to_string(&matched).unwrap());
}
