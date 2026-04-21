# IDL loading

How GulfWatch finds, parses, and registers Solana program IDLs so that
transactions get human-readable instruction names, error names, and
argument values (rendered on the **Explain** tab) instead of opaque
discriminator bytes. If a program shows up in the TUI sidebar with a
`·` glyph and you want to know why, start here.

## The loading order

For every program in `MONITOR_PROGRAMS`, GulfWatch tries three sources at boot
time, in this order. First hit wins:

1. **On-chain PDA fetch** (`crates/gulfwatch-ingest/src/discover.rs`). Derives
   the `anchor:idl` PDA for the program, `getAccountInfo`s it, decompresses
   the payload (zstd for Anchor 0.29, zlib for Anchor 0.30+), and parses the
   JSON. Works for any Anchor program whose maintainer ran `anchor idl init`,
   regardless of format version.
2. **Runtime IDL directory** (`crates/gulfwatch-ingest/src/idl_registry.rs`).
   Scans `.json` files at boot and builds an in-memory
   `program_id → IdlDocument` map. Default seed dir is
   `crates/gulfwatch-ingest/idls/` (ships with the repo). An optional
   `GULFWATCH_IDL_DIR` env var adds a second directory whose entries win on
   program-id collisions with the seed.
3. **Fail with a visible reason.** `IdlStatus::Unavailable` records the
   on-chain error (e.g. `not published`, `unknown on-chain format`) and the
   TUI sidebar renders a short label under the program row.

