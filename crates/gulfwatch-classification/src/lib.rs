use std::collections::HashSet;

use serde::{Deserialize, Serialize};
mod program_ids;
use program_ids::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransactionClassification {
    pub category: String,
    pub classifier: String,
    pub confidence: f32,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClassifierDecision {
    pub classifier: String,
    pub matched: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClassificationDebugTrace {
    pub focal_account: Option<String>,
    pub decisions: Vec<ClassifierDecision>,
    pub legs: Vec<TransferLeg>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LegDirection {
    Inflow,
    Outflow,
    Internal,
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransferLeg {
    pub instruction_index: usize,
    pub instruction_kind: String,
    pub source: String,
    pub destination: String,
    pub amount: u64,
    pub decimals: Option<u8>,
    pub asset_hint: String,
    pub direction: LegDirection,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstructionInputKind {
    SetAuthority,
    Upgrade,
    SystemTransfer { lamports: u64 },
    TokenTransfer { amount: u64 },
    TokenTransferChecked { amount: u64, decimals: u8 },
    StakeDelegate,
    StakeWithdraw,
    Other { name: String },
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstructionInput {
    pub program_id: String,
    pub kind: InstructionInputKind,
    pub accounts: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct ClassificationContext<'a> {
    pub instruction_type: Option<&'a str>,
    pub success: bool,
    pub compute_units: u64,
    pub fee_lamports: u64,
    pub accounts: &'a [String],
    pub instructions: &'a [InstructionInput],
}

pub struct ClassificationOutcome {
    pub classification: TransactionClassification,
    pub debug_trace: ClassificationDebugTrace,
}

#[derive(Debug, Clone)]
struct DerivedFeatures {
    focal_account: Option<String>,
    legs: Vec<TransferLeg>,
    instruction_names: HashSet<String>,
    has_swap_hint: bool,
    has_memo_program: bool,
    has_bridge_program: bool,
    has_privacy_program: bool,
    has_nft_program: bool,
    has_nft_marketplace_program: bool,
    has_stake_program: bool,
    has_stake_delegate_instruction: bool,
    has_stake_withdraw_instruction: bool,
}

#[derive(Debug, Clone)]
struct MatchResult {
    classification: TransactionClassification,
    reason: String,
}

trait Classifier {
    fn name(&self) -> &'static str;
    fn priority(&self) -> u16;
    fn classify(
        &self,
        ctx: &ClassificationContext<'_>,
        derived: &DerivedFeatures,
    ) -> Result<MatchResult, String>;
}

struct AuthorityChangeClassifier;
struct FailedTxClassifier;
struct SolanaPayClassifier;
struct BridgeClassifier;
struct PrivacyCashClassifier;
struct NftMintClassifier;
struct NftTransferClassifier;
struct StakeDepositClassifier;
struct StakeWithdrawClassifier;
struct SwapClassifier;
struct AirdropClassifier;
struct FeeOnlyClassifier;
struct TransferClassifier;
struct FallbackClassifier;

impl Classifier for AuthorityChangeClassifier {
    fn name(&self) -> &'static str {
        "authority-change"
    }

    fn priority(&self) -> u16 {
        100
    }

    fn classify(
        &self,
        _ctx: &ClassificationContext<'_>,
        derived: &DerivedFeatures,
    ) -> Result<MatchResult, String> {
        if derived.instruction_names.contains("setauthority")
            || derived.instruction_names.contains("upgrade")
        {
            return Ok(MatchResult {
                classification: TransactionClassification {
                    category: "security_authority_change".to_string(),
                    classifier: self.name().to_string(),
                    confidence: 0.99,
                    summary: "Authority operation detected".to_string(),
                },
                reason: "Found setAuthority or upgrade instruction".to_string(),
            });
        }

        Err("No authority-change instruction found".to_string())
    }
}

impl Classifier for FailedTxClassifier {
    fn name(&self) -> &'static str {
        "failed-transaction"
    }

    fn priority(&self) -> u16 {
        99
    }

    fn classify(
        &self,
        ctx: &ClassificationContext<'_>,
        _derived: &DerivedFeatures,
    ) -> Result<MatchResult, String> {
        if !ctx.success {
            return Ok(MatchResult {
                classification: TransactionClassification {
                    category: "failed_transaction".to_string(),
                    classifier: self.name().to_string(),
                    confidence: 0.92,
                    summary: "Transaction execution failed".to_string(),
                },
                reason: "Transaction success flag is false".to_string(),
            });
        }

        Err("Transaction succeeded".to_string())
    }
}

impl Classifier for SolanaPayClassifier {
    fn name(&self) -> &'static str {
        "solana-pay"
    }

    fn priority(&self) -> u16 {
        95
    }

    fn classify(
        &self,
        _ctx: &ClassificationContext<'_>,
        derived: &DerivedFeatures,
    ) -> Result<MatchResult, String> {
        if !derived.has_memo_program {
            return Err("No memo program in transaction".to_string());
        }

        if derived.legs.is_empty() {
            return Err("Memo found but no transfer legs".to_string());
        }

        let inflow = sum_legs(&derived.legs, |leg| leg.direction == LegDirection::Inflow);
        let outflow = sum_legs(&derived.legs, |leg| leg.direction == LegDirection::Outflow);

        if inflow == 0 && outflow == 0 {
            return Err("Memo found but no focal account in/out flow".to_string());
        }

        let summary = if outflow >= inflow {
            "Solana Pay transfer with outbound flow"
        } else {
            "Solana Pay transfer with inbound flow"
        };

        Ok(MatchResult {
            classification: TransactionClassification {
                category: "solana_pay".to_string(),
                classifier: self.name().to_string(),
                confidence: 0.93,
                summary: summary.to_string(),
            },
            reason: "Memo program plus transfer legs".to_string(),
        })
    }
}

impl Classifier for BridgeClassifier {
    fn name(&self) -> &'static str {
        "bridge"
    }

    fn priority(&self) -> u16 {
        88
    }

    fn classify(
        &self,
        _ctx: &ClassificationContext<'_>,
        derived: &DerivedFeatures,
    ) -> Result<MatchResult, String> {
        if !derived.has_bridge_program {
            return Err("No bridge program detected".to_string());
        }

        let inflow = sum_legs(&derived.legs, |leg| leg.direction == LegDirection::Inflow);
        let outflow = sum_legs(&derived.legs, |leg| leg.direction == LegDirection::Outflow);

        if inflow == 0 && outflow == 0 {
            return Err("Bridge program detected but no directional flow".to_string());
        }

        if inflow == outflow {
            return Err("Bridge in/out flow is balanced and ambiguous".to_string());
        }

        let (category, reason) = if inflow > outflow {
            (
                "bridge_in",
                "Bridge program with dominant inbound transfer flow",
            )
        } else {
            (
                "bridge_out",
                "Bridge program with dominant outbound transfer flow",
            )
        };

        Ok(MatchResult {
            classification: TransactionClassification {
                category: category.to_string(),
                classifier: self.name().to_string(),
                confidence: 0.92,
                summary: reason.to_string(),
            },
            reason: reason.to_string(),
        })
    }
}

impl Classifier for PrivacyCashClassifier {
    fn name(&self) -> &'static str {
        "privacy-cash"
    }

    fn priority(&self) -> u16 {
        86
    }

    fn classify(
        &self,
        _ctx: &ClassificationContext<'_>,
        derived: &DerivedFeatures,
    ) -> Result<MatchResult, String> {
        if !derived.has_privacy_program {
            return Err("No privacy protocol program detected".to_string());
        }

        let inflow = sum_legs(&derived.legs, |leg| leg.direction == LegDirection::Inflow);
        let outflow = sum_legs(&derived.legs, |leg| leg.direction == LegDirection::Outflow);

        if inflow == 0 && outflow == 0 {
            return Err("Privacy program detected but no directional flow".to_string());
        }

        let (category, reason) = if outflow >= inflow {
            (
                "privacy_deposit",
                "Privacy program with dominant outbound transfer flow",
            )
        } else {
            (
                "privacy_withdraw",
                "Privacy program with dominant inbound transfer flow",
            )
        };

        Ok(MatchResult {
            classification: TransactionClassification {
                category: category.to_string(),
                classifier: self.name().to_string(),
                confidence: 0.9,
                summary: reason.to_string(),
            },
            reason: reason.to_string(),
        })
    }
}

impl Classifier for NftMintClassifier {
    fn name(&self) -> &'static str {
        "nft-mint"
    }

    fn priority(&self) -> u16 {
        85
    }

    fn classify(
        &self,
        _ctx: &ClassificationContext<'_>,
        derived: &DerivedFeatures,
    ) -> Result<MatchResult, String> {
        if !derived.has_nft_program {
            return Err("No NFT protocol program detected".to_string());
        }

        let nft_in = count_legs(&derived.legs, |leg| {
            leg.direction == LegDirection::Inflow && is_nft_like_leg(leg)
        });
        let nft_out = count_legs(&derived.legs, |leg| {
            leg.direction == LegDirection::Outflow && is_nft_like_leg(leg)
        });

        if nft_in == 0 {
            return Err("No inbound NFT-like legs".to_string());
        }

        if nft_out > 0 {
            return Err("Has outbound NFT-like legs; likely transfer not mint".to_string());
        }

        Ok(MatchResult {
            classification: TransactionClassification {
                category: "nft_mint".to_string(),
                classifier: self.name().to_string(),
                confidence: 0.86,
                summary: "NFT protocol with inbound NFT-like leg".to_string(),
            },
            reason: "Detected inbound NFT-like transfer on NFT program".to_string(),
        })
    }
}

