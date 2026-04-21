// See docs/cu-attribution.md for the design, state machine, and validation.

use serde::{Deserialize, Serialize};

pub const NATIVE_PROGRAM_CU: u64 = 150;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invocation {
    pub program_id: String,
    pub depth: u32,
    pub consumed_cu: Option<u64>,
    pub failed: bool,
    // Captured from `Program log: Instruction: <name>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuProfile {
    pub invocations: Vec<Invocation>,
    pub reported_total: u64,
    pub summed_top_level: u64,
    pub native_overhead_cu: u64,
    pub reconstructed_total: u64,
    pub verified: bool,
}

impl CuProfile {
    pub fn top_level(&self) -> impl Iterator<Item = &Invocation> {
        self.invocations.iter().filter(|i| i.depth == 1)
    }

    pub fn top_level_sorted_by_cu(&self) -> Vec<&Invocation> {
        let mut top: Vec<&Invocation> = self.top_level().collect();
        top.sort_by(|a, b| {
            let a_cu = a.consumed_cu.unwrap_or(NATIVE_PROGRAM_CU);
            let b_cu = b.consumed_cu.unwrap_or(NATIVE_PROGRAM_CU);
            b_cu.cmp(&a_cu)
        });
        top
    }

    // Groups depth-1 invocations with their nested CPIs in execution order;
    // outer vector sorted by top-level CU desc.
    pub fn top_level_with_children_sorted_by_cu(
        &self,
    ) -> Vec<(&Invocation, Vec<&Invocation>)> {
        let mut groups: Vec<(&Invocation, Vec<&Invocation>)> = Vec::new();
        for inv in &self.invocations {
            if inv.depth == 1 {
                groups.push((inv, Vec::new()));
            } else if let Some(last) = groups.last_mut() {
                last.1.push(inv);
            }
        }
        groups.sort_by(|a, b| {
            let a_cu = a.0.consumed_cu.unwrap_or(NATIVE_PROGRAM_CU);
            let b_cu = b.0.consumed_cu.unwrap_or(NATIVE_PROGRAM_CU);
            b_cu.cmp(&a_cu)
        });
        groups
    }
}

pub fn parse_logs(logs: &[String], reported_total: u64) -> CuProfile {
    let mut invocations: Vec<Invocation> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();

    for line in logs {
        if let Some((program, depth)) = parse_invoke(line) {
            invocations.push(Invocation {
                program_id: program,
                depth,
                consumed_cu: None,
                failed: false,
                instruction_name: None,
            });
            stack.push(invocations.len() - 1);
            continue;
        }

        if let Some(name) = parse_instruction_name_log(line) {
            if let Some(&top_idx) = stack.last() {
                // First one wins — later debug logs don't overwrite.
                if invocations[top_idx].instruction_name.is_none() {
                    invocations[top_idx].instruction_name = Some(name);
                }
            }
            continue;
        }

        if let Some(consumed) = parse_consumed(line) {
            if let Some(&top_idx) = stack.last() {
                invocations[top_idx].consumed_cu = Some(consumed);
            }
            continue;
        }

        if parse_success(line).is_some() {
            stack.pop();
            continue;
        }

        if parse_failed(line).is_some() {
            if let Some(top_idx) = stack.pop() {
                invocations[top_idx].failed = true;
            }
            continue;
        }
    }

    let summed_top_level: u64 = invocations
        .iter()
        .filter(|i| i.depth == 1)
        .filter_map(|i| i.consumed_cu)
        .sum();

    let opaque_top_level_count = invocations
        .iter()
        .filter(|i| i.depth == 1 && i.consumed_cu.is_none())
        .count() as u64;

    let native_overhead_cu = opaque_top_level_count * NATIVE_PROGRAM_CU;
    let reconstructed_total = summed_top_level + native_overhead_cu;
    let verified = reconstructed_total == reported_total;

    CuProfile {
        invocations,
        reported_total,
        summed_top_level,
        native_overhead_cu,
        reconstructed_total,
        verified,
    }
}

fn parse_invoke(line: &str) -> Option<(String, u32)> {
    let suffix = line.strip_prefix("Program ")?;
    let (program, rest) = suffix.split_once(" invoke [")?;
    let depth_str = rest.strip_suffix(']')?;
    let depth: u32 = depth_str.parse().ok()?;
    Some((program.to_string(), depth))
}

