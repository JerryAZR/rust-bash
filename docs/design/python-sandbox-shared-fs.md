# Design: Shared-Overlay Python Sandbox (WASI CPython)

Status: **research / design proposal** — no implementation yet.

This document captures the research for this fork's target use case:

> Sandbox common operations in bash (and Python scripts later). After each
> operation, report exactly which files were modified. The caller decides
> which changes to apply to disk.

It maps what already exists in rust-bash onto that workflow, identifies the
gaps, and evaluates the options for adding a WASI CPython tool that shares
the same filesystem view.

---

## 1. The expected workflow vs. what exists today

### Provenance (verified against git history)

Fork point: `68d6dba` (upstream v0.7.0). Everything in `d7eb349..82e0009`
is fork work. The change-reporting workflow this fork targets is **the
fork's own addition**, built on upstream's VFS base:

| Piece | Origin |
|---|---|
| `OverlayFs` copy-on-write base (lower = real dir, in-memory upper, whiteouts), `InMemoryFs`, `MountableFs`, `ReadWriteFs` | upstream |
| `VirtualFs: Send + Sync` (`src/vfs/mod.rs`), `RustBashBuilder::fs(Arc<dyn VirtualFs>)` host-owned sharing (`src/api.rs`) | upstream |
| `OverlayFs::diff()` / `OverlayDiff` / `OverlayWrite` | **fork** (`d7eb349`) |
| Windows platform support; unknown-command signaling (`analyze_commands`, `ExecResult::unresolved_commands`, `abort_on_unresolved_commands`) | **fork** (`d7eb349`) |
| `docs/recipes/agent-sandbox-integration.md` + `examples/agent_harness.rs` | **fork** (`9e734b3`) |
| Overlay persistence-across-`exec()` docs/tests; `diff()` invariant docs | **fork** (`e49093d`, `e951ce2`) |
| `OverlayFs::sync()` / `reset()` | **fork** (`d91eed5`) |
| Standalone overlay use across shell recreations | **fork** (`64d77aa`) |

So: the workflow (run in sandbox → `overlay.diff()` → caller per path:
apply / prompt / discard → `overlay.sync()`) is fully built out by the fork;
what remains is extending it to a second tool (Python) and, if needed, to
non-Rust harness surfaces.

Semantics of `diff()` worth relying on for per-operation reporting:

- Modify → one write (copy-up happened internally). Create → one write.
  Delete-then-recreate → a write, not a deletion.
- `rm -rf dir` → one top-most deletion, not one per child.
- Create-then-delete (never on disk) → absent from both lists.
- Subshell writes `( ... )` run on a deep-cloned layer → absent from the diff
  (correct bash semantics).
- `/tmp` writes are tracked like everything else: the overlay root maps to
  VFS `/`, and the default layout (`/bin`, `/dev`, `/home`, `/tmp`, `/usr`)
  lives in the upper layer. Harnesses typically filter those prefixes out of
  the reported set.

## 2. Gaps relative to the fork's workflow

1. **`diff()` is Rust-API-only.** The fork added `OverlayDiff`/`diff()` to
   the crate API but has not yet extended the (upstream-era) WASM
   (`src/wasm.rs`), C FFI (`src/ffi.rs`), MCP (`src/mcp.rs`), or npm
   (`packages/core/`) surfaces to match. A harness written in Node today
   cannot get the change set without going through the Rust API. *Decision
   needed:* which surface does this fork's caller use? If Node, exposing
   diff over napi is a prerequisite task.
2. **Diff granularity is overlay-lifetime, not per-operation.** `diff()`
   accumulates across `exec()` calls. Per-operation change sets require the
   caller to either (a) apply + `sync()` between operations, or (b) snapshot
   and subtract previously handled paths. Option (a) is the documented
   self-verifying loop and fits "caller decides which changes to apply after
   each operation" naturally.
3. **No built-in materialize-to-disk helper.** Applying an `OverlayDiff` is
   ~30 lines of harness code (map VFS path → host path, create parent dirs,
   write bytes, then deletions); the fork's recipe deliberately leaves this
   as caller policy. The fork may want a shared helper since *every* tool
   will need it.

