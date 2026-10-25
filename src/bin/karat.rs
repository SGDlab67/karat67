use std::io::IsTerminal;

use base64::Engine as _;
use clap::{Parser, Subcommand};
use karat67::checks::completeness::{check_completeness, check_orphans, gap_slots};
use karat67::checks::reconcile::{DEFAULT_MAX_SLOT_LAG, SampleRow, reconcile};
use karat67::checks::shape::{AccountSpec, check_shape};
use karat67::checks::specs::{
    LenRule, account_type_names, find_by_account_type, find_by_discriminator,
};
use karat67::fetch::{AccountFetcher, RpcAccountFetcher};
use karat67::gate::{all_pass, check_account};
use karat67::report::{CheckResult, Status};
use karat67::upgrade::{ProgramUpgrade, SampleReport, read_program_upgrade, sample_accounts};

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
    /// Check an observed account payload against the account registry, by
    /// discriminator and size.
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
        /// mismatch before failing, used only when the node has no history
        /// for the account's last touch. Within this window a mismatch is
        /// Skipped (never Pass); the process still exits non-zero.
        #[arg(long = "max-slot-lag", default_value_t = DEFAULT_MAX_SLOT_LAG)]
        max_slot_lag: u64,
    },
    /// Completeness: compare indexed slots to `getBlocks` for a window.
    /// Leader skip-slots omitted by `getBlocks` are not gaps. Exits non-zero
    /// unless the result is Pass — Fail and Skipped both fail the process.
    Completeness {
        /// Solana JSON-RPC endpoint. Falls back to `KARAT_RPC_URL`.
        #[arg(long, env = "KARAT_RPC_URL")]
        rpc_url: String,
        /// Inclusive start of the slot window to check.
        #[arg(long)]
        start: u64,
        /// Inclusive end of the slot window to check.
        #[arg(long)]
        end: u64,
        /// Indexed slot. Repeatable; combined with `--slots` if both are given.
        #[arg(long = "slot")]
        slot: Vec<u64>,
        /// Comma-separated indexed slots, e.g. `100,101,103`. Combined with
        /// repeatable `--slot` values.
        #[arg(long = "slots")]
        slots: Option<String>,
    },
    /// Orphans: the mirror of completeness. Find indexed slots inside the
    /// window that `getBlocks` never produced, meaning the index holds state
    /// for a slot that was skipped or forked away. Unlike completeness, a slot
    /// absent from `getBlocks` is a finding here when the index wrote it.
    /// Exits non-zero unless the result is Pass: Fail and Skipped both fail
    /// the process.
    Orphans {
        /// Solana JSON-RPC endpoint. Falls back to `KARAT_RPC_URL`.
        #[arg(long, env = "KARAT_RPC_URL")]
        rpc_url: String,
        /// Inclusive start of the slot window to check.
        #[arg(long)]
        start: u64,
        /// Inclusive end of the slot window to check.
        #[arg(long)]
        end: u64,
        /// Indexed slot. Repeatable; combined with `--slots` if both are given.
        #[arg(long = "slot")]
        slot: Vec<u64>,
        /// Comma-separated indexed slots, e.g. `100,101,103`. Combined with
        /// repeatable `--slot` values.
        #[arg(long = "slots")]
        slots: Option<String>,
    },
    /// Gaps: the exact backfill list after a stream reconnect. Takes the two
    /// slots a disconnecting datasource knows (Carbon's
    /// `DatasourceDisconnection`: last slot before the drop, first slot after
    /// the reconnect) and lists every produced block from one to the other,
    /// both ends included, since either boundary slot can be partly delivered.
    ///
    /// Exit code: 0 whenever the list was computed, non-empty or not. The
    /// list is the deliverable here, not a verdict. A fetch that cannot be
    /// made exits 1, because a fetch that did not happen is never green.
    Gaps {
        /// Solana JSON-RPC endpoint. Falls back to `KARAT_RPC_URL`.
        #[arg(long, env = "KARAT_RPC_URL")]
        rpc_url: String,
        /// Last slot the stream processed before it dropped.
        #[arg(long)]
        last_before: u64,
        /// First slot the stream processed after it reconnected. Must not be
        /// less than `--last-before`.
        #[arg(long)]
        first_after: u64,
        /// Write the full list to this file, one slot per line, instead of
        /// printing it. The summary still prints, and the JSON drops its
        /// `gaps` array in favor of `out`.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
    /// Inspect: what an account is, what it holds, and whether to trust the
    /// decode. Reads the account, picks the type from its owner and
    /// discriminator, checks the length, decodes the known fields, and
    /// compares the program's deploy slot with the one the registry was
    /// verified against.
    ///
    /// With `--indexed-base64`, also reconciles your stored copy against the
    /// chain (same rules as `reconcile`).
    ///
    /// Exit code: 0 only when the verdict is `ok` and any indexed check
    /// passed.
    Inspect {
        /// Solana JSON-RPC endpoint. Falls back to `KARAT_RPC_URL`.
        #[arg(long, env = "KARAT_RPC_URL")]
        rpc_url: String,
        /// Account pubkey to inspect.
        #[arg(long)]
        account: String,
        /// Your stored copy of the account bytes, base64.
        #[arg(long = "indexed-base64")]
        indexed_base64: Option<String>,
        /// Slot your stored copy was written at.
        #[arg(long = "indexed-slot", requires = "indexed_base64")]
        indexed_slot: Option<u64>,
        /// Slot lag tolerated on a mismatch when the account's last touch is
        /// unknown.
        #[arg(long = "max-slot-lag", default_value_t = DEFAULT_MAX_SLOT_LAG)]
        max_slot_lag: u64,
    },
    /// Sentinel: the slot a program's code last changed at, so a stale
    /// decoder has a boundary instead of a guess. Reads the ProgramData
    /// header for the deploy slot and the upgrade authority, and prints the
    /// slot as a Carbon `SlotRangeFilter` boundary to paste into a versioned
    /// decoder set.
    ///
    /// With `--sample N`, also lists the accounts matching one discriminator,
    /// fetches N of them in full, shape-checks each, and reports the observed
    /// length distribution.
    ///
    /// Exit code: 0 when the read succeeded and nothing below says otherwise.
    /// 1 when `--since` is given and the program was deployed after that slot
    /// (it changed under you), when the read fails, or when a sampled account
    /// fails its shape check.
    Sentinel {
        /// Solana JSON-RPC endpoint. Falls back to `KARAT_RPC_URL`.
        #[arg(long, env = "KARAT_RPC_URL")]
        rpc_url: String,
        /// Program ID to inspect.
        #[arg(long)]
        program: String,
        /// Exit non-zero when the program was deployed after this slot.
        #[arg(long)]
        since: Option<u64>,
        /// Sample this many accounts of one type and report their lengths.
        #[arg(long, value_name = "N")]
        sample: Option<usize>,
        /// Registered account type to sample, e.g. `Pool`. Must belong to
        /// `--program`.
        #[arg(long, conflicts_with = "discriminator")]
        account_type: Option<String>,
        /// Discriminator to sample as 16 hex digits, for a type the registry
        /// does not know yet.
        #[arg(long)]
        discriminator: Option<String>,
    },
    /// Shape-check one account's real indexed bytes, then reconcile that
    /// account against RPC only if shape passes. Prints JSON results. Exits
    /// 0 only when every result is Pass. A shape failure does not call RPC.
    Account {
        /// Account pubkey.
        #[arg(long)]
        account: String,
        /// Base64-encoded indexed account bytes. These exact bytes are shaped
        /// and reconciled — not a reconstructed discriminator.
        #[arg(long)]
        indexed_base64: String,
        /// Slot at which the indexer wrote this account. Omit for strict
        /// mismatch = Fail.
        #[arg(long)]
        indexed_slot: Option<u64>,
        /// Maximum context_slot − indexed_slot lag tolerated on a byte
        /// mismatch before failing, used only when the node has no history
        /// for the account's last touch. Within this window a mismatch is
        /// Skipped (never Pass).
        #[arg(long = "max-slot-lag", default_value_t = DEFAULT_MAX_SLOT_LAG)]
        max_slot_lag: u64,
        /// Solana JSON-RPC endpoint. Falls back to `KARAT_RPC_URL`. Required
        /// only after shape passes; unused when shape fails.
        #[arg(long, env = "KARAT_RPC_URL")]
        rpc_url: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Shape { account_type, len } => {
            let Some(spec) = find_by_account_type(&account_type) else {
                anyhow::bail!(
                    "unknown account type {account_type:?}, expected one of {:?}",
                    account_type_names()
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
        Command::Completeness {
            rpc_url,
            start,
            end,
            slot,
            slots,
        } => {
            if end < start {
                anyhow::bail!("--end {end} is before --start {start}");
            }

            let indexed = collect_indexed_slots(slot, slots)?;

            let fetcher = RpcAccountFetcher::new(rpc_url);
            let result = check_completeness(&indexed, start, end, &fetcher);

            if cli.json || !std::io::stdout().is_terminal() {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                print_completeness_human(start, end, indexed.len(), &result);
            }
            // Exit 0 only on Pass. Skipped (unreachable fetch) and Fail both
            // fail the process — same policy as reconcile.
            if result.status != Status::Pass {
                std::process::exit(1);
            }
        }
        Command::Orphans {
            rpc_url,
            start,
            end,
            slot,
            slots,
        } => {
            if end < start {
                anyhow::bail!("--end {end} is before --start {start}");
            }

            let indexed = collect_indexed_slots(slot, slots)?;

            let fetcher = RpcAccountFetcher::new(rpc_url);
            let result = check_orphans(&indexed, start, end, &fetcher);

            if cli.json || !std::io::stdout().is_terminal() {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                print_orphans_human(start, end, indexed.len(), &result);
            }
            // Same policy as completeness: exit 0 only on Pass.
            if result.status != Status::Pass {
                std::process::exit(1);
            }
        }
        Command::Gaps {
            rpc_url,
            last_before,
            first_after,
            out,
        } => {
            if first_after < last_before {
                anyhow::bail!(
                    "--first-after {first_after} must not be less than --last-before {last_before}"
                );
            }

            let fetcher = RpcAccountFetcher::new(rpc_url);
            let gaps = match gap_slots(last_before, first_after, &fetcher) {
                Ok(gaps) => gaps,
                Err(error) => {
                    // Skipped is never green: a fetch that could not run
                    // exits non-zero here too, in both output modes.
                    let name = format!("gaps([{last_before}, {first_after}])");
                    let result = CheckResult::skipped(name, format!("fetch unreachable: {error}"));
                    if cli.json || !std::io::stdout().is_terminal() {
                        println!("{}", serde_json::to_string_pretty(&result)?);
                    } else {
                        eprintln!("gaps: {error}");
                        eprintln!("exit code 1: a fetch that did not happen is never green");
                    }
                    std::process::exit(1);
                }
            };

            let report = gap_report(last_before, first_after, gaps);
            if let Some(path) = out.as_deref() {
                let body: String = report.gaps.iter().map(|slot| format!("{slot}\n")).collect();
                std::fs::write(path, body)
                    .map_err(|e| anyhow::anyhow!("writing {}: {e}", path.display()))?;
            }
            if cli.json || !std::io::stdout().is_terminal() {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&gaps_json(&report, out.as_deref()))?
                );
            } else {
                print!("{}", render_gaps(&report, out.as_deref()));
            }
        }
        Command::Inspect {
            rpc_url,
            account,
            indexed_base64,
            indexed_slot,
            max_slot_lag,
        } => {
            let indexed = match indexed_base64 {
                None => None,
                Some(encoded) => Some((
                    base64::engine::general_purpose::STANDARD
                        .decode(encoded.trim())
                        .map_err(|e| anyhow::anyhow!("--indexed-base64 is not base64: {e}"))?,
                    indexed_slot,
                )),
            };
            let fetcher = RpcAccountFetcher::new(rpc_url);
            let inspection =
                karat67::inspect::inspect_account(&fetcher, &account, indexed, max_slot_lag)?;
            if cli.json || !std::io::stdout().is_terminal() {
                println!("{}", serde_json::to_string_pretty(&inspection.to_json())?);
            } else {
                print!("{}", render_inspection(&inspection));
            }
            if !inspection.ok() {
                std::process::exit(1);
            }
        }
        Command::Sentinel {
            rpc_url,
            program,
            since,
            sample,
            account_type,
            discriminator,
        } => {
            let fetcher = RpcAccountFetcher::new(rpc_url);
            let upgrade = read_program_upgrade(&fetcher, &program)?;
            let Some(upgrade) = upgrade else {
                // Not upgradeable: no ProgramData, so nothing to go stale at.
                if cli.json || !std::io::stdout().is_terminal() {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "program": program,
                            "upgradeable": false,
                        }))?
                    );
                } else {
                    println!();
                    println!("  karat67  sentinel · {program}");
                    println!();
                    println!("  not upgradeable: no ProgramData account, so no upgrade slot");
                    println!();
                }
                return Ok(());
            };

            let sampled = match sample {
                None => None,
                Some(count) => {
                    let (disc, name) =
                        sample_target(account_type.as_deref(), discriminator.as_deref())?;
                    let report = sample_accounts(&fetcher, &program, disc, count)?;
                    Some(SentinelSample {
                        discriminator: disc,
                        account_type: name,
                        report,
                    })
                }
            };

            if cli.json || !std::io::stdout().is_terminal() {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&sentinel_json(
                        &program,
                        &upgrade,
                        sampled.as_ref(),
                        since
                    ))?
                );
            } else {
                print!("{}", render_sentinel(&program, &upgrade, sampled.as_ref()));
            }

            // Fail closed: a deploy after --since means the program changed
            // under the caller, and a sampled shape failure means the
            // registry's rule no longer matches the chain.
            let deployed_after_since = since.is_some_and(|since| upgrade.deploy_slot > since);
            let sample_failed = sampled.as_ref().is_some_and(|s| s.report.failed() > 0);
            if deployed_after_since || sample_failed {
                std::process::exit(1);
            }
        }
        Command::Account {
            account,
            indexed_base64,
            indexed_slot,
            max_slot_lag,
            rpc_url,
        } => {
            let data = base64::engine::general_purpose::STANDARD
                .decode(indexed_base64.trim())
                .map_err(|e| anyhow::anyhow!("invalid base64 for account {account}: {e}"))?;
            let fetcher = rpc_url.map(RpcAccountFetcher::new);
            let results = check_account(
                &account,
                &data,
                indexed_slot,
                max_slot_lag,
                fetcher.as_ref().map(|fetch| fetch as &dyn AccountFetcher),
            );
            if cli.json || !std::io::stdout().is_terminal() {
                for result in &results {
                    println!("{}", serde_json::to_string_pretty(result)?);
                }
            } else {
                let color = std::env::var_os("NO_COLOR").is_none();
                print!("{}", render_account(&account, data.len(), &results, color));
            }
            if !all_pass(&results) {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

/// Merge repeatable `--slot` values with a comma-separated `--slots` string.
///
/// Shared by the slot-window checks so both read their input identically. An
/// empty result is an error, not a vacuous Pass: a window with no indexed
/// slots supplied proves nothing in either direction.
fn collect_indexed_slots(slot: Vec<u64>, slots: Option<String>) -> anyhow::Result<Vec<u64>> {
    let mut indexed = slot;
    if let Some(csv) = slots {
        for part in csv.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let value: u64 = part
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid slot in --slots {part:?}: {e}"))?;
            indexed.push(value);
        }
    }
    if indexed.is_empty() {
        anyhow::bail!("provide at least one indexed slot via --slot or --slots");
    }
    Ok(indexed)
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
        "  {}  shape check · {} {}",
        paint("1;33", "karat67"),
        spec.program,
        spec.account_type
    );
    println!();
    // A variable-length type's registered size is a floor, not a layout, and
    // printing it as "expected N bytes (IDL layout)" would report a minimum as
    // an exact requirement.
    match spec.len_rule {
        LenRule::Exact => println!(
            "  expected   {:>6} bytes   (IDL layout, discriminator {disc})",
            group(spec.data_len)
        ),
        LenRule::AtLeast => println!(
            "  minimum    {:>6} bytes   (variable layout, discriminator {disc})",
            group(spec.data_len)
        ),
    }
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

/// Terminal report for completeness: window, outcome, skip-slot reminder.
fn print_completeness_human(start: u64, end: u64, indexed_len: usize, result: &CheckResult) {
    let color = std::env::var_os("NO_COLOR").is_none();
    let paint = |code: &str, text: &str| {
        if color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    };

    println!();
    println!(
        "  {}  completeness check · slots [{start}..={end}]",
        paint("1;33", "karat67")
    );
    println!();
    println!("  indexed    {:>6} slot(s) supplied", group(indexed_len));
    println!();
    match result.status {
        Status::Pass => {
            println!(
                "  {}  every getBlocks slot in the window is indexed",
                paint("1;32", "PASS")
            );
            println!("        leader skip-slots (omitted by getBlocks) are not gaps");
        }
        Status::Fail => {
            let detail = result.detail.as_deref().unwrap_or("missing slots");
            let lines = describe_missing(detail);
            println!("  {}  {}", paint("1;31", "FAIL"), lines[0]);
            for line in &lines[1..] {
                println!("        {line}");
            }
            println!();
            println!("  exit code 1: a pipeline or CI job stops here");
        }
        Status::Skipped => {
            let detail = result.detail.as_deref().unwrap_or("check skipped");
            println!("  {}  {detail}", paint("1;33", "SKIP"));
            println!();
            println!("  exit code 1: Skipped is never green");
        }
    }
    println!();
}

/// Terminal report for orphans: window, outcome, and why the skip-slot rule
/// is read the other way around here.
fn print_orphans_human(start: u64, end: u64, indexed_len: usize, result: &CheckResult) {
    let color = std::env::var_os("NO_COLOR").is_none();
    let paint = |code: &str, text: &str| {
        if color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    };

    println!();
    println!(
        "  {}  orphan check · slots [{start}..={end}]",
        paint("1;33", "karat67")
    );
    println!();
    println!("  indexed    {:>6} slot(s) supplied", group(indexed_len));
    println!();
    match result.status {
        Status::Pass => {
            println!(
                "  {}  every indexed slot in the window was produced on chain",
                paint("1;32", "PASS")
            );
            println!("        indexed slots outside the window are not considered");
        }
        Status::Fail => {
            let detail = result.detail.as_deref().unwrap_or("orphaned slots");
            println!("  {}  {detail}", paint("1;31", "FAIL"));
            println!("        the index holds state for slots getBlocks never returned:");
            println!("        skipped or forked away, so that state never landed on chain");
            println!("        writing at `processed` for latency is how this happens");
            println!();
            println!("  exit code 1: a pipeline or CI job stops here");
        }
        Status::Skipped => {
            let detail = result.detail.as_deref().unwrap_or("check skipped");
            println!("  {}  {detail}", paint("1;33", "SKIP"));
            println!();
            println!("  exit code 1: Skipped is never green");
        }
    }
    println!();
}

/// Terminal report for the account gate: one row per stage, shape then
/// reconcile, and the exit code a pipeline would see. Pure so it is testable.
fn render_account(account: &str, len: usize, results: &[CheckResult], color: bool) -> String {
    use std::fmt::Write as _;

    let paint = |code: &str, text: &str| {
        if color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    };
    // Continuation lines line up under the detail column of a stage row.
    let pad = " ".repeat(19);
    let mut out = String::new();

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {}  account check · {account}",
        paint("1;33", "karat67")
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "  indexed    {:>6} bytes", group(len));
    let _ = writeln!(out);

    for result in results {
        let stage = if result.check.starts_with("reconcile") {
            "reconcile"
        } else {
            "shape"
        };
        let tag = match result.status {
            Status::Pass => paint("1;32", "PASS"),
            Status::Fail => paint("1;31", "FAIL"),
            Status::Skipped => paint("1;33", "SKIP"),
        };
        let lines = match (result.status, result.detail.as_deref()) {
            (Status::Pass, _) if stage == "shape" => vec![format!(
                "{} layout matches the IDL",
                result
                    .check
                    .trim_start_matches("shape(")
                    .trim_end_matches(')')
            )],
            (Status::Pass, _) => vec!["indexed bytes match the chain".to_string()],
            (_, Some(detail)) => describe(detail),
            (_, None) => vec!["no detail".to_string()],
        };
        let _ = writeln!(out, "  {stage:<9}  {tag}  {}", lines[0]);
        for line in &lines[1..] {
            let _ = writeln!(out, "{pad}{line}");
        }
    }
    if results.len() == 1 && results[0].status != Status::Pass {
        let _ = writeln!(
            out,
            "  reconcile        not run: shape failed, no RPC call made"
        );
    }

    let _ = writeln!(out);
    if all_pass(results) {
        let _ = writeln!(out, "  exit code 0: safe to decode and use");
    } else if results.iter().any(|r| r.status == Status::Fail) {
        let _ = writeln!(out, "  exit code 1: a pipeline or CI job stops here");
    } else {
        let _ = writeln!(out, "  exit code 1: Skipped is never green");
    }
    let _ = writeln!(out);
    out
}

