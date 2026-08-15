# Agent Sandbox Integration: Native Fallback + Write Tracking

This recipe documents the integration pattern for running rust-bash as a
**best-effort command sandbox** for a coding agent, where the *harness* — not
the sandbox — owns two policy decisions:

1. **Unknown commands.** When a script uses a command the sandbox does not
   implement (e.g. `git`, `node`, `cargo`), the sandbox reports it clearly and
   does nothing else. The harness may rerun the entire script natively on the
   host (e.g. in Git Bash on Windows), with no piped I/O or state migration.
2. **File writes.** When the sandbox runs over a real project directory, all
   writes stay in memory. After execution the harness inspects exactly which
   files were created, modified, or deleted, and decides — per path — whether
   to apply the changes to disk, prompt the user, or discard them.

This is a cooperative, best-effort design. It is not a replacement for a real
isolation boundary (containers, VMs); its goal is that safe work happens in a
cheap in-process sandbox, and anything the sandbox cannot do is *visible*
instead of silently wrong.

All examples are Rust against the crate API. A complete, compilable version
of the final example lives in [`examples/agent_harness.rs`](../../examples/agent_harness.rs).

## Execution model

```
              ┌────────────────────────────────────────────┐
 agent asks   │ 1. analyze_commands(script)                │
 to run a     │    unresolved empty? ── no ──► run natively│
 script       │    unresolved empty? ── yes ──► exec()     │
              │ 2. exec(script)                            │
              │    result.unresolved_commands non-empty?   │
              │         ── yes ──► discard, run natively   │
              │ 3. overlay.diff()                          │
              │    writes/deletions under project ──► apply│
              │    anything else ────────────────► prompt  │
              └────────────────────────────────────────────┘
```

Key property: **a script runs entirely in the sandbox or entirely natively.**
There is no partial execution with hand-off — no piping sandbox output into a
native process, no environment or filesystem state migration in either
direction. This keeps both sides simple and the failure modes obvious.

## Step 1 — Detect unknown commands before running

`RustBash::analyze_commands()` parses the script and returns every literal
command name it would dispatch, split into `commands` (all names) and
`unresolved` (names that would fail resolution: not a builtin, not a
registered command, not a function defined in the script or on the instance):

```rust
let analysis = shell.analyze_commands("grep -r foo src/ && git status")?;
// analysis.commands    == ["grep", "git"]
// analysis.unresolved  == ["git"]
```

If `unresolved` is non-empty, skip the sandbox and run the whole string
natively. Nothing was executed, so there are no side effects to clean up.

**What static analysis cannot see** (documented on the method): dynamically
built names (`cmd=git; $cmd status`), `eval "..."`, and other runtime-only
names are invisible to `analyze_commands`. Execution-time detection below is
the backstop.

## Step 2 — Detect unknown commands during execution

Every `exec()` result carries `unresolved_commands`, deduplicated in
first-encountered order. Misses anywhere in the script are captured — inside
functions, subshells, pipelines, `xargs`/`find -exec` sub-commands, and
`bash -c` bodies:

```rust
let result = shell.exec("echo hi; git status; git log")?;
// result.stdout               == "hi\n"
// result.exit_code            == 127        (bash fidelity)
// result.stderr               contains "git: command not found"
// result.unresolved_commands  == ["git"]
```

By default the interpreter keeps going after a miss (exactly like bash), so
in-sandbox scripts that *handle* missing commands (`command -v git || ...`)
behave correctly. If your contract is "discard and rerun natively", you can
stop at the first miss instead:

```rust
let mut shell = RustBashBuilder::new()
    .abort_on_unresolved_commands(true)   // default: false
    .build()?;
```

With abort enabled, the script unwinds from any nesting depth at the first
miss and returns the output accumulated so far plus the unresolved list.
Since a doomed script's output is discarded anyway, this avoids wasted work
and side effects after the verdict is known.

### Running natively

The crate never spawns processes — by design. "Run natively" means the
*harness* executes the original script string with a real shell on the host
(`bash -c` on Unix, Git Bash on Windows). Recommendation: since the harness
owns the tool description shown to the agent, include a line like:

> Use forward slashes (`/`) in all paths.

Agents follow that reliably, and the sandbox treats `\` as an ordinary
filename character (matching real bash), so stray backslashes degrade the
same way they would in Git Bash rather than misbehaving.

## Step 3 — Track file modifications with OverlayFs

Back the shell with `OverlayFs` rooted at a host directory **of the harness's
choosing** — the project directory, the user's home, or anywhere in between.
Reads come from disk; every write lands in an in-memory upper layer; the
disk is never modified.

Keep a second handle to the overlay so you can export the write set after
execution (the interpreter holds its own clone):

```rust
use rust_bash::{OverlayFs, RustBash, RustBashBuilder};
use std::sync::Arc;

let overlay = Arc::new(OverlayFs::new("C:/Users/jerry/myproject")?);
let mut shell = RustBashBuilder::new()
    .fs(overlay.clone())
    .cwd("/src/app")            // any path inside the overlay, as a VFS path
    .build()?;
```

VFS paths are Unix-style (`/`-separated) on every platform. The overlay root
maps to VFS `/`, so `C:/Users/jerry/myproject/src/app` is `/src/app` inside
the sandbox.

After execution, `diff()` returns the exact change set — no comparison
against disk is needed, because the overlay's internal state *is* the diff:

```rust
shell.exec("echo patched > README.md; rm TODO.md; mkdir -p out")?;

