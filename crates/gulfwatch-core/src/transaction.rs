use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub signature: String,
    pub program_id: String,
    pub block_slot: u64,
    pub timestamp: DateTime<Utc>,
    pub success: bool,
    pub instruction_type: Option<String>,
    pub accounts: Vec<String>,
    pub fee_lamports: u64,
    pub compute_units: u64,
}