## 3. Adding the Python tool: sharing the overlay with WASI CPython

### 3.1 Why no rust-bash changes are needed

The overlay is a plain `Arc<OverlayFs>` owned by the host. Every operation
resolves through whiteouts → upper → lower on each call (no caching), so a
write from any client is visible to every other client on its next read.
rust-bash is just one client of the FS. The Python tool becomes a second
client of the *same* `Arc`:

```
                 ┌─────────────────────────────┐
                 │ host owns Arc<OverlayFs>    │
                 │  (lower = project dir on    │
                 │   disk, upper = memory)     │
                 └──────┬──────────────┬───────┘
            Arc<dyn VirtualFs>    Arc<dyn VirtualFs>
                 │                     │
          ┌──────▼──────┐       ┌──────▼──────────────┐
          │  RustBash   │       │ WASI FS bridge      │
          │  (bash ops) │       │ (host code, new)    │
          └─────────────┘       └──────┬──────────────┘
                                ┌──────▼──────────────┐
                                │ WASI runtime +      │
                                │ CPython wasm module │
                                └─────────────────────┘
```

One `overlay.diff()` at the end of an operation reports writes from *both*
tools; one apply/`sync()` loop serves both. Sandboxing guarantees (disk is
never modified) extend to Python for free.

### 3.2 WASI CPython distribution options (needs verification at build time)

- **Official CPython `wasm32-wasi` builds.** WASI is a supported CPython
  target since 3.11 (support tier raised in later releases). Builds consist
  of a `python.wasm` module plus a stdlib tree that must be provided via a
  preopened directory.
- **Community single-file builds** (e.g. the `python-wasm` lineage from the
  wasm-labs ecosystem) bundle stdlib inside the module or as an embedded
  archive — fewer preopens, less flexibility.
- Either way the runtime needs **two preopens**: (1) a read-only stdlib
  directory (host dir or embedded), (2) the shared workspace — which is where
  the bridge below plugs in.

Open questions here: which distribution, which CPython version, and whether
native-extension PyPI packages matter (pure-Python only under WASI — this
shapes what the tool can promise agents).

### 3.3 Runtime choice: the real decision

| | wasmtime | wasmer |
|---|---|---|
| Custom FS hook | Implement `wasi:filesystem` host traits (p2) or the preview1 `WasiDir`/`WasiFile` shims | Implement the `virtual-fs` crate's `FileSystem` trait (purpose-built for pluggable backends: memfs, hostfs, …) |
| Shape match to `VirtualFs` | Moderate — p2 is stream/descriptor-oriented | Good — trait is a close POSIX-ish match (open/read/write/stat/readdir/rename/symlink) |
| Async | p2 host functions are async (tokio) | Sync-ish |
| Dep weight | Large either way | Large either way |

Exact trait names/locations shift between runtime versions — verify against
the pinned version before implementing. The bridge itself is the same
concept either way.

### 3.4 The bridge: runtime FS traits → `&dyn VirtualFs`

A single host module mapping WASI calls onto the existing `VirtualFs`
methods. The design work is concentrated in four impedance mismatches:

1. **Stateful fds vs. stateless VFS.** WASI fds carry a cursor, seek,
   append mode, and per-fd rights; `VirtualFs` has whole-file ops only. The
   bridge keeps an fd table `fd → { path, offset, flags }`:
   - reads: `read_file` + slice at offset;
   - positional writes: read-modify-write (read whole, splice, `write_file`);
   - append: `append_file` directly.
   At sandbox scale this is fine. **Non-atomic** if both tools write the same
   file concurrently — sequential agent turns (the expected workflow) are
   unaffected. A per-path lock in the bridge would close even that.
2. **Path mapping.** WASI paths are relative-to-preopen with `..` sandboxed
   at the preopen boundary; VFS paths are absolute, `/`-rooted, normalized.
   Mapping is mechanical, but **symlink policy differs**: WASI preview1
   refuses to follow links escaping the preopen, while `OverlayFs` resolves
   through both layers with a depth limit. Define which behavior Python sees
   (recommend: VFS behavior, since the lower layer is the trusted project
   dir and the workspace preopen *is* the whole overlay).
