//! karat67 in a Carbon pipeline: Carbon's own Kamino Lend decoder, wrapped by
//! `KaratDecoder`, fed a live mainnet Obligation and three corrupted copies.
//!
//! `cargo run --manifest-path examples/carbon-kamino/Cargo.toml`
//! (reads `KARAT_RPC_URL`, defaults to the public mainnet endpoint)

use carbon_core::account::{AccountDecoder, AccountProcessorInputType};
use carbon_core::error::CarbonResult;
use carbon_core::metrics::MetricsRegistry;
use carbon_core::pipeline::{Pipeline, PipelineBuilder};
use carbon_core::processor::Processor;
use carbon_kamino_lending_decoder::accounts::KaminoLendingAccount;
use carbon_kamino_lending_decoder::{KaminoLendingDecoder, PROGRAM_ID};
use karat67::carbon::KaratDecoder;
use karat67::fetch::{AccountFetcher, RpcAccountFetcher};

const OBLIGATION: &str = "BYojGuT56e2TUb8PQwRyT1wL5X5Ekv4kZH1HUQgBu6Zg";

/// Stands in for the processor that writes rows to a database.
struct Store;

impl<'a> Processor<AccountProcessorInputType<'a, KaminoLendingAccount>> for Store {
    async fn process(
        &mut self,
        _row: &AccountProcessorInputType<'a, KaminoLendingAccount>,
    ) -> CarbonResult<()> {
        Ok(())
    }
}

/// The whole integration in a real pipeline: wrap the decoder. This compiles
/// against Carbon's own builder, so the fit is checked by rustc.
fn wire(builder: PipelineBuilder) -> PipelineBuilder {
    builder.account(KaratDecoder::new(PROGRAM_ID, KaminoLendingDecoder), Store)
}

fn main() -> anyhow::Result<()> {
    let _ = wire(Pipeline::builder());

    let rpc = std::env::var("KARAT_RPC_URL")
        .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".into());
    let fetched = RpcAccountFetcher::new(rpc).get_multiple_accounts(&[OBLIGATION.into()])?;
    let live = fetched.accounts[0].clone().expect("Obligation exists").data;
    let mut over_long = live.clone();
    over_long.extend([0; 16]);

    let karat = KaratDecoder::new(PROGRAM_ID, KaminoLendingDecoder);
    let cases = [
        ("live Obligation", live.clone(), true, true),
        ("empty payload", Vec::new(), false, false),
        (
            "truncated by 24 bytes",
            live[..live.len() - 24].to_vec(),
            false,
            false,
        ),
        ("16 bytes too long", over_long, true, false),
    ];

    println!("slot {}, account {OBLIGATION}\n", fetched.context_slot);
    println!("  {:<24} {:<14} with karat", "payload", "Carbon alone");
    for (name, data, want_carbon, want_karat) in cases {
        let account = solana_account::Account {
            lamports: 1,
            data,
            owner: PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        };
        let carbon = KaminoLendingDecoder.decode_account(&account).is_some();
        let gated = karat.decode_account(&account).is_some();
        let word = |ok| if ok { "decoded" } else { "dropped" };
        println!("  {name:<24} {:<14} {}", word(carbon), word(gated));
        assert_eq!((carbon, gated), (want_carbon, want_karat), "{name}");
    }

    println!();
    for (name, _, value) in MetricsRegistry::global().snapshot().counters {
        if name.starts_with("karat_") {
            println!("  {name:<32} {value}");
        }
    }
    Ok(())
}
