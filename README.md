# rust-bash

A bash interpreter for AI agent harnesses, built in Rust. Run agent-generated scripts in-process over your real project directory — reads come from disk, **writes stay in memory until you commit them**, and commands the sandbox can't run are **reported back so you can fall back to a real shell**. No containers, no VMs, no subprocesses.

This is a fork of [shantanugoel/rust-bash](https://github.com/shantanugoel/rust-bash) scoped down to a single use case: a library embedded in an agent harness (see [fork scope](docs/design/fork-scope.md)). Upstream's distribution surface — CLI binary, npm/WASM packages, C FFI, MCP server, browser showcase — is not maintained here.

## Highlights

- **Selective, caller-side commits** — `OverlayFs` runs scripts over a real directory: reads from disk, writes to an in-memory layer, disk never touched. `diff()` exports the exact write set so *you* decide per path what to apply, prompt about, or discard; `sync()` reconciles after applying.
- **Unknown-command signaling** — `analyze_commands()` (pre-flight) and `ExecResult::unresolved_commands` (runtime backstop) tell the caller exactly which commands the sandbox can't run, so the harness can rerun the whole script natively (e.g. Git Bash on Windows).
- **Cross-platform** — runs on Linux, macOS, and Windows (CI-tested on Linux and Windows). VFS paths are Unix-style (`/`-separated) everywhere.
- **80 commands** — echo, cat, grep, awk, sed, jq, find, sort, diff, tar, and many more.
- **Full bash syntax** — pipelines, redirections, variables, control flow, functions, command substitution, globs, brace expansion, arithmetic, here-documents, case statements.
- **Execution limits** — optional, caller-configured bounds, off by default (time, commands, loops, output size, call depth, string length, glob results, substitution depth, heredoc size, brace expansion, array elements).
- **Multiple filesystem backends** — InMemoryFs (default), OverlayFs (copy-on-write), MountableFs (composite).
- **Sandboxed Python companion** *(feature `python`)* — CPython (wasm32-wasip1 under wasmtime, stdlib-only) runs against the *same* `VirtualFs` as the shell: bash and Python see each other's pending writes, and one `overlay.diff()` reports both. See [`examples/python_overlay.rs`](examples/python_overlay.rs). Requires the CPython artifact: `scripts/fetch-python-wasm.sh`.
- **Embeddable** — use as a Rust crate with a builder API. Custom commands via the `VirtualCommand` trait.

## What this promises (and what it doesn't)

The problem this fork solves is **approval fatigue**. Approving every agent
command by hand doesn't scale; running everything directly on disk is
careless. rust-bash lets the harness run scripts unattended and compresses
review into one small, exact change set per operation.

**Promised (best-effort):**

- Script writes never touch disk directly — they stay in memory and are
  reported exactly (paths, content bytes, deletions) for the caller to
  apply, prompt about, or discard.
- Commands the sandbox can't run are reported, never silently approximated,
  so the harness can rerun the script natively.
- Runaway scripts terminate (10 configurable execution limits).
- No subprocesses — a script can't escape into a real process.

**Not promised:**

- **This is not a security boundary.** It is a guardrail against careless
  yet destructive mistakes, not a defense against crafted attacks. A script
  can read everything the overlay exposes (i.e. your project) and print any
  of it to stdout; nothing prevents secret leakage. To contain adversarial
  code, use OS-level isolation (containers/VMs) — rust-bash can run inside
  such a boundary but is not one.
- No intent detection: mistakes and attacks are both merely made *visible*;
  telling them apart is the reviewer's job.
- No hard memory/CPU caps, no deterministic output (`date`, `$RANDOM`, the
  real clock leak through).

## The agent sandbox pattern

The primary use case: a coding-agent harness that runs scripts as a
*best-effort* sandbox over the real project directory, catching careless
mistakes before they reach disk. The harness — not the
sandbox — owns the two policy decisions:

1. **Unknown commands** → rerun the whole script natively (never partially).
2. **File writes** → inspect the exact change set, then apply / prompt / discard per path.

```rust
use rust_bash::{OverlayFs, RustBashBuilder};
use std::sync::Arc;

// Reads from the project on disk; writes stay in memory. Disk is never modified.
let overlay = Arc::new(OverlayFs::new("./my_project").unwrap());
let mut shell = RustBashBuilder::new()
    .fs(overlay.clone())
    .cwd("/")
    .abort_on_unresolved_commands(true)  // stop at the first unknown command
    .build()
    .unwrap();
let script = "echo patched > README.md";

// 1. Pre-flight: statically detect commands the sandbox doesn't implement.
let analysis = shell.analyze_commands(script).unwrap();
if !analysis.unresolved.is_empty() {
    // e.g. ["git", "kubectl"] — rerun the whole script natively instead.
}

// 2. Execute in the sandbox. Runtime misses (dynamic names like "$cmd")
//    that pre-flight can't see are reported as the backstop.
let result = shell.exec(script).unwrap();
if !result.unresolved_commands.is_empty() {
    // Skip steps 3–4 (optionally overlay.reset()): discard the partial
    // output AND pending overlay writes, then rerun natively.
}

// 3. Selectively commit the write set. diff() reports exactly what changed:
//    every created/modified path (with content bytes) and every deletion.
let d = overlay.diff();
for w in &d.writes {
    // w.path (VFS path), w.node_type, w.content, w.mode
    // apply inside the project, prompt for paths outside it — your policy.
}
for p in &d.deletions {
    // on-disk paths removed during the session — delete, prompt, or skip.
}

// 4. Reconcile: drop applied shadows; diff() now reports only what's still pending.
overlay.sync();
```

This pattern is a Rust-crate capability — the crate is the only distribution
surface this fork maintains.

Works on Windows out of the box (the overlay root can be `C:/Users/me/project`;
VFS paths inside are Unix-style). See the full workflow in the
[Agent Sandbox Integration recipe](docs/recipes/agent-sandbox-integration.md)
and the runnable [`examples/agent_harness.rs`](examples/agent_harness.rs).

## Installation

This fork is not published to crates.io; add it as a git dependency:

```toml
[dependencies]
rust-bash = { git = "https://github.com/JerryAZR/rust-bash" }
```

## Quick Start

```rust
use rust_bash::RustBashBuilder;
use std::collections::HashMap;

let mut shell = RustBashBuilder::new()
    .files(HashMap::from([
        ("/data.txt".into(), b"hello world".to_vec()),
    ]))
    .env(HashMap::from([
        ("USER".into(), "agent".into()),
    ]))
    .build()
    .unwrap();

let result = shell.exec("cat /data.txt | grep hello").unwrap();
assert_eq!(result.stdout, "hello world\n");
assert_eq!(result.exit_code, 0);
```

### Detecting commands the sandbox can't run

Unknown-command reporting works without an overlay, too. When a command name
resolves to nothing, `ExecResult::unresolved_commands` lists the missing names
(execution continues by default, matching bash's exit-127 behavior; enable
`abort_on_unresolved_commands` to stop at the first miss), and
`analyze_commands()` checks a script statically without executing anything.

## Custom Commands

```rust
use rust_bash::{RustBashBuilder, VirtualCommand, CommandContext, CommandResult};

struct MyCommand;

impl VirtualCommand for MyCommand {
    fn name(&self) -> &str { "my-cmd" }
    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        CommandResult {
            stdout: format!("got {} args\n", args.len()),
            ..Default::default()
        }
    }
}

let mut shell = RustBashBuilder::new()
    .command(Box::new(MyCommand))
    .build()
    .unwrap();

let result = shell.exec("my-cmd foo bar").unwrap();
assert_eq!(result.stdout, "got 2 args\n");
```

## Use Cases

- **Coding-agent harnesses** — best-effort sandbox over the real project directory: writes stay in memory until you commit them selectively (`OverlayFs::diff()`/`sync()`), and unknown commands are signaled so you can rerun natively
- **AI agent tools** — give LLMs a bash sandbox without container overhead
- **Code sandboxes** — run user-submitted scripts safely
- **Testing** — deterministic bash execution with a controlled filesystem
- **Embedded scripting** — add bash scripting to Rust applications
- **Glue-level Python** — let agents transform data with stdlib Python (json/csv/re/pathlib) inside the same reviewed-write sandbox, while project Python work offloads to the host

## Built-in Commands

### Registered commands (80)

| Category | Commands |
|----------|----------|
| **Core** | `echo`, `cat`, `true`, `false`, `pwd`, `touch`, `mkdir`, `ls`, `test`, `[` |
| **File ops** | `cp`, `mv`, `rm`, `tee`, `stat`, `chmod`, `mkfifo`, `ln`, `readlink`, `rmdir`, `du`, `split` |
| **Text** | `grep`, `egrep`, `fgrep`, `sort`, `uniq`, `cut`, `head`, `tail`, `wc`, `tr`, `rev`, `fold`, `nl`, `printf`, `paste`, `od`, `tac`, `comm`, `join`, `fmt`, `column`, `expand`, `unexpand`, `strings` |
| **Text processing** | `sed`, `awk`, `jq`, `diff` |
| **Search** | `rg` |
| **Navigation** | `realpath`, `basename`, `dirname`, `tree`, `find` |
| **Utilities** | `expr`, `date`, `sleep`, `seq`, `env`, `printenv`, `which`, `base64`, `md5sum`, `sha1sum`, `sha256sum`, `whoami`, `hostname`, `uname`, `yes`, `xargs`, `timeout`, `file`, `bc`, `clear` |
| **Compression** | `gzip`, `gunzip`, `zcat`, `tar` |

All commands support `--help` for built-in usage information.

### Interpreter builtins (40)

`exit`, `cd`, `export`, `unset`, `set`, `shift`, `readonly`, `declare`, `read`, `eval`, `source` / `.`, `break`, `continue`, `:` / `colon`, `let`, `local`, `return`, `trap`, `shopt`, `type`, `command`, `builtin`, `getopts`, `mapfile` / `readarray`, `pushd`, `popd`, `dirs`, `hash`, `wait`, `alias`, `unalias`, `printf`, `exec`, `sh` / `bash`, `help`, `history`

Additionally, `if`/`then`/`elif`/`else`/`fi`, `for`/`while`/`until`/`do`/`done`, `case`/`esac`, `((...))`, `[[ ]]`, and `time` are handled as shell syntax by the interpreter.

## Configuration (Rust)

```rust
use rust_bash::{RustBashBuilder, ExecutionLimits};
use std::collections::HashMap;
use std::time::Duration;

let mut shell = RustBashBuilder::new()
    .files(HashMap::from([
        ("/app/script.sh".into(), b"echo hello".to_vec()),
    ]))
    .env(HashMap::from([
        ("HOME".into(), "/home/agent".into()),
    ]))
    .cwd("/app")
    .execution_limits(ExecutionLimits {
        max_command_count: 1_000,
        max_execution_time: Duration::from_secs(5),
        ..Default::default()
    })
    .build()
    .unwrap();
```

### Execution limits (opt-in)

Limits are **off by default** — the harness decides whether and how to bound
scripts (and tells the model). `ExecutionLimits::default()` is unbounded;
`ExecutionLimits::agent_preset()` is a guardrail preset for agent workloads:

| Limit | `agent_preset()` |
|-------|---------|
| `max_call_depth` | 25 |
| `max_command_count` | 10,000 |
| `max_loop_iterations` | 10,000 |
| `max_execution_time` | 30 s |
| `max_output_size` | 10 MB |
| `max_string_length` | 10 MB |
| `max_glob_results` | 100,000 |
| `max_substitution_depth` | 50 |
| `max_heredoc_size` | 10 MB |
| `max_brace_expansion` | 10,000 |
| `max_array_elements` | 100,000 |

## Filesystem Backends

| Backend | Description |
|---------|-------------|
| `InMemoryFs` | Default. All data in memory. Zero host access. |
| `OverlayFs` | Copy-on-write over a real directory. Reads from disk, writes stay in memory. `diff()` exports the write set. |
| `MountableFs` | Compose backends at different mount points. |

> **Windows:** all features (including the host-filesystem backends) build and test on Windows. VFS paths are Unix-style (`/`-separated) on every platform.

### OverlayFs — Read real files, sandbox writes

```rust
use rust_bash::{OverlayFs, RustBashBuilder};
use std::sync::Arc;

// Reads from ./my_project on disk; writes stay in memory.
// Keep a handle to the overlay so you can export the write set later.
let overlay = Arc::new(OverlayFs::new("./my_project").unwrap());
let mut shell = RustBashBuilder::new()
    .fs(overlay.clone())
    .cwd("/")
    .build()
    .unwrap();

let result = shell.exec("cat /src/main.rs").unwrap();    // reads from disk
shell.exec("echo patched > /src/main.rs").unwrap();       // writes to memory only
```

`diff()` exports the exact change set (writes with content bytes, plus
deletions) for selective caller-side commits, and `sync()` reconciles with
disk after applying — see [The agent sandbox pattern](#the-agent-sandbox-pattern).

### MountableFs — Combine backends at mount points

```rust
use rust_bash::{RustBashBuilder, InMemoryFs, MountableFs, OverlayFs};
use std::sync::Arc;

let mountable = MountableFs::new()
    .mount("/", Arc::new(InMemoryFs::new()))                                // in-memory root
    .mount("/project", Arc::new(OverlayFs::new("./myproject").unwrap()))    // overlay on real project
    .mount("/tmp", Arc::new(InMemoryFs::new()));                            // separate temp space

let mut shell = RustBashBuilder::new()
    .fs(Arc::new(mountable))
    .cwd("/")
    .build()
    .unwrap();

shell.exec("cat /project/README.md").unwrap();   // reads from disk
shell.exec("echo scratch > /tmp/work").unwrap(); // writes to in-memory /tmp
```

## Public API (Rust)

| Type | Description |
|------|-------------|
| `RustBashBuilder` | Builder for configuring and constructing a shell instance |
| `RustBash` | The shell instance — call `.exec(script)` to run commands, or `.analyze_commands(script)` for parse-time analysis |
| `ExecResult` | Returned by `exec()`: `stdout`, `stderr`, `exit_code`, `unresolved_commands` (names that failed command resolution, deduplicated) |
| `CommandAnalysis` | Returned by `analyze_commands()`: `commands` (all literal command names) and `unresolved` (names that would not resolve) |
| `ExecutionLimits` | Configurable resource bounds |
| `VirtualCommand` | Trait for registering custom commands |
| `CommandContext` | Passed to command implementations (fs, cwd, env, stdin, limits) |
| `CommandResult` | Returned by command implementations |
| `RustBashError` | Top-level error: `Parse`, `Execution`, `LimitExceeded`, `Vfs`, `Timeout` |
| `VfsError` | Filesystem errors: `NotFound`, `AlreadyExists`, `PermissionDenied`, etc. |
| `Variable` | A shell variable with `value`, `exported`, `readonly` metadata |
| `ShellOpts` | Shell option flags: `errexit`, `nounset`, `pipefail`, `xtrace` |
| `ExecutionCounters` | Per-`exec()` resource usage counters |
| `InterpreterState` | Full mutable shell state (advanced: direct inspection/manipulation) |
| `ExecCallback` | Callback type for sub-command execution (`xargs`, `find -exec`) |
| `InMemoryFs` | In-memory filesystem backend |
| `OverlayFs` | Copy-on-write overlay backend |
| `OverlayDiff` / `OverlayWrite` | Write-set export from `OverlayFs::diff()` |
| `MountableFs` | Composite backend with path-based mount delegation |
| `VirtualFs` | Trait for filesystem backends |

## Documentation

- [Guidebook](docs/guidebook/) — architecture, design, and implementation details
- [Recipes](docs/recipes/) — task-oriented guides for common use cases

## Roadmap

- Planned: Embedded runtimes — SQLite, yq, Python, JavaScript
- Planned: Platform features — cancellation, lazy files, AST transforms, fuzz testing

## License

MIT
