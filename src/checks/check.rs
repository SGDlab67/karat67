//! The `Check` trait: a uniform seam every integrity check hangs off.
//!
//! Reconciliation and completeness reuse the same `CheckResult` contract as
//! shape, so the CLI and any future Carbon processor can run checks without
//! knowing which one they hold. The roadmap's "IDL-driven check trait: a new
//! program costs zero code" is this seam: adding a program means adding data
//! (an `AccountSpec`), not a new code path.

use crate::report::CheckResult;

/// One integrity check against one target.
///
/// A check is self-contained: it owns whatever input it needs and produces a
/// single [`CheckResult`]. Batch checks that fan out over many accounts (such
/// as reconciliation) build on the same [`CheckResult`] type but expose a
/// function returning several results rather than implementing `Check` once.
pub trait Check {
    /// Stable identifier for this check, e.g. `shape(UserMetadata)`. Matches
    /// the `check` field of the [`CheckResult`] the check produces.
    fn name(&self) -> String;

    /// Run the check and report the outcome.
    fn run(&self) -> CheckResult;
}
