#![cfg(all(feature = "python", feature = "native-fs"))]
//! Conformance tests for the shared-overlay Python sandbox: the bridge
//! semantics (fs operations through WASI p1), cross-tool visibility with
//! bash, and `OverlayFs::diff()` accounting for Python-originated changes.
//!
//! These tests MUST NOT silently skip: if the CPython artifact is missing
//! they fail with remediation (`scripts/fetch-python-wasm.sh`).

use std::path::Path;
use std::sync::{Arc, OnceLock};

use rust_bash::python::PythonInterpreter;
use rust_bash::{OverlayFs, RustBash, RustBashBuilder};

const WASM_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/third_party/python-wasm/python-3.12.0.wasm"
);

fn interpreter() -> &'static PythonInterpreter {
    static INTERPRETER: OnceLock<PythonInterpreter> = OnceLock::new();
    INTERPRETER.get_or_init(|| {
        let bytes = std::fs::read(WASM_PATH).unwrap_or_else(|e| {
            panic!(
                "cannot read {WASM_PATH}: {e}\n\
                 run `scripts/fetch-python-wasm.sh` to download the CPython artifact"
            )
        });
        PythonInterpreter::new(&bytes).expect("compile CPython module")
    })
}

struct Fixture {
    _tmp: tempfile::TempDir,
    overlay: Arc<OverlayFs>,
    shell: RustBash,
}

/// A temp project on disk (seeded with `disk_files`), an overlay over it,
/// and a bash shell on the same overlay.
fn fixture(disk_files: &[(&str, &[u8])]) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    for (path, content) in disk_files {
        let full = tmp.path().join(path.trim_start_matches('/'));
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, content).unwrap();
    }
    let overlay = Arc::new(OverlayFs::new(tmp.path()).unwrap());
    let shell = RustBashBuilder::new()
        .fs(overlay.clone())
        .cwd("/")
        .build()
        .unwrap();
    Fixture {
        _tmp: tmp,
        overlay,
        shell,
    }
}

fn run_python(overlay: Arc<OverlayFs>, source: &str) -> rust_bash::python::PythonOutput {
    interpreter()
        .run(
            &["python".into(), "-c".into(), source.into()],
            &[],
            overlay,
            b"",
            None,
        )
        .expect("python run")
}

// ── Basic execution ────────────────────────────────────────────────

#[test]
fn python_executes_inline_source() {
    let f = fixture(&[]);
    let out = run_python(f.overlay, "print('hello from python')");
    assert_eq!(out.stdout, b"hello from python\n");
    assert_eq!(out.exit_code, 0);
}

#[test]
fn python_runs_script_file_from_vfs() {
    let f = fixture(&[("/script.py", b"print('from file')\n")]);
    let out = interpreter()
        .run(
            &["python".into(), "/script.py".into()],
            &[],
            f.overlay,
            b"",
            None,
        )
        .expect("python run");
    assert_eq!(out.stdout, b"from file\n");
    assert_eq!(out.exit_code, 0);
}

#[test]
fn python_exit_code_propagates() {
    let f = fixture(&[]);
    let out = run_python(f.overlay, "import sys; sys.exit(3)");
    assert_eq!(out.exit_code, 3);
}

#[test]
fn python_stdin_is_readable() {
    let f = fixture(&[]);
    let out = interpreter()
        .run(
            &[
                "python".into(),
                "-c".into(),
                "import sys; print(sys.stdin.read().upper())".into(),
            ],
            &[],
            f.overlay,
            b"piped in\n",
            None,
        )
        .expect("python run");
    assert_eq!(out.stdout, b"PIPED IN\n\n");
}

// ── Cross-tool visibility through the shared overlay ───────────────

#[test]
fn python_reads_bash_pending_writes() {
    let mut f = fixture(&[]);
    f.shell.exec("echo staged-by-bash > /staged.txt").unwrap();
    let out = run_python(f.overlay, "print(open('/staged.txt').read().strip())");
    assert_eq!(out.stdout, b"staged-by-bash\n");
}