/// Terminal report for `inspect`: verdict first, then identity, then the
/// decoded fields. Pure so it is testable.
fn render_inspection(inspection: &karat67::inspect::Inspection) -> String {
    use std::fmt::Write as _;
    let color = std::env::var_os("NO_COLOR").is_none();
    let paint = |code: &str, text: &str| {
        if color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    };
    let json = inspection.to_json();
    let verdict = json["verdict"].as_str().unwrap_or("?");
    let badge = match verdict {
        "ok" => paint("1;32", "OK"),
        "unregistered_program" | "missing" => paint("1;33", &verdict.to_uppercase()),
        _ => paint("1;31", &verdict.to_uppercase()),
    };
    let mut out = String::new();
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {}  inspect · {}",
        paint("1;33", "karat67"),
        inspection.account
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "  {badge}  {}", inspection.reason);
    let _ = writeln!(out);
    let row = |out: &mut String, key: &str, value: String| {
        let _ = writeln!(out, "  {key:<14} {value}");
    };
    if let (Some(program), Some(spec)) = (inspection.program, inspection.spec) {
        row(
            &mut out,
            "type",
            format!("{} {}", program.name, spec.account_type),
        );
    } else if let Some(owner) = &inspection.owner {
        row(&mut out, "owner", owner.clone());
    }
    if let Some(len) = inspection.data_len {
        row(&mut out, "length", format!("{} bytes", group(len)));
    }
    row(&mut out, "read at slot", group(inspection.slot));
    if let Some(program) = inspection.program {
        let deploy = inspection
            .program_deploy_slot
            .map(group)
            .unwrap_or_else(|| "immutable or not read".into());
        row(
            &mut out,
            "deploy slot",
            format!(
                "{deploy}  (registry verified at {})",
                group(program.verified_deploy_slot)
            ),
        );
    }
    if let Some(fields) = &inspection.fields {
        let _ = writeln!(out);
        for (name, value) in fields {
            let value = value
                .as_str()
                .map(str::to_string)
                .unwrap_or(value.to_string());
            let _ = writeln!(out, "    {name:<30} {value}");
        }
    }
    if let Some(indexed) = &inspection.indexed {
        let _ = writeln!(out);
        for result in indexed {
            let status = match result.status {
                Status::Pass => paint("1;32", "PASS"),
                Status::Fail => paint("1;31", "FAIL"),
                Status::Skipped => paint("1;33", "SKIP"),
            };
            let _ = writeln!(out, "  indexed copy   {status}");
            if let Some(detail) = &result.detail {
                for line in describe(detail) {
                    let _ = writeln!(out, "                 {line}");
                }
            }
        }
    }
    let _ = writeln!(out);
    out
}

