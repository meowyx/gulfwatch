use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Duration, Utc};

use crate::alert::AlertEvent;
use crate::detections::Detection;
use crate::transaction::{InstructionKind, Transaction};

const AUTHORITY_TYPE_PERMANENT_DELEGATE: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Suspicion {
    FailedThenSuccess,
    LargeTransfer,
    PermanentDelegate,
}

impl Suspicion {
    fn label(self) -> &'static str {
        match self {
            Suspicion::FailedThenSuccess => "failed_then_success",
            Suspicion::LargeTransfer => "large_transfer",
            Suspicion::PermanentDelegate => "permanent_delegate",
        }
    }
}

#[derive(Debug, Clone)]
struct Touch {
    program: String,
    at: DateTime<Utc>,
}

pub struct CrossProgramCorrelationDetection {
    min_programs: usize,
    window: Duration,
    large_transfer_threshold: u64,
    touches: HashMap<String, HashMap<Suspicion, VecDeque<Touch>>>,
    pending_failures: HashMap<(String, String), DateTime<Utc>>,
    last_fired: HashMap<(String, Suspicion), DateTime<Utc>>,
}

impl CrossProgramCorrelationDetection {
    pub fn new(min_programs: usize, window_secs: u64, large_transfer_threshold: u64) -> Self {
        Self {
            min_programs,
            window: Duration::seconds(window_secs as i64),
            large_transfer_threshold,
            touches: HashMap::new(),
            pending_failures: HashMap::new(),
            last_fired: HashMap::new(),
        }
    }

    pub fn is_inert(&self) -> bool {
        self.min_programs <= 1
    }

    fn evict(&mut self, signer: &str, kind: Suspicion, now: DateTime<Utc>) {
        let cutoff = now - self.window;
        let mut empty = false;
        if let Some(by_kind) = self.touches.get_mut(signer) {
            if let Some(queue) = by_kind.get_mut(&kind) {
                while let Some(front) = queue.front() {
                    if front.at < cutoff {
                        queue.pop_front();
                    } else {
                        break;
                    }
                }
                empty = queue.is_empty();
            }
            if empty {
                by_kind.remove(&kind);
                if by_kind.is_empty() {
                    self.touches.remove(signer);
                }
            }
        }
    }

    fn record(&mut self, signer: &str, kind: Suspicion, program: &str, at: DateTime<Utc>) {
        let by_kind = self.touches.entry(signer.to_string()).or_default();
        let queue = by_kind.entry(kind).or_default();
        if let Some(existing) = queue.iter_mut().find(|t| t.program == program) {
            existing.at = at;
        } else {
            queue.push_back(Touch {
                program: program.to_string(),
                at,
            });
        }
    }

    fn distinct_program_count(&self, signer: &str, kind: Suspicion) -> usize {
        self.touches
            .get(signer)
            .and_then(|m| m.get(&kind))
            .map(|q| q.len())
            .unwrap_or(0)
    }

    fn classify(&mut self, tx: &Transaction, signer: &str) -> Vec<Suspicion> {
        let mut kinds = Vec::new();

        let key = (signer.to_string(), tx.program_id.clone());
        if !tx.success {
            self.pending_failures.insert(key, tx.timestamp);
        } else if let Some(failed_at) = self.pending_failures.remove(&key) {
            if tx.timestamp - failed_at <= self.window {
                kinds.push(Suspicion::FailedThenSuccess);
            }
        }

        let has_large_transfer = self.large_transfer_threshold != u64::MAX
            && tx.instructions.iter().any(|ix| match ix.kind {
                InstructionKind::TokenTransfer { amount }
                | InstructionKind::TokenTransferChecked { amount, .. } => {
                    amount >= self.large_transfer_threshold
                }
                _ => false,
            });
        if has_large_transfer {
            kinds.push(Suspicion::LargeTransfer);
        }

        let has_permanent_delegate = tx.instructions.iter().any(|ix| {
            matches!(
                ix.kind,
                InstructionKind::InitializePermanentDelegate
                    | InstructionKind::SetAuthority {
                        authority_type: AUTHORITY_TYPE_PERMANENT_DELEGATE
                    }
            )
        });
        if has_permanent_delegate {
            kinds.push(Suspicion::PermanentDelegate);
        }

        kinds
    }