impl Classifier for NftTransferClassifier {
    fn name(&self) -> &'static str {
        "nft-transfer"
    }

    fn priority(&self) -> u16 {
        84
    }

    fn classify(
        &self,
        _ctx: &ClassificationContext<'_>,
        derived: &DerivedFeatures,
    ) -> Result<MatchResult, String> {
        let has_nft_signal = derived.has_nft_program
            || derived.has_nft_marketplace_program
            || derived.legs.iter().any(is_nft_like_leg);
        if !has_nft_signal {
            return Err("No NFT signal in programs or legs".to_string());
        }

        let nft_in = count_legs(&derived.legs, |leg| {
            leg.direction == LegDirection::Inflow && is_nft_like_leg(leg)
        });
        let nft_out = count_legs(&derived.legs, |leg| {
            leg.direction == LegDirection::Outflow && is_nft_like_leg(leg)
        });

        if nft_in == 0 && nft_out == 0 {
            return Err("NFT program detected but no NFT-like movement".to_string());
        }

        let fungible_in = count_legs(&derived.legs, |leg| {
            leg.direction == LegDirection::Inflow && !is_nft_like_leg(leg)
        });
        let fungible_out = count_legs(&derived.legs, |leg| {
            leg.direction == LegDirection::Outflow && !is_nft_like_leg(leg)
        });

        let (category, reason) = if nft_in > 0 && fungible_out > 0 {
            ("nft_purchase", "Inbound NFT with outbound fungible payment")
        } else if nft_out > 0 && fungible_in > 0 {
            ("nft_sale", "Outbound NFT with inbound fungible proceeds")
        } else if nft_in > 0 {
            ("nft_receive", "Inbound NFT without payment leg")
        } else {
            ("nft_send", "Outbound NFT without proceeds leg")
        };

        Ok(MatchResult {
            classification: TransactionClassification {
                category: category.to_string(),
                classifier: self.name().to_string(),
                confidence: 0.83,
                summary: reason.to_string(),
            },
            reason: reason.to_string(),
        })
    }
}