/// Reconcile details are JSON strings; turn the known fields into short
/// lines. Anything else (missing account, unreachable RPC) is word-wrapped.
fn describe(detail: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(detail) else {
        return wrap(detail, 60);
    };
    let num = |key: &str| v[key].as_u64().map(group).unwrap_or_else(|| "?".into());
    let mut lines = Vec::new();
    if v["reason"] == "rpc behind index" {
        lines.push(format!(
            "bytes differ at offset {}, but the indexed write is at slot {}",
            num("first_diff_offset"),
            num("indexed_slot")
        ));
        lines.push("the RPC has not reached that slot yet: recheck".into());
    } else if v["reason"] == "written after indexed slot" {
        lines.push(format!(
            "bytes differ at offset {}; account written again at slot {}",
            num("first_diff_offset"),
            num("last_touch_slot")
        ));
        lines.push(format!(
            "indexer is behind on this account (indexed at {}), not corrupt",
            num("indexed_slot")
        ));
    } else if v["reason"] == "slot lag" {
        lines.push(format!(
            "bytes differ at offset {}, indexer {} slots behind",
            num("first_diff_offset"),
            num("lag")
        ));
        lines.push("inside the slot-lag window: recheck, do not trust yet".into());
    } else {
        lines.push(format!(
            "bytes differ from chain at offset {}",
            num("first_diff_offset")
        ));
        lines.push(format!(
            "indexed {} bytes, on-chain {} bytes",
            num("indexed_len"),
            num("onchain_len")
        ));
    }
    if v["last_touch_slot"].is_u64() && v["reason"].is_null() {
        lines.push(format!(
            "untouched since slot {}, before the indexed write: corrupt, not lag",
            num("last_touch_slot")
        ));
    }
    lines.push(format!("chain read at slot {}", num("context_slot")));
    lines
}

