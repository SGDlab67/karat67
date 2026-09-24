//! The integrity checks, in priority order: shape, reconciliation,
//! completeness.
//!
//! Freshness and liveness are out of scope by design: every indexer already
//! has them.
//!
//! Account layouts for shape live in [`specs`] — the IDL-informed registry.
//! A new program or account type is new data there, not a new check code path.

pub mod check;
pub mod reconcile;
pub mod shape;
pub mod specs;
