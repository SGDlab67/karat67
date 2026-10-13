//! karat67 in a Carbon pipeline: Carbon's own Kamino Lend decoder, wrapped by
//! `KaratDecoder`, fed a live mainnet Obligation and three corrupted copies.
//! Also the disconnect path: a `DatasourceDisconnection` through the
//! datasource's `disconnect_notifier` channel, turned into the exact
//! backfill list by `karat67::carbon::gaps_for_disconnection`.
//!
//! `cargo run --manifest-path examples/carbon-kamino/Cargo.toml`
//! (reads `KARAT_RPC_URL`, defaults to the public mainnet endpoint)

use carbon_core::account::{AccountDecoder, AccountProcessorInputType};
use carbon_core::datasource::DatasourceDisconnection;
use carbon_core::error::CarbonResult;
use carbon_core::metrics::MetricsRegistry;
use carbon_core::pipeline::{Pipeline, PipelineBuilder};
use carbon_core::processor::Processor;
use carbon_kamino_lending_decoder::accounts::KaminoLendingAccount;
use carbon_kamino_lending_decoder::{KaminoLendingDecoder, PROGRAM_ID};
use carbon_yellowstone_grpc_datasource::{
    BlockFilters, YellowstoneGrpcClientConfig, YellowstoneGrpcGeyserClient,
};
use karat67::carbon::{KaratDecoder, gaps_for_disconnection};
use karat67::fetch::{AccountFetcher, RpcAccountFetcher};
use std::collections::HashMap;
use tokio::sync::mpsc;

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
    let fetched = RpcAccountFetcher::new(&rpc).get_multiple_accounts(&[OBLIGATION.into()])?;
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

    // Disconnect wiring, live: an event through the datasource's notifier
    // channel becomes the exact backfill list for the window it names.
    let first_after = fetched.context_slot.saturating_sub(64);
    let last_before = first_after.saturating_sub(160);
    println!("\n  disconnect window ({last_before}, {first_after}):");
    let backfill = wire_disconnects(&rpc, last_before, first_after)?;
    assert!(
        backfill
            .iter()
            .all(|slot| *slot > last_before && *slot < first_after),
        "every backfill slot is inside the open interval"
    );
    assert!(
        backfill.windows(2).all(|pair| pair[0] < pair[1]),
        "the backfill list is sorted and unique"
    );

    println!();
    for (name, _, value) in MetricsRegistry::global().snapshot().counters {
        if name.starts_with("karat_") {
            println!("  {name:<32} {value}");
        }
    }
    Ok(())
}

/// The disconnect path end to end: the datasource holds the notifier sender,
/// and the receiving end turns each event into the exact backfill list.
///
/// No live gRPC endpoint is involved here, so the event the datasource would
/// emit is replayed through the same channel it holds. The list itself is
/// live: it comes from `getBlocks` on the window the event names.
fn wire_disconnects(rpc: &str, last_before: u64, first_after: u64) -> anyhow::Result<Vec<u64>> {
    let (tx, mut rx) = mpsc::channel::<DatasourceDisconnection>(8);

    // The whole integration point: hand the datasource the sender. Filters
    // are empty because this demo subscribes to nothing.
    let datasource = YellowstoneGrpcGeyserClient::new(
        "https://unused.invalid:443".to_string(),
        None,
        None,
        HashMap::new(),
        HashMap::new(),
        BlockFilters::default(),
        YellowstoneGrpcClientConfig::default(),
        Some(tx.clone()),
        None,
    );
    assert!(datasource.disconnect_notifier.is_some());

    tx.try_send(DatasourceDisconnection {
        source: "yellowstone-grpc".to_string(),
        disconnect_time: Default::default(),
        last_slot_before_disconnect: last_before,
        first_slot_after_reconnect: first_after,
        missed_slots: first_after.saturating_sub(last_before),
    })
    .map_err(|_| anyhow::anyhow!("disconnect channel full"))?;

    let fetcher = RpcAccountFetcher::new(rpc);
    let mut backfill = Vec::new();
    while let Ok(event) = rx.try_recv() {
        backfill = gaps_for_disconnection(&event, &fetcher)?;
        println!(
            "    drop  {} -> {}: Carbon estimates {} missed, the list has {} blocks",
            event.last_slot_before_disconnect,
            event.first_slot_after_reconnect,
            event.missed_slots,
            backfill.len()
        );
        if let Some(first) = backfill.first() {
            println!("    first backfill slot {first}");
        }
    }
    Ok(backfill)
}