#[test]
fn bash_reads_python_pending_writes() {
    let mut f = fixture(&[]);
    let out = run_python(f.overlay, "open('/py.txt', 'w').write('made-by-python\\n')");
    assert_eq!(
        out.exit_code,
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let r = f.shell.exec("cat /py.txt").unwrap();
    assert_eq!(r.stdout, "made-by-python\n");
}

#[test]
fn python_reads_disk_through_overlay_lower_layer() {
    let f = fixture(&[("/on-disk.txt", b"from the lower layer\n")]);
    let out = run_python(f.overlay, "print(open('/on-disk.txt').read().strip())");
    assert_eq!(out.stdout, b"from the lower layer\n");
}

// ── diff() accounting for Python-originated changes ────────────────

#[test]
fn diff_reports_python_writes_and_deletions() {
    let f = fixture(&[("/victim.txt", b"delete me\n")]);
    let out = run_python(
        f.overlay.clone(),
        "import os; open('/created.txt', 'w').write('new\\n'); os.remove('/victim.txt')",
    );
    assert_eq!(
        out.exit_code,
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let d = f.overlay.diff();
    assert!(
        d.writes.iter().any(|w| w.path == Path::new("/created.txt")),
        "expected /created.txt in diff writes: {:?}",
        d.writes.iter().map(|w| &w.path).collect::<Vec<_>>()
    );
    assert_eq!(d.deletions, vec![Path::new("/victim.txt").to_path_buf()]);
}

#[test]
fn python_write_then_delete_within_run_absent_from_diff() {
    let f = fixture(&[]);
    let out = run_python(
        f.overlay.clone(),
        "import os; open('/ephemeral.txt', 'w').write('x'); os.remove('/ephemeral.txt')",
    );
    assert_eq!(out.exit_code, 0);
    let d = f.overlay.diff();
    assert!(
        !d.writes
            .iter()
            .any(|w| w.path == Path::new("/ephemeral.txt"))
    );
    assert!(d.deletions.is_empty());
}

// ── Bridge semantics: directory ops, errors, seek/append, symlinks ──

#[test]
fn python_listdir_merges_disk_and_pending_writes() {
    let mut f = fixture(&[("/dir/disk-file.txt", b"d\n")]);
    f.shell.exec("echo u > /dir/upper-file.txt").unwrap();
    let out = run_python(f.overlay, "import os; print(sorted(os.listdir('/dir')))");
    assert_eq!(out.stdout, b"['disk-file.txt', 'upper-file.txt']\n");
}

#[test]
fn python_missing_file_raises_file_not_found() {
    let f = fixture(&[]);
    let out = run_python(f.overlay, "open('/nope.txt')");
    assert_eq!(out.exit_code, 1);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("FileNotFoundError"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn python_seek_and_positional_write() {
    let f = fixture(&[("/data.bin", b"aaaaaaaaaa")]);
    let out = run_python(
        f.overlay.clone(),
        "f = open('/data.bin', 'r+b'); f.seek(3); f.write(b'BBB'); f.close()",
    );
    assert_eq!(
        out.exit_code,
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let r = rust_bash::VirtualFs::read_file(&*f.overlay, Path::new("/data.bin")).unwrap();
    assert_eq!(r, b"aaaBBBaaaa");
}

#[test]
fn python_append_mode_appends() {
    let f = fixture(&[("/log.txt", b"first\n")]);
    let out = run_python(
        f.overlay.clone(),
        "open('/log.txt', 'a').write('second\\n')",
    );
    assert_eq!(out.exit_code, 0);
    let r = rust_bash::VirtualFs::read_file(&*f.overlay, Path::new("/log.txt")).unwrap();
    assert_eq!(r, b"first\nsecond\n");
}

#[test]
fn python_follows_symlinks_with_vfs_semantics() {
    let mut f = fixture(&[("/real.txt", b"via symlink\n")]);
    f.shell.exec("ln -s /real.txt /link.txt").unwrap();
    let out = run_python(f.overlay, "print(open('/link.txt').read().strip())");
    assert_eq!(out.stdout, b"via symlink\n");
}

#[test]
fn python_pathlib_glob_over_overlay() {
    let mut f = fixture(&[("/data/a.json", b"{}"), ("/data/b.json", b"{}")]);
    f.shell.exec("echo '{}' > /data/c.json").unwrap();
    let out = run_python(
        f.overlay,
        "from pathlib import Path; print(sorted(p.name for p in Path('/data').glob('*.json')))",
    );
    assert_eq!(out.stdout, b"['a.json', 'b.json', 'c.json']\n");
}

// ── Conformance: open flags, errno paths, offset safety ────────────

#[test]
fn python_exclusive_create_on_existing_fails() {
    let f = fixture(&[("/exists.txt", b"here\n")]);
    let out = run_python(f.overlay, "open('/exists.txt', 'x')");
    assert_eq!(out.exit_code, 1);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("FileExistsError"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn python_opening_directory_as_file_fails() {
    let f = fixture(&[("/dir/file.txt", b"x")]);
    let out = run_python(f.overlay, "open('/dir', 'w')");
    assert_eq!(out.exit_code, 1);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("IsADirectoryError"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn python_cross_directory_rename() {
    let f = fixture(&[("/a/f.txt", b"moved\n"), ("/b/keep.txt", b"k\n")]);
    let out = run_python(
        f.overlay.clone(),
        "import os; os.rename('/a/f.txt', '/b/f.txt')",
    );
    assert_eq!(
        out.exit_code,
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let d = f.overlay.diff();
    // Rename of a disk file = write at the destination + deletion at the source.
    assert!(d.writes.iter().any(|w| w.path == Path::new("/b/f.txt")));
    assert!(d.deletions.contains(&Path::new("/a/f.txt").to_path_buf()));
}

#[test]
fn python_write_past_eof_zero_fills() {
    let f = fixture(&[("/gap.bin", b"ab")]);
    let out = run_python(
        f.overlay.clone(),
        "f = open('/gap.bin', 'r+b'); f.seek(5); f.write(b'Z'); f.close()",
    );
    assert_eq!(
        out.exit_code,
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let r = rust_bash::VirtualFs::read_file(&*f.overlay, Path::new("/gap.bin")).unwrap();
    assert_eq!(r, b"ab\0\0\0Z");
}

#[test]
fn python_huge_offset_write_fails_without_host_abort() {
    let f = fixture(&[("/small.bin", b"x")]);
    // A guest-controlled offset must hit the bridge's EFBIG guard, not a
    // host allocation. If the host survives to assert, the guard worked.
    // (Relies on try_reserve(1 PiB) failing on 47-bit-VA hosts; a host with
    // 5-level paging could theoretically satisfy the reservation — the guard
    // then degrades to real memory pressure, still without a clean abort
    // guarantee. Practically fine on every CI/host target today.)
    let out = run_python(
        f.overlay.clone(),
        r#"import os
fd = os.open('/small.bin', os.O_RDWR)
print('seek ->', os.lseek(fd, 2**50, 0))
try:
    n = os.write(fd, b'y')
    print('WRITE SUCCEEDED', n)
except OSError as e:
    print('write raised errno', e.errno, e.strerror)
"#,
    );
    // WASI errno 22 = FBIG ("File too large"): the bridge's size guard.
    assert_eq!(
        out.stdout,
        b"seek -> 1125899906842624\nwrite raised errno 22 File too large\n"
    );
    assert_eq!(out.exit_code, 0);
    // The file must be untouched.
    let r = rust_bash::VirtualFs::read_file(&*f.overlay, Path::new("/small.bin")).unwrap();
    assert_eq!(r, b"x");
}

#[test]
fn python_huge_truncate_fails_without_host_abort() {
    let f = fixture(&[("/small.bin", b"x")]);
    let out = run_python(
        f.overlay.clone(),
        r#"import os, errno
try:
    os.truncate('/small.bin', 2**60)
    print('TRUNCATE SUCCEEDED')
except OSError as e:
    print('truncate raised EFBIG:', e.errno == errno.EFBIG)
"#,
    );
    assert_eq!(out.stdout, b"truncate raised EFBIG: True\n");
    let r = rust_bash::VirtualFs::read_file(&*f.overlay, Path::new("/small.bin")).unwrap();
    assert_eq!(r, b"x");
}

#[test]
fn python_nofollow_open_on_symlink_fails() {
    let mut f = fixture(&[("/real.txt", b"target\n")]);
    f.shell.exec("ln -s /real.txt /link.txt").unwrap();
    let out = run_python(
        f.overlay,
        r#"import os, errno
try:
    os.open('/link.txt', os.O_RDONLY | os.O_NOFOLLOW)
    print('OPEN SUCCEEDED')
except OSError as e:
    print('open raised ELOOP:', e.errno == errno.ELOOP)
"#,
    );
    assert_eq!(out.stdout, b"open raised ELOOP: True\n");
}

#[test]
fn python_env_is_passed_through() {
    let f = fixture(&[]);
    let out = interpreter()
        .run(
            &[
                "python".into(),
                "-c".into(),
                "import os; print(os.environ['MARKER'])".into(),
            ],
            &[("MARKER".into(), "env-works".into())],
            f.overlay,
            b"",
            None,
        )
        .expect("python run");
    assert_eq!(out.stdout, b"env-works\n");
}

#[test]
fn python_bridge_created_symlink_and_hardlink() {
    let f = fixture(&[("/real.txt", b"linked\n")]);
    let out = run_python(
        f.overlay.clone(),
        "import os; os.symlink('/real.txt', '/sym.txt'); os.link('/real.txt', '/hard.txt'); \
         print(os.readlink('/sym.txt')); print(open('/hard.txt').read().strip())",
    );
    assert_eq!(
        out.stdout,
        b"/real.txt\nlinked\n",
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn python_seek_from_end_and_negative_seek_error() {
    let f = fixture(&[("/data.txt", b"0123456789")]);
    let out = run_python(
        f.overlay,
        r#"
f = open('/data.txt', 'rb')
f.seek(-3, 2)  # SEEK_END
print(f.read())
try:
    f.seek(-100, 0)  # SEEK_SET to a negative offset
except OSError as e:
    print('negative seek rejected')
"#,
    );
    assert_eq!(
        out.stdout,
        b"b'789'\nnegative seek rejected\n",
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── Opt-in limits (PythonLimits) ───────────────────────────────────

#[test]
fn python_fuel_budget_stops_runaway_script() {
    let f = fixture(&[]);
    // Budget rationale: measured CPython boot (wasi-vfs init + interpreter
    // startup) costs ~2–5×10^8 fuel on this artifact/host; 2×10^9 leaves
    // 4–10× margin for startup variance, and the loop burns the rest.
    let limits = rust_bash::python::PythonLimits {
        fuel: Some(2_000_000_000),
        ..Default::default()
    };
    let result = interpreter().run(
        &[
            "python".into(),
            "-c".into(),
            "import sys; sys.stderr.write('ERRMARK\\n'); sys.stderr.flush(); sys.stdout.write('lost-at-trap');\nwhile True: pass".into(),
        ],
        &[],
        f.overlay,
        b"",
        Some(&limits),
    );
    // No wall-clock assertion: the Trap + fuel-exhaustion match IS the
    // assertion; if metering broke, run() would hang regardless.
    match result {
        Err(rust_bash::python::PythonError::Trap(e, out)) => {
            assert!(
                format!("{e:#}").contains("fuel exhausted"),
                "expected fuel exhaustion, got: {e:#}"
            );
            // Unbuffered stderr written before the trap is preserved;
            // libc-buffered stdout is lost on trap, as with a real process.
            assert_eq!(out.stderr, b"ERRMARK\n");
        }
        other => panic!("expected Trap, got: {other:?}"),
    }
}

#[test]
fn python_without_limits_runs_unbounded_by_default() {
    // A nontrivial-but-finite loop completes with no limits configured.
    let f = fixture(&[]);
    let out = run_python(
        f.overlay,
        "total = 0\nfor i in range(2_000_000):\n    total += i\nprint(total)",
    );
    assert_eq!(out.stdout, b"1999999000000\n");
}

#[test]
fn python_max_file_size_cap_enforced() {
    let f = fixture(&[]);
    let limits = rust_bash::python::PythonLimits {
        max_file_size: Some(16),
        ..Default::default()
    };
    let out = interpreter()
        .run(
            &[
                "python".into(),
                "-c".into(),
                r#"f = open('/capped.bin', 'wb')
try:
    f.write(b'x' * 100); f.flush(); print('WRITE SUCCEEDED')
except OSError as e:
    print('write raised errno', e.errno)
"#
                .into(),
            ],
            &[],
            f.overlay,
            b"",
            Some(&limits),
        )
        .expect("python run");
    assert_eq!(out.stdout, b"write raised errno 22\n");
}

// ── Errno mapping coverage ─────────────────────────────────────────

#[test]
fn python_path_through_file_raises_not_found() {
    // NOTE: the VFS resolves `/file.txt/child` to ENOENT where POSIX would
    // give ENOTDIR (it does not walk intermediate components) — same
    // behavior bash sees through the same VFS. Pinned to document the
    // divergence; if the VFS ever learns ENOTDIR, update this test.
    let f = fixture(&[("/file.txt", b"x")]);
    let out = run_python(f.overlay, "open('/file.txt/child')");
    assert_eq!(out.exit_code, 1);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("FileNotFoundError"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn python_rmdir_nonempty_raises_enotempty() {
    let f = fixture(&[("/dir/child.txt", b"x")]);
    let out = run_python(
        f.overlay,
        "import os, errno\ntry:\n    os.rmdir('/dir')\n    print('RMDIR SUCCEEDED')\nexcept OSError as e:\n    print('rmdir raised ENOTEMPTY:', e.errno == errno.ENOTEMPTY)\n",
    );
    assert_eq!(out.stdout, b"rmdir raised ENOTEMPTY: True\n");
}

#[test]
fn python_read_on_write_only_fd_raises_ebadf() {
    let f = fixture(&[]);
    let out = run_python(
        f.overlay,
        "import os, errno\nfd = os.open('/w.txt', os.O_WRONLY | os.O_CREAT)\ntry:\n    os.read(fd, 1)\n    print('READ SUCCEEDED')\nexcept OSError as e:\n    print('read raised EBADF:', e.errno == errno.EBADF)\n",
    );
    assert_eq!(out.stdout, b"read raised EBADF: True\n");
}

// ── max_file_size cap boundary ─────────────────────────────────────

#[test]
fn python_max_file_size_boundary_exact() {
    let f = fixture(&[]);
    let limits = rust_bash::python::PythonLimits {
        max_file_size: Some(16),
        ..Default::default()
    };
    // Exactly-at-cap write succeeds; one byte over fails with EFBIG and
    // leaves no partial file behind (splice rejects before writing).
    let out = interpreter()
        .run(
            &["python".into(), "-c".into(),
              "import os, errno\nf = open('/cap.bin', 'wb')\nf.write(b'x' * 16)\nf.flush()\nprint('at-cap ok')\ntry:\n    f.write(b'y')\n    f.flush()\n    print('over-cap SUCCEEDED')\nexcept OSError as e:\n    print('over-cap EFBIG:', e.errno == errno.EFBIG)\ntry:\n    f.close()\nexcept OSError:\n    pass\nprint('size:', os.path.getsize('/cap.bin'))".into()],
            &[],
            f.overlay,
            b"",
            Some(&limits),
        )
        .expect("python run");
    assert_eq!(out.stdout, b"at-cap ok\nover-cap EFBIG: True\nsize: 16\n");
}

#[test]
fn python_truncate_over_configured_cap_fails() {
    let f = fixture(&[("/small.bin", b"x")]);
    let limits = rust_bash::python::PythonLimits {
        max_file_size: Some(16),
        ..Default::default()
    };
    let out = interpreter()
        .run(
            &["python".into(), "-c".into(),
              "import os, errno\ntry:\n    os.truncate('/small.bin', 100)\n    print('TRUNCATE SUCCEEDED')\nexcept OSError as e:\n    print('truncate raised EFBIG:', e.errno == errno.EFBIG)\n".into()],
            &[],
            f.overlay,
            b"",
            Some(&limits),
        )
        .expect("python run");
    assert_eq!(out.stdout, b"truncate raised EFBIG: True\n");
}

// ── Error taxonomy & isolation ─────────────────────────────────────

#[test]
fn python_compile_error_on_invalid_module() {
    let result = PythonInterpreter::new(b"this is not wasm");
    assert!(
        matches!(result, Err(rust_bash::python::PythonError::Compile(_))),
        "expected Compile error, got: {:?}",
        result.map(|_| ())
    );
}

#[test]
fn python_runs_are_state_isolated() {
    let f = fixture(&[]);
    let out1 = run_python(f.overlay.clone(), "GLOBAL_X = 42\nprint('first run')");
    assert_eq!(out1.stdout, b"first run\n");
    // A second run on the same interpreter gets a fresh interpreter state:
    // GLOBAL_X from the first run must not exist.
    let out2 = run_python(f.overlay, "print('GLOBAL_X' in dir())");
    assert_eq!(out2.stdout, b"False\n");
}

#[test]
fn python_write_through_dangling_symlink_creates_target() {
    // POSIX open(O_CREAT) semantics (shared with bash): writing through a
    // DANGLING symlink creates the link TARGET; the link itself is preserved.
    let mut f = fixture(&[]);
    f.shell.exec("ln -s /target.txt /dangling").unwrap();
    let out = run_python(f.overlay.clone(), "open('/dangling', 'w').write('filled')");
    assert_eq!(
        out.exit_code,
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let r = f
        .shell
        .exec("test -L /dangling && echo is-link || echo not-link")
        .unwrap();
    assert_eq!(r.stdout, "is-link\n");
    let r = f.shell.exec("cat /target.txt").unwrap();
    assert_eq!(r.stdout, "filled");
}
