//! Architecture guard tests: mechanically enforce the crate's sandboxing
//! rules by scanning `src/` sources.
//!
//! 1. **No process spawning** — `std::process::Command` must not appear in
//!    interpreter or command code; all commands are in-process Rust
//!    implementations. (`std::process::id()` for entropy mixing is fine and
//!    is not matched.)
//! 2. **No direct filesystem access** — `std::fs` may only be used inside
//!    `src/vfs/` (the OverlayFs lower layer and shared helpers) and
//!    `src/platform.rs`. Everything else must go through `VirtualFs`.
//!
//! Tests may use `std::fs`; this file walks `src/` with it.

use std::path::{Path, PathBuf};

/// Files allowed to reference `std::process::Command`, relative to `src/`.
/// Empty today — nothing in `src/` spawns processes.
const PROCESS_COMMAND_ALLOWLIST: &[&str] = &[];

/// Files (relative to `src/`) allowed to use `std::fs` in addition to
/// everything under `src/vfs/`. `platform.rs` is a policy allowance (the
/// platform-abstraction home); it does not use `std::fs` today.
const STD_FS_ALLOWLIST: &[&str] = &["platform.rs"];

/// Recursively collect all `.rs` files under `dir`, returned relative to
/// `dir` with `/` separators (stable across platforms).
fn collect_rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_rs_files_into(dir, dir, &mut out);
    out.sort();
    out
}

fn collect_rs_files_into(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files_into(root, &path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path.strip_prefix(root).unwrap().to_path_buf());
        }
    }
}

/// `/`-separated relative path string for allowlist comparisons.
fn rel_string(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

#[test]
fn no_std_process_command_in_src() {
    let src = src_dir();
    let mut offenders = Vec::new();
    for rel in collect_rs_files(&src) {
        let rel_str = rel_string(&rel);
        if PROCESS_COMMAND_ALLOWLIST.contains(&rel_str.as_str()) {
            continue;
        }
        let content = std::fs::read_to_string(src.join(&rel)).unwrap();
        // Substring bans catch every import form (`use std::process::
        // {Command}`, aliased `use ... as`, fully-qualified paths).
        if content.contains("process::Command") || content.contains("process::Command::new") {
            offenders.push(rel_str);
        }
    }
    assert!(
        offenders.is_empty(),
        "std::process::Command is banned in src/ (all commands are in-process \
         Rust implementations); offending files: {offenders:?}"
    );
}

#[test]
fn no_process_exit_or_host_network_in_src() {
    // `std::process::exit` would terminate the EMBEDDING HOST process; the
    // `exit` builtin must go through the interpreter's should_exit flag.
    // `std::net` bypasses the sandbox entirely (no network in rust-bash).
    const BANNED: &[&str] = &["process::exit", "std::net::"];
    let src = src_dir();
    let mut offenders = Vec::new();
    for rel in collect_rs_files(&src) {
        let rel_str = rel_string(&rel);
        let content = std::fs::read_to_string(src.join(&rel)).unwrap();
        for needle in BANNED {
            if content.contains(needle) {
                offenders.push(format!("{rel_str} ({needle})"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "process::exit / std::net are banned in src/ (they terminate or \
         escape the embedding host); offending: {offenders:?}"
    );
}

#[test]
fn no_std_fs_outside_vfs_and_platform() {
    let src = src_dir();
    let mut offenders = Vec::new();
    for rel in collect_rs_files(&src) {
        let rel_str = rel_string(&rel);
        if rel_str.starts_with("vfs/") || STD_FS_ALLOWLIST.contains(&rel_str.as_str()) {
            continue;
        }
        let content = std::fs::read_to_string(src.join(&rel)).unwrap();
        if content.contains("std::fs") {
            offenders.push(rel_str);
        }
    }
    assert!(
        offenders.is_empty(),
        "std::fs is only allowed under src/vfs/ and src/platform.rs (all file \
         operations must go through VirtualFs); offending files: {offenders:?}"
    );
}

#[test]
fn divergence_registry_references_resolve() {
    // Every `tests/….rs[::test_name]` reference in the known-divergences
    // registry must point at a real file (and a real #[test] fn when a
    // name is given). Stale pins rot silently otherwise — a "pinning"
    // reference to a deleted test is worse than none.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let registry = std::fs::read_to_string(root.join("docs/guidebook/11-known-divergences.md"))
        .expect("registry must exist");
    let mut bad = Vec::new();
    let mut rest = registry.as_str();
    while let Some(start) = rest.find("`tests/") {
        let after = &rest[start + 1..];
        let end = after.find('`').expect("unterminated backtick in registry");
        let reference = &after[..end];
        rest = &after[end..];
        let (file, test_name) = match reference.split_once("::") {
            Some((f, t)) => (f, Some(t)),
            None => (reference, None),
        };
        let path = root.join(file);
        let exists = if file.contains('*') {
            // Glob reference (e.g. `tests/fixtures/comparison/text/*.toml`):
            // the parent dir must contain at least one match.
            let parent = path.parent().unwrap().to_path_buf();
            let suffix = file.rsplit('*').next().unwrap_or_default().to_string();
            std::fs::read_dir(&parent).is_ok_and(|mut rd| {
                rd.any(|e| e.is_ok_and(|e| e.file_name().to_string_lossy().ends_with(&suffix)))
            })
        } else {
            path.exists()
        };
        if !exists {
            bad.push(format!("{reference}: file does not exist"));
            continue;
        }
        if file.contains('*') {
            continue; // no per-test-name check for glob references
        }
        if let Some(name) = test_name {
            let content = std::fs::read_to_string(&path).unwrap();
            if !content.contains(&format!("fn {name}(")) {
                bad.push(format!("{reference}: no `fn {name}(` in {file}"));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "stale divergence-registry references:\n{}",
        bad.join("\n")
    );
}
