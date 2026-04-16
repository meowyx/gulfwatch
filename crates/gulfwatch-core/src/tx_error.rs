use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransactionError {
    pub instruction_index: Option<usize>,
    pub kind: String,
    pub custom_code: Option<u32>,
    pub raw: String,
}
