//! Security detection rules. The processing worker runs each registered
//! `Detection` against every monitored transaction.

pub mod authority_change;
pub mod correlation;
pub mod failed_tx_cluster;
pub mod large_transfer;
pub mod token2022;

pub use authority_change::AuthorityChangeDetection;
pub use correlation::CrossProgramCorrelationDetection;
pub use failed_tx_cluster::FailedTxClusterDetection;
pub use large_transfer::LargeTransferDetection;
pub use token2022::{
    DefaultAccountStateFrozenDetection, PermanentDelegateDetection, TransferFeeAuthorityChangeDetection,
    TransferHookUpgradeDetection,
};

use crate::alert::AlertEvent;
use crate::transaction::Transaction;

pub trait Detection: Send {
    fn name(&self) -> &str;
    fn evaluate(&mut self, tx: &Transaction) -> Option<AlertEvent>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    /// No-op detection — used purely to verify the trait shape compiles
    /// and that the worker can hold a `Vec<Box<dyn Detection>>`.
    struct NoopDetection;

    impl Detection for NoopDetection {
        fn name(&self) -> &str {
            "noop"
        }

        fn evaluate(&mut self, _tx: &Transaction) -> Option<AlertEvent> {
            None
        }
    }

    fn sample_tx() -> Transaction {
        Transaction {
            signature: "sig".to_string(),
            program_id: "prog".to_string(),
            block_slot: 1,
            timestamp: Utc::now(),
            success: true,
            instruction_type: None,
            accounts: vec![],
            fee_lamports: 0,
            compute_units: 0,
            instructions: vec![],
            cu_profile: None,
            classification: None,
            classification_debug: None,
            logs: vec![],
            balance_diff: None,
            tx_error: None,
        }
    }

    #[test]
    fn noop_detection_never_fires() {
        let mut det = NoopDetection;
        assert!(det.evaluate(&sample_tx()).is_none());
        assert_eq!(det.name(), "noop");
    }

    #[test]
    fn detections_are_object_safe() {
        // Confirms `Box<dyn Detection>` works — the worker stores them this way.
        let detections: Vec<Box<dyn Detection>> = vec![Box::new(NoopDetection)];
        assert_eq!(detections.len(), 1);
    }
}
