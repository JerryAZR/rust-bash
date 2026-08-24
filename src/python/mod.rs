//! Sandboxed CPython over a shared `VirtualFs`.
//!
//! This module runs a CPython `wasm32-wasip1` module under wasmtime, with the
//! guest's preopened filesystem rooted at a caller-supplied `VirtualFs` —
//! typically the same `Arc<OverlayFs>` a `RustBash` shell runs against, so
//! bash and Python see each other's pending writes and one `overlay.diff()`
//! reports both (docs/design/python-sandbox-shared-fs.md).
//!
//! The WASI preview1 host layer is wasi-common; only the filesystem traits
//! are implemented here (`vfs_dir::VfsDir`, `vfs_file::VfsFile`).
//!
//! Python is stdlib-only (no pip, no native extensions — WASI cannot provide
//! them). It is a scripting-glue companion to the bash sandbox, not a project
//! development environment.
//!
//! Runtime pair (pinned): wasmtime 46.0.3 + wasi-common 46.0.3.
//!
//! # Current limitations (embedders: read before exposing to agents)
//!
//! - **Execution limits are opt-in** via [`PythonLimits`] (fuel for CPU,
//!   `max_file_size` for the FS bridge). With no limits configured the guest
//!   runs unbounded: a `while True: pass` hangs the calling thread and
//!   unbounded memory growth is uncapped — the harness / tool wrapper owns
//!   that policy and communicates it to the model. Module-compile caching is
//!   a tracked follow-up (first compile costs seconds).
//! - **Guest cwd is always `/`.** WASI p1 has no per-process working
//!   directory; the preopen root *is* the guest's cwd. Harnesses that want
//!   "run with the shell's cwd" must rewrite paths themselves.
//! - **Trap semantics:** on a guest trap (fuel kill, crash), libc-buffered
//!   stdout is lost while unbuffered stderr survives — as with a real
//!   process dying. Treat stderr as the diagnostics channel for killed runs.

mod vfs_dir;
mod vfs_file;

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use wasi_common::pipe::{ReadPipe, WritePipe};
use wasi_common::sync::{add_to_linker, clocks_ctx, random_ctx, sched_ctx};
use wasi_common::{I32Exit, Table, WasiCtx};
use wasmtime::{Config, Engine, Linker, Module, Store};

use crate::vfs::VirtualFs;

use vfs_dir::VfsDir;

/// Result of one Python invocation.
#[derive(Debug, Clone)]
pub struct PythonOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}

/// Optional per-invocation bounds. **Everything is opt-in**: `None` (or a
/// field left `None`) means unbounded — the library makes no policy
/// decision about how much work a script may do; that belongs to the
/// harness / tool wrapper, which also communicates the bounds to the model.
#[derive(Debug, Clone, Default)]
pub struct PythonLimits {
    /// Fuel budget (roughly "wasm instructions"). `Some(n)` caps guest CPU;
    /// exhaustion traps the run and surfaces as [`PythonError::Trap`] with
    /// the output captured so far. `None` = unbounded.
    pub fuel: Option<u64>,
    /// File size cap in bytes, enforced by the FS bridge on writes and
    /// truncations (over-limit => `EFBIG`). `None` = bounded only by
    /// available memory (allocation failures also surface as `EFBIG`, never
    /// host aborts).
    pub max_file_size: Option<u64>,
}

/// Errors from setting up or running the interpreter.
#[derive(Debug)]
pub enum PythonError {
    /// The CPython module failed to compile.
    Compile(wasmtime::Error),
    /// WASI context or linker setup failed.
    Setup(wasmtime::Error),
    /// The guest trapped (a real crash, not a `sys.exit`). Carries the
    /// output captured before the trap — usually the best debugging artifact.
    Trap(wasmtime::Error, PythonOutput),
}

impl std::fmt::Display for PythonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PythonError::Compile(e) => write!(f, "failed to compile CPython module: {e}"),
            PythonError::Setup(e) => write!(f, "failed to set up WASI context: {e}"),
            PythonError::Trap(e, out) => {
                write!(
                    f,
                    "python execution trapped: {e:#} (captured {} bytes of stdout before the trap)",
                    out.stdout.len()
                )
            }
        }
    }
}

impl std::error::Error for PythonError {}

/// A compiled CPython interpreter, reusable across invocations.
///
/// Compiling the ~25 MB module is the dominant startup cost; construct once
/// and call [`run`](Self::run) per script.
pub struct PythonInterpreter {
    engine: Engine,
    module: Module,
}