impl Classifier for StakeDepositClassifier {
    fn name(&self) -> &'static str {
        "stake-deposit"
    }

    fn priority(&self) -> u16 {
        82
    }

    fn classify(
        &self,
        _ctx: &ClassificationContext<'_>,
        derived: &DerivedFeatures,
    ) -> Result<MatchResult, String> {
        if !derived.has_stake_program {
            return Err("No stake program detected".to_string());
        }
        if !derived.has_stake_delegate_instruction {
            return Err("No stake delegate instruction".to_string());
        }

        let sol_out = sum_legs(&derived.legs, |leg| {
            leg.direction == LegDirection::Outflow && is_sol_leg(leg)
        });
        if sol_out == 0 {
            return Err("Stake delegate without outbound SOL".to_string());
        }

        Ok(MatchResult {
            classification: TransactionClassification {
                category: "stake_deposit".to_string(),
                classifier: self.name().to_string(),
                confidence: 0.89,
                summary: "Stake delegation with outbound SOL flow".to_string(),
            },
            reason: "Stake delegate plus SOL outflow".to_string(),
        })
    }
}

impl Classifier for StakeWithdrawClassifier {
    fn name(&self) -> &'static str {
        "stake-withdraw"
    }

    fn priority(&self) -> u16 {
        81
    }

    fn classify(
        &self,
        _ctx: &ClassificationContext<'_>,
        derived: &DerivedFeatures,
    ) -> Result<MatchResult, String> {
        if !derived.has_stake_program {
            return Err("No stake program detected".to_string());
        }
        if !derived.has_stake_withdraw_instruction {
            return Err("No stake withdraw instruction".to_string());
        }

        let sol_in = sum_legs(&derived.legs, |leg| {
            leg.direction == LegDirection::Inflow && is_sol_leg(leg)
        });
        if sol_in == 0 {
            return Err("Stake withdraw without inbound SOL".to_string());
        }

        Ok(MatchResult {
            classification: TransactionClassification {
                category: "stake_withdraw".to_string(),
                classifier: self.name().to_string(),
                confidence: 0.89,
                summary: "Stake withdrawal with inbound SOL flow".to_string(),
            },
            reason: "Stake withdraw plus SOL inflow".to_string(),
        })
    }
}