/// Completeness Fail details are JSON: count plus the first missing slots.
/// Anything that does not parse is printed raw, word-wrapped.
fn describe_missing(detail: &str) -> Vec<String> {
    let parsed = serde_json::from_str::<serde_json::Value>(detail).ok();
    let Some(v) = parsed.filter(|v| v["missing_count"].is_u64()) else {
        return wrap(detail, 60);
    };
    let count = v["missing_count"].as_u64().unwrap_or_default();
    let sample: Vec<String> = v["missing_sample"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|slot| slot.as_u64())
        .map(group)
        .collect();
    let label = if (sample.len() as u64) < count {
        "first missing:"
    } else {
        "missing:"
    };
    let mut lines = vec![format!(
        "{} of {} slots in getBlocks are missing from the index",
        group(count),
        v["produced_count"]
            .as_u64()
            .map(group)
            .unwrap_or_else(|| "?".into())
    )];
    lines.extend(wrap(&format!("{label} {}", sample.join(" ")), 60));
    lines
}

/// Greedy word wrap at `width` columns.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split_whitespace() {
        let line = lines.last_mut().expect("never empty");
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            lines.push(word.to_string());
        } else {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    lines
}

/// 3344 -> "3,344".
fn group(n: impl std::fmt::Display) -> String {
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

/// How many gap slots the terminal report lists before pointing at `--out`.
const GAP_SAMPLE_LIMIT: usize = 16;

/// Everything `karat gaps` reports, computed from the two boundary slots and
/// the produced list so both output modes and the tests read one source.
struct GapReport {
    last_before: u64,
    first_after: u64,
    /// Produced blocks in the closed range: the backfill list.
    gaps: Vec<u64>,
    /// Heights in the closed range with no block: leader skip-slots.
    skipped: u64,
}

impl GapReport {
    /// Carbon's own estimate, for the side-by-side: `first - last` is one
    /// short of the `first - last + 1` heights in the closed range, and
    /// cannot tell a produced block from a skip-slot.
    fn carbon_missed_slots(&self) -> u64 {
        self.first_after.saturating_sub(self.last_before)
    }
}

/// Build the report from the boundary slots and the produced list.
///
/// Callers pass `first_after >= last_before` (the CLI rejects anything
/// else), so the closed range always holds at least one height.
fn gap_report(last_before: u64, first_after: u64, gaps: Vec<u64>) -> GapReport {
    let window = first_after - last_before + 1;
    let skipped = window.saturating_sub(gaps.len() as u64);
    GapReport {
        last_before,
        first_after,
        gaps,
        skipped,
    }
}

/// Machine output. Carries the full `gaps` array, or the `out` path when
/// `--out` already wrote it to a file.
fn gaps_json(report: &GapReport, out: Option<&std::path::Path>) -> serde_json::Value {
    let mut json = serde_json::json!({
        "last_before": report.last_before,
        "first_after": report.first_after,
        "window_start": report.last_before,
        "window_end": report.first_after,
        "window_slots": report.first_after - report.last_before + 1,
        "produced_blocks": report.gaps.len(),
        "skipped_slots": report.skipped,
        "carbon_missed_slots": report.carbon_missed_slots(),
        "gap_count": report.gaps.len(),
    });
    match out {
        Some(path) => json["out"] = serde_json::json!(path.display().to_string()),
        None => json["gaps"] = serde_json::json!(report.gaps),
    }
    json
}

/// Terminal report: the window, the numbers, and Carbon's formula next to
/// them so the difference is visible rather than argued.
fn render_gaps(report: &GapReport, out: Option<&std::path::Path>) -> String {
    use std::fmt::Write as _;

    let color = std::env::var_os("NO_COLOR").is_none();
    let paint = |code: &str, text: &str| {
        if color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    };
    let mut text = String::new();

    let _ = writeln!(text);
    let _ = writeln!(
        text,
        "  {}  gaps · after slot {}, before slot {}",
        paint("1;33", "karat67"),
        group(report.last_before),
        group(report.first_after)
    );
    let _ = writeln!(text);
    let _ = writeln!(
        text,
        "  window      {:>6} slots    [{}..={}]",
        group(report.first_after - report.last_before + 1),
        group(report.last_before),
        group(report.first_after)
    );
    let _ = writeln!(
        text,
        "  backfill    {:>6} blocks   every block the chain produced inside it",
        group(report.gaps.len())
    );
    let _ = writeln!(
        text,
        "  skip-slots  {:>6}          leader skips: no block was ever produced",
        group(report.skipped)
    );
    let _ = writeln!(
        text,
        "  carbon      {:>6} slots    first minus last: one short of the range, counts skip-slots, names nothing",
        group(report.carbon_missed_slots())
    );
    let _ = writeln!(text);

    if report.gaps.is_empty() {
        let _ = writeln!(
            text,
            "  nothing to backfill: the chain produced no block in the window"
        );
    } else {
        let sample: Vec<String> = report
            .gaps
            .iter()
            .take(GAP_SAMPLE_LIMIT)
            .map(|slot| group(*slot))
            .collect();
        let label = if report.gaps.len() > GAP_SAMPLE_LIMIT {
            "first blocks:"
        } else {
            "backfill:"
        };
        for line in wrap(&format!("{label} {}", sample.join(" ")), 60) {
            let _ = writeln!(text, "  {line}");
        }
        if report.gaps.len() > GAP_SAMPLE_LIMIT {
            let _ = writeln!(
                text,
                "  {} more; --out writes the full list",
                group(report.gaps.len() - GAP_SAMPLE_LIMIT)
            );
        }
    }
    if let Some(path) = out {
        let _ = writeln!(text, "  full list: {}", path.display());
    }
    let _ = writeln!(text);
    text
}

/// The sample half of a sentinel report.
struct SentinelSample {
    /// The sampled discriminator.
    discriminator: [u8; 8],
    /// Registry name for that discriminator, when there is one.
    account_type: Option<String>,
    report: SampleReport,
}

/// A discriminator as 16 hex digits.
fn hex8(bytes: &[u8; 8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Resolve `--account-type` or `--discriminator` into a discriminator, plus
/// the registry name when the type is registered.
fn sample_target(
    account_type: Option<&str>,
    discriminator: Option<&str>,
) -> anyhow::Result<([u8; 8], Option<String>)> {
    match (account_type, discriminator) {
        (Some(name), _) => {
            let spec = find_by_account_type(name).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown account type {name:?}, expected one of {:?}",
                    account_type_names()
                )
            })?;
            Ok((spec.discriminator, Some(spec.account_type.to_string())))
        }
        (None, Some(hex)) => Ok((parse_discriminator_hex(hex)?, None)),
        (None, None) => anyhow::bail!(
            "--sample needs --account-type <NAME> (registered) or --discriminator <HEX16>"
        ),
    }
}