3. **Metadata/rights.** WASI wants filetype, size, timestamps, and rights
   checks (WASI is capability-based — the runtime checks declared rights
   before calling the FS). Map from `vfs::Metadata`; declare full rights on
   the workspace preopen, read-only on the stdlib preopen.
4. **Error mapping.** `VfsError` → WASI errno equivalents (`NotFound` →
   `NOENT`, `IsADirectory` → `EISDIR`, …). Mechanical but must be complete,
   or CPython surfaces confusing `OSError`s.

### 3.5 Alternatives considered

**Direction 2 — swap the backend for a shared third-party FS** (e.g.
implement `VirtualFs` over wasmer's `virtual-fs`, mount that in both runtimes).

- Pros: ecosystem-provided bridge pieces.
- Cons: inverts the dependency (rust-bash core grows a runtime-flavored FS
  dep for a host-level concern); overlay semantics (whiteouts, `diff()`,
  `sync()`, `reset()`) must be re-implemented on the new backend or are lost
  — and the change-reporting workflow *is* those semantics. Rejected.

**Hybrid — share only `/tmp` via `MountableFs`.** Mount one shared
`Arc<InMemoryFs>` at `/tmp` in both tools; everything else stays separate.

- Pros: smallest possible shared surface; no symlink-policy questions for
  project files.
- Cons: Python then can't see/bash can't see each other's *project* writes
  until applied to disk — which contradicts the workflow (an agent pipeline
  like `bash step writes data → python step transforms it → bash step
  packages it` needs full cross-tool visibility of pending writes).
  Useful only if cross-tool sharing is deliberately scoped to scratch space.

**Recommendation: Direction 1** — one shared `Arc<OverlayFs>`, bridge in the
harness, no rust-bash changes.

### 3.6 Concurrency model

`VirtualFs` is `Send + Sync` with internal locking, and WASI host calls run
on the runtime's thread — concurrent use is safe at the data-race level.
The expected workflow is sequential (one tool call at a time, diff after
each), where even the non-atomic positional-write emulation is a non-issue.
If the fork later runs bash and Python in parallel, add per-path write locks
in the bridge (the bash side's whole-file `write_file` is already atomic
w.r.t. VFS state).

## 4. What the per-operation reporting loop looks like with both tools

```
op = agent's next tool call (bash script | python script)
run op against the shared Arc<OverlayFs>
d = overlay.diff()
caller: per path in d.writes / d.deletions → apply | prompt | discard
overlay.sync()            # applied paths drop out; failures stay in diff
report = overlay.diff()   # exactly the unapplied remainder (usually empty)
```

Identical to today's bash-only loop — Python writes appear in the same
`OverlayDiff` with the same semantics (including: python `os.remove` →
deletion; write-then-delete within one op → absent from the diff).

## 5. Task list implied by this design (not scheduled)

1. Decide harness surface; if Node, expose `diff()` (+ apply helper?) over
   napi in `packages/core`.
2. Choose WASI runtime (wasmtime vs wasmer — §3.3) and CPython WASI
   distribution (§3.2); pin versions; verify current FS-trait APIs.
3. Implement the FS bridge module (fd table, path mapping, errno mapping) —
   host-side, no rust-bash changes.
4. Shared apply-to-disk helper used by both tools' reporting loop.
5. Conformance tests: cross-tool visibility cases (bash writes → python
   reads pending write, and vice versa), diff semantics for python-originated
   changes, symlink policy cases.

## 6. Open questions for the caller

1. **Harness surface** — Rust-native, or Node/npm (requiring napi diff
   exposure first)?
2. **Runtime** — wasmtime or wasmer? (See §3.3; slight lean to wasmer for
   trait fit, but verify maintenance/version state before committing.)
3. **Python capability scope** — pure-Python stdlib only, or is there a
   story needed for native-extension packages? (WASI can't provide the
   latter; the tool description shown to agents should say so.)
4. **Diff cadence** — per operation (apply+sync each time) or per turn
   (accumulate, one decision round)? Both work; per-operation matches the
   stated workflow.
5. **Symlink policy for the Python preopen** — strict WASI preview1
   semantics or VFS semantics (recommended)?
