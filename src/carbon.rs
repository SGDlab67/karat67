//! Adapter for running karat integrity checks inside a Carbon-style processor.
//!
//! No Carbon crate dependency: a pipeline calls these methods with decoded
//! bytes and, when it has one, an [`AccountFetcher`](crate::fetch::AccountFetcher).
use crate::checks::shape::check_shape;
use crate::checks::specs;
use crate::fetch::AccountFetcher;
use crate::report::CheckResult;

pub struct KaratIntegrityProcessor;

impl KaratIntegrityProcessor {
    /// Run a shape check on decoded account bytes for `account_type`.
    pub fn check_account(account_type: &str, data: &[u8]) -> CheckResult {
        match specs::find_by_account_type(account_type) {
            None => CheckResult::fail(
                format!("shape({account_type})"),
                format!("unknown account type {account_type:?}"),
            ),
            Some(_) => check_shape(data),
        }
    }

    /// Run the account gate: shape `data`, then reconcile `account` only if
    /// shape passes. `fetcher` is not called when shape is not Pass.
    pub fn gate_account(
        account: &str,
        data: &[u8],
        indexed_slot: Option<u64>,
        max_slot_lag: u64,
        fetcher: &dyn AccountFetcher,
    ) -> Vec<CheckResult> {
        crate::gate::check_account(account, data, indexed_slot, max_slot_lag, Some(fetcher))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Status;

    #[test]
    fn empty_user_metadata_fails() {
        let r = KaratIntegrityProcessor::check_account("UserMetadata", &[]);
        assert_eq!(r.status, Status::Fail);
    }

    #[test]
    fn valid_user_metadata_passes() {
        let spec = specs::find_by_account_type("UserMetadata").unwrap();
        let mut data = spec.discriminator.to_vec();
        data.resize(spec.data_len, 0);
        let r = KaratIntegrityProcessor::check_account("UserMetadata", &data);
        assert_eq!(r.status, Status::Pass);
    }

    #[test]
    fn gate_account_empty_bytes_fail_without_fetch() {
        let results = KaratIntegrityProcessor::gate_account(
            "acct",
            &[],
            None,
            crate::checks::reconcile::DEFAULT_MAX_SLOT_LAG,
            &crate::fetch::MockFetcher::unreachable(),
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, Status::Fail);
    }
}