    fn cooldown_ok(&self, signer: &str, kind: Suspicion, now: DateTime<Utc>) -> bool {
        match self.last_fired.get(&(signer.to_string(), kind)) {
            Some(&t) => now - t > self.window,
            None => true,
        }
    }

    fn evict_pending_failures(&mut self, now: DateTime<Utc>) {
        let cutoff = now - self.window;
        self.pending_failures.retain(|_, &mut at| at >= cutoff);
    }
}

impl Detection for CrossProgramCorrelationDetection {
    fn name(&self) -> &str {
        "cross_program_correlation"
    }

    fn evaluate(&mut self, tx: &Transaction) -> Option<AlertEvent> {
        if self.is_inert() {
            return None;
        }
        let signer = tx.accounts.first()?.clone();

        self.evict_pending_failures(tx.timestamp);

        let kinds = self.classify(tx, &signer);
        if kinds.is_empty() {
            return None;
        }

        for kind in &kinds {
            self.evict(&signer, *kind, tx.timestamp);
            self.record(&signer, *kind, &tx.program_id, tx.timestamp);
        }

        for kind in kinds {
            let count = self.distinct_program_count(&signer, kind);
            if count >= self.min_programs && self.cooldown_ok(&signer, kind, tx.timestamp) {
                self.last_fired
                    .insert((signer.clone(), kind), tx.timestamp);
                return Some(AlertEvent {
                    rule_id: format!(
                        "cross_program_correlation:{}:{}:{}",
                        kind.label(),
                        signer,
                        tx.signature
                    ),
                    rule_name: format!(
                        "Cross-program correlation: {} across {} programs",
                        kind.label(),
                        count
                    ),
                    program_id: tx.program_id.clone(),
                    metric: format!("cross_program_correlation:{}", kind.label()),
                    value: count as f64,
                    threshold: self.min_programs as f64,
                    fired_at: tx.timestamp,
                });
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::ParsedInstruction;

    fn ix(kind: InstructionKind) -> ParsedInstruction {
        ParsedInstruction {
            program_id: "any".to_string(),
            kind,
            accounts: vec![],
            discriminator: None,
            anchor_name: None,
        }
    }

    fn tx(
        signer: &str,
        program: &str,
        success: bool,
        instructions: Vec<ParsedInstruction>,
        at: DateTime<Utc>,
    ) -> Transaction {
        Transaction {
            signature: format!("sig_{}_{}_{}", signer, program, at.timestamp_millis()),
            program_id: program.to_string(),
            block_slot: 100,
            timestamp: at,
            success,
            instruction_type: Transaction::derive_instruction_type(&instructions),
            accounts: vec![signer.to_string()],
            fee_lamports: 5_000,
            compute_units: 200_000,
            instructions,
            cu_profile: None,
            classification: None,
            classification_debug: None,
            logs: vec![],
            balance_diff: None,
            tx_error: None,
        }
    }

    fn transfer_ix(amount: u64) -> ParsedInstruction {
        ix(InstructionKind::TokenTransfer { amount })
    }

    fn permanent_delegate_ix() -> ParsedInstruction {
        ix(InstructionKind::InitializePermanentDelegate)
    }

    fn benign_swap_ix() -> ParsedInstruction {
        ix(InstructionKind::Other {
            name: "swap".to_string(),
        })
    }

    #[test]
    fn fires_on_failed_then_success_across_n_programs() {
        let mut det = CrossProgramCorrelationDetection::new(3, 300, 1_000);
        let now = Utc::now();
        let programs = ["RAY", "JUP", "ORCA"];

        for (i, p) in programs.iter().enumerate() {
            let t = now + Duration::seconds(i as i64);
            assert!(
                det.evaluate(&tx("attacker", p, false, vec![], t)).is_none(),
                "failure should not fire"
            );
        }

        for (i, p) in programs[..2].iter().enumerate() {
            let t = now + Duration::seconds(10 + i as i64);
            assert!(
                det.evaluate(&tx("attacker", p, true, vec![], t)).is_none(),
                "below min_programs should not fire"
            );
        }

        let t = now + Duration::seconds(12);
        let event = det
            .evaluate(&tx("attacker", "ORCA", true, vec![], t))
            .expect("should fire on third program");
        assert_eq!(event.value, 3.0);
        assert_eq!(event.threshold, 3.0);
        assert!(event.metric.contains("failed_then_success"));
        assert!(event.rule_id.contains("attacker"));
    }

    #[test]
    fn fires_on_large_transfers_across_n_programs() {
        let mut det = CrossProgramCorrelationDetection::new(3, 300, 1_000);
        let now = Utc::now();

        for (i, p) in ["RAY", "JUP", "ORCA"].iter().enumerate() {
            let t = now + Duration::seconds(i as i64);
            let result = det.evaluate(&tx("drainer", p, true, vec![transfer_ix(5_000)], t));
            if i < 2 {
                assert!(result.is_none());
            } else {
                let event = result.expect("third program should fire");
                assert_eq!(event.value, 3.0);
                assert!(event.metric.contains("large_transfer"));
            }
        }
    }

    #[test]
    fn fires_on_permanent_delegate_across_n_programs() {
        let mut det = CrossProgramCorrelationDetection::new(2, 300, u64::MAX);
        let now = Utc::now();

        let r1 = det.evaluate(&tx(
            "issuer",
            "MINT_A",
            true,
            vec![permanent_delegate_ix()],
            now,
        ));
        assert!(r1.is_none());

        let r2 = det.evaluate(&tx(
            "issuer",
            "MINT_B",
            true,
            vec![permanent_delegate_ix()],
            now + Duration::seconds(1),
        ));
        let event = r2.expect("two programs should fire");
        assert_eq!(event.value, 2.0);
        assert!(event.metric.contains("permanent_delegate"));
    }

    #[test]
    fn does_not_fire_for_single_program_activity() {
        let mut det = CrossProgramCorrelationDetection::new(3, 300, 1_000);
        let now = Utc::now();

        for i in 0..10 {
            let t = now + Duration::seconds(i);
            assert!(
                det.evaluate(&tx("user", "RAY", true, vec![transfer_ix(5_000)], t))
                    .is_none()
            );
        }
    }

    #[test]
    fn does_not_fire_for_benign_n_program_activity() {
        let mut det = CrossProgramCorrelationDetection::new(3, 300, 1_000);
        let now = Utc::now();

        for (i, p) in ["RAY", "JUP", "ORCA", "METEORA"].iter().enumerate() {
            let t = now + Duration::seconds(i as i64);
            assert!(
                det.evaluate(&tx("normal_user", p, true, vec![benign_swap_ix()], t))
                    .is_none(),
                "benign cross-program activity should not fire"
            );
        }
    }

    #[test]
    fn evicts_touches_outside_window() {
        let mut det = CrossProgramCorrelationDetection::new(3, 10, 1_000);
        let now = Utc::now();

        det.evaluate(&tx("attacker", "RAY", true, vec![transfer_ix(5_000)], now));
        det.evaluate(&tx(
            "attacker",
            "JUP",
            true,
            vec![transfer_ix(5_000)],
            now + Duration::seconds(20),
        ));

        let r = det.evaluate(&tx(
            "attacker",
            "ORCA",
            true,
            vec![transfer_ix(5_000)],
            now + Duration::seconds(25),
        ));
        assert!(r.is_none(), "evicted touches should not count");
    }

    #[test]
    fn does_not_refire_within_cooldown_window() {
        let mut det = CrossProgramCorrelationDetection::new(3, 60, 1_000);
        let now = Utc::now();

        for (i, p) in ["RAY", "JUP", "ORCA"].iter().enumerate() {
            let t = now + Duration::seconds(i as i64);
            det.evaluate(&tx("attacker", p, true, vec![transfer_ix(5_000)], t));
        }
        let r = det.evaluate(&tx(
            "attacker",
            "METEORA",
            true,
            vec![transfer_ix(5_000)],
            now + Duration::seconds(5),
        ));
        assert!(r.is_none(), "within cooldown should not re-fire");
    }

    #[test]
    fn refires_after_cooldown_elapses() {
        let mut det = CrossProgramCorrelationDetection::new(2, 5, 1_000);
        let now = Utc::now();

        det.evaluate(&tx("attacker", "RAY", true, vec![transfer_ix(5_000)], now));
        let first = det.evaluate(&tx(
            "attacker",
            "JUP",
            true,
            vec![transfer_ix(5_000)],
            now + Duration::seconds(1),
        ));
        assert!(first.is_some());

        let later = now + Duration::seconds(20);
        det.evaluate(&tx("attacker", "RAY", true, vec![transfer_ix(5_000)], later));
        let second = det.evaluate(&tx(
            "attacker",
            "ORCA",
            true,
            vec![transfer_ix(5_000)],
            later + Duration::seconds(1),
        ));
        assert!(second.is_some(), "fresh cluster after cooldown should fire");
    }

    #[test]
    fn duplicate_program_does_not_advance_count() {
        let mut det = CrossProgramCorrelationDetection::new(3, 300, 1_000);
        let now = Utc::now();

        for (i, p) in ["RAY", "RAY", "JUP", "RAY", "JUP"].iter().enumerate() {
            let t = now + Duration::seconds(i as i64);
            assert!(
                det.evaluate(&tx("attacker", p, true, vec![transfer_ix(5_000)], t))
                    .is_none(),
                "two distinct programs should never satisfy min=3"
            );
        }
    }

    #[test]
    fn separates_signers() {
        let mut det = CrossProgramCorrelationDetection::new(3, 300, 1_000);
        let now = Utc::now();

        for (i, p) in ["RAY", "JUP", "ORCA"].iter().enumerate() {
            let signer = format!("user_{}", i);
            let t = now + Duration::seconds(i as i64);
            assert!(
                det.evaluate(&tx(&signer, p, true, vec![transfer_ix(5_000)], t))
                    .is_none(),
                "different signers should never aggregate"
            );
        }
    }

    #[test]
    fn inert_when_min_programs_is_one() {
        let mut det = CrossProgramCorrelationDetection::new(1, 300, 1_000);
        assert!(det.is_inert());
        let r = det.evaluate(&tx(
            "attacker",
            "RAY",
            true,
            vec![transfer_ix(5_000)],
            Utc::now(),
        ));
        assert!(r.is_none());
    }

    #[test]
    fn large_transfer_inert_when_threshold_is_max() {
        let mut det = CrossProgramCorrelationDetection::new(2, 300, u64::MAX);
        let now = Utc::now();
        det.evaluate(&tx(
            "user",
            "RAY",
            true,
            vec![transfer_ix(u64::MAX - 1)],
            now,
        ));
        let r = det.evaluate(&tx(
            "user",
            "JUP",
            true,
            vec![transfer_ix(u64::MAX - 1)],
            now + Duration::seconds(1),
        ));
        assert!(r.is_none(), "large-transfer suspicion is gated by threshold");
    }

    #[test]
    fn permanent_delegate_via_set_authority_8_counts() {
        let mut det = CrossProgramCorrelationDetection::new(2, 300, u64::MAX);
        let now = Utc::now();
        det.evaluate(&tx(
            "issuer",
            "MINT_A",
            true,
            vec![ix(InstructionKind::SetAuthority {
                authority_type: AUTHORITY_TYPE_PERMANENT_DELEGATE,
            })],
            now,
        ));
        let r = det.evaluate(&tx(
            "issuer",
            "MINT_B",
            true,
            vec![ix(InstructionKind::SetAuthority {
                authority_type: AUTHORITY_TYPE_PERMANENT_DELEGATE,
            })],
            now + Duration::seconds(1),
        ));
        assert!(r.is_some());
    }

    #[test]
    fn no_signer_does_not_panic() {
        let mut det = CrossProgramCorrelationDetection::new(2, 300, 1_000);
        let mut t = tx("a", "RAY", true, vec![transfer_ix(5_000)], Utc::now());
        t.accounts.clear();
        assert!(det.evaluate(&t).is_none());
    }

    #[test]
    fn name_is_stable() {
        let det = CrossProgramCorrelationDetection::new(3, 300, 1_000);
        assert_eq!(det.name(), "cross_program_correlation");
    }
}
