# GulfWatch docs

Deep-dive documentation for the GulfWatch security monitor. The main [README](../README.md) covers what GulfWatch is, how to run it, and the public API surface — start there if you've never seen the project before. The docs in this folder go one layer deeper: how the pieces fit together, how transactions get classified, and how the security detections work.

## What's in here

| Doc | What it covers | Read it when |
|---|---|---|
| [`architecture.md`](architecture.md) | Crate layout, the end-to-end flow from `logsSubscribe` to alert delivery, the in-memory state model, why there's no database. | You want a mental model of the whole system before touching code. |
| [`classification.md`](classification.md) | How `gulfwatch-ingest::parser` walks raw transactions and labels every instruction (top-level + inner CPIs) with a typed `InstructionKind`. The bridge between raw bytes and pattern-matchable detections. | You're adding support for a new program, debugging why an instruction was classified as `Other`, or trying to understand what the detections actually see. |
| [`transaction-classification.md`](transaction-classification.md) | How `gulfwatch-classification` derives high-level transaction types (`swap`, `bridge_out`, `nft_send`, etc.), how classifier priority works, and how debug traces are produced. | You're debugging why TUI/API shows a specific tx type, tuning classifier behavior, or adding a new classifier. |
| [`detections.md`](detections.md) | The three Phase 1 security rules, Authority Change, Failed Tx Cluster, Large Transfer Anomaly, including what each one fires on, why it matters, the alert payload, and what it deliberately doesn't catch. | You're building a UI for alerts, evaluating detection coverage, or planning a new detection rule. |

## Suggested reading order

**If you're seeing GulfWatch for the first time** (judge, recruiter, curious dev):

1. [Root README](../README.md) — what it is, what it does, how to run it
2. [`architecture.md`](architecture.md) — the system in one picture
3. [`transaction-classification.md`](transaction-classification.md) — how tx type labels are produced
4. [`detections.md`](detections.md) — the security pitch made concrete

**If you're implementing against GulfWatch** (frontend dev, integration, new detection):

1. [`architecture.md`](architecture.md) — find the seam you're working at
2. [`classification.md`](classification.md) — parser/instruction-level classification
3. [`transaction-classification.md`](transaction-classification.md) — high-level tx classifier behavior
4. [`detections.md`](detections.md) — the alert payload table and "adding a new detection" recipe

**If you just want to ship one specific thing**, jump straight to:

| Goal | Start here |
|---|---|
| Render alerts in a frontend | [`detections.md` → alert payload tables](detections.md#the-alert-flow) |
| Add a fourth detection | [`detections.md` → adding a new detection](detections.md#adding-a-new-detection) |
| Teach the parser a new program | [`classification.md` → how to add support for a new program](classification.md#how-to-add-support-for-a-new-program) |
| Debug why a tx is labeled `fallback` or `swap` | [`transaction-classification.md`](transaction-classification.md) |
| Understand the ingest → worker → broadcast loop | [`architecture.md` → the flow in one picture](architecture.md#the-flow-in-one-picture) |

## What's not in here (yet)

- **A frontend integration guide.** The web dashboard in `web/` consumes the REST + WebSocket API documented in the [root README](../README.md). A dedicated frontend doc is on the roadmap once the surface stabilizes.
- **Deployment / ops.** GulfWatch is a single binary per role with no external state, there's not much to deploy. A production hardening guide (TLS, auth, rate limiting on the public WebSocket) is post-Phase-1.
- **Phase 2/3 detection roadmap.** Brief list at the bottom of [`detections.md`](detections.md#whats-not-implemented-yet).
