use std::path::{Path, PathBuf};

use directories::ProjectDirs;

const TEMPLATE: &str = r#"# GulfWatch config. Fill in your Solana RPC URLs, save, and re-run `gulfwatch`.
#
# Get RPC endpoints from Helius, Quicknode, Triton, or any Solana RPC provider.

# Required
solana_ws_url = ""
solana_rpc_url = ""

# Programs to monitor
monitor_programs = [
    "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
]

# Optional
# watched_accounts = []
# large_transfer_threshold = 10000000000
# rolling_window_minutes = 10
# correlation_min_programs = 3
# correlation_window_secs = 300
# listen_addr = "0.0.0.0:3001"
"#;

pub fn init() {
    load_dotenv_walk();

    let Some(path) = config_toml_path() else {
        return;
    };

    if path.exists() {
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

    if let Err(e) = write_template(&path) {
        eprintln!("gulfwatch: failed to write {}: {e}", path.display());
        std::process::exit(1);
    }
    eprintln!("Welcome to GulfWatch. Wrote config template to:");
    eprintln!("  {}", path.display());
    eprintln!();
    eprintln!("Edit it with your Solana RPC URLs, then re-run `gulfwatch`.");
    std::process::exit(0);
}

fn config_toml_path() -> Option<PathBuf> {
    ProjectDirs::from("", "", "gulfwatch").map(|p| p.config_dir().join("config.toml"))
}

fn write_template(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, TEMPLATE)
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
