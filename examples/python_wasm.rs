//! Bring-up check for the CPython artifact: compile it with
//! `PythonInterpreter` (timing the dominant startup cost) and run a small
//! script on an in-memory filesystem.
//!
//! Requires the artifact: `scripts/fetch-python-wasm.sh`.

use std::sync::Arc;
use std::time::Instant;

use rust_bash::InMemoryFs;
use rust_bash::python::PythonInterpreter;

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

    let t = Instant::now();
    let python = PythonInterpreter::new(&bytes)?;
    println!("compile: {:?}", t.elapsed());

    let script = "import sys, json; print(sys.version); print(json.dumps({'ok': True}))";
    let t = Instant::now();
    let out = python.run(
        &["python".into(), "-c".into(), script.into()],
        &[],
        Arc::new(InMemoryFs::new()),
        b"",
        None,
    )?;
    println!("execute: {:?} (exit {})", t.elapsed(), out.exit_code);
    println!("--- stdout ---\n{}", String::from_utf8_lossy(&out.stdout));
    println!("--- stderr ---\n{}", String::from_utf8_lossy(&out.stderr));
    Ok(())
}
