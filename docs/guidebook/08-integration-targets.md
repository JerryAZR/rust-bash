# Chapter 8: Integration Targets

## Overview

This fork maintains exactly one integration surface: the **Rust crate**, embedded
directly in an agent harness. Upstream's other targets (CLI binary, C FFI, WASM,
npm package, MCP server, AI SDK tool definitions) were removed — see
`docs/design/fork-scope.md`. This chapter covers the crate API and the embedding
pattern the fork is built around.

## Rust Crate API

The only interface. The embedding patterns below build on it.

```rust
use rust_bash::{RustBashBuilder, ExecResult};
use std::collections::HashMap;

let mut shell = RustBashBuilder::new()
    .files(HashMap::from([
        ("/data.txt".into(), b"hello world".to_vec()),
        ("/config.json".into(), b"{}".to_vec()),
    ]))
    .env(HashMap::from([
        ("USER".into(), "agent".into()),
        ("HOME".into(), "/home/agent".into()),
    ]))
    .cwd("/")
    .build()
    .unwrap();

let result: ExecResult = shell.exec("cat /data.txt | grep hello").unwrap();
assert_eq!(result.stdout, "hello world\n");
assert_eq!(result.exit_code, 0);
```

### RustBashBuilder

```rust
RustBashBuilder::new()
    .files(HashMap<String, Vec<u8>>)     // Seed VFS with files (path → bytes)
    .env(HashMap<String, String>)        // Set environment variables
    .cwd("/path")                        // Set working directory (created automatically)
    .execution_limits(limits)            // Configure limits
    .fs(Arc<dyn VirtualFs>)              // Use a custom filesystem backend
    .command(Box::new(custom_cmd))       // Register a custom command
    .abort_on_unresolved_commands(bool)  // Stop the script at the first unknown command
                                         // (default false: bash-fidelity continue, exit 127)
    .build()                             // Returns Result<RustBash, RustBashError>
```

### ExecResult

```rust
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub unresolved_commands: Vec<String>,
}
```

`unresolved_commands` lists command names that failed resolution ("command not
found"), deduplicated in first-encountered order. This lets agent harnesses
detect that the sandbox cannot run a script and rerun it natively on the host.
By default execution continues after a miss (bash fidelity); with
`abort_on_unresolved_commands(true)` the first miss stops the script and the
result carries the output accumulated so far.

### Pre-Flight Command Analysis

```rust
let analysis = shell.analyze_commands("git status && kubectl get pods").unwrap();
assert_eq!(analysis.unresolved, vec!["git", "kubectl"]);
```

`RustBash::analyze_commands(&self, script) -> Result<CommandAnalysis, RustBashError>`
parses the script and statically walks the AST collecting every literal
simple-command name (including names inside function bodies, subshells, and
compound-command bodies) without executing anything. `CommandAnalysis::commands`
holds all collected names; `CommandAnalysis::unresolved` filters out names that
resolve: builtins, commands registered on the instance, and functions defined on
the instance or within the script itself. Dynamic names (`eval "..."`,
`$cmd status`) are not statically analyzable and are not reported.

## Embedding in an Agent Harness

The fork's raison d'être — the full pattern (shared `Arc<OverlayFs>`, per-exec
`diff()`, caller-side apply, `sync()`) is documented in:

- [Agent Sandbox Integration recipe](../recipes/agent-sandbox-integration.md)
- [`examples/agent_harness.rs`](../../examples/agent_harness.rs) — runnable reference
- [Shared-overlay Python sandbox design](../design/python-sandbox-shared-fs.md) —
  extending the same overlay to a second (WASI CPython) tool, with no rust-bash
  changes

Key points for embedders:

- The shell and the overlay are independent `Arc`s; the host keeps its own
  `Arc<OverlayFs>` handle and calls `diff()`/`sync()`/`reset()` on it.
- `VirtualFs: Send + Sync`, so a single overlay can back multiple tools and
  sequential `exec()` calls (or even recreated shells) without losing pending
  writes.
- Custom commands implement `VirtualCommand` and register via
  `RustBashBuilder::command(...)`.
- With the `python` feature, sandboxed CPython runs as a *second client* of
  the same `Arc<OverlayFs>` (`python::PythonInterpreter`) — bash and Python
  share pending writes and one `diff()`; see
  [`examples/python_overlay.rs`](../../examples/python_overlay.rs). Python is
  stdlib-only glue by design; `python` stays an unresolved command in bash
  so project Python work offloads to the host. **Caveat:** guest execution
  limits are opt-in (`PythonLimits` — fuel, `max_file_size`); without them
  the guest runs unbounded. The feature requires the CPython artifact:
  `scripts/fetch-python-wasm.sh`.
