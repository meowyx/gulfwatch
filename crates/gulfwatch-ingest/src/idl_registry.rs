use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gulfwatch_core::{parse_idl_json, IdlDocument};
use tracing::{debug, warn};

// Repo-relative seed directory, baked at compile time.
pub fn default_idl_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("idls")
}

pub fn user_idl_dir() -> Option<PathBuf> {
    std::env::var("GULFWATCH_IDL_DIR").ok().map(PathBuf::from)
}

pub struct ScannedIdl {
    pub program_id: String,
    pub idl: IdlDocument,
    pub source_path: PathBuf,
}

// Bad drop-ins are logged and skipped rather than aborting discovery.
pub fn scan_idl_directory(dir: &Path) -> Vec<ScannedIdl> {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            debug!(
                path = %dir.display(),
                error = %e,
                "IDL directory not readable (this is fine if it doesn't exist)"
            );
            return Vec::new();
        }
    };

    let mut out = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "failed to read IDL file");
                continue;
            }
        };
        let idl = match parse_idl_json(&bytes) {
            Ok(i) => i,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "failed to parse IDL file");
                continue;
            }
        };
        let filename_stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());
        let program_id = match idl.address.clone().or(filename_stem) {
            Some(pid) => pid,
            None => {
                warn!(path = %path.display(), "IDL has no address and no usable filename stem");
                continue;
            }
        };
        out.push(ScannedIdl {
            program_id,
            idl,
            source_path: path,
        });
    }
    out
}

// User dir overrides seed on program_id collisions.
pub fn load_idl_registry() -> HashMap<String, IdlDocument> {
    let mut map: HashMap<String, IdlDocument> = HashMap::new();
    for s in scan_idl_directory(&default_idl_dir()) {
        map.insert(s.program_id, s.idl);
    }
    if let Some(user) = user_idl_dir() {
        for s in scan_idl_directory(&user) {
            if map.contains_key(&s.program_id) {
                debug!(
                    program_id = %s.program_id,
                    source = %s.source_path.display(),
                    "user IDL override takes precedence over seed"
                );
            }
            map.insert(s.program_id, s.idl);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn unique_dir(name: &str) -> PathBuf {
        let mut d = std::env::temp_dir();
        d.push(format!(
            "gulfwatch-idl-test-{}-{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn scan_empty_dir_returns_nothing() {
        let dir = unique_dir("empty");
        let out = scan_idl_directory(&dir);
        assert!(out.is_empty());
    }

    #[test]
    fn scan_nonexistent_dir_returns_nothing_without_panic() {
        let path = PathBuf::from("/definitely/does/not/exist/xyz-42");
        let out = scan_idl_directory(&path);
        assert!(out.is_empty());
    }

    #[test]
    fn scan_extracts_program_id_from_idl_address_field() {
        let dir = unique_dir("address-field");
        let v030 = r#"{
          "address":"TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
          "metadata":{"name":"x","version":"0.1.0","spec":"0.1.0"},
          "instructions":[{"name":"swap","discriminator":[1,2,3,4,5,6,7,8]}]
        }"#;
        fs::write(dir.join("weird-filename.json"), v030).unwrap();

        let out = scan_idl_directory(&dir);
        assert_eq!(out.len(), 1);
        // Address from the IDL beats the filename.
        assert_eq!(
            out[0].program_id,
            "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"
        );
    }

    #[test]
    fn scan_falls_back_to_filename_stem_when_idl_has_no_address() {
        let dir = unique_dir("filename-fallback");
        let legacy = r#"{"name":"my_prog","instructions":[]}"#;
        fs::write(dir.join("SomeProgramId1234567890.json"), legacy).unwrap();

        let out = scan_idl_directory(&dir);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].program_id, "SomeProgramId1234567890");
    }

    #[test]
    fn scan_extracts_program_id_from_codama_public_key() {
        let dir = unique_dir("codama-pk");
        let codama = r#"{
          "kind":"rootNode",
          "program":{"name":"t","publicKey":"TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"}
        }"#;
        fs::write(dir.join("spl-token.json"), codama).unwrap();

        let out = scan_idl_directory(&dir);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].program_id, "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
    }

    #[test]
    fn scan_skips_non_json_and_malformed_files_without_aborting() {
        let dir = unique_dir("mixed");
        fs::write(dir.join("README.md"), "not json").unwrap();
        fs::write(dir.join("broken.json"), "{ this is not valid").unwrap();
        fs::write(
            dir.join("good.json"),
            r#"{"name":"ok","instructions":[]}"#,
        )
        .unwrap();

        let out = scan_idl_directory(&dir);
        assert_eq!(
            out.len(),
            1,
            "only the good JSON should have loaded, got {:?}",
            out.iter().map(|s| &s.program_id).collect::<Vec<_>>()
        );
        assert_eq!(out[0].program_id, "good");
    }

    #[test]
    fn seed_dir_scan_finds_token2022_and_spl_token() {
        let out = scan_idl_directory(&default_idl_dir());
        let ids: std::collections::HashSet<&str> =
            out.iter().map(|s| s.program_id.as_str()).collect();
        assert!(
            ids.contains("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"),
            "seed dir should expose Token-2022, got {ids:?}"
        );
        assert!(
            ids.contains("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"),
            "seed dir should expose SPL Token, got {ids:?}"
        );
    }
}
