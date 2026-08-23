# Research: CPython→WASM artifacts & wasmer 7 integration

Status: **research findings** — web research conducted 2026-08-23. **Superseded
in part:** the wasmer recommendation was later rejected (wasmer 7.3 removed
its Windows JIT backends); the shipped runtime is wasmtime 46.0.3 +
wasi-common 46.0.3 — see [python-sandbox-shared-fs.md](python-sandbox-shared-fs.md)
§0. The artifact analysis (§1, §4) remains accurate and is what shipped. Companion to
[python-sandbox-shared-fs.md](python-sandbox-shared-fs.md): this document answers
*which* CPython-wasm artifact to embed and *how* to wire it under wasmer 7 with a
custom `virtual-fs::FileSystem`.

Scope assumptions: short agent-generated scripts, pure-Python stdlib only (no
pip, no native extensions), sandboxed, interruptible, guest FS is our own
virtual filesystem mounted as the WASI preopen (never the host FS).

---

## 1. Prebuilt CPython→wasm32-wasi artifacts

### 1.1 astral-sh/python-build-standalone — ❌ dead end

Its `ci-targets.yaml` (authoritative build list) contains only
darwin/linux/windows triples — **no wasm target exists**. Latest release
(`20260814`, CPython 3.10–3.15rc1) is native-only; no
`cpython-*-wasm32-wasip1-*.tar.zst` artifacts exist, and Astral's stewardship
roadmap does not mention wasm.

- https://raw.githubusercontent.com/astral-sh/python-build-standalone/main/ci-targets.yaml
- https://github.com/indygreg/python-build-standalone/blob/main/docs/running.rst
- https://github.com/astral-sh/python-build-standalone/releases

### 1.2 wasmer `python/python` WAPM package — ⚠️ WASIX-only

