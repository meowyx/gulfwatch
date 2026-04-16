//! Tracks per-signer failures in a sliding window. Fires when a signer who
//! has produced at least `failure_threshold` failures within `window_secs`
//! produces a successful transaction — the classic attacker probe-then-land
//! pattern. Signer is `tx.accounts[0]` (Solana convention: fee payer).

use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Duration, Utc};

use crate::alert::AlertEvent;
use crate::detections::Detection;
use crate::transaction::Transaction;

pub struct FailedTxClusterDetection {
    failure_threshold: usize,
    window: Duration,
    failures: HashMap<String, VecDeque<DateTime<Utc>>>,
}

impl FailedTxClusterDetection {
    pub fn new(failure_threshold: usize, window_secs: u64) -> Self {
        Self {
            failure_threshold,
            window: Duration::seconds(window_secs as i64),
            failures: HashMap::new(),
        }
    }

    fn evict_old(&mut self, signer: &str, tx_time: DateTime<Utc>) {
        let cutoff = tx_time - self.window;
        let mut should_remove = false;
        if let Some(queue) = self.failures.get_mut(signer) {
            while let Some(&front) = queue.front() {
                if front < cutoff {
                    queue.pop_front();
                } else {
                    break;
                }
            }
            if queue.is_empty() {
                should_remove = true;
            }
        }
        if should_remove {
            self.failures.remove(signer);
        }
    }
}

impl Default for FailedTxClusterDetection {
    fn default() -> Self {
        Self::new(10, 60)
    }
}

impl Detection for FailedTxClusterDetection {
    fn name(&self) -> &str {
        "failed_tx_cluster"
    }

    fn evaluate(&mut self, tx: &Transaction) -> Option<AlertEvent> {
        let signer = tx.accounts.first()?.clone();

        self.evict_old(&signer, tx.timestamp);

        if !tx.success {
            self.failures
                .entry(signer)
                .or_insert_with(VecDeque::new)
                .push_back(tx.timestamp);
            return None;
        }

        let failure_count = self.failures.get(&signer).map(|q| q.len()).unwrap_or(0);

        if failure_count >= self.failure_threshold {
            // Clear so the next routine success from this signer doesn't re-fire.
            self.failures.remove(&signer);

            return Some(AlertEvent {
                rule_id: format!("failed_tx_cluster:{}:{}", signer, tx.signature),
                rule_name: "Failed transaction cluster detected".to_string(),
                program_id: tx.program_id.clone(),
                metric: "failed_tx_cluster".to_string(),
                value: failure_count as f64,
                threshold: self.failure_threshold as f64,
                fired_at: tx.timestamp,
            });
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tx(signer: &str, success: bool, timestamp: DateTime<Utc>) -> Transaction {
        Transaction {
            signature: format!("sig_{}_{}", signer, timestamp.timestamp_millis()),
            program_id: "monitored_prog".to_string(),
            block_slot: 100,
            timestamp,
            success,
            instruction_type: None,
            accounts: vec![signer.to_string()],
            fee_lamports: 5000,
            compute_units: 200_000,
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
    fn fires_on_n_failures_then_success_within_window() {
        let mut det = FailedTxClusterDetection::new(3, 60);
        let now = Utc::now();

        for i in 0..3 {
            let tx = make_tx("attacker", false, now + Duration::seconds(i));
            assert!(det.evaluate(&tx).is_none(), "failures should not fire");
        }

        let success = make_tx("attacker", true, now + Duration::seconds(4));
        let event = det
            .evaluate(&success)
            .expect("success after cluster should fire");
        assert_eq!(event.metric, "failed_tx_cluster");
        assert_eq!(event.value, 3.0);
        assert_eq!(event.threshold, 3.0);
        assert_eq!(event.program_id, "monitored_prog");
        assert!(event.rule_id.contains("attacker"));
    }

    #[test]
    fn does_not_fire_below_threshold() {
        let mut det = FailedTxClusterDetection::new(5, 60);
        let now = Utc::now();

        for i in 0..3 {
            let tx = make_tx("user", false, now + Duration::seconds(i));
            assert!(det.evaluate(&tx).is_none());
        }
        let success = make_tx("user", true, now + Duration::seconds(4));
        assert!(det.evaluate(&success).is_none());
    }

    #[test]
    fn does_not_fire_when_failures_from_different_signers() {
        let mut det = FailedTxClusterDetection::new(3, 60);
        let now = Utc::now();

        for i in 0..3 {
            let signer = format!("user_{}", i);
            let tx = make_tx(&signer, false, now + Duration::seconds(i));
            assert!(det.evaluate(&tx).is_none());
        }

        let success = make_tx("user_99", true, now + Duration::seconds(4));
        assert!(det.evaluate(&success).is_none());
    }

    #[test]
    fn failures_outside_window_are_evicted() {
        // Failures spaced wider than the window — only the most recent one
        // is still inside the window when the success arrives, so count = 1.
        let mut det = FailedTxClusterDetection::new(3, 10);
        let now = Utc::now();

        det.evaluate(&make_tx("attacker", false, now));
        det.evaluate(&make_tx("attacker", false, now + Duration::seconds(20)));
        det.evaluate(&make_tx("attacker", false, now + Duration::seconds(40)));

        let success = make_tx("attacker", true, now + Duration::seconds(50));
        assert!(det.evaluate(&success).is_none());
    }

    #[test]
    fn does_not_re_fire_immediately_after_first_fire() {
        let mut det = FailedTxClusterDetection::new(2, 60);
        let now = Utc::now();

        det.evaluate(&make_tx("attacker", false, now));
        det.evaluate(&make_tx("attacker", false, now + Duration::seconds(1)));

        let s1 = make_tx("attacker", true, now + Duration::seconds(2));
        assert!(det.evaluate(&s1).is_some());

        let s2 = make_tx("attacker", true, now + Duration::seconds(3));
        assert!(det.evaluate(&s2).is_none());
    }

    #[test]
    fn empty_signer_queues_are_removed_after_eviction() {
        let mut det = FailedTxClusterDetection::new(100, 1);
        let now = Utc::now();

        det.evaluate(&make_tx("user", false, now));
        assert_eq!(det.failures.len(), 1);

        let later = now + Duration::seconds(10);
        det.evaluate(&make_tx("user", true, later));

        assert_eq!(det.failures.len(), 0);
    }

    #[test]
    fn name_is_stable() {
        let det = FailedTxClusterDetection::default();
        assert_eq!(det.name(), "failed_tx_cluster");
    }

    #[test]
    fn no_signer_does_not_panic() {
        let mut det = FailedTxClusterDetection::default();
        let mut tx = make_tx("user", true, Utc::now());
        tx.accounts.clear();
        assert!(det.evaluate(&tx).is_none());
    }

    #[test]
    fn boundary_case_exact_threshold_fires() {
        let mut det = FailedTxClusterDetection::new(10, 60);
        let now = Utc::now();
        for i in 0..10 {
            det.evaluate(&make_tx("a", false, now + Duration::milliseconds(i * 10)));
        }
        let s = make_tx("a", true, now + Duration::seconds(1));
        let event = det.evaluate(&s).expect("exactly threshold should fire");
        assert_eq!(event.value, 10.0);
    }
}
