use std::io::IsTerminal;

use base64::Engine as _;
use clap::{Parser, Subcommand};
use karat67::checks::reconcile::{DEFAULT_MAX_SLOT_LAG, SampleRow, reconcile};
use karat67::checks::shape::{AccountSpec, KAMINO_ACCOUNTS, check_shape};
use karat67::fetch::RpcAccountFetcher;
use karat67::report::{CheckResult, Status};

/// Integrity checks for Solana indexers: verify indexed data is correct,
/// not just flowing.
#[derive(Parser)]
#[command(name = "karat", version, about, max_term_width = 100)]
struct Cli {
    /// Print JSON even on a terminal. Piped output is always JSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check an observed account payload against the Kamino account
    /// registry, by discriminator and exact size.
    Shape {
        /// Account type to check against, e.g. `Obligation`.
        #[arg(long)]
        account_type: String,
        /// Observed account data length in bytes; 0 models an empty payload.
        #[arg(long)]
        len: usize,
    },
    /// Reconcile indexed account bytes against on-chain state: fetch each
    /// account via `getMultipleAccounts` and diff it against the indexed
    /// bytes. Prints one JSON result per account. Exits non-zero unless
    /// every account is Pass — Fail and Skipped (including slot-lag) both
    /// fail the process.
    Reconcile {
        /// Solana JSON-RPC endpoint. Falls back to `KARAT_RPC_URL`.
        #[arg(long, env = "KARAT_RPC_URL")]
        rpc_url: String,
        /// Account pubkey to reconcile. Repeatable; paired by position with
        /// the `--indexed-base64` values (and `--indexed-slot` when given).
        #[arg(long = "account", required = true)]
        accounts: Vec<String>,
        /// Base64-encoded indexed bytes for the account at the same position.
        /// Repeatable; must be given once per `--account`.
        #[arg(long = "indexed-base64", required = true)]
        indexed_base64: Vec<String>,
        /// Slot at which the indexer wrote the account at the same position.
        /// Optional: omit entirely for strict mismatch=Fail. If any are
        /// given, the count must match `--account`.
        #[arg(long = "indexed-slot")]
        indexed_slots: Vec<u64>,
        /// Maximum context_slot − indexed_slot lag tolerated on a byte
        /// mismatch before failing. Within this window a mismatch is
        /// Skipped (never Pass); the process still exits non-zero.
        #[arg(long = "max-slot-lag", default_value_t = DEFAULT_MAX_SLOT_LAG)]
        max_slot_lag: u64,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Shape { account_type, len } => {
            let Some(spec) = KAMINO_ACCOUNTS
                .iter()
                .find(|spec| spec.account_type == account_type)
            else {
                anyhow::bail!(
                    "unknown account type {account_type:?}, expected one of {:?}",
                    KAMINO_ACCOUNTS
                        .iter()
                        .map(|spec| spec.account_type)
                        .collect::<Vec<_>>()
                );
            };

            let mut data = spec.discriminator.to_vec();
            data.resize(len, 0);
            let result = check_shape(&data);
            if cli.json || !std::io::stdout().is_terminal() {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                print_human(spec, len, &result);
            }
            if result.status != Status::Pass {
                std::process::exit(1);
            }
        }
        Command::Reconcile {
            rpc_url,
            accounts,
            indexed_base64,
            indexed_slots,
            max_slot_lag,
        } => {
            if accounts.len() != indexed_base64.len() {
                anyhow::bail!(
                    "got {} --account values but {} --indexed-base64 values; \
                     each account needs exactly one indexed payload",
                    accounts.len(),
                    indexed_base64.len()
                );
            }
            if !indexed_slots.is_empty() && indexed_slots.len() != accounts.len() {
                anyhow::bail!(
                    "got {} --indexed-slot values but {} --account values; \
                     supply one slot per account, or omit --indexed-slot entirely",
                    indexed_slots.len(),
                    accounts.len()
                );
            }

            let mut sample: Vec<SampleRow> = Vec::with_capacity(accounts.len());
            for (i, (pubkey, encoded)) in accounts.into_iter().zip(indexed_base64).enumerate() {
                let data = base64::engine::general_purpose::STANDARD
                    .decode(&encoded)
                    .map_err(|e| anyhow::anyhow!("invalid base64 for account {pubkey}: {e}"))?;
                let indexed_slot = indexed_slots.get(i).copied();
                sample.push((pubkey, data, indexed_slot));
            }

            let fetcher = RpcAccountFetcher::new(rpc_url);
            let results = reconcile(&sample, &fetcher, max_slot_lag);

            for result in &results {
                println!("{}", serde_json::to_string_pretty(result)?);
            }
            // Exit 0 only when every result is Pass. Skipped (slot lag or
            // unreachable fetch) and Fail both fail the process.
            if results
                .iter()
                .any(|r: &CheckResult| r.status != Status::Pass)
            {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

/// Terminal report: what was expected, what was seen, and what it means.
fn print_human(spec: &AccountSpec, len: usize, result: &CheckResult) {
    let color = std::env::var_os("NO_COLOR").is_none();
    let paint = |code: &str, text: &str| {
        if color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    };
    let disc: String = spec
        .discriminator
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    println!();
    println!(
        "  {}  shape check · Kamino Lend {}",
        paint("1;33", "karat67"),
        spec.account_type
    );
    println!();
    println!(
        "  expected   {:>6} bytes   (IDL layout, discriminator {disc})",
        group(spec.data_len)
    );
    println!("  observed   {:>6} bytes", group(len));
    println!();
    match result.status {
        Status::Pass => {
            println!("  {}  layout matches the IDL", paint("1;32", "PASS"));
            println!("        this row is safe to decode");
        }
        _ => {
            let detail = result.detail.as_deref().unwrap_or("shape mismatch");
            println!("  {}  {detail}", paint("1;31", "FAIL"));
            println!(
                "        no real {} account on-chain looks like this: the indexer wrote bad data",
                spec.account_type
            );
            println!("        liveness and freshness checks cannot see this");
            println!();
            println!("  exit code 1: a pipeline or CI job stops here");
        }
    }
    println!();
}

/// 3344 -> "3,344".
fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}