fn parse_instruction_name_log(line: &str) -> Option<String> {
    let rest = line.strip_prefix("Program log: Instruction: ")?;
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

fn parse_consumed(line: &str) -> Option<u64> {
    let suffix = line.strip_prefix("Program ")?;
    let (_program, rest) = suffix.split_once(" consumed ")?;
    let (x_str, tail) = rest.split_once(" of ")?;
    if !tail.contains(" compute units") {
        return None;
    }
    x_str.parse().ok()
}

fn parse_success(line: &str) -> Option<&str> {
    let suffix = line.strip_prefix("Program ")?;
    let (program, rest) = suffix.split_once(' ')?;
    if rest == "success" { Some(program) } else { None }
}

fn parse_failed(line: &str) -> Option<&str> {
    let suffix = line.strip_prefix("Program ")?;
    let (program, rest) = suffix.split_once(' ')?;
    if rest.starts_with("failed:") {
        Some(program)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(s: &str) -> Vec<String> {
        s.trim()
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }

    #[test]
    fn single_top_level_program_exact_match() {
        let logs = log(
            "Program AAA invoke [1]
             Program log: hello
             Program AAA consumed 5000 of 200000 compute units
             Program AAA success",
        );
        let profile = parse_logs(&logs, 5000);
        assert_eq!(profile.invocations.len(), 1);
        assert_eq!(profile.invocations[0].program_id, "AAA");
        assert_eq!(profile.invocations[0].depth, 1);
        assert_eq!(profile.invocations[0].consumed_cu, Some(5000));
        assert!(!profile.invocations[0].failed);
        assert_eq!(profile.summed_top_level, 5000);
        assert_eq!(profile.native_overhead_cu, 0);
        assert_eq!(profile.reconstructed_total, 5000);
        assert!(profile.verified);
    }

    #[test]
    fn compute_budget_contributes_150_cu_overhead_per_invocation() {
        let logs = log(
            "Program ComputeBudget111111111111111111111111111111 invoke [1]
             Program ComputeBudget111111111111111111111111111111 success
             Program ComputeBudget111111111111111111111111111111 invoke [1]
             Program ComputeBudget111111111111111111111111111111 success
             Program AAA invoke [1]
             Program AAA consumed 10000 of 199700 compute units
             Program AAA success",
        );
        let profile = parse_logs(&logs, 10300);
        assert_eq!(profile.invocations.len(), 3);
        assert_eq!(profile.summed_top_level, 10000);
        assert_eq!(profile.native_overhead_cu, 300);
        assert_eq!(profile.reconstructed_total, 10300);
        assert!(profile.verified);
    }

    #[test]
    fn nested_cpi_top_level_sum_already_includes_nested() {
        let logs = log(
            "Program OUTER invoke [1]
             Program INNER invoke [2]
             Program INNER consumed 1000 of 199000 compute units
             Program INNER success
             Program OUTER consumed 5000 of 200000 compute units
             Program OUTER success",
        );
        let profile = parse_logs(&logs, 5000);
        assert_eq!(profile.invocations.len(), 2);
        assert_eq!(profile.invocations[0].program_id, "OUTER");
        assert_eq!(profile.invocations[0].depth, 1);
        assert_eq!(profile.invocations[0].consumed_cu, Some(5000));
        assert_eq!(profile.invocations[1].program_id, "INNER");
        assert_eq!(profile.invocations[1].depth, 2);
        assert_eq!(profile.invocations[1].consumed_cu, Some(1000));
        assert_eq!(profile.summed_top_level, 5000);
        assert!(profile.verified);
    }

    #[test]
    fn deeply_nested_cpi_tracks_all_depths() {
        let logs = log(
            "Program OUTER invoke [1]
             Program MID invoke [2]
             Program INNER invoke [3]
             Program INNER consumed 500 of 199500 compute units
             Program INNER success
             Program MID consumed 2000 of 200000 compute units
             Program MID success
             Program OUTER consumed 7000 of 200000 compute units
             Program OUTER success",
        );
        let profile = parse_logs(&logs, 7000);
        assert_eq!(profile.invocations.len(), 3);
        assert_eq!(profile.invocations[0].depth, 1);
        assert_eq!(profile.invocations[1].depth, 2);
        assert_eq!(profile.invocations[2].depth, 3);
        assert_eq!(profile.summed_top_level, 7000);
        assert!(profile.verified);
    }

    #[test]
    fn failed_program_captures_consumed_and_flags_failure() {
        let logs = log(
            "Program AAA invoke [1]
             Program log: Instruction: Swap
             Program AAA consumed 7500 of 200000 compute units
             Program AAA failed: custom program error: 0x1771",
        );
        let profile = parse_logs(&logs, 7500);
        assert_eq!(profile.invocations.len(), 1);
        assert_eq!(profile.invocations[0].consumed_cu, Some(7500));
        assert!(profile.invocations[0].failed);
        assert_eq!(profile.summed_top_level, 7500);
        assert!(profile.verified);
    }

    #[test]
    fn system_program_contributes_150_cu_overhead() {
        let logs = log(
            "Program 11111111111111111111111111111111 invoke [1]
             Program 11111111111111111111111111111111 success
             Program AAA invoke [1]
             Program AAA consumed 4000 of 199850 compute units
             Program AAA success",
        );
        let profile = parse_logs(&logs, 4150);
        assert_eq!(profile.summed_top_level, 4000);
        assert_eq!(profile.native_overhead_cu, 150);
        assert_eq!(profile.reconstructed_total, 4150);
        assert!(profile.verified);
    }

    #[test]
    fn mixed_native_and_cpi_transaction_matches() {
        let logs = log(
            "Program ComputeBudget111111111111111111111111111111 invoke [1]
             Program ComputeBudget111111111111111111111111111111 success
             Program ComputeBudget111111111111111111111111111111 invoke [1]
             Program ComputeBudget111111111111111111111111111111 success
             Program 11111111111111111111111111111111 invoke [1]
             Program 11111111111111111111111111111111 success
             Program ROUTER invoke [1]
             Program TOKEN invoke [2]
             Program TOKEN consumed 4500 of 199000 compute units
             Program TOKEN success
             Program ROUTER consumed 80000 of 200000 compute units
             Program ROUTER success",
        );
        let profile = parse_logs(&logs, 80450);
        assert_eq!(profile.summed_top_level, 80000);
        assert_eq!(profile.native_overhead_cu, 450);
        assert!(profile.verified);
    }

    #[test]
    fn verification_fails_when_reported_total_does_not_reconcile() {
        let logs = log(
            "Program AAA invoke [1]
             Program AAA consumed 1000 of 200000 compute units
             Program AAA success",
        );
        let profile = parse_logs(&logs, 1234);
        assert!(!profile.verified);
        assert_eq!(profile.summed_top_level, 1000);
        assert_eq!(profile.reconstructed_total, 1000);
    }

    #[test]
    fn top_level_sorted_by_cu_orders_desc_with_native_fallback() {
        let logs = log(
            "Program ComputeBudget111111111111111111111111111111 invoke [1]
             Program ComputeBudget111111111111111111111111111111 success
             Program SMALL invoke [1]
             Program SMALL consumed 1000 of 200000 compute units
             Program SMALL success
             Program BIG invoke [1]
             Program BIG consumed 50000 of 199000 compute units
             Program BIG success",
        );
        let profile = parse_logs(&logs, 51150);
        let sorted = profile.top_level_sorted_by_cu();
        assert_eq!(sorted[0].program_id, "BIG");
        assert_eq!(sorted[1].program_id, "SMALL");
        assert_eq!(
            sorted[2].program_id,
            "ComputeBudget111111111111111111111111111111"
        );
        assert!(profile.verified);
    }

    #[test]
    fn arbitrary_program_log_lines_are_ignored() {
        let logs = log(
            "Program AAA invoke [1]
             Program log: some arbitrary text
             Program log: line mentioning failed: in passing
             Program data: base64stuff
             Program return: AAA base64
             Program AAA consumed 3000 of 200000 compute units
             Program AAA success",
        );
        let profile = parse_logs(&logs, 3000);
        assert_eq!(profile.invocations.len(), 1);
        assert_eq!(profile.invocations[0].consumed_cu, Some(3000));
        assert!(profile.verified);
    }

    #[test]
    fn truncated_logs_do_not_panic_and_mark_unverified() {
        // Simulates Solana's ~10 KB log truncation — the trailing success
        // line is missing. Parser should still produce a partial profile.
        let logs = log(
            "Program AAA invoke [1]
             Program AAA consumed 3000 of 200000 compute units",
        );
        let profile = parse_logs(&logs, 5000);
        assert_eq!(profile.invocations.len(), 1);
        assert_eq!(profile.invocations[0].consumed_cu, Some(3000));
        assert_eq!(profile.summed_top_level, 3000);
        assert!(!profile.verified);
    }

    #[test]
    fn top_level_with_children_groups_by_execution_order_and_sorts_desc() {
        let logs = log(
            "Program ComputeBudget111111111111111111111111111111 invoke [1]
             Program ComputeBudget111111111111111111111111111111 success
             Program OUTER invoke [1]
             Program MID invoke [2]
             Program INNER invoke [3]
             Program INNER consumed 500 of 199500 compute units
             Program INNER success
             Program MID consumed 2000 of 199500 compute units
             Program MID success
             Program SIBLING invoke [2]
             Program SIBLING consumed 1500 of 197000 compute units
             Program SIBLING success
             Program OUTER consumed 9000 of 200000 compute units
             Program OUTER success
             Program SMALL invoke [1]
             Program SMALL consumed 1000 of 200000 compute units
             Program SMALL success",
        );
        let profile = parse_logs(&logs, 10150);
        let groups = profile.top_level_with_children_sorted_by_cu();

        assert_eq!(groups.len(), 3, "three top-level invocations");

        // Sorted by CU desc: OUTER (9000), SMALL (1000), ComputeBudget (native, 150).
        assert_eq!(groups[0].0.program_id, "OUTER");
        let outer_children: Vec<&str> =
            groups[0].1.iter().map(|c| c.program_id.as_str()).collect();
        assert_eq!(outer_children, vec!["MID", "INNER", "SIBLING"]);
        let outer_depths: Vec<u32> = groups[0].1.iter().map(|c| c.depth).collect();
        assert_eq!(outer_depths, vec![2, 3, 2]);

        assert_eq!(groups[1].0.program_id, "SMALL");
        assert!(groups[1].1.is_empty(), "SMALL has no CPIs");

        assert_eq!(
            groups[2].0.program_id,
            "ComputeBudget111111111111111111111111111111"
        );
        assert!(groups[2].1.is_empty(), "native program has no CPIs");
    }

    #[test]
    fn empty_logs_produce_empty_profile() {
        let profile = parse_logs(&[], 0);
        assert!(profile.invocations.is_empty());
        assert_eq!(profile.summed_top_level, 0);
        assert_eq!(profile.native_overhead_cu, 0);
        assert!(profile.verified);
    }

    #[test]
    fn instruction_name_log_is_captured_on_invocation() {
        let logs = log(
            "Program AAA invoke [1]
             Program log: Instruction: Swap
             Program AAA consumed 5000 of 200000 compute units
             Program AAA success",
        );
        let profile = parse_logs(&logs, 5000);
        assert_eq!(profile.invocations.len(), 1);
        assert_eq!(
            profile.invocations[0].instruction_name.as_deref(),
            Some("Swap")
        );
    }

    #[test]
    fn first_instruction_log_wins_within_an_invocation() {
        let logs = log(
            "Program AAA invoke [1]
             Program log: Instruction: Swap
             Program log: Instruction: PostHocNote
             Program AAA success",
        );
        let profile = parse_logs(&logs, 150);
        assert_eq!(
            profile.invocations[0].instruction_name.as_deref(),
            Some("Swap")
        );
    }

    #[test]
    fn nested_invocations_each_get_their_own_instruction_name() {
        let logs = log(
            "Program OUTER invoke [1]
             Program log: Instruction: Route
             Program INNER invoke [2]
             Program log: Instruction: Transfer
             Program INNER success
             Program OUTER success",
        );
        let profile = parse_logs(&logs, 300);
        assert_eq!(profile.invocations.len(), 2);
        assert_eq!(
            profile.invocations[0].instruction_name.as_deref(),
            Some("Route")
        );
        assert_eq!(
            profile.invocations[1].instruction_name.as_deref(),
            Some("Transfer")
        );
    }

    #[test]
    fn invocation_without_instruction_log_has_none() {
        let logs = log(
            "Program AAA invoke [1]
             Program log: some debug stuff
             Program AAA success",
        );
        let profile = parse_logs(&logs, 150);
        assert_eq!(profile.invocations[0].instruction_name, None);
    }
}