- Current: **CPython 3.13.17**, actively maintained (https://wapm.io/python/python).
- Built from `wasix-org/cpython 3.13.0-wasix` — a **WASIX build** (wasmer's
  preview1 superset adding threads/sockets/fork), **not plain wasip1**
  (https://github.com/wasix-org/build-scripts,
  https://wasmer.io/posts/announcing-wasix).
- Not a single portable .wasm: a webc package with the stdlib as a separate
  mounted filesystem volume, fetched by the wasmer toolchain.
- Usable from Rust only via `wasmer-wasix`. Drags threads/sockets/fork
  machinery into the sandbox model; locks to wasmer's ABI. Does **not**
  satisfy the "plain wasip1" constraint.

### 1.3 Official CPython artifacts — ❌ no binaries, ✅ Tier 2 support

- **PEP 11: `wasm32-unknown-wasip1` is Tier 2 since Python 3.13** (Tier 3 in
  3.11/3.12). Contacts: Brett Cannon, Michael Droettboom, Savannah Ostrowski.
  Tier 2 = reliable buildbot + release-blocking failures, i.e. CI-gated on
  every change.
  - https://peps.python.org/pep-0011/
  - https://discuss.python.org/t/pep-11-updated-to-list-wasm32-wasip1-as-the-supported-triple/70493
- **No downloadable binaries from python.org or CI.** Only a July 2026 PoC
  (Brett Cannon's `wasi-package` branch): `python-3.16.0a0-wasm32-wasip1.tar.xz`
  with `python3.16d.wasm` + loose `**/*.py` stdlib + LICENSE.txt — unreleased,
  wasmtime-oriented (https://discuss.python.org/t/wasi-distribution-poc/108168).
- WASI SDK pinning per PEP 816: **SDK 24 for 3.13/3.14, SDK 33 for 3.15**
  (SDK 26/27 skipped — they hang CPython). WASI 0.2 skipped entirely; upstream
  intends to go straight to WASI 0.3.

### 1.4 vmware-labs/webassembly-language-runtimes — ✅ usable prebuilt, frozen

- Repo is being archived; community fork
  (https://github.com/webassemblylabs/webassembly-language-runtimes/releases)
  has **zero releases** so far.
- Latest: **CPython 3.12.0** (`python/3.12.0+20231211-040d5a6`, 2023-12-11 —
  frozen ~2.5 years):
  - `python-3.12.0.wasm` — **25.1 MB, single file, stdlib fully embedded** via
    an in-module VFS (wasi-vfs lineage), plain wasip1, wasi-sdk 20
  - `python-3.12.0-wasi-sdk-20.0.tar.gz` — 11 MB, no stdlib (preopen
    `/usr/local/lib/python3.12` yourself)
  - `libpython-3.12.0-wasi-sdk-20.0.tar.gz` — 24 MB static lib
  - ⚠️ avoid the `-wasmedge.wasm` variant (needs WasmEdge socket extensions)
  - https://github.com/vmware-labs/webassembly-language-runtimes/releases/expanded_assets/python%2F3.12.0%2B20231211-040d5a6
- Proven on **stock runtimes with no stdlib preopen** by multiple downstream
  projects. Caveats: 3.12.0 only, no security patches since Dec 2023,
  wasi-sdk 20 (older than the SDK 24 used by 3.13/3.14).

### 1.5 Stdlib supply & licensing

| Source | Stdlib mechanism | License |
|---|---|---|
| WLR `python-3.12.0.wasm` | embedded in-module VFS | PSF-2.0 — assets don't bundle LICENSE; **ship the PSF text yourself** |
| WLR `.tar.gz` variant | separate dir (or `python3xx.zip` zipimport), preopen `/` | PSF-2.0 |
| wasmer `python/python` | webc volume mounted into guest FS | PSF-2.0; wasmer-wasix MIT |
| Self-build | `make install` → `usr/local/lib/python3.YY/` + `python3.YY.wasm` | PSF-2.0 |

PSF-2.0 is permissive: retain license + copyright notice, note changes in
derivative works. Nothing blocks redistribution.

### 1.6 Self-building from source — feasible, officially supported

- One-shot: `python3 Tools/wasm/wasi.py build -- --config-cache` (3.13;
  `Platforms/WASI build` on 3.15+) with PEP-816-pinned wasi-sdk. A ready-made
  devcontainer pins tool versions
  (https://devguide.python.org/getting-started/setup-building/#wasi).
- Two full CPython builds (build python → cross wasm python); expect tens of
  minutes; CI-proven (CPython's own Tier-2 buildbots + community GitHub
  Actions).
- Pain points:
  - `make install` broken on 3.14/3.15 (`build-details.json` stat failure,
    https://github.com/python/cpython/issues/137878) — assemble the stdlib
    manually.
  - No dynamic loading (pure-Python stdlib only — fine for this use case).
  - No threads on plain wasip1.

---

## 2. wasmer 7 integration mechanics

### 2.1 Crate layout — important correction

**There is no separate `wasmer-wasi` crate in wasmer 7** — it was
merged/renamed into **`wasmer-wasix` (v0.702.x)**, which supports plain
`wasi_snapshot_preview1` as well as WASIX. A plain wasip1 python.wasm runs
through `wasmer-wasix` without enabling WASIX behavior.

Also: `virtual-fs` 0.703.0 is **not yet on crates.io**; latest published is
**0.702.1** (matches wasmer 7.2/7.3). Pin to what exists.

- https://docs.rs/crate/wasmer-wasix/latest
- https://crates.io/crates/virtual-fs

### 2.2 Compatibility with CPython-wasip1

- The 2023 blocker (`sock_accept` import missing — CPython imports it
  unconditionally) is **fixed** in current wasmer
  (https://github.com/vmware-labs/webassembly-language-runtimes/issues/106).
- `proc_exit` surfaces as a `RuntimeError` wrapping `ExitCode` — handle it as
  a normal exit.
- A directory-read EBADF discrepancy vs wasmtime was reported via Go's wasip1
  port (2023-era); current status unverified — smoke-test
  `os.listdir`/`pathlib` on the chosen build early.

### 2.3 Custom `FileSystem` (virtual-fs 0.70x)

Required trait methods (from wasmer `main`):

- `readlink(&self, path) -> Result<PathBuf>`
- `read_dir(&self, path) -> Result<ReadDir>` — iterator of `DirEntry`
- `create_dir`, `remove_dir`, `remove_file`
- `rename<'a>(&'a self, from, to) -> BoxFuture<'a, Result<()>>` — **async**
- `metadata`, `symlink_metadata` (currently identical; symlinks unimplemented)
- `new_open_options(&self) -> OpenOptions<'_>`

`create_symlink`/`hard_link` default to `FsError::Unsupported`.

- https://raw.githubusercontent.com/wasmerio/wasmer/main/lib/virtual-fs/src/lib.rs

File handles: implement
`FileOpener::open(path, &OpenOptionsConfig) -> Box<dyn VirtualFile + Send + Sync>`.
`VirtualFile` is tokio-async (`AsyncRead + AsyncWrite + AsyncSeek + Unpin +
Send`) with required `set_len`, `size`, `unlink`, `poll_read_ready`,
`poll_write_ready`, and timestamps. Reusable in-crate building blocks:
`mem_fs`, `OverlayFileSystem`, `MountFileSystem`, `RootFileSystemBuilder`,
`limiter` (memory caps).

**⚠️ Version caveat:** the trait shape differs between docs.rs's last
successful build (0.601.0-rc.5, has required `mount()`) and current `main` (no
`mount`, new symlink defaults). Implement against the exact pinned version.

### 2.4 Mounting as preopen

On `WasiEnv::builder("prog")` (wasmer-wasix):

- **`set_fs(Arc<dyn FileSystem>)`** — documented as "in case a custom
  virtual_fs::FileSystem is needed" — or
  `set_fs_root(WasiFsRoot::from_filesystem(...))`
- `add_preopen_dir("/")`
- execute via `run_with_store(module, &mut store)`

This bypasses the host FS entirely (host-dir `map_dir` preopens sit behind a
separate feature). One cost: wasmer-wasix defaults to a tokio runtime; to
embed tokio-free, supply a `PluggableRuntime` with a `VirtualTaskManager` impl
+ `UnsupportedVirtualNetworking`
(https://github.com/wasmerio/wasmer/discussions/4299).

### 2.5 Interruptibility

Wasmer has **no wasmtime-style `interrupt_handle()`**. The mechanism is
**`wasmer-middlewares::metering::Metering`** (fuel):

- `Metering::new(limit, cost_fn)` pushed via
  `CompilerConfig::push_middleware`; exhaustion traps.
- External kill = another thread calls
  `set_remaining_points(&mut store, &instance, 0)`.
- Supported on **all three backends** (Singlepass/Cranelift/LLVM).
- Caveats:
  - A `Metering` instance **must not be shared across modules** (panics).
  - **Metering only counts guest CPU** — a script blocked inside a host call
    (`fd_read` on stdin, `poll_oneoff` sleep) burns no fuel. Host-layer
    timeouts in the FS/pipe layer are needed for that class.

https://docs.rs/wasmer-middlewares/7.2.1/wasmer_middlewares/metering/index.html

### 2.6 Backend & startup

For a ~25 MB python.wasm, JIT compilation dominates startup. wasmer's own
(dated, 2.2-era) numbers on a 70 MB module: **Singlepass ~4 s, Cranelift
10–35 min, LLVM gave up** (https://wasmer.io/posts/wasmer_2_2). Cranelift has
improved since, but treat Cranelift/LLVM JIT on CPython-sized modules as
seconds-to-minutes.

**Correct pattern regardless of backend: compile once, `Module::serialize()` →
cache the artifact, `Module::deserialize()` thereafter** (wasmtime analogue cut
python.wasm startup 140 ms → 17 ms). If JIT in-process is unavoidable, use
Singlepass. Run each script on a dedicated thread with its own `Store`;
`Module` clones are cheap.

---

## 3. Wildcard check (2025-era developments)

- **wasip2/component-model CPython doesn't exist upstream** — PEP 11 lists only
  `wasm32-unknown-wasip1` (Tier 2) through 3.15; upstream skipped WASI 0.2 and
  targets 0.3 eventually. Component-model Python lives in the **wasmtime +
  componentize-py** ecosystem (actively maintained) — a full runtime swap, not
  an upgrade path.
- **wasmer 7.x has no wasip2/component support** — `wasmer-wasix` documents
  only `wasi_unstable`/`wasi_snapshot_preview1`; wasmer's strategy is WASIX
  (7.0 added WASIX dynamic linking specifically for Python + numpy). **wasip1
  is forced by the runtime choice, and it is also CPython's only supported
  wasm target — the constraints align.**
- **RustPython**: covers the target scope (`re`, `json`, `pathlib`, `math`,
  `datetime`), embeddable via `InterpreterConfig` + `freeze-stdlib`. But:
  significantly slower, self-declared "not for production," and **provides no
  sandbox** — the stdlib crate hands guest code host FS/process/network with
  no fuel/memory limits. Dead end *for this use case*.
- Ecosystem trend: 2025-era agent sandboxing (NVIDIA/Pyodide, Pydantic
  mcp-run-python, eryx/componentize-py) confirms wasm-based Python sandboxing
  is the consensus pattern.

**Nothing found invalidates the wasip1 + wasmer 7 + CPython + custom-VFS
plan.**

---

## 4. Recommendation — ranked

Ranking criteria: (a) time-to-hello-world under wasmer with a custom
FileSystem, (b) artifact cleanliness for CI caching/redistribution, (c)
maintenance outlook.

1. **vmware-labs `python-3.12.0.wasm`** (25.1 MB, stdlib embedded, plain
   wasip1) — fastest start + cleanest artifact. Single file → trivial CI
   caching and redistribution (add PSF license text); nothing to mount for
   stdlib, so our custom `FileSystem` is the *only* FS and cleanly becomes the
   preopen root via `set_fs` + `add_preopen_dir("/")`. Risks: frozen at
   3.12.0, no patches since Dec 2023, upstream archived. Acceptable for a
   sandboxed stdlib-only interpreter; treat as the bootstrap.
2. **Self-build CPython 3.14 with wasi-sdk 24** via `Tools/wasm/wasi.py build`
   — the right long-term answer. Tier-2 upstream support, current CPython,
   full control of stdlib packaging (zipimport `lib.zip` is the cleanest
   single-artifact option). One-time cost: a CI job + the `make install`
   workaround. Do this once option 1 validates the harness.
3. **wasmer `python/python` 3.13.17 via WASIX** — only if threads/native deps
   are wanted later. Same-runtime upgrade path, but a heavier sandbox surface
   and wasmer lock-in.

### Explicit dead ends

- astral-sh/python-build-standalone — no wasm builds at all
- python.org official binaries — don't exist; PoC unreleased
- tiran/cpython-wasm-test — 3.11.0, 2022, stale
- singlestore-labs/python-wasi — build tooling only, no artifacts
- webassemblylabs community fork — no releases yet
- RustPython — no sandboxing; not credible for untrusted code

### Uncertainties to smoke-test early

- Directory-read semantics under wasmer 7.3 (`os.listdir`/`pathlib.glob`)
- Exact `FileSystem` trait shape at the pinned virtual-fs version
- Compile time of the chosen backend on the 25 MB module (mitigated by
  artifact caching)