/// 16 hex digits into the 8 discriminator bytes. A `0x` prefix is accepted.
fn parse_discriminator_hex(text: &str) -> anyhow::Result<[u8; 8]> {
    let text = text.trim();
    let text = text.strip_prefix("0x").unwrap_or(text);
    if text.len() != 16 {
        anyhow::bail!("discriminator {text:?} must be 16 hex digits (8 bytes)");
    }
    let mut out = [0u8; 8];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)
            .map_err(|e| anyhow::anyhow!("discriminator {text:?} is not hex: {e}"))?;
    }
    Ok(out)
}

/// Terminal report for the sentinel: deploy slot, upgrade authority, the
/// Carbon boundary to paste, and the sample when one was taken. Pure so it is
/// testable.
fn render_sentinel(
    program: &str,
    upgrade: &ProgramUpgrade,
    sample: Option<&SentinelSample>,
) -> String {
    use std::fmt::Write as _;

    let color = std::env::var_os("NO_COLOR").is_none();
    let paint = |code: &str, text: &str| {
        if color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    };
    let mut text = String::new();

    let _ = writeln!(text);
    let _ = writeln!(text, "  {}  sentinel · {program}", paint("1;33", "karat67"));
    let _ = writeln!(text);
    let _ = writeln!(text, "  deploy slot   {}", group(upgrade.deploy_slot));
    let _ = writeln!(
        text,
        "  authority     {}",
        upgrade
            .upgrade_authority
            .as_deref()
            .unwrap_or("none (immutable)")
    );
    let _ = writeln!(text);
    let _ = writeln!(
        text,
        "  carbon boundary, paste into a versioned decoder set:"
    );
    let _ = writeln!(
        text,
        "    SlotRangeFilter::from({}, None)   the current layout, from this slot on",
        upgrade.deploy_slot
    );
    let _ = writeln!(
        text,
        "    SlotRangeFilter::to({}, None)     the layout before it",
        upgrade.deploy_slot
    );

    if let Some(sample) = sample {
        let _ = writeln!(text);
        let _ = writeln!(
            text,
            "  sample        discriminator {} ({})",
            hex8(&sample.discriminator),
            sample.account_type.as_deref().unwrap_or("unregistered")
        );
        let _ = writeln!(
            text,
            "  matched       {} accounts",
            group(sample.report.matched)
        );
        let _ = writeln!(text, "  fetched       {}", group(sample.report.fetched));
        let _ = writeln!(text, "  lengths");
        for (length, stat) in &sample.report.lengths {
            let _ = writeln!(
                text,
                "    {:>10}  {:>7} fetched  {:>7} pass shape",
                group(*length),
                group(stat.accounts),
                group(stat.passed)
            );
        }
        if let Some(spec) = find_by_discriminator(sample.discriminator) {
            let _ = writeln!(
                text,
                "  registry      {} expects {} bytes at the current layout",
                spec.account_type,
                group(spec.data_len)
            );
        }
        if sample.report.failed() > 0 {
            let _ = writeln!(
                text,
                "  {}  {} fetched account(s) fail the shape check",
                paint("1;31", "FAIL"),
                group(sample.report.failed())
            );
            let _ = writeln!(
                text,
                "        the registry's rule no longer matches what the program writes"
            );
        }
    }
    let _ = writeln!(text);
    text
}

