use clap::{Parser, Subcommand};
use karat67::checks::shape::{KAMINO_ACCOUNTS, check_shape};
use karat67::report::Status;

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
    }
    Ok(())
}
