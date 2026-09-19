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
        if content.contains("std::process::Command") || content.contains("process::Command::new") {
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