impl PythonInterpreter {
    /// Compile a CPython `wasm32-wasip1` module from its bytes.
    ///
    /// The engine always enables fuel metering so that per-run
    /// [`PythonLimits::fuel`] budgets work; it costs a small instrumentation
    /// overhead even when unused.
    pub fn new(python_wasm: &[u8]) -> Result<Self, PythonError> {
        let engine = Engine::new(Config::new().consume_fuel(true)).map_err(PythonError::Compile)?;
        let module = Module::new(&engine, python_wasm).map_err(PythonError::Compile)?;
        Ok(Self { engine, module })
    }

    /// Run Python with the given argv, environment, stdin, and filesystem.
    ///
    /// `args` is the full argv (including `argv[0]`, conventionally
    /// `"python"`): `["python", "-c", source]` for inline code,
    /// `["python", "/script.py", ...]` for a file on the given `fs`.
    ///
    /// The guest's filesystem is rooted at `fs` (preopened at `/`). Writes go
    /// through the `VirtualFs` exactly as bash's do — with an `OverlayFs`,
    /// they stay in memory and appear in `diff()`.
    ///
    /// `limits` is opt-in: `None` runs unbounded (see [`PythonLimits`]).
    pub fn run(
        &self,
        args: &[String],
        env: &[(String, String)],
        fs: Arc<dyn VirtualFs>,
        stdin: &[u8],
        limits: Option<&PythonLimits>,
    ) -> Result<PythonOutput, PythonError> {
        let mut ctx = WasiCtx::new(random_ctx(), clocks_ctx(), sched_ctx(), Table::new());
        for arg in args {
            ctx.push_arg(arg).map_err(|e| {
                PythonError::Setup(wasmtime::Error::msg(format!("invalid arg: {e}")))
            })?;
        }
        for (k, v) in env {
            ctx.push_env(k, v).map_err(|e| {
                PythonError::Setup(wasmtime::Error::msg(format!("invalid env: {e}")))
            })?;
        }

        let stdout = WritePipe::new(Cursor::new(Vec::<u8>::new()));
        let stderr = WritePipe::new(Cursor::new(Vec::<u8>::new()));
        ctx.set_stdin(Box::new(ReadPipe::from(stdin.to_vec())));
        ctx.set_stdout(Box::new(stdout.clone()));
        ctx.set_stderr(Box::new(stderr.clone()));
        let (fuel, max_file_size) = limits
            .map(|l| (l.fuel, l.max_file_size))
            .unwrap_or((None, None));
        ctx.push_preopened_dir(
            Box::new(VfsDir::new(fs, Path::new("/"), max_file_size)),
            Path::new("/"),
        )
        .map_err(|e| PythonError::Setup(wasmtime::Error::msg(e.to_string())))?;

        let (exit_code, trap) = {
            let mut store = Store::new(&self.engine, ctx);
            // Infallible here: the engine is always built with fuel enabled.
            store
                .set_fuel(fuel.unwrap_or(u64::MAX))
                .expect("engine always has fuel metering enabled");
            let mut linker = Linker::new(&self.engine);
            add_to_linker(&mut linker, |cx| cx).map_err(PythonError::Setup)?;
            let instance = linker
                .instantiate(&mut store, &self.module)
                .map_err(PythonError::Setup)?;
            let start = instance
                .get_typed_func::<(), ()>(&mut store, "_start")
                .map_err(PythonError::Setup)?;
            let result = start.call(&mut store, ());
            let fuel_exhausted = store.get_fuel().is_ok_and(|f| f == 0);
            // Drop the store before extracting pipe contents (it holds clones).
            drop(store);
            match result {
                Ok(()) => (0, None),
                Err(e) => match e.downcast_ref::<I32Exit>() {
                    Some(exit) => (exit.0, None),
                    None => {
                        let e = if fuel_exhausted {
                            e.context("fuel exhausted (PythonLimits::fuel)")
                        } else {
                            e
                        };
                        // The exit_code here is meaningless (the guest did
                        // not exit) — consumers must use the `Err` variant,
                        // not the payload's exit_code.
                        (0, Some(e))
                    }
                },
            }
        };

        let out = |pipe: WritePipe<Cursor<Vec<u8>>>| -> Vec<u8> {
            pipe.try_into_inner()
                .expect("store dropped; stdio pipe uniquely owned")
                .into_inner()
        };
        let output = PythonOutput {
            stdout: out(stdout),
            stderr: out(stderr),
            exit_code,
        };
        match trap {
            Some(e) => Err(PythonError::Trap(e, output)),
            None => Ok(output),
        }
    }
}
