# Fork Scope: Components Not Maintained / Candidates for Trimming

Status: **decision record (draft)** — lists what this fork will not maintain,
what is already decided vs. still open, and the housekeeping that follows.

This fork targets one use case: a library embedded in an agent harness that
runs bash (and later Python, see
[python-sandbox-shared-fs.md](python-sandbox-shared-fs.md)) over a real
project directory, reports the exact change set after each operation, and
lets the caller decide what to apply. Upstream's *distribution surface* —
the channels through which rust-bash reaches end users — is out of scope.

## Not maintained in this fork (trim candidates)

| Component | Files | Size / deps | Rationale |
|---|---|---|---|
| WASM bindings | `src/wasm.rs`, `packages/core/wasm/`, `browser.ts`, `wasm-loader.ts`, `profile.release-wasm` | ~760 lines; wasm-bindgen, js-sys, web-time, serde-wasm-bindgen | Upstream's browser showcase channel. The fork embeds natively; the Python plan is WASI-in-process, not browser WASM |
| Website example | `examples/website/`, `.github/workflows/deploy-website.yml` | full Vite/Cloudflare app | rustbash.dev is upstream's homepage; this fork has none |
| C FFI | `src/ffi.rs`, `examples/ffi/`, `include/`, `cbindgen.toml`, `cdylib` crate-type | ~680 lines; pulls in `serde` | Only serves non-Rust/non-Node hosts; `cdylib` taxes every build |
| MCP server | `src/mcp.rs` (gated under `cli`) | ~574 lines | The harness calls the library directly; no MCP transport needed |
| CLI binary + REPL | `src/main.rs`, `examples/shell.rs`; clap, rustyline | ~420 lines + 2 deps | Dev convenience only; dropping `cli` also removes MCP |
| curl / `network` feature | `src/network.rs`, `src/commands/net.rs`; ureq, url | ~1,040 lines | Philosophically aligned with the fork's model: unregistered, curl becomes an *unresolved command* → signaled → native rerun ("visible instead of silently wrong"). Also removes the sandbox's network attack surface. Registration is already `#[cfg(feature = "network")]`-gated, so this degrades gracefully |
| npm publish workflow | `.github/workflows/npm-publish.yml` | — | Tied to the npm package decision below |

Total: ~5,800 lines of Rust, a web app, two CI workflows, ~8 dependency
crates.

## Open decisions (settle before trimming)

1. **npm package** (`packages/core/`) — depends on the harness surface
   (open question in the Python design doc):
   - Rust-native harness → drop `packages/` entirely.
   - Node harness → keep the napi half only; drop the wasm fallback; this
     becomes the surface where `diff()`/`OverlayFs` get exposed.
2. **`jq`** (`src/commands/jq_cmd.rs`; jaq-core/jaq-json/jaq-std
   `3.0.0-gamma`) — ~850 lines plus three **pre-release** dependencies.
   Agents use jq often; but if gamma-quality deps are unacceptable, dropping
   degrades gracefully to native rerun.
3. **`compression`** (`src/commands/compression.rs`; flate2, tar) — ~1,500
   lines. Agents frequently untar; default keep, drop for the leanest core.
4. **`ReadWriteFs`** — passthrough FS writing straight to disk; contradicts
   the "writes stay in memory until committed" model. Shares the `native-fs`
   gate with `OverlayFs` (which stays), so this is a small cleanup. Lean:
   drop.

## Remains maintained

Interpreter core (brush-parser, walker, expansion, builtins), command set
(minus whatever is trimmed above), `OverlayFs` / `InMemoryFs` /
`MountableFs` under `native-fs`, regex stack, execution limits,
unknown-command signaling, `examples/agent_harness.rs`, CI, guidebook +
recipes (updated to reflect trims), oils/comparison/spec test infrastructure
(fidelity safety net for the interpreter).

## Housekeeping that follows from the scope change

- `Cargo.toml` metadata pointed at upstream (fixed alongside this doc):
  `repository` → the fork, `homepage`/`documentation` removed (no homepage,
  not release-ready), `wasm` category dropped.
- Crate name `rust-bash` is upstream's crates.io name. Not an issue while
  the fork is unpublished; rename only if a release ever becomes relevant.
- Default features `["cli", "network", "native-fs"]` should shrink to
  `["native-fs"]` when the trims land — and per this repo's no-legacy rule,
  deleted components should be **deleted**, not left behind feature gates.
- Guidebook Ch. 8 (Integration Targets) and the affected recipes need
  pruning when the trims land.
