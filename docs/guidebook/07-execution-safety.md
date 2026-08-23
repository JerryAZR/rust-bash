# Chapter 7: Execution Safety

## Overview

rust-bash is designed to run AI-generated scripts unattended, catching careless
mistakes before they reach disk. This chapter covers what the sandbox promises
(best-effort guardrails), what it explicitly does not promise (a security
boundary), and the execution limits that bound runaway scripts.

## Execution Limits

```rust
pub struct ExecutionLimits {
    pub max_call_depth: usize,           // default: 25
    pub max_command_count: usize,        // default: 10,000
    pub max_loop_iterations: usize,      // default: 10,000
    pub max_execution_time: Duration,    // default: 30s
    pub max_output_size: usize,          // default: 10MB
    pub max_string_length: usize,        // default: 10MB
    pub max_glob_results: usize,         // default: 100,000
    pub max_substitution_depth: usize,   // default: 50
    pub max_heredoc_size: usize,         // default: 10MB
    pub max_brace_expansion: usize,      // default: 10,000
}
```

### Enforcement Points

| Limit | Checked At |
|-------|-----------|
| `max_call_depth` | Every function call and `source` invocation |
| `max_command_count` | Every command dispatch (simple or compound) |
| `max_loop_iterations` | Each iteration of `for`, `while`, `until` loops |
| `max_execution_time` | Periodically during execution (wall-clock check) |
| `max_output_size` | Every stdout/stderr append |
| `max_string_length` | Variable assignment and string concatenation |
| `max_glob_results` | After glob expansion completes |
| `max_substitution_depth` | Nested `$()` command substitutions |
| `max_heredoc_size` | When processing here-document content |
| `max_brace_expansion` | When expanding `{1..N}` or `{a,b,...}` |

### Execution Counters

```rust
pub struct ExecutionCounters {
    pub command_count: usize,
    pub call_depth: usize,
    pub output_size: usize,
    pub start_time: Instant,
    pub substitution_depth: usize,
}
```

Counters are stored in `InterpreterState` and **reset at the start of each `exec()` call**. This means each `exec()` gets a fresh budget. Accumulated state (VFS, env) persists, but resource consumption is bounded per call.

### Limit Exceeded Behavior

When a limit is exceeded, execution stops immediately with a structured error:

```rust
RustBashError::LimitExceeded {
    limit_name: "max_loop_iterations",
    limit_value: 10_000,
    actual_value: 10_001,
}
```

This error is returned as `Err(RustBashError::LimitExceeded{...})` from `shell.exec()`. The sandbox remains usable for subsequent `exec()` calls — hitting a limit does not poison the sandbox or its state.

## The Model: Guardrails, Not a Security Boundary

The problem this fork solves is **approval fatigue**. Approving every agent
command by hand doesn't scale; running everything directly on disk is
careless. rust-bash lets a harness run scripts unattended and compresses the
human review into one small, exact change set per operation (`OverlayFs::diff()`).

That makes rust-bash a guardrail against **careless yet destructive mistakes**
— the wrong path, the unintended overwrite, the fat-fingered `rm` — by holding
their effects in memory until someone (or some policy) approves them. It is
**not** a sandbox in the security sense, and it does not try to be one.

### What we promise (best-effort)

1. **Writes never touch disk directly** — with `OverlayFs` (or `InMemoryFs`),
   every script write goes to memory and is reported exactly (paths, content
   bytes, deletions). The caller decides per path what lands on disk.
2. **Unknown commands are visible, never silently wrong** — commands the
   sandbox doesn't implement are reported (pre-flight and at runtime) so the
   harness can rerun the script natively.
3. **Bounded execution** — the limits above terminate runaway scripts
   (infinite loops, unbounded output, memory-hungry expansions).
4. **No process spawning** — the codebase contains zero calls to
   `std::process::Command`; a script can't escape into a real subprocess.

### What we don't promise

1. **No protection against a hostile script.** A script can read everything
   the filesystem backend exposes (with `OverlayFs`, that *is* your project),
   and can print any of it to stdout — which the agent loop then handles.
   Nothing here prevents secret leakage. If you need to contain adversarial
   code, use OS-level isolation (containers, VMs) instead; rust-bash can run
   inside such a boundary but is not one itself.
2. **No intent detection.** The guardrail makes effects reviewable before
   they become real; it cannot distinguish a mistake from an attack. Both are
   merely *visible*.
3. **No hard memory/CPU caps** — limits bound strings, output, iterations,
   and wall-clock time, but a pathological script can still use significant
   memory within those bounds, and `max_execution_time` is wall-clock, not
   CPU time.
4. **No deterministic output** — `date`, `$RANDOM`, and the real clock leak
   through. For deterministic testing, inject fixed values via environment
   variables.

### Design properties

Engineering facts that the guardrails rest on (useful when reasoning about
what a script *can* affect):

| Property | Mechanism |
|----------|-----------|
| All file ops virtualized | `VirtualFs` trait everywhere; `InMemoryFs` has zero `std::fs` calls |
| No subprocesses | No `std::process::Command` anywhere; all commands are in-process Rust |
| Reads confined to the overlay root | `OverlayFs` resolves paths under its configured base directory; VFS normalization handles `..` |
| No network code in the crate | No HTTP client and no network command; a script that needs the network fails command resolution and is rerun natively by the host (a curl may return if a faithful-enough design is ever found — see `docs/design/fork-scope.md`) |
| Panics don't kill the VFS | `parking_lot::RwLock` (non-poisoning) |
| Runaway scripts terminate | The execution limits above |

## Configuration

```rust
let mut shell = RustBashBuilder::new()
    .execution_limits(ExecutionLimits {
        max_command_count: 1_000,
        max_execution_time: Duration::from_secs(5),
        ..Default::default()
    })
    .build()
    .unwrap();
```

All limits have sensible defaults. You only need to configure limits you want to change.
