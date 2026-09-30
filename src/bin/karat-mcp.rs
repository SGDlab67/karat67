//! Stdio MCP server: one tool, `check_account`, backed by the account gate.
//!
//! `cargo run --bin karat-mcp`

fn main() -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    karat67::mcp::serve(&mut input, &mut output)
}