impl Classifier for SwapClassifier {
    fn name(&self) -> &'static str {
        "swap"
    }

    fn priority(&self) -> u16 {
        80
    }

    fn classify(
        &self,
        _ctx: &ClassificationContext<'_>,
        derived: &DerivedFeatures,
    ) -> Result<MatchResult, String> {
        if !derived.has_swap_hint {
            return Err("No swap-like protocol or instruction hint".to_string());
        }

        if derived.legs.len() < 2 {
            return Err("Swap hint present but fewer than 2 transfer legs".to_string());
        }

        let has_inflow = derived
            .legs
            .iter()
            .any(|leg| leg.direction == LegDirection::Inflow);
        let has_outflow = derived
            .legs
            .iter()
            .any(|leg| leg.direction == LegDirection::Outflow);

        if has_inflow && has_outflow {
            return Ok(MatchResult {
                classification: TransactionClassification {
                    category: "defi_swap".to_string(),
                    classifier: self.name().to_string(),
                    confidence: 0.95,
                    summary: "Swap-like transaction detected from bidirectional legs".to_string(),
                },
                reason: "Swap hint plus both inflow and outflow legs".to_string(),
            });
        }

        if derived.focal_account.is_none() {
            return Ok(MatchResult {
                classification: TransactionClassification {
                    category: "defi_swap".to_string(),
                    classifier: self.name().to_string(),
                    confidence: 0.82,
                    summary: "Swap-like transaction detected from protocol legs".to_string(),
                },
                reason: "Swap hint plus transfer legs without focal account".to_string(),
            });
        }

        Err("Swap hint present but leg flow is not bidirectional".to_string())
    }
}

impl Classifier for AirdropClassifier {
    fn name(&self) -> &'static str {
        "airdrop"
    }

    fn priority(&self) -> u16 {
        70
    }

    fn classify(
        &self,
        _ctx: &ClassificationContext<'_>,
        derived: &DerivedFeatures,
    ) -> Result<MatchResult, String> {
        if derived.has_bridge_program || derived.has_privacy_program || derived.has_stake_program {
            return Err("Protocol-specific flow should not be treated as airdrop".to_string());
        }

        let token_in = sum_legs(&derived.legs, |leg| {
            leg.direction == LegDirection::Inflow && !is_sol_leg(leg)
        });
        let token_out = sum_legs(&derived.legs, |leg| {
            leg.direction == LegDirection::Outflow && !is_sol_leg(leg)
        });

        if token_in > 0 && token_out == 0 {
            return Ok(MatchResult {
                classification: TransactionClassification {
                    category: "airdrop".to_string(),
                    classifier: self.name().to_string(),
                    confidence: 0.8,
                    summary: "Inbound token transfer without outbound token leg".to_string(),
                },
                reason: "Token inflow without corresponding token outflow".to_string(),
            });
        }

        Err("No airdrop-like one-way token inflow".to_string())
    }
}

impl Classifier for FeeOnlyClassifier {
    fn name(&self) -> &'static str {
        "fee-only"
    }

    fn priority(&self) -> u16 {
        60
    }

    fn classify(
        &self,
        ctx: &ClassificationContext<'_>,
        derived: &DerivedFeatures,
    ) -> Result<MatchResult, String> {
        if ctx.fee_lamports > 0 && derived.legs.is_empty() {
            return Ok(MatchResult {
                classification: TransactionClassification {
                    category: "fee_only".to_string(),
                    classifier: self.name().to_string(),
                    confidence: 0.95,
                    summary: "Fee charged without transfer legs".to_string(),
                },
                reason: "No transfer legs and non-zero fee".to_string(),
            });
        }

        Err("Transfer legs present or zero fee".to_string())
    }
}

