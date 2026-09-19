//! Wall-clock benchmarks for OverlayFs workloads that mirror real agent
//! sessions (ported from the just-bash fork's overlay-fs.perf.test.ts, plus
//! a whiteout-storm case). Measurement-only: these print `PERF <name>` lines
//! and assert functional sanity, never timing thresholds (CI-safe).
//!
//! These pins caught two real regressions once (rm -rf walking the lower
//! subtree; per-component lower lstats in resolve_path) — keep them.
//!
//! Run: cargo test --release --all-features --test overlay_perf -- --ignored --nocapture
#![cfg(feature = "native-fs")]

use rust_bash::vfs::{OverlayFs, VirtualFs};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn body() -> String {
    "x".repeat(120)
}

fn disk_rel(i: u32, depth: u32) -> String {
    (0..depth)
        .map(|d| format!("d{}", (i >> (d * 4)) % 16))
        .collect::<Vec<_>>()
        .join("/")
}

fn write_disk_files(root: &Path, prefix: &str, count: u32, depth: u32) {
    for i in 0..count {
        let dir = root.join(prefix).join(disk_rel(i, depth));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("f{i}.txt")), body()).unwrap();
    }
}

fn walk_and_stat(fs: &OverlayFs, dir: &Path) -> usize {
    let mut count = 0;
    for e in fs.readdir(dir).unwrap() {
        let p = dir.join(&e.name);
        count += 1;
        let _ = fs.stat(&p);
        if e.node_type == rust_bash::vfs::NodeType::Directory {
            count += walk_and_stat(fs, &p);
        }
    }
    count
}

fn timed<F: FnMut()>(name: &str, mut f: F) {
    let mut runs = Vec::new();
    for _ in 0..3 {
        let t0 = Instant::now();
        f();
        runs.push(t0.elapsed().as_millis());
    }
    println!(
        "PERF {name}: best={}ms runs={runs:?}",
        runs.iter().min().unwrap()
    );
}

fn fresh_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("rb-perf-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
#[ignore = "wall-clock benchmark; run explicitly in release mode"]
fn perf_explore() {
    let dir = fresh_dir("explore");
    write_disk_files(&dir, "src", 1200, 3);
    write_disk_files(&dir, "node_modules", 800, 4);
    let fs = OverlayFs::new(&dir).unwrap();
    let mut seen = 0;
    timed("explore", || {
        seen = walk_and_stat(&fs, Path::new("/"));
        for i in 0..200u32 {
            let _ = fs
                .read_file(Path::new(&format!(
                    "/node_modules/{}/f{i}.txt",
                    disk_rel(i, 4)
                )))
                .unwrap();
        }
    });
    assert!(seen > 2000);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "wall-clock benchmark; run explicitly in release mode"]
fn perf_edit_churn() {
    let dir = fresh_dir("churn");
    write_disk_files(&dir, "src", 400, 2);
    let fs = OverlayFs::new(&dir).unwrap();
    timed("edit-churn", || {
        for i in 0..400u32 {
            fs.write_file(
                Path::new(&format!("/src/{}/f{i}.txt", disk_rel(i, 2))),
                body().repeat(2).as_bytes(),
            )
            .unwrap();
        }
        for log in 0..20 {
            for _ in 0..25 {
                fs.append_file(
                    Path::new(&format!("/src/d0/d0/f{log}.txt")),
                    body().as_bytes(),
                )
                .unwrap();
            }
        }
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "wall-clock benchmark; run explicitly in release mode"]
fn perf_delete_heavy() {
    let dir = fresh_dir("del");
    write_disk_files(&dir, "deps", 5000, 3);
    write_disk_files(&dir, "src", 300, 2);
    let fs = OverlayFs::new(&dir).unwrap();
    fs.remove_dir_all(Path::new("/deps")).unwrap();
    assert!(!fs.exists(Path::new("/deps")));
    timed("delete-heavy:post-rm-work", || {
        for i in 0..300 {
            let _ = fs.readdir(Path::new("/src")).unwrap();
            let _ = fs.stat(Path::new(&format!("/src/d{}", i % 16)));
            let _ = fs.exists(Path::new(&format!("/deps/d0/f{i}.txt")));
        }
    });
    timed("delete-heavy:rm-rf-5k", || {
        let fresh = OverlayFs::new(&dir).unwrap();
        fresh.remove_dir_all(Path::new("/deps")).unwrap();
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "wall-clock benchmark; run explicitly in release mode"]
fn perf_delete_recreate() {
    let dir = fresh_dir("delre");
    write_disk_files(&dir, "pkg", 500, 2);
    let fs = OverlayFs::new(&dir).unwrap();
    timed("delete-recreate", || {
        for _ in 0..10 {
            fs.remove_dir_all(Path::new("/pkg")).unwrap();
            for i in 0..500u32 {
                fs.write_file(
                    Path::new(&format!("/pkg/d{}/d{}/f{i}.txt", i % 16, (i >> 4) % 16)),
                    body().as_bytes(),
                )
                .unwrap();
            }
            let _ = fs.readdir(Path::new("/pkg")).unwrap();
        }
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "wall-clock benchmark; run explicitly in release mode"]
fn perf_deep_stat() {
    let dir = fresh_dir("deep");
    let fs = OverlayFs::new(&dir).unwrap();
    let deep = format!(
        "/{}/",
        (0..12)
            .map(|d| format!("l{d}"))
            .collect::<Vec<_>>()
            .join("/")
    );
    for i in 0..100 {
        fs.write_file(Path::new(&format!("{deep}f{i}.txt")), body().as_bytes())
            .unwrap();
    }
    timed("deep-stat", || {
        for i in 0..3000 {
            let _ = fs.stat(Path::new(&format!("{deep}f{}.txt", i % 100)));
            let _ = fs.exists(Path::new(&format!("{deep}f{}.txt", (i + 1) % 100)));
        }
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// Whiteout storm: thousands of INDIVIDUAL deletions (not one rm -rf),
/// then diff(). Their suite lacks this; it stresses our side-table's
/// O(w²) top-most filter.
#[test]
#[ignore = "wall-clock benchmark; run explicitly in release mode"]
fn perf_whiteout_storm_diff() {
    let dir = fresh_dir("storm");
    write_disk_files(&dir, "many", 3000, 3);
    let fs = OverlayFs::new(&dir).unwrap();
    for i in 0..3000u32 {
        fs.remove_file(Path::new(&format!("/many/{}/f{i}.txt", disk_rel(i, 3))))
            .unwrap();
    }
    timed("whiteout-storm:diff", || {
        let d = fs.diff();
        assert_eq!(d.deletions.len(), 3000);
    });
    // Also: path resolution cost with 3000 whiteouts present
    timed("whiteout-storm:stat-missing", || {
        for i in 0..3000u32 {
            let _ = fs.exists(Path::new(&format!("/many/{}/f{i}.txt", disk_rel(i, 3))));
        }
    });
    let _ = std::fs::remove_dir_all(&dir);
}
