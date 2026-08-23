//! Bash and sandboxed CPython working over one shared `OverlayFs`.
//!
//! This demonstrates the fork's two-tool sandbox pattern
//! (docs/design/python-sandbox-shared-fs.md): one `Arc<OverlayFs>` over a
//! real project directory, two clients. Reads come from disk; every write —
//! bash's or Python's — stays in the overlay's in-memory layer, and a single
//! `overlay.diff()` at the end reports the combined change set. Disk is
//! never touched.
//!
//! Run with: cargo run --example python_overlay --features python,native-fs
//! Requires the CPython artifact: `scripts/fetch-python-wasm.sh`.

use std::sync::Arc;

use rust_bash::python::PythonInterpreter;
use rust_bash::{OverlayFs, RustBashBuilder};

const WASM_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/third_party/python-wasm/python-3.12.0.wasm"
);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A "project" on disk with one data file.
    let tmp = tempfile::tempdir()?;
    std::fs::write(tmp.path().join("data.csv"), "name,score\nalice,3\nbob,7\n")?;

    // One shared overlay: reads from disk, writes to memory.
    let overlay = Arc::new(OverlayFs::new(tmp.path())?);

    // Client 1: the bash shell.
    let mut shell = RustBashBuilder::new()
        .fs(overlay.clone())
        .cwd("/")
        .build()?;

    // Client 2: the sandboxed Python interpreter.
    let bytes = std::fs::read(WASM_PATH).unwrap_or_else(|e| {
        eprintln!("cannot read {WASM_PATH}: {e}");
        eprintln!("run `scripts/fetch-python-wasm.sh` first");
        std::process::exit(1);
    });
    let python = PythonInterpreter::new(&bytes)?;

    // ── Step 1: bash counts the rows ─────────────────────────────────
    let r = shell.exec("tail -n +2 /data.csv | wc -l")?;
    println!("bash sees {} data rows", r.stdout.trim());

    // ── Step 2: bash stages a config; Python picks it up immediately ──
    shell.exec("echo '{\"scale\": 10}' > /config.json")?;

    let script = r#"
import json, csv

with open('/config.json') as f:
    scale = json.load(f)['scale']

with open('/data.csv') as f:
    rows = list(csv.DictReader(f))

with open('/report.txt', 'w') as f:
    for row in rows:
        f.write(f"{row['name']}: {int(row['score']) * scale}\n")
print('wrote /report.txt')
"#;
    let out = python.run(
        &["python".into(), "-c".into(), script.into()],
        &[],
        overlay.clone(),
        b"",
        None,
    )?;
    print!("{}", String::from_utf8_lossy(&out.stdout));
    eprint!("{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.exit_code, 0);

    // ── Step 3: bash reads Python's pending write ────────────────────
    let r = shell.exec("cat /report.txt")?;
    print!("{}", r.stdout);

    // ── Step 4: one diff() reports BOTH tools' writes ────────────────
    let d = overlay.diff();
    // The shell seeds a default layout (/bin, /dev, /home, /tmp, /usr) in the
    // upper layer; harnesses filter those prefixes out of the reported set.
    let layout = ["/bin", "/dev", "/home", "/tmp", "/usr"];
    let interesting: Vec<_> = d
        .writes
        .iter()
        .map(|w| w.path.display().to_string())
        .filter(|p| !layout.iter().any(|pre| p.starts_with(pre)))
        .collect();
    println!("pending writes (filtered): {interesting:?}");
    assert_eq!(interesting, ["/config.json", "/report.txt"]);
    assert!(d.deletions.is_empty());

    // Nothing hit disk.
    assert!(!tmp.path().join("report.txt").exists());
    assert!(!tmp.path().join("config.json").exists());
    println!(
        "disk untouched: only data.csv exists on disk = {:?}",
        std::fs::read_dir(tmp.path())?.count() == 1
    );
    Ok(())
}
