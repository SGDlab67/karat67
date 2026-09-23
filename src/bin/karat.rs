use base64::Engine as _;
use clap::{Parser, Subcommand};
use karat67::checks::reconcile::reconcile;
use karat67::checks::shape::{KAMINO_ACCOUNTS, check_shape};
use karat67::fetch::RpcAccountFetcher;
use karat67::report::{CheckResult, Status};

/// Integrity checks for Solana indexers: verify indexed data is correct,
/// not just flowing.
#[derive(Parser)]
#[command(name = "karat", version, about, max_term_width = 100)]
struct Cli {
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
    /// bytes. Prints one JSON result per account and exits non-zero unless
    /// every account passed.
    Reconcile {
        /// Solana JSON-RPC endpoint. Falls back to `KARAT_RPC_URL`.
        #[arg(long, env = "KARAT_RPC_URL")]
        rpc_url: String,
        /// Account pubkey to reconcile. Repeatable; paired by position with
        /// the `--indexed-base64` values.
        #[arg(long = "account", required = true)]
        accounts: Vec<String>,
        /// Base64-encoded indexed bytes for the account at the same position.
        /// Repeatable; must be given once per `--account`.
        #[arg(long = "indexed-base64", required = true)]
        indexed_base64: Vec<String>,
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
            println!("{}", serde_json::to_string_pretty(&result)?);
            if result.status != Status::Pass {
                std::process::exit(1);
            }
        }
        Command::Reconcile {
            rpc_url,
            accounts,
            indexed_base64,
        } => {
            if accounts.len() != indexed_base64.len() {
                anyhow::bail!(
                    "got {} --account values but {} --indexed-base64 values; \
                     each account needs exactly one indexed payload",
                    accounts.len(),
                    indexed_base64.len()
                );
            }

            let mut sample = Vec::with_capacity(accounts.len());
            for (pubkey, encoded) in accounts.into_iter().zip(indexed_base64) {
                let data = base64::engine::general_purpose::STANDARD
                    .decode(&encoded)
                    .map_err(|e| anyhow::anyhow!("invalid base64 for account {pubkey}: {e}"))?;
                sample.push((pubkey, data));
            }

            let fetcher = RpcAccountFetcher::new(rpc_url);
            let results = reconcile(&sample, &fetcher);

            for result in &results {
                println!("{}", serde_json::to_string_pretty(result)?);
            }
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
