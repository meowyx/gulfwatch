use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use crossterm::execute;
use crossterm::style::{Attribute, Color as CtColor, Print, ResetColor, SetAttribute, SetForegroundColor};

use crate::init_wizard;

const CONFIG_FILENAME: &str = "gulfwatch.toml";

fn render_template(ws_url: &str, rpc_url: &str) -> String {
    format!(
        r#"# GulfWatch config
# Re-run `gulfwatch` from this directory after any change.

# ─── Required ──────────────────────────────────────────────────────────────

# Solana WebSocket endpoint. GulfWatch streams transactions from here.
# Get one from Helius, Quicknode, Triton, or any Solana RPC provider.
# Format: wss://...
solana_ws_url = "{ws}"

# Solana HTTPS RPC endpoint. Used to fetch IDLs and account state on demand.
# Usually the HTTPS endpoint paired with your WebSocket URL above.
# Format: https://...
solana_rpc_url = "{rpc}"

# ─── Programs to monitor ───────────────────────────────────────────────────

# Solana program IDs to track. Add any program you care about.
# Common examples:
#   TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA  - SPL Token
#   TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb  - Token-2022
#   675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8 - Raydium AMM v4
#   CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK - Raydium CLMM
#   CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C - Raydium CPMM
#   JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4  - Jupiter Aggregator v6
monitor_programs = [
    "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
]

# ─── Alerting (optional) ───────────────────────────────────────────────────

# Accounts to flag when they appear as the source or destination of a large
# transfer. Useful for monitoring treasury wallets, exchange hot wallets,
# or known bad actors. Combine with `large_transfer_threshold` below.
# watched_accounts = [
#     "DQyrAcCrDXQ7NeoqGgDCZwBvWDcYmFCjSb1JtteuC5BZ",
# ]

# Lamports threshold for the "large transfer" detection.
# 1 SOL = 1_000_000_000 lamports. Default below = 10 SOL.
# Without this set, the large-transfer detection stays effectively off.
# large_transfer_threshold = 10000000000

# ─── Metrics window ────────────────────────────────────────────────────────

# Rolling window (in minutes) for the dashboard's tx/sec, error-rate, and
# CU metrics. Smaller is more reactive but noisier. Default: 10.
# rolling_window_minutes = 10

# ─── Cross-program correlation detection ───────────────────────────────────

# Fires an alert when a single transaction touches at least
# `correlation_min_programs` of the monitored programs within
# `correlation_window_secs`. Helpful for spotting arbitrage / sandwich
# patterns that hop across multiple AMMs.
# correlation_min_programs = 3
# correlation_window_secs = 300

# ─── Embedded HTTP server ──────────────────────────────────────────────────

# Address the HTTP server binds to. Routes exposed:
#
#   GET  /api/programs              - list / manage monitored programs
#   GET  /api/transactions/recent   - recent transactions (with filters)
#   GET  /api/transactions/{{sig}}    - full detail for one signature
#   GET  /api/alerts/recent         - alerts that fired in the rolling window
#   GET  /api/alerts                - manage alert rules
#   GET  /api/metrics/summary       - tx/sec, error rate, CU summary
#   GET  /api/metrics/timeseries    - timeseries metrics for charts
#   GET  /metrics                   - Prometheus scrape endpoint
#   WS   /ws/feed                   - live transaction + alert stream
#
# Default: 0.0.0.0:3001. Change if the port is taken.
# listen_addr = "0.0.0.0:3001"

# ─── AI agents via MCP ─────────────────────────────────────────────────────
#
# GulfWatch ships a companion MCP server (`gulfwatch-mcp`) that lets you ask
# natural-language questions about your stream from Claude Code, Claude
# Desktop, or any MCP client. It is read-only — it just calls the HTTP API
# above.
#
# Install once:
#     cargo install gulfwatch-mcp
#
# Add to your MCP client config (e.g. Claude Code's ~/.claude.json):
#     {{
#       "mcpServers": {{
#         "gulfwatch": {{ "command": "gulfwatch-mcp" }}
#       }}
#     }}
#
# Then ask things like:
#     - "Why did transaction <signature> fail?"
#     - "What alerts fired in the last hour?"
#     - "What's the error rate on Raydium right now?"
#     - "Show me the most recent large transfers."
#
# The MCP server reads GULFWATCH_BASE_URL (default http://localhost:3001),
# so it just works as long as `gulfwatch` is running.
"#,
        ws = escape_toml(ws_url),
        rpc = escape_toml(rpc_url),
    )
}

pub fn init() {
    load_dotenv_walk();

    if let Some(path) = find_config_toml() {
        if let Err(e) = apply_toml_as_env(&path) {
            eprintln!("gulfwatch: failed to load {}: {e}", path.display());
            std::process::exit(1);
        }
        if !required_config_present() {
            eprintln!("gulfwatch: required fields missing in {}", path.display());
            eprintln!("Set solana_ws_url, solana_rpc_url, and monitor_programs, then re-run.");
            std::process::exit(1);
        }
        return;
    }

    if required_config_present() {
        return;
    }

    let Ok(cwd) = std::env::current_dir() else {
        eprintln!("gulfwatch: could not determine current directory");
        std::process::exit(1);
    };

    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    if !interactive {
        write_template_fallback(&cwd);
    }

    match init_wizard::run(cwd.clone()) {
        Ok(Some(setup)) => {
            let path = setup.dir.join(CONFIG_FILENAME);
            let contents = render_filled_template(&setup);
            if let Err(e) = std::fs::write(&path, contents) {
                eprintln!("gulfwatch: failed to write {}: {e}", path.display());
                std::process::exit(1);
            }
            let gitignore_added = ensure_gitignored(&setup.dir);
            print_success(&path, &setup.dir, &cwd, gitignore_added);
            std::process::exit(0);
        }
        Ok(None) => std::process::exit(0),
        Err(e) => {
            eprintln!("gulfwatch: wizard error: {e}");
            std::process::exit(1);
        }
    }
}

