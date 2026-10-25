//! Running karat integrity checks inside a Carbon pipeline.
//!
//! [`KaratIntegrityProcessor`] has no Carbon crate dependency: any decode path
//! calls it with bytes and, when it has one, an
//! [`AccountFetcher`](crate::fetch::AccountFetcher).
//!
//! With the `carbon` feature, `KaratDecoder` wraps a real Carbon 2
//! `AccountDecoder`, so a pipeline gets the shape gate by changing one line:
//! `.account(KaratDecoder::new(PROGRAM_ID, decoder), processor)`.
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

#[cfg(feature = "carbon")]
pub use pipeline::KaratDecoder;

/// The backfill list for a Carbon disconnection event.
///
/// One call on the receiving end of a datasource's `disconnect_notifier`
/// channel: hand it the event and a [`SlotCoverageFetcher`]
/// ([`RpcAccountFetcher`](crate::fetch::RpcAccountFetcher) in production,
/// [`MockFetcher`](crate::fetch::MockFetcher) in tests) and get the produced
/// blocks in the closed range it names, both boundary slots included.
///
/// Carbon's own `missed_slots` field is a height delta: it counts leader
/// skip-slots, is one short of the closed range, and is sent with
/// `try_send`, so the event itself can be dropped (sevenlabs-hq/carbon#609).
/// This is the list those slots can actually be backfilled from.
///
/// [`SlotCoverageFetcher`]: crate::fetch::SlotCoverageFetcher
#[cfg(feature = "carbon")]
pub fn gaps_for_disconnection(
    disconnection: &carbon_core::datasource::DatasourceDisconnection,
    fetcher: &dyn crate::fetch::SlotCoverageFetcher,
) -> anyhow::Result<Vec<u64>> {
    crate::checks::completeness::gap_slots(
        disconnection.last_slot_before_disconnect,
        disconnection.first_slot_after_reconnect,
        fetcher,
    )
}

#[cfg(all(test, feature = "carbon"))]
mod disconnection_tests {
    use super::*;
    use crate::fetch::MockFetcher;
    use carbon_core::datasource::DatasourceDisconnection;

    #[test]
    fn carbon_disconnection_becomes_a_backfill_list() {
        let event = DatasourceDisconnection {
            source: "yellowstone-grpc".to_string(),
            disconnect_time: Default::default(),
            last_slot_before_disconnect: 99,
            first_slot_after_reconnect: 106,
            // Carbon's estimate for this window: 106 - 99.
            missed_slots: 7,
        };
        // The chain produced 99..=106 except 102 and 105, which were leader
        // skips. Both boundary blocks are in the list.
        let fetcher = MockFetcher::new(0).with_blocks([99, 100, 101, 103, 104, 106]);
        let gaps = gaps_for_disconnection(&event, &fetcher).expect("reachable");
        assert_eq!(gaps, vec![99, 100, 101, 103, 104, 106]);
        assert_eq!(event.missed_slots, 7);

        // Adjacent slots: Carbon reports 1 missed slot, but both boundary
        // blocks may be partly delivered, so both are re-fetched.
        let adjacent = DatasourceDisconnection {
            last_slot_before_disconnect: 100,
            first_slot_after_reconnect: 101,
            missed_slots: 1,
            ..event
        };
        let fetcher = MockFetcher::new(0).with_blocks([100, 101]);
        assert_eq!(
            gaps_for_disconnection(&adjacent, &fetcher).expect("reachable"),
            vec![100, 101]
        );
    }
}

#[cfg(feature = "carbon")]
mod pipeline {
    use crate::checks::shape::check_shape;
    use crate::checks::specs::find_by_discriminator;
    use crate::report::Status;
    use carbon_core::account::{AccountDecoder, DecodedAccount};
    use carbon_core::metrics::{Counter, MetricsRegistry};
    use solana_pubkey::Pubkey;
    use std::sync::Once;

    pub(super) static CHECKED: Counter = Counter::new(
        "karat_accounts_checked_total",
        "Decoded accounts whose bytes passed the karat shape check",
    );
    pub(super) static REJECTED: Counter = Counter::new(
        "karat_accounts_rejected_total",
        "Decoded accounts dropped because their bytes failed the karat shape check",
    );
    pub(super) static UNCHECKED: Counter = Counter::new(
        "karat_accounts_unchecked_total",
        "Decoded accounts passed through because karat has no spec for their type",
    );
    pub(super) static DECODE_FAILURES: Counter = Counter::new(
        "karat_decode_failures_total",
        "Accounts owned by the program that its decoder could not decode",
    );