let d = overlay.diff();
for w in &d.writes {
    // w.path: VFS path, e.g. "/README.md" or "/out" (a directory)
    // w.node_type: File | Directory | Symlink
    // w.content: file bytes; symlink target for symlinks; empty for dirs
    // w.mode: Unix-style mode to apply when materializing
}
for p in &d.deletions {
    // Lower-layer (on-disk) paths removed during the session,
    // e.g. "/TODO.md" — always absolute VFS paths.
}
```

`diff()` semantics worth relying on:

- **Modify** — a copy-up happens internally; the diff shows one write with
  the new content.
- **Create** — one write, including new directories.
- **Delete then recreate** — reported as a write, not a deletion.
- **`rm -rf dir`** — one deletion for `dir`, not one per child (top-most
  whiteouts only).
- **Create then delete (never on disk)** — appears in neither list.
- **Subshell writes** — `( ... )` runs on a deep-cloned layer, so writes
  inside a subshell do not appear in the diff (correct bash semantics: they
  never happened as far as the parent is concerned).
- Diff state accumulates across `exec()` calls on the same instance for the
  session; recreate the shell/overlay per session (or per agent turn) if you
  want per-turn diffs.

### Applying or prompting

Map VFS paths back to host paths by stripping the leading `/` and joining
onto the overlay root, then classify:

- Paths under the project directory → write bytes to disk (create parent
  directories for directory writes first; apply deletions after writes).
- Anything else → prompt the user, log, or discard, per your policy.

Note that with a wide overlay root (e.g. the home directory), the sandbox's
own default layout (`/bin`, `/dev`, `/home`, `/tmp`, `/usr` — command stubs
and directories) also lives in the upper layer and shows up in `diff()` as
writes — filter those prefixes out, or mount a narrow overlay at a sub-path
via `MountableFs` if you prefer structural separation. The worked example
additionally routes writes into the project's `.git` directory to the prompt
path — a conservative default worth copying.

On Windows, `w.mode` comes from an optimistic mapping (`0o755` normally,
`0o555` when the read-only attribute is set) — treat it as advisory; there
are no Unix permission bits to apply.

## Complete harness loop

```rust
use rust_bash::{OverlayFs, RustBash, RustBashBuilder};
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct Harness {
    shell: RustBash,
    overlay: Arc<OverlayFs>,
    project_root: PathBuf,          // host path of the overlay root
}

enum Outcome {
    Sandboxed,                      // ran in sandbox; diff applied/prompted
    RanNatively,                    // harness reran the script outside
}

impl Harness {
    fn new(project_root: impl Into<PathBuf>) -> Result<Self, rust_bash::RustBashError> {
        let root = project_root.into();
        let overlay = Arc::new(OverlayFs::new(&root)?);
        let shell = RustBashBuilder::new()
            .fs(overlay.clone())
            .cwd("/")
            .abort_on_unresolved_commands(true)
            .build()?;
        Ok(Self { shell, overlay, project_root: root })
    }

    fn run(&mut self, script: &str) -> Result<Outcome, rust_bash::RustBashError> {
        // 1. Pre-flight: anything the sandbox can't run?
        let analysis = self.shell.analyze_commands(script)?;
        if !analysis.unresolved.is_empty() {
            self.run_natively(script, &analysis.unresolved);
            return Ok(Outcome::RanNatively);
        }

        // 2. Execute in the sandbox (aborts at the first miss).
        let result = self.shell.exec(script)?;
        if !result.unresolved_commands.is_empty() {
            // Dynamically-built names that pre-flight couldn't see.
            self.run_natively(script, &result.unresolved_commands);
            return Ok(Outcome::RanNatively);
        }

        // 3. Apply or prompt about the write set.
        let diff = self.overlay.diff();
        for w in &diff.writes {
            let host = self.host_path(&w.path);
            if self.is_inside_project(&host) {
                self.apply_write(&host, w);
            } else {
                self.prompt_apply(&host, w);
            }
        }
        for p in &diff.deletions {
            let host = self.host_path(p);
            if self.is_inside_project(&host) {
                let _ = std::fs::remove_file(&host);
            } else {
                self.prompt_delete(&host);
            }
        }
        Ok(Outcome::Sandboxed)
    }

    fn host_path(&self, vfs: &Path) -> PathBuf {
        let rel = vfs.to_string_lossy().trim_start_matches('/');
        self.project_root.join(rel)
    }
    // run_natively / is_inside_project / apply_write / prompt_* are
    // harness policy — see examples/agent_harness.rs for a working version.
}
```

## Known boundaries

- **No state migration.** A native rerun does not see sandbox writes (they
  are still only in memory unless you applied them), and sandbox state does
  not see native-run disk changes made after the overlay's reads. If a
  session alternates between sandbox and native runs on the same files,
  re-create the shell (fresh overlay) per run, or apply diffs before the
  native rerun.
- **`abort_on_unresolved_commands` is instance-level.** It applies to every
  `exec()` on that shell; use separate shell instances if some calls need
  bash-fidelity continuation.
- **`analyze_commands` false positives.** Function bodies are scanned even
  if the function is never called, so a never-called helper mentioning `git`
  routes the whole script to native. That is the safe direction.
- **FFI surface.** The C FFI does not expose `unresolved_commands` (the CLI
  `--json`, npm, and MCP surfaces do).
- **Binary content.** `OverlayWrite.content` is raw bytes — safe for
  gzip/tar output produced in the sandbox.