impl Classifier for TransferClassifier {
    fn name(&self) -> &'static str {
        "transfer"
    }

    fn priority(&self) -> u16 {
        20
    }

    fn classify(
        &self,
        _ctx: &ClassificationContext<'_>,
        derived: &DerivedFeatures,
    ) -> Result<MatchResult, String> {
        if derived.legs.is_empty() {
            return Err("No transfer legs".to_string());
        }

        if derived.has_swap_hint || derived.has_bridge_program || derived.has_privacy_program {
            return Err("Transfer legs likely belong to specialized protocol flow".to_string());
        }

        if derived
            .legs
            .iter()
            .any(|leg| is_nft_like_leg(leg) && leg.direction != LegDirection::Internal)
        {
            return Err("NFT-like movement should be handled by NFT classifiers".to_string());
        }

        if derived.legs.len() == 1 {
            return Ok(MatchResult {
                classification: TransactionClassification {
                    category: "token_transfer".to_string(),
                    classifier: self.name().to_string(),
                    confidence: 0.92,
                    summary: "Single transfer leg detected".to_string(),
                },
                reason: "Exactly one transfer leg".to_string(),
            });
        }

        Ok(MatchResult {
            classification: TransactionClassification {
                category: "token_transfer_batch".to_string(),
                classifier: self.name().to_string(),
                confidence: 0.74,
                summary: "Multiple transfer legs without stronger protocol hint".to_string(),
            },
            reason: "Multiple transfer legs and no higher-priority match".to_string(),
        })
    }
}

impl Classifier for FallbackClassifier {
    fn name(&self) -> &'static str {
        "fallback"
    }

    fn priority(&self) -> u16 {
        0
    }

    fn classify(
        &self,
        ctx: &ClassificationContext<'_>,
        derived: &DerivedFeatures,
    ) -> Result<MatchResult, String> {
        Ok(MatchResult {
            classification: TransactionClassification {
                category: "other".to_string(),
                classifier: self.name().to_string(),
                confidence: 0.5,
                summary: format!(
                    "No specific classifier matched ({} instructions, {} CU, {} legs)",
                    ctx.instructions.len(),
                    ctx.compute_units,
                    derived.legs.len()
                ),
            },
            reason: "Fallback classifier".to_string(),
        })
    }
}

pub struct ClassificationService {
    classifiers: Vec<Box<dyn Classifier + Send + Sync>>,
}

impl ClassificationService {
    pub fn new() -> Self {
        let mut classifiers: Vec<Box<dyn Classifier + Send + Sync>> = vec![
            Box::new(AuthorityChangeClassifier),
            Box::new(FailedTxClassifier),
            Box::new(SolanaPayClassifier),
            Box::new(BridgeClassifier),
            Box::new(PrivacyCashClassifier),
            Box::new(NftMintClassifier),
            Box::new(NftTransferClassifier),
            Box::new(StakeDepositClassifier),
            Box::new(StakeWithdrawClassifier),
            Box::new(SwapClassifier),
            Box::new(AirdropClassifier),
            Box::new(FeeOnlyClassifier),
            Box::new(TransferClassifier),
            Box::new(FallbackClassifier),
        ];
        classifiers.sort_by_key(|c| std::cmp::Reverse(c.priority()));
        Self { classifiers }
    }

    pub fn classify(&self, ctx: &ClassificationContext<'_>) -> ClassificationOutcome {
        let derived = derive_features(ctx);
        let mut decisions = Vec::with_capacity(self.classifiers.len());

        for classifier in &self.classifiers {
            match classifier.classify(ctx, &derived) {
                Ok(matched) => {
                    decisions.push(ClassifierDecision {
                        classifier: classifier.name().to_string(),
                        matched: true,
                        reason: matched.reason,
                    });

                    return ClassificationOutcome {
                        classification: matched.classification,
                        debug_trace: ClassificationDebugTrace {
                            focal_account: derived.focal_account,
                            decisions,
                            legs: derived.legs,
                        },
                    };
                }
                Err(reason) => decisions.push(ClassifierDecision {
                    classifier: classifier.name().to_string(),
                    matched: false,
                    reason,
                }),
            }
        }

        ClassificationOutcome {
            classification: TransactionClassification {
                category: "other".to_string(),
                classifier: "none".to_string(),
                confidence: 0.0,
                summary: "No classifier evaluated".to_string(),
            },
            debug_trace: ClassificationDebugTrace {
                focal_account: derived.focal_account,
                decisions,
                legs: derived.legs,
            },
        }
    }
}

