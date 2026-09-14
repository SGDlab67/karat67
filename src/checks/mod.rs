//! The integrity checks, in priority order: shape, reconciliation,
//! completeness.
//!
//! Freshness and liveness are out of scope by design: every indexer already
//! has them.

pub mod shape;
