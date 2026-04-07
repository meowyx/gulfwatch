use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct InstructionCount {
    pub instruction_type: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricSummary {
    pub program_id: String,
    pub window_minutes: u64,
    pub tx_count: u64,
    pub error_count: u64,
    pub error_rate: f64,
    pub avg_compute_units: f64,
    pub top_instructions: Vec<InstructionCount>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TimeBucket {
    pub timestamp: DateTime<Utc>,
    pub tx_count: u64,
    pub error_count: u64,
    pub error_rate: f64,
    pub avg_compute_units: f64,
}
