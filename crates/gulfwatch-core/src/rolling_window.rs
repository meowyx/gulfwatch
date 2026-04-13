use std::collections::{HashMap, VecDeque};

use chrono::{Duration, Utc};

use crate::metrics::{InstructionCount, MetricSummary, TimeBucket};
use crate::transaction::Transaction;

#[derive(Debug)]
pub struct RollingWindow {
    transactions: VecDeque<Transaction>,
    max_age: Duration,
}

impl RollingWindow {
    pub fn new(max_age_minutes: i64) -> Self {
        Self {
            transactions: VecDeque::new(),
            max_age: Duration::try_minutes(max_age_minutes).expect("invalid max age minutes"),
        }
    }

    pub fn push(&mut self, tx: Transaction) {
        self.evict();
        self.transactions.push_back(tx);
    }

    fn evict(&mut self) {
        let cutoff = Utc::now() - self.max_age;
        while let Some(front) = self.transactions.front() {
            if front.timestamp < cutoff {
                self.transactions.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn summary(&self, program_id: &str) -> MetricSummary {
        let tx_count = self.transactions.len() as u64;
        let error_count = self.transactions.iter().filter(|tx| !tx.success).count() as u64;

        let error_rate = if tx_count > 0 {
            error_count as f64 / tx_count as f64
        } else {
            0.0
        };

        let avg_compute_units = if tx_count > 0 {
            self.transactions
                .iter()
                .map(|tx| tx.compute_units as f64)
                .sum::<f64>()
                / tx_count as f64
        } else {
            0.0
        };

        let mut instruction_counts: HashMap<String, u64> = HashMap::new();
        for tx in &self.transactions {
            if let Some(ref instr) = tx.instruction_type {
                *instruction_counts.entry(instr.clone()).or_insert(0) += 1;
            }
        }

        let mut top_instructions: Vec<InstructionCount> = instruction_counts
            .into_iter()
            .map(|(instruction_type, count)| InstructionCount {
                instruction_type,
                count,
            })
            .collect();
        top_instructions.sort_by(|a, b| b.count.cmp(&a.count));

        MetricSummary {
            program_id: program_id.to_string(),
            window_minutes: self.max_age.num_minutes() as u64,
            tx_count,
            error_count,
            error_rate,
            avg_compute_units,
            top_instructions,
        }
    }

    /// Bucket transactions by time interval and compute per-bucket metrics.
    /// Returns buckets sorted oldest-first.
    pub fn timeseries(&self, bucket_secs: i64) -> Vec<TimeBucket> {
        if self.transactions.is_empty() {
            return vec![];
        }

        let mut buckets: HashMap<i64, Vec<&Transaction>> = HashMap::new();

        for tx in &self.transactions {
            let key = tx.timestamp.timestamp() / bucket_secs * bucket_secs;
            buckets.entry(key).or_default().push(tx);
        }

        let mut result: Vec<TimeBucket> = buckets
            .into_iter()
            .map(|(ts, txs)| {
                let tx_count = txs.len() as u64;
                let error_count = txs.iter().filter(|t| !t.success).count() as u64;
                let error_rate = if tx_count > 0 {
                    error_count as f64 / tx_count as f64
                } else {
                    0.0
                };
                let avg_compute_units = if tx_count > 0 {
                    txs.iter().map(|t| t.compute_units as f64).sum::<f64>() / tx_count as f64
                } else {
                    0.0
                };

                TimeBucket {
                    timestamp: chrono::DateTime::from_timestamp(ts, 0)
                        .unwrap_or_else(|| Utc::now()),
                    tx_count,
                    error_count,
                    error_rate,
                    avg_compute_units,
                }
            })
            .collect();

        result.sort_by_key(|b| b.timestamp);
        result
    }

    pub fn recent(&self, limit: usize) -> Vec<Transaction> {
        self.transactions
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};

    fn make_tx(
        success: bool,
        instruction_type: Option<&str>,
        timestamp: DateTime<Utc>,
    ) -> Transaction {
        Transaction {
            signature: "test_sig".to_string(),
            program_id: "test_program".to_string(),
            block_slot: 100,
            timestamp,
            success,
            instruction_type: instruction_type.map(|s| s.to_string()),
            accounts: vec!["acc1".to_string()],
            fee_lamports: 5000,
            compute_units: 200_000,
            instructions: vec![],
            cu_profile: None,
        }
    }

    #[test]
    fn summary_computes_correct_metrics() {
        let mut window = RollingWindow::new(10);
        let now = Utc::now();

        window.push(make_tx(true, Some("swap"), now));
        window.push(make_tx(true, Some("swap"), now));
        window.push(make_tx(false, Some("addLiquidity"), now));
        window.push(make_tx(true, None, now));

        let summary = window.summary("test_program");

        assert_eq!(summary.tx_count, 4);
        assert_eq!(summary.error_count, 1);
        assert!((summary.error_rate - 0.25).abs() < f64::EPSILON);
        assert!((summary.avg_compute_units - 200_000.0).abs() < f64::EPSILON);
        assert_eq!(summary.top_instructions[0].instruction_type, "swap");
        assert_eq!(summary.top_instructions[0].count, 2);
        assert_eq!(summary.top_instructions[1].instruction_type, "addLiquidity");
        assert_eq!(summary.top_instructions[1].count, 1);
    }

    #[test]
    fn eviction_removes_old_entries() {
        let mut window = RollingWindow::new(5);
        let now = Utc::now();
        let ten_minutes_ago = now - Duration::try_minutes(10).expect("valid duration");

        window.push(make_tx(true, Some("swap"), ten_minutes_ago));
        window.push(make_tx(true, Some("swap"), now));

        let summary = window.summary("test_program");
        assert_eq!(summary.tx_count, 1);
    }

    #[test]
    fn recent_returns_newest_first() {
        let mut window = RollingWindow::new(10);
        let now = Utc::now();

        for i in 0..5 {
            let mut tx = make_tx(true, Some("swap"), now);
            tx.signature = format!("sig_{}", i);
            window.push(tx);
        }

        let recent = window.recent(3);

        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].signature, "sig_4");
        assert_eq!(recent[1].signature, "sig_3");
        assert_eq!(recent[2].signature, "sig_2");
    }

    #[test]
    fn empty_window_returns_zero_metrics() {
        let window = RollingWindow::new(10);
        let summary = window.summary("test_program");

        assert_eq!(summary.tx_count, 0);
        assert_eq!(summary.error_count, 0);
        assert!((summary.error_rate - 0.0).abs() < f64::EPSILON);
        assert!((summary.avg_compute_units - 0.0).abs() < f64::EPSILON);
        assert!(summary.top_instructions.is_empty());
    }
}
