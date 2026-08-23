# Chapter 1: What We Are Building

## The Problem

AI agents need a bash tool. Today's options all have significant drawbacks:

| Approach | Problem |
|----------|---------|
| Real shell on host | Nothing is reviewable — a careless `rm` or overwrite hits disk immediately, so every command needs a human approval to be safe |
| Docker/VM per agent | Heavy — 100-500ms startup, memory overhead, orchestration complexity |
| Node.js sandbox (just-bash) | Requires Node.js runtime, limited to JavaScript embedding |
| Restricted shell (rbash) | Only restricts *some* operations; still touches real filesystem |

There is no lightweight, embeddable, zero-dependency bash sandbox that can be dropped into any language or platform.

## The Solution

**rust-bash** is a sandboxed bash environment built in Rust. It parses and executes bash scripts entirely in-process, with all filesystem operations going through a virtual filesystem (VFS). No real files are touched, no processes are spawned, no network requests are made — unless explicitly allowed.

It deploys as a **Rust crate** embedded directly in an agent harness.

> **Fork note:** this chapter originally described upstream's broader vision
> (CLI binary, C FFI, WASM, npm package). This fork maintains only the Rust
> crate, scoped to the agent-harness embedding use case — see
> `docs/design/fork-scope.md` and the promise/non-promise model in Chapter 7.
> Competitive-positioning tables below are kept
> for context but no longer describe distribution targets of this fork.

## Design Principles

1. **Zero runtime dependencies** — pure library, embeddable anywhere Rust runs. No Node.js, no Python, no containers.

2. **No real OS access by default** — all filesystem operations go through a virtual filesystem. The default `InMemoryFs` has zero `std::fs` calls.

3. **No process spawning** — all commands are implemented in Rust, in-process. There is no `std::process::Command` anywhere in the codebase.

4. **Composable filesystem backends** — `InMemoryFs` for full sandboxing, `OverlayFs` for copy-on-write over real directories, `MountableFs` for mixing backends at different mount points.

5. **Execution limits** — prevent runaway scripts with configurable limits on depth, count, time, output size, and more.

6. **Parser reuse** — leverage `brush-parser`'s battle-tested bash grammar instead of hand-rolling a parser. We focus on execution, not parsing.

## Non-Goals

- **Full POSIX compliance** — we target the bash subset that AI agents actually use, not every obscure POSIX feature.
- **Interactive terminal features** — no job control (`fg`, `bg`, `jobs`), no signal handling, no `readline`. This is a scripting sandbox, not a terminal emulator.
- **Multi-process semantics** — no `fork()`, no background processes (`&`), no `wait`. Commands execute sequentially.
- **Performance at the expense of safety** — we prefer correctness and sandboxing guarantees over raw throughput.

## Target Users

1. **AI agent frameworks** — provide a bash tool that agents can use safely without container overhead.
2. **Code sandbox providers** — embed rust-bash for lightweight code execution environments.
3. **Testing tools** — run bash scripts in isolated environments for deterministic testing.

## Competitive Positioning

We evaluated six approaches to giving AI agents bash capabilities:

| Approach | Example | How it works |
|----------|---------|-------------|
| Container/MicroVM | E2B, Modal, Fly.io | Real `/bin/bash` inside an isolated VM or container |
| just-bash (TypeScript) | Vercel just-bash | Reimplemented bash interpreter + 75 commands in TypeScript |
| **rust-bash (this project)** | — | brush-parser + custom Rust interpreter + in-memory VFS |
| WASM bash binary | BusyBox → Emscripten | Real C bash/busybox compiled to WebAssembly |
| Real bash (no sandbox) | `std::process::Command` | Shell out to `/bin/bash` on the host |
| Restricted real bash | firejail, nsjail, bubblewrap | Real bash with OS-level sandboxing (seccomp, namespaces) |

### Summary Scorecard

Core interpreter, text processing, execution safety, and filesystem backends are complete and maintained. Upstream's distribution-surface milestones (C FFI, WASM, CLI binary, npm package) were removed from this fork's scope.

| Metric | Container | just-bash | **rust-bash** | WASM bash | Real bash | Restricted bash |
|--------|-----------|-----------|---------------|-----------|-----------|----------------|
| Startup latency | ⚠️ 150ms–12s | ⚠️ 50–100ms | ✅ **<1ms** | ⚠️ 50–200ms | ✅ 3ms | ⚠️ 10–50ms |
| Memory per sandbox | ❌ 30–128MB | ⚠️ 20–50MB | ✅ **1–5MB** | ⚠️ 10–30MB | ✅ 5MB | ✅ 5MB |
| Dependencies | ❌ Heavy | ⚠️ Node.js | ✅ **None** | ⚠️ WASM runtime | ✅ OS | ⚠️ Linux only |
| Bash compatibility | ✅ Perfect | ✅ Good | ⚠️ Growing | ✅ Perfect | ✅ Perfect | ✅ Perfect |
| Isolation | ✅ Strong | ✅ Good | ⚠️ **Guardrails, not a security boundary** | ⚠️ Medium | ❌ None | ⚠️ Medium |
| Browser support | ❌ No | ✅ Yes | ❌ **No (fork)** | ✅ Yes (large) | ❌ No | ❌ No |
| Polyglot embedding | ❌ HTTP only | ❌ TS only | ⚠️ **Rust only (fork)** | ⚠️ Via WASM | ✅ Subprocess | ⚠️ Linux only |
| Cost | ❌ Cloud billing | ✅ Free | ✅ **Free** | ✅ Free | ✅ Free | ✅ Free |
| Maturity | ✅ Production | ✅ Production | ❌ **Early dev** | ❌ Experimental | ✅ Decades | ⚠️ Niche |

### When to Use What

| Scenario | Best approach | Why |
|----------|--------------|-----|
| Full-featured cloud agent (needs pip, git, arbitrary binaries) | Container (E2B/Modal) | Only real OS can run arbitrary binaries |
| Lightweight agent tool (no infra, basic bash scripting) | **rust-bash** | Zero dependencies, sub-ms latency, library call |
| Existing TypeScript/Node.js agent | just-bash | Native integration, production-proven |

### rust-bash's Advantages

- **Latency**: sub-ms per exec, no VM boot or GC pause
- **Memory**: ~1–5MB per sandbox vs 20–128MB for alternatives
- **Zero dependencies**: pure Rust crate — no runtime to install
- **In-process embedding**: a library call, not a subprocess or service

### rust-bash's Disadvantages

- **Maturity**: early development, not yet production-proven
- **Compatibility**: growing command set, doesn't cover every bash edge case
- **No real processes**: can't run `pip install`, `git clone`, or other real binaries

## Reference Implementation

[just-bash](https://github.com/vercel-labs/just-bash) by Vercel is the primary behavioral reference. It implements a sandboxed bash environment in TypeScript with an in-memory virtual filesystem. Our goal is functional equivalence with just-bash, plus the additional capabilities enabled by Rust (OverlayFs over real directories, better performance).
