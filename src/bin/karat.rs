use clap::{Parser, Subcommand};
use karat67::checks::shape::{Layouts, check_shape};
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
    /// Check an observed account data length against IDL-declared layouts.
    Shape {
        /// Observed account data length in bytes; 0 models an empty payload.
        #[arg(long)]
        len: usize,
        /// Declared lengths from the IDL, comma-separated.
        #[arg(long, value_delimiter = ',', required = true)]
        layouts: Vec<usize>,
        /// Account type name for the report.
        #[arg(long, default_value = "account")]
        account_type: String,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Shape {
            len,
            layouts,
            account_type,
        } => {
            let result = check_shape(
                &Layouts {
                    account_type,
                    data_lens: layouts,
                },
                len,
            );
            println!("{}", serde_json::to_string_pretty(&result)?);
            if result.status != Status::Pass {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}
