# Filesystem Backends

## Goal

Choose and configure the right virtual filesystem backend for your use case: fully sandboxed, copy-on-write over real files, or a composite of both.

## Overview

| Backend | Reads from | Writes to | Host access | Best for |
|---------|-----------|-----------|-------------|----------|
| `InMemoryFs` | Memory | Memory | None | Sandboxed execution, testing, AI agents |
| `OverlayFs` | Disk (lower) + Memory (upper) | Memory only | Read-only | Code analysis, safe experimentation |
| `MountableFs` | Delegated per mount | Delegated per mount | Depends on mounts | Composite environments |

## InMemoryFs (Default)

This is what you get with `RustBashBuilder::new().build()`. All data lives in memory; the host filesystem is never touched.

```rust
use rust_bash::RustBashBuilder;
use std::collections::HashMap;

let mut shell = RustBashBuilder::new()
    .files(HashMap::from([
        ("/src/main.rs".into(), b"fn main() {}".to_vec()),
        ("/src/lib.rs".into(), b"pub fn hello() {}".to_vec()),
    ]))
    .build()
    .unwrap();

// Files exist only in memory
let result = shell.exec("find / -name '*.rs'").unwrap();
assert!(result.stdout.contains("/src/main.rs"));
assert!(result.stdout.contains("/src/lib.rs"));

// Writes stay in memory — no host files are created
shell.exec("echo new > /src/new.rs").unwrap();
```

## OverlayFs — Read Real Files, Sandbox Writes

Reads from a real directory on disk but all mutations stay in memory. The disk is never modified.

```rust
use rust_bash::{RustBashBuilder, OverlayFs};
use std::sync::Arc;

// Point at a real directory on the host
let overlay = OverlayFs::new("./my_project").unwrap();
let mut shell = RustBashBuilder::new()
    .fs(Arc::new(overlay))
    .cwd("/")
    .build()
    .unwrap();

// Read files from disk (paths are relative to the overlay root)
let result = shell.exec("cat /Cargo.toml").unwrap();
println!("{}", result.stdout); // actual Cargo.toml contents

// Writes go to the in-memory upper layer
shell.exec("echo modified > /Cargo.toml").unwrap();
let result = shell.exec("cat /Cargo.toml").unwrap();
assert_eq!(result.stdout, "modified\n"); // reads the in-memory version

// Disk file is untouched:
// assert_eq!(std::fs::read_to_string("./my_project/Cargo.toml"), original)
```

### Deletions are tracked with whiteouts

```rust
use rust_bash::{RustBashBuilder, OverlayFs};
use std::sync::Arc;

let overlay = OverlayFs::new("./my_project").unwrap();
let mut shell = RustBashBuilder::new()
    .fs(Arc::new(overlay))
    .cwd("/")
    .build()
    .unwrap();

// Delete a file that exists on disk — it becomes invisible but the disk file remains
shell.exec("rm /README.md").unwrap();
let result = shell.exec("cat /README.md").unwrap();
assert_ne!(result.exit_code, 0); // file appears deleted
// But on disk: std::fs::metadata("./my_project/README.md").is_ok() == true
```

## MountableFs — Combine Backends

Delegate different path prefixes to different backends. Longest-prefix matching determines which backend handles each operation.

```rust
use rust_bash::{RustBashBuilder, InMemoryFs, MountableFs, OverlayFs};
use std::sync::Arc;

let mountable = MountableFs::new()
    .mount("/", Arc::new(InMemoryFs::new()))                             // in-memory root
    .mount("/project", Arc::new(OverlayFs::new("./myproject").unwrap())) // overlay on real project
    .mount("/tmp", Arc::new(InMemoryFs::new()));                         // separate temp space

let mut shell = RustBashBuilder::new()
    .fs(Arc::new(mountable))
    .cwd("/")
    .build()
    .unwrap();

// /project reads from disk via OverlayFs
shell.exec("cat /project/Cargo.toml").unwrap();

// /tmp is a separate in-memory space
shell.exec("echo scratch > /tmp/work.txt").unwrap();

// / is the default in-memory backend
shell.exec("echo hello > /root-file.txt").unwrap();
```

### Real-world example: multi-directory analysis workspace

```rust
use rust_bash::{RustBashBuilder, InMemoryFs, MountableFs, OverlayFs};
use std::sync::Arc;

let mountable = MountableFs::new()
    .mount("/", Arc::new(InMemoryFs::new()))
    .mount("/project", Arc::new(OverlayFs::new("./myproject").unwrap()))
    .mount("/fixtures", Arc::new(OverlayFs::new("./test-fixtures").unwrap()));

let mut shell = RustBashBuilder::new()
    .fs(Arc::new(mountable))
    .cwd("/")
    .build()
    .unwrap();

// Both real directories are readable side by side
shell.exec("diff -r /project/expected /fixtures/expected").unwrap();

// Reports land in the in-memory root — the host is never modified
shell.exec("grep -r TODO /project > /todo-report.txt").unwrap();
```

## Seeding Files from a Host Directory

The builder's `.files()` method accepts a `HashMap<String, Vec<u8>>`. To load files from a host directory:

```rust
use rust_bash::RustBashBuilder;
use std::collections::HashMap;
use std::path::Path;

fn load_dir(dir: &Path, prefix: &str) -> HashMap<String, Vec<u8>> {
    let mut files = HashMap::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = format!("{prefix}/{}", entry.file_name().to_string_lossy());
            if path.is_file() {
                if let Ok(data) = std::fs::read(&path) {
                    files.insert(name, data);
                }
            } else if path.is_dir() {
                files.extend(load_dir(&path, &name));
            }
        }
    }
    files
}

let files = load_dir(Path::new("./test-fixtures"), "");
let mut shell = RustBashBuilder::new()
    .files(files)
    .build()
    .unwrap();
```

This copies files into the InMemoryFs at build time. For large directories, prefer `OverlayFs` to avoid the upfront memory cost.
