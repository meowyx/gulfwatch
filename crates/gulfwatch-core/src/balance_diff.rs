use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BalanceDiff {
    pub sol: Vec<SolDelta>,
    pub tokens: Vec<TokenDelta>,
}

impl BalanceDiff {
    pub fn is_empty(&self) -> bool {
        self.sol.is_empty() && self.tokens.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SolDelta {
    pub account: String,
    pub account_index: usize,
    pub pre_lamports: u64,
    pub post_lamports: u64,
    pub delta_lamports: i128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenDelta {
    pub account: String,
    pub account_index: usize,
    pub mint: String,
    pub owner: Option<String>,
    pub pre_amount: u64,
    pub post_amount: u64,
    pub delta: i128,
    pub decimals: u8,
}