fn write_template_fallback(cwd: &Path) -> ! {
    let path = cwd.join(CONFIG_FILENAME);
    if let Err(e) = std::fs::write(&path, render_template("", "")) {
        eprintln!("gulfwatch: failed to write {}: {e}", path.display());
        std::process::exit(1);
    }
    eprintln!("Welcome to GulfWatch. Wrote config template to:");
    eprintln!("  {}", path.display());
    eprintln!();
    eprintln!("Edit it with your Solana RPC URLs, then re-run `gulfwatch` from this directory.");
    std::process::exit(0);
}

fn render_filled_template(setup: &init_wizard::Setup) -> String {
    render_template(&setup.ws_url, &setup.rpc_url)
}

fn escape_toml(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn print_success(config_path: &Path, target_dir: &Path, cwd: &Path, gitignore_added: bool) {
    let green = CtColor::Rgb { r: 0x9f, g: 0xd6, b: 0x7a };
    let amber = CtColor::Rgb { r: 0xe8, g: 0xb7, b: 0x5a };
    let same_dir = target_dir == cwd;

    let mut stdout = io::stdout();
    let _ = execute!(
        stdout,
        Print("\n"),
        SetForegroundColor(green),
        SetAttribute(Attribute::Bold),
        Print("✓ All set."),
        SetAttribute(Attribute::Reset),
        ResetColor,
        Print(format!("\n\n  Config saved at {}\n", config_path.display())),
    );

    if gitignore_added {
        let _ = execute!(
            stdout,
            Print("  Added gulfwatch.toml to .gitignore (it holds your RPC keys).\n"),
        );
    }

    let _ = execute!(stdout, Print("\n  Next steps:\n"));

    if !same_dir {
        if let Some(name) = target_dir.file_name() {
            let _ = execute!(
                stdout,
                SetForegroundColor(amber),
                Print(format!("    cd {}\n", name.to_string_lossy())),
                ResetColor,
            );
        }
    }

    let _ = execute!(
        stdout,
        SetForegroundColor(amber),
        Print("    gulfwatch\n"),
        ResetColor,
        Print("\n  Want to chat with your stream? Install the MCP companion:\n"),
        SetForegroundColor(amber),
        Print("    cargo install gulfwatch-mcp\n"),
        ResetColor,
        Print("  Then add it to Claude Code / Claude Desktop and ask things like\n"),
        Print("  \"why did this tx fail\" or \"what alerts fired today\".\n"),
        Print("\n  See gulfwatch.toml for the full list of options.\n\n"),
    );
}

fn ensure_gitignored(dir: &Path) -> bool {
    if !inside_git_repo(dir) {
        return false;
    }
    let path = dir.join(".gitignore");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == CONFIG_FILENAME) {
        return false;
    }
    let mut new_contents = existing;
    if !new_contents.is_empty() && !new_contents.ends_with('\n') {
        new_contents.push('\n');
    }
    if !new_contents.is_empty() {
        new_contents.push('\n');
    }
    new_contents.push_str("# GulfWatch config (contains RPC API keys)\n");
    new_contents.push_str(CONFIG_FILENAME);
    new_contents.push('\n');
    std::fs::write(&path, new_contents).is_ok()
}

fn inside_git_repo(start: &Path) -> bool {
    let mut cur = start.to_path_buf();
    loop {
        if cur.join(".git").exists() {
            return true;
        }
        if !cur.pop() {
            return false;
        }
    }
}

fn find_config_toml() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join(CONFIG_FILENAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn apply_toml_as_env(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let value: toml::Value = toml::from_str(&text).map_err(|e| e.to_string())?;
    let table = value.as_table().ok_or("config must be a TOML table")?;
    for (key, v) in table {
        let env_key = key.to_ascii_uppercase();
        if std::env::var(&env_key).is_ok() {
            continue;
        }
        let env_value = toml_value_to_env(v);
        if env_value.is_empty() {
            continue;
        }
        // SAFETY: called before any threads are spawned
        unsafe { std::env::set_var(&env_key, env_value); }
    }
    Ok(())
}

fn toml_value_to_env(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Array(items) => items
            .iter()
            .map(|item| match item {
                toml::Value::String(s) => s.clone(),
                _ => item.to_string(),
            })
            .collect::<Vec<_>>()
            .join(","),
        other => other.to_string(),
    }
}

fn required_config_present() -> bool {
    std::env::var("SOLANA_WS_URL").is_ok()
        && std::env::var("SOLANA_RPC_URL").is_ok()
        && (std::env::var("MONITOR_PROGRAMS").is_ok()
            || std::env::var("MONITOR_PROGRAM").is_ok())
}

fn load_dotenv_walk() {
    let mut dir = std::env::current_dir().ok();
    while let Some(d) = dir {
        let env_file = d.join(".env");
        if env_file.exists() {
            if let Ok(contents) = std::fs::read_to_string(&env_file) {
                for line in contents.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    if let Some((key, value)) = line.split_once('=') {
                        let key = key.trim();
                        let value = value.trim().trim_matches('"').trim_matches('\'');
                        if std::env::var(key).is_err() {
                            // SAFETY: called before any threads are spawned
                            unsafe { std::env::set_var(key, value); }
                        }
                    }
                }
            }
            break;
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
}
