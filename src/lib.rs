//! karat67 verifies that Solana indexer data is correct, not just flowing.
//!
//! An indexer can pass every freshness and throughput check while writing
//! empty or malformed accounts, and every dashboard, risk engine, and AI
//! agent built on that data inherits the error. This crate checks indexed
//! data against the chain: account layouts against the program IDL, sampled
//! rows reconciled against RPC, and slot coverage.
//!
//! Freshness and liveness are out of scope by design: every indexer already
//! has them.
//!
//! Early development. The API is unstable.

pub mod checks;
pub mod fetch;
pub mod report;