Separate from these three: a **log-based instruction name fallback** runs
after IDL resolution on every transaction — see
[Log fallback](#log-fallback) below. It fills in readable instruction names
for programs that have no IDL at all but emit the standard
`Program log: Instruction: <name>` line.

There is also `POST /api/programs/:id/idl` at runtime for uploading one-off
IDLs — see [Adding an IDL](#adding-an-idl-three-ways).

## What loads automatically

Nothing else to do. These just work.

| Program kind | How it loads | Notes |
|---|---|---|
| Anchor 0.29 program that ran `anchor idl init` | On-chain, step 1 | Payload is zstd-compressed. |
| Anchor 0.30+ program that ran `anchor idl init` | On-chain, step 1 | Payload is zlib-compressed. Dispatched by magic bytes. |
| A program whose IDL JSON is present in `crates/gulfwatch-ingest/idls/` | Runtime dir, step 2 | Currently seeded with Token-2022 and SPL Token. |
| A program with an IDL dropped into `$GULFWATCH_IDL_DIR` | Runtime dir, step 2 | User override; wins over seed on collision. |

## What does NOT load automatically

These are the common cases where you'll see `·` in the sidebar with a reason
like `not published` or no seed entry to fall back to.

- **Native (non-Anchor) programs with no IDL shipped.** Raydium V4 AMM, OpenBook
  DEX v1, Serum, most pre-Anchor DeFi. No `anchor:idl` PDA exists on-chain,
  no community Codama mirror is maintained, and curating a correct one is
  half a day of work per program. Partial decoding still works via the log
  fallback.
- **Anchor programs that skipped `anchor idl init`.** Some teams ship Anchor
  programs without publishing their IDL on-chain (Jupiter did this before
  migrating to Anchor 0.30). The on-chain fetch returns `account not found`.
  If you have the IDL JSON from the team's repo, drop it into the IDL
  directory and it'll load at next boot.
- **Codama `rootNode` IDLs for programs that aren't in the seed dir.** The
  parser supports them; they just need to land in a directory we scan.
- **IDLs in formats we don't parse yet.** See
  [Supported formats](#supported-idl-formats). An unrecognized format is
  surfaced as `parse failed` in the TUI.

## Adding an IDL (three ways)

### 1. Drop a file in the IDL directory (permanent, no code change)

```
# From repo root, using the default seed dir:
cp ~/Downloads/my-program-idl.json crates/gulfwatch-ingest/idls/

# Or via the override env var (doesn't require touching the repo):
mkdir -p ~/my-gulfwatch-idls
cp ~/Downloads/my-program-idl.json ~/my-gulfwatch-idls/
export GULFWATCH_IDL_DIR=~/my-gulfwatch-idls
```

Relaunch the TUI/server. The program_id is extracted from the IDL itself:

- **Anchor 0.30+ / Codama** — from `address` (Anchor) or `program.publicKey`
  (Codama) at the JSON root.
- **Anchor 0.29 legacy** — from `metadata.address`.
- **Fallback** — the filename stem. So `TokenzQd....json` works even if the
  IDL itself has no address field.

A bad file (non-JSON, wrong format) is logged at `warn` level and skipped —
it won't take down discovery for every other program.

### 2. POST to the running server (ephemeral, per-process)

```
curl -X POST http://localhost:3001/api/programs/<PROGRAM_ID>/idl \
  -H "content-type: application/json" \
  --data @path/to/idl.json
```

Works against any of the three parsed formats. Accepted IDLs update the
server's `AppState` and flip the program's `IdlStatus` to `Loaded`. Rejected
IDLs return HTTP 400 with an `error` field explaining why, and the status
flips to `Unavailable` with that reason.

**Important caveat:** TUI and server run as separate processes with separate
state. A POST to the server does **not** update the TUI's IDL registry, and
vice versa. If you want both surfaces to resolve the same program, use option 1
(the IDL directory — both processes scan it at boot).

IDLs registered via POST do not persist — they're lost when the server
restarts. For permanent registration, use option 1.

### 3. Edit the parser / source (last resort)

If the IDL is in a format we don't support yet (see
[Supported formats](#supported-idl-formats)), the fix is in
`crates/gulfwatch-core/src/idl.rs`, not in your IDL file. See the
[`parse_codama`](../crates/gulfwatch-core/src/idl.rs) function for the
pattern — format detection in `detect_format`, a dedicated `parse_*`
function, and tests.

## Supported IDL formats

The parser auto-detects format and takes the right branch.

| Format | Detection signal | What discriminators look like |
|---|---|---|
| Anchor 0.29 legacy | Root `name` field, no `metadata.spec` | Derived at parse time: `sha256("global:<instruction_name>")[..8]` |
| Anchor 0.30+ (Codama-spec aligned) | `metadata.spec` present | Explicit 8-byte arrays embedded on each instruction |
| Codama rootNode | Root `kind == "rootNode"` or `standard == "codama"` | Extracted from `discriminators[]` + matching `arguments[]` defaults. Supports: numeric u8/u16/u32/u64 (little-endian), hex-encoded `bytesValueNode`. Multi-node discriminators concatenate in offset order. |

**What breaks:**

- **Codama types we don't encode yet.** e.g., `constantValueNode`,
  `base64`-encoded bytes. The parser returns `BadInstruction` with the
  unsupported shape named.
- **Codama discriminators at non-zero offsets with gaps.** Multi-node
  discriminators must form a contiguous run starting at byte 0. A gap (e.g.,
  byte 0 + byte 3) errors out.
- **Anchor-0.29-shaped mirrors of native programs.** These parse fine but
  produce *wrong* discriminators — `sha256("global:<name>")` vs the
  program's real 1-byte enum tag. The sidebar would show `✓` while
  instruction-name resolution silently fails. We deliberately don't ship
  these as seeds. If you drop one in your IDL directory anyway, you'll get
  misleading results — use a Codama version if you can find one.

## Reading the TUI sidebar

Each monitored program gets a glyph next to its row in the left sidebar.

| Glyph | Meaning |
|---|---|
| `⋯` (cyan) | `IdlStatus::Loading` — discovery task is running. |
| `✓` (green) | `IdlStatus::Loaded` — an IDL is registered and actively resolving instructions. |
| `·` (dim) | `IdlStatus::Unavailable` — no IDL for this program. A one-line reason is rendered below the row (yellow text), summarizing the on-chain error. |

Common short reasons you'll see under `·`:

- `not published` — program doesn't have an `anchor:idl` on-chain account
  (native program, or Anchor program that skipped `anchor idl init`). Next
  step: drop the IDL in `idls/` if you have one.
- `unknown on-chain format` — account exists but the payload isn't zstd or
  zlib. File a bug with the program_id.
- `rpc error` — transient RPC issue. Often resolves on restart.
- `parse failed` — account fetched and decompressed, but the JSON isn't in a
  format we parse. Unlikely in practice.

## Log fallback

Separate from IDL loading. After IDL resolution runs on every transaction,
the pipeline does a second pass for instructions whose `anchor_name` is still
empty. It looks at `Program log: Instruction: <name>` lines from
`meta.logMessages`, matches them to unresolved instructions by program_id in
execution order, and fills them in.

This means you can get readable instruction names (e.g., `Transfer`,
`CloseAccount`) for programs that have no IDL at all, as long as the program
emits the conventional instruction log line. IDL-resolved names remain
authoritative — the log fallback only fills gaps, never overrides.

The sidebar glyph does **not** flip to `✓` based on log-fallback hits, which
is intentional: log names are instructions, not an IDL. Error codes,
account layouts, and argument types still need a real IDL.

## Debugging a missing IDL

When you add a program and expect it to resolve but it doesn't:

1. **Check the sidebar glyph and its reason line.** The reason tells you
   which stage failed (on-chain not published, parse failed, etc.).
2. **If reason is `not published`:** the program has no on-chain IDL. Find
   the IDL JSON (program team's GitHub is the usual place) and drop it in
   `crates/gulfwatch-ingest/idls/` or `$GULFWATCH_IDL_DIR`.
3. **If reason is `parse failed` and you dropped a file:** your IDL is in a
   format we don't recognize. Run
   `cargo test -p gulfwatch-core --lib idl::tests` for format examples we do
   support. The server's `warn!` log on upload rejection also shows the
   specific parse error.
4. **If the glyph stays `⋯` (Loading) forever:** RPC call is hanging. Check
   `SOLANA_RPC_URL` and network connectivity.
5. **If the glyph is `✓` but transactions still show opaque types:** the IDL
   loaded but its discriminators don't match what the program actually
   dispatches on. Most commonly this happens when a native program's IDL is
   mirrored in Anchor-0.29 shape (see
   [Supported formats](#supported-idl-formats) above). Remove the bad IDL;
   let the log fallback take over.

The ignored `#[ignore]` tests in `crates/gulfwatch-ingest/src/discover.rs`
(`probe_onchain_idl_layouts`, `fetch_onchain_idl_end_to_end_jupiter_v6`) are
useful for diagnosing live mainnet behavior — run with
`cargo test -- --ignored --nocapture`.
