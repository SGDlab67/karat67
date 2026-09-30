//! Library-level integrity gate: what a Carbon-style processor does per account.
//!
//! Run with `cargo run --example integrity_gate`. Exits non-zero if any
//! payload the gate should reject is accepted, so it doubles as a self-check.

use karat67::carbon::KaratIntegrityProcessor;
use karat67::checks::specs;
use karat67::report::Status;

/// Build a well-formed payload for `account_type`: real discriminator, real length.
fn well_formed(account_type: &str) -> Vec<u8> {
    let spec = specs::find_by_account_type(account_type).expect("registered account type");
    let mut data = spec.discriminator.to_vec();
    data.resize(spec.data_len, 0);
    data
}

fn main() {
    let obligation = well_formed("Obligation");

    // Every payload a decode path can hand the gate, and what it should do.
    let cases: [(&str, &str, Vec<u8>, Status); 5] = [
        ("well-formed row", "Obligation", obligation.clone(), Status::Pass),
        ("empty payload (the 158.91-hour failure)", "Obligation", vec![], Status::Fail),
        ("truncated row", "Obligation", obligation[..3000].to_vec(), Status::Fail),
        ("over-long row", "Obligation", [obligation.clone(), vec![0; 16]].concat(), Status::Fail),
        ("foreign discriminator", "Obligation", vec![7; 3344], Status::Fail),
    ];

    let mut wrong = 0;
    for (label, account_type, data, expected) in cases {
        let r = KaratIntegrityProcessor::check_account(account_type, &data);
        println!(
            "{:<40} {:>5} bytes  {:?}  {}",
            label,
            data.len(),
            r.status,
            r.detail.unwrap_or_else(|| "-".into())
        );
        if r.status != expected {
            eprintln!("  expected {expected:?}");
            wrong += 1;
        }
    }

    // An account type absent from the registry is a Fail, not a silent skip:
    // the gate never passes data it cannot describe.
    let r = KaratIntegrityProcessor::check_account("SomeOtherProgramAccount", &obligation);
    println!(
        "{:<40} {:>5} bytes  {:?}  {}",
        "unregistered account type",
        obligation.len(),
        r.status,
        r.detail.unwrap_or_else(|| "-".into())
    );
    if r.status != Status::Fail {
        wrong += 1;
    }

    assert_eq!(wrong, 0, "{wrong} case(s) did not behave as documented");
    println!("\nall cases behaved as documented");
}
