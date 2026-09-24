//! The `Check` trait: a uniform seam every integrity check hangs off.
//!
//! Reconciliation and completeness reuse the same `CheckResult` contract as
//! shape, so the CLI and any future Carbon processor can run checks without
//! knowing which one they hold.
//!
//! The IDL-driven lock is: a new program (or account type) costs zero check
//! code. Add [`AccountSpec`](crate::checks::specs::AccountSpec) rows to the
//! registry in [`crate::checks::specs`]; [`crate::checks::shape::check_shape`]
//! and [`ShapeCheck`](crate::checks::shape::ShapeCheck) already consult that
//! table. The trait stays the uniform seam; the specs are the data.

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