/// Machine output for the sentinel.
fn sentinel_json(
    program: &str,
    upgrade: &ProgramUpgrade,
    sample: Option<&SentinelSample>,
    since: Option<u64>,
) -> serde_json::Value {
    let mut json = serde_json::json!({
        "program": program,
        "upgradeable": true,
        "deploy_slot": upgrade.deploy_slot,
        "upgrade_authority": upgrade.upgrade_authority,
        "slot_range_filter": {
            "new_layout_from": format!("SlotRangeFilter::from({}, None)", upgrade.deploy_slot),
            "old_layout_to": format!("SlotRangeFilter::to({}, None)", upgrade.deploy_slot),
        },
    });
    if let Some(since) = since {
        json["since"] = serde_json::json!(since);
        json["deployed_after_since"] = serde_json::json!(upgrade.deploy_slot > since);
    }
    if let Some(sample) = sample {
        json["sample"] = serde_json::json!({
            "discriminator": hex8(&sample.discriminator),
            "account_type": sample.account_type,
            "matched": sample.report.matched,
            "fetched": sample.report.fetched,
            "failed_shape": sample.report.failed(),
            "lengths": sample
                .report
                .lengths
                .iter()
                .map(|(length, stat)| serde_json::json!({
                    "length": length,
                    "accounts": stat.accounts,
                    "shape_pass": stat.passed,
                }))
                .collect::<Vec<_>>(),
        });
    }
    json
}