impl Default for ClassificationService {
    fn default() -> Self {
        Self::new()
    }
}

fn derive_features(ctx: &ClassificationContext<'_>) -> DerivedFeatures {
    let focal_account = ctx.accounts.first().cloned();
    let legs = build_transfer_legs(ctx.instructions, focal_account.as_deref());

    let program_ids: HashSet<String> = ctx
        .instructions
        .iter()
        .map(|instruction| instruction.program_id.clone())
        .collect();

    let instruction_names: HashSet<String> = ctx
        .instructions
        .iter()
        .map(|instruction| match &instruction.kind {
            InstructionInputKind::SetAuthority => "setAuthority".to_string(),
            InstructionInputKind::Upgrade => "upgrade".to_string(),
            InstructionInputKind::SystemTransfer { .. } => "systemTransfer".to_string(),
            InstructionInputKind::TokenTransfer { .. } => "transfer".to_string(),
            InstructionInputKind::TokenTransferChecked { .. } => "transferChecked".to_string(),
            InstructionInputKind::StakeDelegate => "stakeDelegate".to_string(),
            InstructionInputKind::StakeWithdraw => "stakeWithdraw".to_string(),
            InstructionInputKind::Other { name } => name.clone(),
            InstructionInputKind::Unknown => "unknown".to_string(),
        })
        .map(|name| name.to_ascii_lowercase())
        .collect();

    let has_swap_hint = instruction_names
        .iter()
        .any(|name| name.contains("swap") || name.contains("route"))
        || has_any_program(&program_ids, &DEX_PROGRAM_IDS)
        || matches!(ctx.instruction_type, Some("swap") | Some("route"));

    let has_memo_program =
        has_any_program(&program_ids, &[SPL_MEMO_PROGRAM_ID, MEMO_V1_PROGRAM_ID]);
    let has_bridge_program = has_any_program(
        &program_ids,
        &[
            WORMHOLE_PROGRAM_ID,
            WORMHOLE_TOKEN_BRIDGE_ID,
            DEGODS_BRIDGE_PROGRAM_ID,
            DEBRIDGE_PROGRAM_ID,
            ALLBRIDGE_PROGRAM_ID,
        ],
    );
    let has_privacy_program = has_any_program(&program_ids, &[PRIVACY_CASH_PROGRAM_ID]);
    let has_stake_program =
        has_any_program(&program_ids, &[STAKE_PROGRAM_ID, STAKE_POOL_PROGRAM_ID]);

    let has_nft_program = has_any_program(
        &program_ids,
        &[
            METAPLEX_PROGRAM_ID,
            CANDY_MACHINE_V3_PROGRAM_ID,
            CANDY_GUARD_PROGRAM_ID,
            BUBBLEGUM_PROGRAM_ID,
            MAGIC_EDEN_CANDY_MACHINE_ID,
        ],
    );
    let has_nft_marketplace_program = has_any_program(
        &program_ids,
        &[
            MAGIC_EDEN_V2_PROGRAM_ID,
            MAGIC_EDEN_MMM_PROGRAM_ID,
            TENSOR_SWAP_PROGRAM_ID,
            TENSOR_MARKETPLACE_PROGRAM_ID,
            TENSOR_AMM_PROGRAM_ID,
            HADESWAP_PROGRAM_ID,
            METAPLEX_AUCTION_HOUSE_PROGRAM_ID,
            FORMFUNCTION_PROGRAM_ID,
        ],
    );

    let has_stake_delegate_instruction = instruction_names.contains("stakedelegate")
        || instruction_names.contains("delegatestake")
        || matches!(ctx.instruction_type, Some("stakeDelegate"));
    let has_stake_withdraw_instruction = instruction_names.contains("stakewithdraw")
        || instruction_names.contains("withdraw")
        || matches!(ctx.instruction_type, Some("stakeWithdraw"));

    DerivedFeatures {
        focal_account,
        legs,
        instruction_names,
        has_swap_hint,
        has_memo_program,
        has_bridge_program,
        has_privacy_program,
        has_nft_program,
        has_nft_marketplace_program,
        has_stake_program,
        has_stake_delegate_instruction,
        has_stake_withdraw_instruction,
    }
}

