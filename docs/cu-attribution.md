# CU Attribution

How GulfWatch reconstructs per-instruction compute-unit consumption for any Solana transaction, why it works, and where it lives in the pipeline.

## The question

Given an arbitrary Solana transaction, how do we attribute compute-unit consumption to individual instructions (top-level and inner) so the deep-dive view can show "what ate the budget"?

Three paths were on the table:

1. Parse `meta.logMessages` from a single `getTransaction` response.
2. Re-run the signature through `simulateTransaction` on demand.
3. Run a custom validator with per-instruction metering instrumentation.

## The answer

Option 1 works and is exact. Per-instruction CU is fully reconstructible from the log stream Solana already emits in every `getTransaction` response. No replay, no extra RPC, no custom node. Validated against three real mainnet fixtures on 2026-04-13 (a Jupiter success, a Raydium swap with heavy CPI nesting, and a Jupiter slippage failure). Every reconstruction matched `meta.computeUnitsConsumed` exactly.

## The log format we rely on

Solana's runtime emits three relevant log-line shapes during every program invocation:

```
Program <pubkey> invoke [<depth>]
Program <pubkey> consumed <x> of <y> compute units
Program <pubkey> success
```

Or, on failure:

```
Program <pubkey> failed: <reason>
```

- `<depth>` starts at 1 for top-level instructions and increments on each CPI.
- `<x>` is the CU consumed during this invocation, **inclusive of all nested CPIs** — this is the critical property that makes depth-1 sums equal the total.
- `<y>` is the budget remaining at the moment the program was invoked.
- `consumed` and `success`/`failed:` are **two separate lines**. A program that succeeds emits `consumed` first, then `success`. A program that fails emits `consumed` first, then `failed:`. The `consumed` line is emitted even on failure.

## State machine

| Log line | Action |
|---|---|
| `Program X invoke [N]` | Push X onto stack at depth N |
| `Program X consumed A of B compute units` | Record A on top-of-stack. **Do not pop.** |
| `Program X success` | Pop stack |
| `Program X failed: ...` | Pop stack, mark failed |

The critical detail is that `consumed` does not pop. Popping on `consumed` corrupts the stack on any transaction with CPIs. Pop only on terminal lines.

## The native-program overhead

Some programs never emit a `consumed` line because they are native Solana runtime programs, not BPF programs. Confirmed from fixtures:

- `ComputeBudget111111111111111111111111111111`
- `11111111111111111111111111111111` (System)

Each of these still charges exactly **150 CU** per top-level invocation, billed by the runtime. The same is almost certainly true for other native programs not yet hit in validation (Stake, Vote, Address Lookup Table, BPF Loader).

**Self-calibrating invariant:** after summing all top-level `consumed` values, the delta against `meta.computeUnitsConsumed` must be exactly `150 × N`, where N = the count of top-level invocations that had no `consumed` line. If it isn't, the parser has a bug or Solana changed its log format. Use this as a test invariant rather than maintaining a hardcoded allowlist of native programs — more robust to runtime changes.

## Validation results

| Fixture | Reported CU | Summed top-level | Delta | Native programs accounting for delta |
|---|---|---|---|---|
| Jupiter V6 success | 80,687 | 80,387 | 300 | 2 × ComputeBudget |
| Raydium swap success | 353,562 | 353,112 | 450 | 2 × ComputeBudget + 1 × System |
| Jupiter V6 failed (slippage) | 204,718 | 204,418 | 300 | 2 × ComputeBudget |

Every delta is exactly `150 × (native program count)`. Failed transactions work because the failing program still emits its `consumed` line before the `failed:` line, so full CU cost up to the failure point is captured and the `failed` flag lands on the correct invocation.

## Where this lives in the pipeline

The CU parser is an in-pipeline transformation, not a separate service:

- **Crate:** `gulfwatch-core`, new module `cu_attribution`, pure function over `&[String]` → `Vec<Invocation>` plus the delta assertion.
- **Call site:** `gulfwatch-ingest::processor`, called once per transaction as part of the existing `getTransaction` parsing step. The result is stored on the rolling-window entry so the deep-dive view can render instantly.
- **Exposed via:** the Phase 2.5 Feature C deep-dive endpoint `GET /api/transactions/:signature`.
- **Rendered by:** the TUI deep-dive detail view (new "CU profile" tab) and the web `/tx/<signature>` page (new tab with a Recharts horizontal bar chart — teammate).
- **Drill-down:** every `Invocation` in the parser output carries its depth and consumed value, so "click a top-level instruction to see its nested CPIs" is the same data structure, just a different render. No extra work.
- **No new RPC endpoint, no new external dependency, no replay.** The existing pipeline already fetches `meta.logMessages`.

