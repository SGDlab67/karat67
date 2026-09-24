//! Adapter for running karat integrity checks inside a Carbon-style processor.
use crate::checks::shape::check_shape;
use crate::checks::specs;
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
}