fn build_transfer_legs(
    instructions: &[InstructionInput],
    focal_account: Option<&str>,
) -> Vec<TransferLeg> {
    let mut legs = Vec::new();

    for (instruction_index, instruction) in instructions.iter().enumerate() {
        let (amount, decimals, destination_index, instruction_kind, asset_hint) =
            match &instruction.kind {
                InstructionInputKind::TokenTransfer { amount } => (
                    *amount,
                    None,
                    1usize,
                    "transfer".to_string(),
                    "spl_token".to_string(),
                ),
                InstructionInputKind::TokenTransferChecked { amount, decimals } => (
                    *amount,
                    Some(*decimals),
                    2usize,
                    "transferChecked".to_string(),
                    "spl_token".to_string(),
                ),
                InstructionInputKind::SystemTransfer { lamports } => (
                    *lamports,
                    Some(9),
                    1usize,
                    "systemTransfer".to_string(),
                    "sol".to_string(),
                ),
                _ => continue,
            };

        let Some(source) = instruction.accounts.first() else {
            continue;
        };
        let Some(destination) = instruction.accounts.get(destination_index) else {
            continue;
        };

        let direction = match focal_account {
            Some(focal) if focal == source && focal == destination => LegDirection::Internal,
            Some(focal) if focal == source => LegDirection::Outflow,
            Some(focal) if focal == destination => LegDirection::Inflow,
            Some(_) => {
                if source == destination {
                    LegDirection::Internal
                } else {
                    LegDirection::External
                }
            }
            None => {
                if source == destination {
                    LegDirection::Internal
                } else {
                    LegDirection::External
                }
            }
        };

        legs.push(TransferLeg {
            instruction_index,
            instruction_kind,
            source: source.clone(),
            destination: destination.clone(),
            amount,
            decimals,
            asset_hint,
            direction,
        });
    }

    legs
}

fn has_any_program(program_ids: &HashSet<String>, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| program_ids.contains(*candidate))
}

fn sum_legs<F>(legs: &[TransferLeg], mut predicate: F) -> u128
where
    F: FnMut(&TransferLeg) -> bool,
{
    legs.iter()
        .filter(|leg| predicate(leg))
        .map(|leg| leg.amount as u128)
        .sum()
}

fn count_legs<F>(legs: &[TransferLeg], mut predicate: F) -> usize
where
    F: FnMut(&TransferLeg) -> bool,
{
    legs.iter().filter(|leg| predicate(leg)).count()
}

fn is_nft_like_leg(leg: &TransferLeg) -> bool {
    leg.asset_hint == "spl_token" && leg.decimals == Some(0) && leg.amount == 1
}