## Gotchas

- **Log truncation.** Solana truncates `logMessages` at ~10 KB per transaction. On very log-heavy transactions, some `consumed` lines can be lost. The self-calibrating delta assertion will catch this (delta won't be `150 × N`). The profiler should degrade gracefully: show partial attribution plus a "logs truncated, reconstruction incomplete" badge.
- **Log format stability.** The `invoke [N]` / `consumed X of Y compute units` / `success` / `failed:` shapes have been stable for years and are emitted by `solana-program-runtime` itself. Low drift risk, but keep the parser as a single well-tested module so any future drift is a one-file fix.
- **Custom program logs.** Programs can emit arbitrary text via `sol_log`, but the `Program ...` prefix is reserved for the runtime — programs cannot forge it. Not a practical concern.
- **Native-program allowlist temptation.** Don't hardcode one. The self-calibrating `150 × N` invariant auto-handles any native program that charges the standard runtime overhead, including ones not yet encountered.

## Implementation status (as of 2026-04-13)

Pulled forward from Phase 3 because the spike finished with time to spare and the parser turned out to be ~150 lines. Backend + TUI vertical slice is live on branch `cu-attribution`, all 67 workspace tests green, zero warnings.

### Done

- [x] Port the parser to Rust inside `gulfwatch-core::cu_attribution`, same state machine, same self-calibrating delta assertion.
- [x] Unit tests covering single top-level, ComputeBudget/System native overhead, deeply nested CPIs, failed-with-consumed, verification drift, sort order, arbitrary log-line noise, truncated logs, and empty input (12 tests total).
- [x] `Transaction.cu_profile: Option<CuProfile>` added with `#[serde(default)]` for wire-format backwards compatibility. All 10 construction sites updated.
- [x] Wire into `gulfwatch-ingest::parser::parse_transaction` — every transaction the ingest parses now extracts `meta.logMessages` and builds a `CuProfile` before returning. Result flows through the existing pipeline (mpsc → processing worker → rolling window → broadcast → REST) with zero changes in those layers because it's just another field on `Transaction`.
- [x] REST endpoints that return `Transaction` (`/api/transactions/recent`, WebSocket feed) now automatically include the profile in their JSON response shape.
- [x] TUI: CU Profile section in the existing transaction detail view (between Fee and Accounts), top-level invocations sorted by CU desc with a 20-char text bar chart, verified/incomplete badge, percentage of total, native program tag, FAILED tag, total line, and native-overhead summary. Graceful fallback when `cu_profile` is `None` or top-level is empty.

### Remaining (unblocked — can ship any time)

- [ ] **Wiring test in `gulfwatch-ingest::parser`** — one integration test that feeds a mock `getTransaction` JSON with real-looking `logMessages` through `parse_transaction` and asserts the resulting `Transaction.cu_profile` is `Some(_)` with the expected top-level breakdown. Deferred because `gulfwatch-core::cu_attribution` has 12 unit tests and the wiring is a ~20-line adapter, but worth adding before the hackathon submission so regressions in the adapter get caught.
- [ ] **`GET /api/transactions/:signature`** — tracked under Phase 2.5 Feature C (Apr 17-18) in `plan.md`. The profile is already on the in-memory `Transaction` in the rolling window, so this endpoint only needs to look up by signature and serialize — no CU-specific work.
- [ ] **Web CU profile tab** on `/tx/<signature>` with a Recharts horizontal bar chart (teammate, after Feature C lands).
- [ ] **Drill-down into nested CPIs** — every `Invocation` in the profile already carries its depth and consumed value, so the backend data is sufficient. Only the TUI/web render needs a depth-2+ expansion. Can land as a Phase 3 polish pass.
- [ ] **Dedicated "CU profile" tab** (as opposed to a panel inside the existing detail view) once the Feature C deep-dive tab scaffolding lands Apr 17-18. The current placement as a panel inside `draw_tx_detail` is the working version — lift it into a proper tab when the tab container exists.

## Decisions locked in

- **Option 1 is the chosen path.** Options 2 (`simulateTransaction` replay) and 3 (custom replay node) are not needed for the hackathon. Keep them documented here as fallbacks in case a future edge case breaks Option 1.
- **LaserStream gRPC is not a fourth path.** Helius dev-plan LaserStream is devnet-only, the demo is mainnet, and Option 1 renders the question moot regardless.
- **Self-calibrating delta assertion** is the validation strategy, not a hardcoded native-program list.
- Research spike for this decision ran on 2026-04-13 on branch `cu-attribution`, ~1 hour, three real mainnet fixtures. Parser and fixtures were throwaway and are not committed.
