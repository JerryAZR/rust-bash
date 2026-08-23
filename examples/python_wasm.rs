//! Run the CPython wasm32-wasip1 artifact under wasmtime + wasi-common and
//! execute Python code.
//!
//! This is the bring-up check for the shared-overlay Python design
//! (docs/design/python-sandbox-shared-fs.md): it proves the 25 MB artifact
//! loads, compiles, and runs on the pinned runtime pair (wasmtime 46.0.3 +
//! wasi-common 46.0.3, sync embedding), and measures compile time (the
//! dominant startup cost). No directories are preopened: the artifact embeds
//! its stdlib, so no filesystem access is needed for this smoke test.
//!
//! Requires the artifact: `scripts/fetch-python-wasm.sh`.

use std::io::Cursor;
use std::time::Instant;

use wasi_common::pipe::WritePipe;
use wasi_common::sync::{WasiCtxBuilder, add_to_linker};
use wasmtime::{Config, Engine, Linker, Module, Store};

const WASM_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/third_party/python-wasm/python-3.12.0.wasm"
);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read(WASM_PATH).unwrap_or_else(|e| {
        eprintln!("cannot read {WASM_PATH}: {e}");
        eprintln!("run `scripts/fetch-python-wasm.sh` first");
        std::process::exit(1);
    });
    println!("artifact: {} bytes", bytes.len());

    let engine = Engine::new(&Config::new())?;
    let t = Instant::now();
    let module = Module::new(&engine, &bytes)?;
    println!("compile: {:?}", t.elapsed());

    let stdout = WritePipe::new(Cursor::new(Vec::<u8>::new()));
    let stderr = WritePipe::new(Cursor::new(Vec::<u8>::new()));

    let script = "import sys, json; print(sys.version); print(json.dumps({'ok': True}))";
    let wasi = WasiCtxBuilder::new()
        .arg("python")?
        .arg("-c")?
        .arg(script)?
        .stdout(Box::new(stdout.clone()))
        .stderr(Box::new(stderr.clone()))
        .build();

    let t = Instant::now();
    let run_result = {
        let mut store = Store::new(&engine, wasi);
        let mut linker = Linker::new(&engine);
        add_to_linker(&mut linker, |cx| cx)?;
        let instance = linker.instantiate(&mut store, &module)?;
        let start = instance.get_typed_func::<(), ()>(&mut store, "_start")?;
        let r = start.call(&mut store, ());
        // Dropping the store releases the ctx's clones of the stdio pipes.
        drop(store);
        r
    };
    println!("execute: {:?}", t.elapsed());
    match run_result {
        Ok(()) => println!("exit: clean"),
        Err(e) => println!("exit: trap/error: {e:#}"),
    }

    let out = stdout
        .try_into_inner()
        .map_err(|_| "stdout pipe shared")?
        .into_inner();
    let err = stderr
        .try_into_inner()
        .map_err(|_| "stderr pipe shared")?
        .into_inner();
    println!("--- stdout ---\n{}", String::from_utf8_lossy(&out));
    println!("--- stderr ---\n{}", String::from_utf8_lossy(&err));
    Ok(())
}