    /// Wraps a Carbon `AccountDecoder` so only accounts with a verified shape
    /// reach the processor, and so the accounts Carbon drops get counted.
    ///
    /// Carbon reads a decoder's `None` as "not my account" and skips it with
    /// no log and no metric. For an account the program owns, `None` means the
    /// bytes did not decode: an empty or truncated payload disappears there.
    /// `karat_decode_failures_total` counts exactly those, separately from
    /// accounts that belong to some other program (carbon issue #591).
    ///
    /// A decoded account is shape-checked against the registry. Pass goes
    /// through. A registered type at the wrong length is dropped, because borsh
    /// decodes an over-long payload without complaint. A type karat has no spec
    /// for goes through and is counted as unchecked, never as checked.
    ///
    /// Counters register with Carbon's global `MetricsRegistry`, so the
    /// pipeline's existing log or Prometheus exporter reports them.
    // ponytail: no pubkey in the debug log, because Carbon hands a decoder the
    // account but not its metadata. Wrap the processor too if pubkeys matter.
    pub struct KaratDecoder<D> {
        program_id: Pubkey,
        inner: D,
    }

    impl<D> KaratDecoder<D> {
        pub fn new(program_id: Pubkey, inner: D) -> Self {
            static REGISTER: Once = Once::new();
            REGISTER.call_once(|| {
                let registry = MetricsRegistry::global();
                for counter in [&CHECKED, &REJECTED, &UNCHECKED, &DECODE_FAILURES] {
                    registry.register_counter(counter);
                }
            });
            Self { program_id, inner }
        }
    }

    impl<'a, D: AccountDecoder<'a>> AccountDecoder<'a> for KaratDecoder<D> {
        type AccountType = D::AccountType;

        fn decode_account(
            &self,
            account: &'a solana_account::Account,
        ) -> Option<DecodedAccount<Self::AccountType>> {
            let data = account.data.as_slice();
            let Some(decoded) = self.inner.decode_account(account) else {
                if account.owner == self.program_id {
                    DECODE_FAILURES.inc();
                    log::debug!(
                        "karat: {} owns a {}-byte account its decoder dropped: {}",
                        self.program_id,
                        data.len(),
                        check_shape(data).detail.unwrap_or_default()
                    );
                }
                return None;
            };

            let shape = check_shape(data);
            if shape.status == Status::Pass {
                CHECKED.inc();
                return Some(decoded);
            }
            let registered = data
                .get(..8)
                .and_then(|d| find_by_discriminator(d.try_into().ok()?))
                .is_some();
            if !registered && data.len() >= 8 {
                UNCHECKED.inc();
                return Some(decoded);
            }
            REJECTED.inc();
            log::debug!(
                "karat: dropped a decoded {}-byte account: {}",
                data.len(),
                shape.detail.unwrap_or_default()
            );
            None
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::checks::specs::find_by_account_type;

        /// Stand-in for a Carbon decoder: decodes everything, or nothing.
        struct Fixed(bool);

        impl<'a> AccountDecoder<'a> for Fixed {
            type AccountType = ();

            fn decode_account(
                &self,
                account: &'a solana_account::Account,
            ) -> Option<DecodedAccount<()>> {
                self.0.then_some(DecodedAccount {
                    lamports: account.lamports,
                    data: (),
                    owner: account.owner,
                    executable: account.executable,
                    rent_epoch: account.rent_epoch,
                })
            }
        }

        fn account(owner: Pubkey, data: Vec<u8>) -> solana_account::Account {
            solana_account::Account {
                lamports: 1,
                data,
                owner,
                executable: false,
                rent_epoch: 0,
            }
        }

        // One test, run in order: the counters are process-wide statics.
        #[test]
        fn gates_decoded_accounts_and_counts_owned_drops() {
            let program = Pubkey::new_from_array([7; 32]);
            let other = Pubkey::new_from_array([9; 32]);
            let spec = find_by_account_type("Obligation").unwrap();
            let mut good = spec.discriminator.to_vec();
            good.resize(spec.data_len, 0);
            let mut long = good.clone();
            long.extend([0; 16]);
            let unknown = vec![0xAB; 64];
            let counts = || [&CHECKED, &REJECTED, &UNCHECKED, &DECODE_FAILURES].map(|c| c.get());

            let decodes = KaratDecoder::new(program, Fixed(true));
            let drops = KaratDecoder::new(program, Fixed(false));
            let before = counts();

            assert!(decodes.decode_account(&account(program, good)).is_some());
            assert!(decodes.decode_account(&account(program, long)).is_none());
            assert!(decodes.decode_account(&account(program, unknown)).is_some());
            assert!(drops.decode_account(&account(program, vec![])).is_none());
            assert!(drops.decode_account(&account(other, vec![])).is_none());

            let after = counts();
            let delta: Vec<u64> = after.iter().zip(before).map(|(a, b)| a - b).collect();
            // checked, rejected, unchecked, decode failures. The account owned
            // by another program is "not mine" and counts nowhere.
            assert_eq!(delta, [1, 1, 1, 1]);
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