fn is_sol_leg(leg: &TransferLeg) -> bool {
    leg.asset_hint == "sol"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ix(kind: InstructionInputKind, accounts: &[&str]) -> InstructionInput {
        InstructionInput {
            program_id: "token".to_string(),
            kind,
            accounts: accounts.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn with_program(
        program_id: &str,
        kind: InstructionInputKind,
        accounts: &[&str],
    ) -> InstructionInput {
        InstructionInput {
            program_id: program_id.to_string(),
            kind,
            accounts: accounts.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn classifies_swap_from_leg_flow() {
        let service = ClassificationService::new();
        let accounts = vec!["wallet".to_string()];
        let instructions = vec![
            with_program(
                "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8",
                InstructionInputKind::Other {
                    name: "swap".to_string(),
                },
                &[],
            ),
            ix(
                InstructionInputKind::TokenTransfer { amount: 10_000 },
                &["wallet", "pool_vault"],
            ),
            ix(
                InstructionInputKind::TokenTransfer { amount: 20_000 },
                &["pool_vault_2", "wallet"],
            ),
        ];
        let ctx = ClassificationContext {
            instruction_type: Some("swap"),
            success: true,
            compute_units: 50_000,
            fee_lamports: 5000,
            accounts: &accounts,
            instructions: &instructions,
        };

        let out = service.classify(&ctx);
        assert_eq!(out.classification.category, "defi_swap");
        assert_eq!(out.classification.classifier, "swap");
    }

    #[test]
    fn classifies_bridge_out_when_program_present() {
        let service = ClassificationService::new();
        let accounts = vec!["wallet".to_string()];
        let instructions = vec![
            with_program(
                WORMHOLE_TOKEN_BRIDGE_ID,
                InstructionInputKind::Other {
                    name: "bridge".to_string(),
                },
                &[],
            ),
            ix(
                InstructionInputKind::TokenTransfer { amount: 2_000_000 },
                &["wallet", "bridge_vault"],
            ),
        ];
        let ctx = ClassificationContext {
            instruction_type: Some("bridge"),
            success: true,
            compute_units: 80_000,
            fee_lamports: 5000,
            accounts: &accounts,
            instructions: &instructions,
        };

        let out = service.classify(&ctx);
        assert_eq!(out.classification.classifier, "bridge");
        assert_eq!(out.classification.category, "bridge_out");
    }

    #[test]
    fn classifies_fee_only_when_no_legs() {
        let service = ClassificationService::new();
        let accounts = vec!["wallet".to_string()];
        let instructions = vec![with_program(
            "ComputeBudget111111111111111111111111111111",
            InstructionInputKind::Other {
                name: "setComputeUnitLimit".to_string(),
            },
            &[],
        )];
        let ctx = ClassificationContext {
            instruction_type: Some("setComputeUnitLimit"),
            success: true,
            compute_units: 12_000,
            fee_lamports: 5000,
            accounts: &accounts,
            instructions: &instructions,
        };

        let out = service.classify(&ctx);
        assert_eq!(out.classification.classifier, "fee-only");
        assert_eq!(out.classification.category, "fee_only");
    }

    #[test]
    fn classifies_stake_withdraw_from_system_inflow() {
        let service = ClassificationService::new();
        let accounts = vec!["wallet".to_string()];
        let instructions = vec![
            with_program(
                STAKE_PROGRAM_ID,
                InstructionInputKind::StakeWithdraw,
                &["stake_account", "wallet"],
            ),
            with_program(
                "11111111111111111111111111111111",
                InstructionInputKind::SystemTransfer {
                    lamports: 500_000_000,
                },
                &["stake_account", "wallet"],
            ),
        ];
        let ctx = ClassificationContext {
            instruction_type: Some("stakeWithdraw"),
            success: true,
            compute_units: 40_000,
            fee_lamports: 5000,
            accounts: &accounts,
            instructions: &instructions,
        };

        let out = service.classify(&ctx);
        assert_eq!(out.classification.classifier, "stake-withdraw");
        assert_eq!(out.classification.category, "stake_withdraw");
    }

    #[test]
    fn classifies_nft_send() {
        let service = ClassificationService::new();
        let accounts = vec!["wallet".to_string()];
        let instructions = vec![
            with_program(
                METAPLEX_PROGRAM_ID,
                InstructionInputKind::Other {
                    name: "executeSale".to_string(),
                },
                &[],
            ),
            ix(
                InstructionInputKind::TokenTransferChecked {
                    amount: 1,
                    decimals: 0,
                },
                &["wallet", "mint", "receiver"],
            ),
        ];
        let ctx = ClassificationContext {
            instruction_type: Some("executeSale"),
            success: true,
            compute_units: 70_000,
            fee_lamports: 5000,
            accounts: &accounts,
            instructions: &instructions,
        };

        let out = service.classify(&ctx);
        assert_eq!(out.classification.classifier, "nft-transfer");
        assert_eq!(out.classification.category, "nft_send");
    }

    #[test]
    fn falls_back_to_other() {
        let service = ClassificationService::new();
        let accounts = vec!["wallet".to_string()];
        let instructions = vec![InstructionInput {
            program_id: "system".to_string(),
            kind: InstructionInputKind::Other {
                name: "initialize".to_string(),
            },
            accounts: vec![],
        }];
        let ctx = ClassificationContext {
            instruction_type: Some("initialize"),
            success: true,
            compute_units: 50_000,
            fee_lamports: 0,
            accounts: &accounts,
            instructions: &instructions,
        };

        let out = service.classify(&ctx);
        assert_eq!(out.classification.category, "other");
        assert_eq!(out.classification.classifier, "fallback");
    }

    #[test]
    fn builds_system_transfer_leg() {
        let instructions = vec![ix(
            InstructionInputKind::SystemTransfer { lamports: 42 },
            &["src", "dst"],
        )];
        let legs = build_transfer_legs(&instructions, Some("src"));
        assert_eq!(legs.len(), 1);
        assert_eq!(legs[0].asset_hint, "sol");
        assert_eq!(legs[0].amount, 42);
        assert_eq!(legs[0].direction, LegDirection::Outflow);
    }
}