#[cfg(test)]
mod tests {
    use super::*;
    use karat67::upgrade::LengthStat;

    #[test]
    fn account_report_names_the_failing_stage() {
        let empty = [CheckResult::fail(
            "shape",
            "empty payload: 0 bytes, every allocated account starts with an 8-byte discriminator",
        )];
        let out = render_account("acct", 0, &empty, false);
        assert!(out.contains("shape      FAIL  empty payload"));
        assert!(out.contains("not run: shape failed, no RPC call made"));
        assert!(out.contains("exit code 1"));
        assert!(out.lines().all(|line| line.chars().count() <= 80));

        let diff = r#"{"account":"acct","context_slot":455814836,"first_diff_offset":100,"indexed_len":3344,"onchain_len":3344}"#;
        let mismatch = [
            CheckResult::pass("shape(Obligation)"),
            CheckResult::fail("reconcile(acct)", diff),
        ];
        let out = render_account("acct", 3344, &mismatch, false);
        assert!(out.contains("shape      PASS  Obligation layout matches the IDL"));
        assert!(out.contains("reconcile  FAIL  bytes differ from chain at offset 100"));
        assert!(out.contains("indexed 3,344 bytes, on-chain 3,344 bytes"));
        assert!(out.contains("slot 455,814,836"));
        assert!(!out.contains("not run"));

        let gap = r#"{"end":104,"indexed_count":2,"missing_count":2,"missing_sample":[101,104],"produced_count":4,"start":100}"#;
        let lines = describe_missing(gap);
        assert_eq!(
            lines[0],
            "2 of 4 slots in getBlocks are missing from the index"
        );
        assert_eq!(lines[1], "missing: 101 104");
        assert_eq!(describe_missing("not json"), vec!["not json"]);
    }

