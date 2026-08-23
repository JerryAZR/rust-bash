# Fork Scope: Trimmed Components (Decision Record)

Status: **executed**. The trim described below has landed; this document is
kept as the decision record explaining what was removed and why.

This fork targets one use case: a library embedded in an agent harness that
runs bash (and later Python, see
[python-sandbox-shared-fs.md](python-sandbox-shared-fs.md)) over a real
project directory, reports the exact change set after each operation, and
lets the caller decide what to apply. Upstream's *distribution surface* —
the channels through which rust-bash reaches end users — is out of scope.

## Removed (deleted, not feature-gated)

| Component | What was deleted | Rationale |
|---|---|---|
| WASM bindings | `src/wasm.rs`, `wasm` feature, `profile.release-wasm`, wasm-bindgen/js-sys/web-time/serde-wasm-bindgen deps | Upstream's browser showcase channel. The fork embeds natively; the Python plan is WASI-in-process, not browser WASM |
| Website example | `examples/website/`, `deploy-website.yml` workflow | rustbash.dev is upstream's homepage; this fork has none |
| C FFI | `src/ffi.rs`, `examples/ffi/`, `include/`, `cbindgen.toml`, `tests/ffi.rs`, `cdylib` crate-type, `ffi` feature, `serde` dep | Only served non-Rust/non-Node hosts |
| MCP server | `src/mcp.rs` | The harness calls the library directly; no MCP transport needed |
| CLI binary + REPL | `src/main.rs`, `examples/shell.rs`, `tests/cli.rs`, clap/rustyline, `cli` feature | Dev convenience only |
| curl / `network` feature | `src/network.rs`, `src/commands/net.rs`, `NetworkPolicy` (incl. the `network_policy` thread through builder/interpreter/CommandContext), ureq/url/tiny_http deps, `RustBashError::Network` | The upstream implementation was built for a threat model this fork doesn't have (URL allow-lists, method restrictions — a *security* gate). Without the policy, what remained was a partial curl reimplementation (ureq-based, ~20 flags, single URL, in-memory bodies) where divergence from real curl fails scripts subtly instead of loudly. Removed in favor of the unresolved-command → native-rerun path. *Decision (deferred):* curl has legitimate need (fetched content written through the VFS fits the diff/review workflow exactly), but no beautiful in-sandbox solution exists — a shim over ureq is a partial reimplementation whose flag-level gaps can confuse agents (silently wrong instead of loudly absent), and the `curl` crate (real libcurl) buys transfer fidelity at the cost of a C toolchain in the build, per-build capability variance (`Version::get()` inspection), and duplication of the fidelity the native-rerun path already provides perfectly. Native rerun remains the honest path until a better design appears |
| npm package | `packages/` (napi native addon + WASM fallback + TS sources), `npm-publish.yml`, npm-related `scripts/`, root `package-lock.json`, `packages/core/AGENTS.md` validation tests | Harness is Rust-native. If a Node harness (e.g. pi) materializes, a purpose-built napi-only package exposing `OverlayFs`/`diff()` will be designed then — upstream's dual native+wasm shape would not be revived as-is. Git history preserves it for reference |
| `ReadWriteFs` | `src/vfs/readwrite.rs` + tests, exports, docs | Passthrough-to-disk contradicts the "writes stay in memory until committed" model; trusted execution should just use native bash, not "a sandbox without the sandbox" |

## Kept (with changes)

- **`jq`** — kept, and the pre-release pins were resolved: `jaq-core`
  3.0.0-gamma → **3.1.0 stable**, `jaq-std` → 3.0.2, `jaq-json` → 2.0.2
  (the 3.0 API the code targets went stable; no other pure-Rust jq exists —
  alternatives are C-jq FFI bindings).
- **`compression`** (gzip/gunzip/zcat/tar) — kept as-is. The ~1,500 lines
  are command-line emulation and VFS glue over the `flate2`/`tar` crates
  (codecs were already external); no crate can absorb the glue.

## Remains maintained

Interpreter core (brush-parser, walker, expansion, builtins), command set,
`OverlayFs` / `InMemoryFs` / `MountableFs` under `native-fs`, regex stack,
execution limits, unknown-command signaling, `examples/agent_harness.rs`,
CI, guidebook + recipes, oils/comparison/spec test infrastructure (fidelity
safety net for the interpreter).

## Housekeeping outcomes

- Default features are now just `["native-fs"]`; the only remaining feature
  flag is `native-fs`.
- `Cargo.toml`: `[[bin]]` removed, `crate-type` back to plain `lib`,
  `exclude` list pruned.
- CI: fmt + clippy (default and `--no-default-features`) + tests, on Linux
  and Windows. WASM/npm/website jobs removed.
- Guidebook Ch. 8 rewritten (Rust crate + agent-harness embedding only);
  Ch. 1/2/4/5/6/7/9/10 scrubbed; recipes pruned to the Rust-only set.
- Crate name `rust-bash` is upstream's crates.io name. Not an issue while
  the fork is unpublished; rename only if a release ever becomes relevant.