    #[test]
    fn gaps_report_counts_blocks_skips_and_carbons_estimate() {
        // Window [99, 106] covers heights 99..=106; the chain produced six
        // of them (102 and 105 were leader skips).
        let report = gap_report(99, 106, vec![99, 100, 101, 103, 104, 106]);
        assert_eq!(report.skipped, 2);
        // Carbon: 106 - 99 = 7, one short of the 8 heights in the range, and
        // it counts the two skips as missed.
        assert_eq!(report.carbon_missed_slots(), 7);

        let text = render_gaps(&report, None);
        assert!(text.contains("[99..=106]"));
        assert!(text.contains("99 100 101 103 104 106"));
        assert!(text.contains("6 blocks"));
        assert!(text.contains("7 slots"));

        let json = gaps_json(&report, None);
        assert_eq!(json["window_start"], 99);
        assert_eq!(json["window_end"], 106);
        assert_eq!(json["window_slots"], 8);
        assert_eq!(json["gap_count"], 6);
        assert_eq!(json["skipped_slots"], 2);
        assert_eq!(json["carbon_missed_slots"], 7);
        assert_eq!(
            json["gaps"],
            serde_json::json!([99, 100, 101, 103, 104, 106])
        );

        // Adjacent slots: Carbon claims one missed slot, but both boundary
        // blocks may be partly delivered, so the backfill is two.
        let adjacent = gap_report(100, 101, vec![100, 101]);
        assert_eq!(adjacent.skipped, 0);
        assert_eq!(adjacent.carbon_missed_slots(), 1);
        let json = gaps_json(&adjacent, None);
        assert_eq!(json["window_slots"], 2);
        assert_eq!(json["gap_count"], 2);

        // Neither boundary block exists (both forked off): nothing to fetch.
        let empty = gap_report(100, 101, vec![]);
        assert_eq!(empty.skipped, 2);
        assert!(render_gaps(&empty, None).contains("nothing to backfill"));

        // With --out, the list is a file and the JSON says which one.
        let json = gaps_json(&report, Some(std::path::Path::new("/tmp/gaps.txt")));
        assert!(json.get("gaps").is_none());
        assert_eq!(json["out"], "/tmp/gaps.txt");
    }

    #[test]
    fn sentinel_report_prints_the_carbon_boundary() {
        let upgrade = ProgramUpgrade {
            deploy_slot: 440_486_775,
            upgrade_authority: Some("GzFgdRJXmawPhGeBsyRCDLx4jAKPsvbUqoqitzppkzkW".to_string()),
        };
        let text = render_sentinel(
            "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA",
            &upgrade,
            None,
        );
        assert!(text.contains("SlotRangeFilter::from(440486775, None)"));
        assert!(text.contains("SlotRangeFilter::to(440486775, None)"));
        assert!(text.contains("440,486,775"));
        assert!(text.contains("GzFgdRJXmawPhGeBsyRCDLx4jAKPsvbUqoqitzppkzkW"));

        let immutable = ProgramUpgrade {
            deploy_slot: 7,
            upgrade_authority: None,
        };
        assert!(render_sentinel("prog", &immutable, None).contains("none (immutable)"));

        let json = sentinel_json("prog", &immutable, None, Some(10));
        assert_eq!(json["deploy_slot"], 7);
        assert_eq!(json["deployed_after_since"], false);
        assert_eq!(json["upgrade_authority"], serde_json::Value::Null);
        let json = sentinel_json("prog", &immutable, None, Some(5));
        assert_eq!(json["deployed_after_since"], true);
        assert_eq!(
            json["slot_range_filter"]["old_layout_to"],
            "SlotRangeFilter::to(7, None)"
        );
    }

    #[test]
    fn sentinel_sample_report_counts_lengths_and_failures() {
        let mut lengths = std::collections::BTreeMap::new();
        lengths.insert(
            245,
            LengthStat {
                accounts: 198,
                passed: 198,
            },
        );
        lengths.insert(
            243,
            LengthStat {
                accounts: 2,
                passed: 0,
            },
        );
        let sample = SentinelSample {
            discriminator: [241, 154, 109, 4, 17, 177, 109, 188],
            account_type: Some("Pool".to_string()),
            report: SampleReport {
                matched: 1_234_567,
                fetched: 200,
                lengths,
            },
        };
        let upgrade = ProgramUpgrade {
            deploy_slot: 100,
            upgrade_authority: None,
        };
        let text = render_sentinel("prog", &upgrade, Some(&sample));
        assert!(text.contains("f19a6d0411b16dbc"));
        assert!(text.contains("1,234,567 accounts"));
        assert!(text.contains("2 fetched account(s) fail"));

        let json = sentinel_json("prog", &upgrade, Some(&sample), None);
        assert_eq!(json["sample"]["failed_shape"], 2);
        assert_eq!(json["sample"]["matched"], 1234567);
        assert_eq!(json["sample"]["lengths"][0]["length"], 243);
        assert_eq!(json["sample"]["lengths"][1]["shape_pass"], 198);
    }

    #[test]
    fn sample_targets_resolve_or_error() {
        let (disc, name) = sample_target(Some("Obligation"), None).expect("registered");
        assert_eq!(disc, [168, 206, 141, 106, 88, 76, 172, 167]);
        assert_eq!(name.as_deref(), Some("Obligation"));

        let (disc, name) = sample_target(None, Some("0xF19A6D0411B16DBC")).expect("raw hex");
        assert_eq!(disc, [241, 154, 109, 4, 17, 177, 109, 188]);
        assert_eq!(name, None);

        assert!(sample_target(None, None).is_err());
        assert!(sample_target(Some("NotAType"), None).is_err());
        assert!(parse_discriminator_hex("f19a").is_err());
        assert!(parse_discriminator_hex("zzzzzzzzzzzzzzzz").is_err());
        assert_eq!(
            hex8(&[241, 154, 109, 4, 17, 177, 109, 188]),
            "f19a6d0411b16dbc"
        );
    }
}
