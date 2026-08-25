//! Coverage-triage tests for `src/commands/file_ops.rs`
//! (cp, mv, rm, tee, stat, chmod, mkfifo, ln, readlink, rmdir, du, split).
//!
//! Every test here exists to cover a previously-uncovered region of that
//! file: untested flag combinations, error paths, and edge-case operands.
//! Where behavior diverges from real bash/GNU coreutils, the actual behavior
//! is pinned with a comment (runtime behavior is intentionally not changed).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use rust_bash::{
    DirEntry, ExecResult, InMemoryFs, Metadata, RustBash, RustBashBuilder, VfsError, VirtualFs,
};

fn shell() -> RustBash {
    RustBashBuilder::new().build().unwrap()
}

fn run(script: &str) -> ExecResult {
    shell().exec(script).unwrap()
}

fn run_with_files(files: &[(&str, &str)], script: &str) -> ExecResult {
    let map: HashMap<String, Vec<u8>> = files
        .iter()
        .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
        .collect();
    RustBashBuilder::new()
        .files(map)
        .build()
        .unwrap()
        .exec(script)
        .unwrap()
}

// ── cp ───────────────────────────────────────────────────────────────

#[test]
fn cp_ignores_unknown_flag_chars() {
    let r = run_with_files(&[("/a", "hello\n")], "cp -q /a /b && cat /b");
    assert_eq!(r.stdout, "hello\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn cp_multiple_sources_to_non_directory_fails() {
    let r = run_with_files(&[("/a", "1\n"), ("/b", "2\n")], "cp /a /b /notdir");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "cp: target '/notdir' is not a directory\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn cp_recursive_into_path_below_a_file_fails() {
    // copy_dir_recursive -> mkdir_p fails because /f is a regular file.
    let r = run_with_files(&[("/d/f", "x\n"), ("/f", "y\n")], "cp -r /d /f/sub");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "cp: Not a directory: /f/sub\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn cp_file_into_missing_directory_fails() {
    let r = run_with_files(&[("/a", "hello\n")], "cp /a /nodir/out");
    assert_eq!(r.stdout, "");
    assert_eq!(
        r.stderr,
        "cp: cannot copy '/a': No such file or directory: /nodir\n"
    );
    assert_eq!(r.exit_code, 1);
}

#[test]
fn cp_recursive_copies_nested_subdirectories() {
    let r = run_with_files(
        &[("/d/sub/deep.txt", "deep\n"), ("/d/top.txt", "top\n")],
        "cp -r /d /e && cat /e/top.txt /e/sub/deep.txt",
    );
    assert_eq!(r.stdout, "top\ndeep\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

// ── mv ───────────────────────────────────────────────────────────────

#[test]
fn mv_double_dash_and_ignored_flags() {
    let r = run_with_files(
        &[("/a", "1\n"), ("/b", "2\n")],
        "mv -- /a /c && mv -f /b /d && cat /c /d",
    );
    assert_eq!(r.stdout, "1\n2\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn mv_multiple_sources_to_non_directory_fails() {
    let r = run_with_files(&[("/a", "1\n"), ("/b", "2\n")], "mv /a /b /notdir");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "mv: target '/notdir' is not a directory\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn mv_nonexistent_source_fails() {
    let r = run("mv /nonexistent /x");
    assert_eq!(r.stdout, "");
    assert_eq!(
        r.stderr,
        "mv: cannot move '/nonexistent': No such file or directory: /nonexistent\n"
    );
    assert_eq!(r.exit_code, 1);
}

// ── rm ───────────────────────────────────────────────────────────────

#[test]
fn rm_double_dash_and_unknown_flag_chars() {
    let r = run_with_files(
        &[("/a", "1\n"), ("/b", "2\n")],
        "rm -- /a && rm -q /b && test ! -e /a && test ! -e /b && echo removed",
    );
    assert_eq!(r.stdout, "removed\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn rm_recursive_on_symlink_to_directory_fails() {
    // stat follows the symlink and reports a directory, but
    // InMemoryFs::remove_dir_all refuses to remove a symlink node.
    let r = run_with_files(&[("/real/f", "x\n")], "ln -s /real /link; rm -r /link");
    assert_eq!(r.stdout, "");
    assert_eq!(
        r.stderr,
        "rm: cannot remove '/link': Not a directory: /link\n"
    );
    assert_eq!(r.exit_code, 1);
}

// ── tee ──────────────────────────────────────────────────────────────

#[test]
fn tee_double_dash() {
    let r = run("echo data | tee -- /f && cat /f");
    assert_eq!(r.stdout, "data\ndata\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn tee_write_error_still_echoes_stdout() {
    let r = run("echo data | tee /nodir/f");
    assert_eq!(r.stdout, "data\n");
    assert_eq!(
        r.stderr,
        "tee: /nodir/f: No such file or directory: /nodir\n"
    );
    assert_eq!(r.exit_code, 1);
}

// ── stat ─────────────────────────────────────────────────────────────

#[test]
fn stat_double_dash_and_ignored_flags() {
    let r = run_with_files(&[("/f", "hello\n")], "stat -- /f; stat -L /f");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout.matches("File: /f").count(), 2);
}

#[test]
fn stat_missing_operand() {
    let r = run("stat");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "stat: missing operand\n");
    assert_eq!(r.exit_code, 1);
}

// ── chmod ────────────────────────────────────────────────────────────

#[test]
fn chmod_double_dash() {
    let r = run_with_files(&[("/f", "x\n")], "chmod -- 755 /f && stat /f");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Mode: (0755/-)"));
}

#[test]
fn chmod_symbolic_on_nonexistent_file_fails() {
    let r = run("chmod u+x /nonexistent");
    assert_eq!(r.stdout, "");
    assert_eq!(
        r.stderr,
        "chmod: cannot change mode of '/nonexistent': No such file or directory: /nonexistent\n"
    );
    assert_eq!(r.exit_code, 1);
}

#[test]
fn chmod_absolute_on_nonexistent_file_fails() {
    let r = run("chmod 755 /nonexistent");
    assert_eq!(r.stdout, "");
    assert_eq!(
        r.stderr,
        "chmod: cannot change mode of '/nonexistent': No such file or directory: /nonexistent\n"
    );
    assert_eq!(r.exit_code, 1);
}

#[test]
fn chmod_symbolic_other_and_sticky_bit() {
    let r = run_with_files(
        &[("/f", "x\n")],
        "chmod 600 /f && chmod o+r /f && stat /f && chmod +t /f && stat /f",
    );
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Mode: (0604/-)"));
    assert!(r.stdout.contains("Mode: (1604/-)"));
}

#[test]
fn chmod_rejects_invalid_modes() {
    // "+" with empty perms, invalid who char, setuid on "other", invalid perm char
    let r = run_with_files(
        &[("/f", "x\n")],
        "chmod + /f; chmod q+r /f; chmod o+s /f; chmod u+q /f",
    );
    assert_eq!(r.stdout, "");
    assert_eq!(
        r.stderr,
        "chmod: invalid mode: '+'\n\
         chmod: invalid mode: 'q+r'\n\
         chmod: invalid mode: 'o+s'\n\
         chmod: invalid mode: 'u+q'\n"
    );
    assert_eq!(r.exit_code, 1);
}

// ── mkfifo ───────────────────────────────────────────────────────────

#[test]
fn mkfifo_missing_operand() {
    let r = run("mkfifo");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "mkfifo: missing operand\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn mkfifo_existing_path_fails() {
    let r = run_with_files(&[("/f", "x\n")], "mkfifo /f");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "mkfifo: cannot create fifo '/f': File exists\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn mkfifo_in_missing_directory_fails() {
    let r = run("mkfifo /nodir/p");
    assert_eq!(r.stdout, "");
    assert_eq!(
        r.stderr,
        "mkfifo: cannot create fifo '/nodir/p': No such file or directory: /nodir\n"
    );
    assert_eq!(r.exit_code, 1);
}

#[test]
fn mkfifo_creates_fifo_with_pipe_mode() {
    let r = run("mkfifo /p && stat /p");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Mode: (10644/-)"));
}

// ── ln ───────────────────────────────────────────────────────────────

#[test]
fn ln_double_dash() {
    let r = run_with_files(&[("/a", "1\n")], "ln -- /a /b && cat /b");
    assert_eq!(r.stdout, "1\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn ln_hardlink_nonexistent_target_fails() {
    let r = run("ln /nonexistent /b");
    assert_eq!(r.stdout, "");
    assert_eq!(
        r.stderr,
        "ln: failed to create link '/b': No such file or directory: /nonexistent\n"
    );
    assert_eq!(r.exit_code, 1);
}

#[test]
fn ln_symlink_over_existing_path_fails() {
    let r = run_with_files(&[("/a", "1\n"), ("/b", "2\n")], "ln -s /a /b");
    assert_eq!(r.stdout, "");
    assert_eq!(
        r.stderr,
        "ln: failed to create link '/b': Already exists: /b\n"
    );
    assert_eq!(r.exit_code, 1);
}

// ── readlink ─────────────────────────────────────────────────────────

#[test]
fn readlink_absorbs_unknown_flags() {
    let r = run_with_files(&[("/a", "1\n")], "ln -s /a /link && readlink -z /link");
    assert_eq!(r.stdout, "/a\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn readlink_missing_operand() {
    let r = run("readlink");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "readlink: missing operand\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn readlink_e_on_dangling_symlink_fails_in_canonicalize() {
    let r = run("ln -s /gone /link && readlink -e /link");
    assert_eq!(r.stdout, "");
    assert_eq!(
        r.stderr,
        "readlink: /link: No such file or directory: /gone\n"
    );
    assert_eq!(r.exit_code, 1);
}

#[test]
fn readlink_f_on_nonexistent_path_fails() {
    let r = run("readlink -f /nonexistent");
    assert_eq!(r.stdout, "");
    assert_eq!(
        r.stderr,
        "readlink: /nonexistent: No such file or directory: /nonexistent\n"
    );
    assert_eq!(r.exit_code, 1);
}

#[test]
fn readlink_m_normalizes_without_existence_check() {
    let r = run("readlink -m /a/../b");
    assert_eq!(r.stdout, "/b\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn readlink_on_regular_file_fails() {
    let r = run_with_files(&[("/f", "x\n")], "readlink /f");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "readlink: /f: Invalid path: not a symlink: /f\n");
    assert_eq!(r.exit_code, 1);
}

// ── rmdir ────────────────────────────────────────────────────────────

#[test]
fn rmdir_missing_operand() {
    let r = run("rmdir");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "rmdir: missing operand\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn rmdir_nonexistent_fails() {
    let r = run("rmdir /nonexistent");
    assert_eq!(r.stdout, "");
    assert_eq!(
        r.stderr,
        "rmdir: failed to remove '/nonexistent': No such file or directory: /nonexistent\n"
    );
    assert_eq!(r.exit_code, 1);
}

#[test]
fn rmdir_symlink_to_empty_directory_fails() {
    // readdir follows the symlink and reports an empty directory, but
    // InMemoryFs::remove_dir refuses to remove a symlink node.
    let r = run("mkdir /real && ln -s /real /link && rmdir /link");
    assert_eq!(r.stdout, "");
    assert_eq!(
        r.stderr,
        "rmdir: failed to remove '/link': Not a directory: /link\n"
    );
    assert_eq!(r.exit_code, 1);
}

#[test]
fn rmdir_parents_removes_empty_ancestors_up_to_root() {
    let r = run("mkdir -p /a/b/c && rmdir -p /a/b/c && test ! -e /a && echo gone");
    assert_eq!(r.stdout, "gone\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn rmdir_parents_stops_at_non_empty_ancestor() {
    let r = run("mkdir -p /x/y && touch /x/keep && rmdir -p /x/y; ls /x");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "keep\n");
}

#[test]
fn rmdir_parents_stops_when_ancestor_is_a_symlink() {
    // After removing /link/sub, the -p walk tries to remove /link itself;
    // remove_dir on a symlink fails, hitting the is_err break.
    let r = run("mkdir -p /real/sub && ln -s /real /link && rmdir -p /link/sub; ls /");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("link"));
    assert!(r.stdout.contains("real"));
}

// ── du ───────────────────────────────────────────────────────────────

#[test]
fn du_reports_subdirectories_and_total() {
    // 1500-byte file (2 blocks) plus 5-byte file in a subdir (1 block).
    let r =
        run("mkdir -p /d/sub && printf '%01500d' 0 > /d/f && printf '12345' > /d/sub/g && du /d");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1\t/d/sub\n2\t/d\n");
}

#[test]
fn du_all_files_lists_regular_files() {
    let r = run(
        "mkdir -p /d/sub && printf '%01500d' 0 > /d/f && printf '12345' > /d/sub/g && du -a /d",
    );
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "2\t/d/f\n1\t/d/sub/g\n1\t/d/sub\n2\t/d\n");
}

#[test]
fn du_max_depth_limits_printed_levels() {
    let setup = "mkdir -p /d/sub && printf '12345' > /d/sub/g";
    let r = run(&format!("{setup} && du -d 0 /d"));
    assert_eq!(r.stdout, "1\t/d\n");
    let r = run(&format!("{setup} && du -d1 /d"));
    assert_eq!(r.stdout, "1\t/d/sub\n1\t/d\n");
}

#[test]
fn du_human_readable_small_sizes() {
    let r = run("mkdir /d && printf '12345' > /d/f && du -h /d");
    assert_eq!(r.stdout, "5B\t/d\n");
    let r = run("mkdir /e && printf '%02048d' 0 > /e/f && du -h /e");
    assert_eq!(r.stdout, "2.0K\t/e\n");
}

#[test]
fn du_default_target_is_dot() {
    let r = run("mkdir -p /w/d && printf 'x' > /w/f && cd /w && du");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "0\t./d\n1\t.\n");
}

#[test]
fn du_combined_flags_and_unknown_flag_char() {
    // -sh: summary + human; -hz: unknown 'z' is silently ignored.
    let r = run("mkdir /d && printf '12345' > /d/f && du -sh /d && du -hz /d");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "5B\t/d\n5B\t/d\n");

    // -ah: 'a' in a combined flag lists regular files too.
    let r = run("mkdir /d && printf '12345' > /d/f && du -ah /d");
    assert_eq!(r.stdout, "5B\t/d/f\n5B\t/d\n");
}

#[test]
fn du_skips_children_whose_stat_fails() {
    // A dangling symlink makes stat() fail for that child; it is silently
    // skipped (contributes nothing to the total or the output).
    let r = run("mkdir /d && printf '12345' > /d/f && ln -s /gone /d/link && du /d");
    assert_eq!(r.stdout, "1\t/d\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn du_nonexistent_target_fails() {
    let r = run("du /nonexistent");
    assert_eq!(r.stdout, "");
    assert_eq!(
        r.stderr,
        "du: cannot access '/nonexistent': No such file or directory: /nonexistent\n"
    );
    assert_eq!(r.exit_code, 1);
}

#[test]
fn du_on_plain_file_prints_nothing_without_summary() {
    // PINNED DIVERGENCE: GNU `du /f` prints "1\t/f"; rust-bash's du_walk
    // returns an empty output string for non-directory targets, so nothing
    // is printed. Actual behavior pinned; runtime not changed.
    let r = run_with_files(&[("/f", "12345")], "du /f");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
    // Summary mode does report the file.
    let r = run_with_files(&[("/f", "12345")], "du -s /f");
    assert_eq!(r.stdout, "1\t/f\n");
}

/// VirtualFs wrapper that multiplies all reported node sizes (files,
/// directories, symlinks) by a constant factor, so du's human-readable M/G
/// formatting can be exercised without allocating megabytes/gigabytes of
/// content in memory. It deliberately presents an inconsistent view
/// (stat size != content length) and is only valid for du-style consumers
/// that read sizes from stat and never cross-check against content.
struct InflatedSizeFs {
    inner: Arc<dyn VirtualFs>,
    factor: u64,
}

impl InflatedSizeFs {
    fn inflate(&self, mut meta: Metadata) -> Metadata {
        meta.size = meta.size.saturating_mul(self.factor);
        meta
    }
}

impl VirtualFs for InflatedSizeFs {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        self.inner.read_file(path)
    }
    fn write_file(&self, path: &Path, content: &[u8]) -> Result<(), VfsError> {
        self.inner.write_file(path, content)
    }
    fn append_file(&self, path: &Path, content: &[u8]) -> Result<(), VfsError> {
        self.inner.append_file(path, content)
    }
    fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        self.inner.remove_file(path)
    }
    fn mkdir(&self, path: &Path) -> Result<(), VfsError> {
        self.inner.mkdir(path)
    }
    fn mkdir_p(&self, path: &Path) -> Result<(), VfsError> {
        self.inner.mkdir_p(path)
    }
    fn readdir(&self, path: &Path) -> Result<Vec<DirEntry>, VfsError> {
        self.inner.readdir(path)
    }
    fn remove_dir(&self, path: &Path) -> Result<(), VfsError> {
        self.inner.remove_dir(path)
    }
    fn remove_dir_all(&self, path: &Path) -> Result<(), VfsError> {
        self.inner.remove_dir_all(path)
    }
    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }
    fn stat(&self, path: &Path) -> Result<Metadata, VfsError> {
        self.inner.stat(path).map(|m| self.inflate(m))
    }
    fn lstat(&self, path: &Path) -> Result<Metadata, VfsError> {
        self.inner.lstat(path).map(|m| self.inflate(m))
    }
    fn chmod(&self, path: &Path, mode: u32) -> Result<(), VfsError> {
        self.inner.chmod(path, mode)
    }
    fn utimes(&self, path: &Path, mtime: SystemTime) -> Result<(), VfsError> {
        self.inner.utimes(path, mtime)
    }
    fn symlink(&self, target: &Path, link: &Path) -> Result<(), VfsError> {
        self.inner.symlink(target, link)
    }
    fn hardlink(&self, src: &Path, dst: &Path) -> Result<(), VfsError> {
        self.inner.hardlink(src, dst)
    }
    fn readlink(&self, path: &Path) -> Result<PathBuf, VfsError> {
        self.inner.readlink(path)
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, VfsError> {
        self.inner.canonicalize(path)
    }
    fn copy(&self, src: &Path, dst: &Path) -> Result<(), VfsError> {
        self.inner.copy(src, dst)
    }
    fn rename(&self, src: &Path, dst: &Path) -> Result<(), VfsError> {
        self.inner.rename(src, dst)
    }
    fn glob(&self, pattern: &str, cwd: &Path) -> Result<Vec<PathBuf>, VfsError> {
        self.inner.glob(pattern, cwd)
    }
    fn deep_clone(&self) -> Arc<dyn VirtualFs> {
        Arc::new(Self {
            inner: self.inner.deep_clone(),
            factor: self.factor,
        })
    }
}

fn run_with_inflated_sizes(factor: u64, script: &str) -> ExecResult {
    let inner = Arc::new(InMemoryFs::new());
    inner.mkdir_p(Path::new("/big")).unwrap();
    inner.write_file(Path::new("/big/f.txt"), b"abc").unwrap();
    let fs = Arc::new(InflatedSizeFs { inner, factor });
    RustBashBuilder::new()
        .fs(fs)
        .build()
        .unwrap()
        .exec(script)
        .unwrap()
}

#[test]
fn du_human_readable_megabytes_and_gigabytes() {
    // 3 bytes * 2^20 = 3 MiB -> "3.0M"; 3 bytes * 2^30 = 3 GiB -> "3.0G".
    let r = run_with_inflated_sizes(1 << 20, "du -h /big");
    assert_eq!(r.stdout, "3.0M\t/big\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);

    let r = run_with_inflated_sizes(1 << 30, "du -h /big");
    assert_eq!(r.stdout, "3.0G\t/big\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

// ── split ────────────────────────────────────────────────────────────

#[test]
fn split_by_bytes() {
    let r = run_with_files(
        &[("/f", "1234567")],
        "cd / && split -b 3 /f && cat xaa xab xac",
    );
    assert_eq!(r.stdout, "1234567");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn split_with_suffix_length_and_custom_prefix() {
    let r = run_with_files(
        &[("/f", "a\nb\nc\n")],
        "cd / && split -a 3 -l 1 /f part_ && cat part_aaa part_aab part_aac",
    );
    assert_eq!(r.stdout, "a\nb\nc\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn split_attached_option_forms() {
    // -l2, -b2, -a1 attached to their flags.
    let r = run_with_files(
        &[("/f", "a\nb\nc\nd\n")],
        "cd / && split -l2 /f && cat xaa xab",
    );
    assert_eq!(r.stdout, "a\nb\nc\nd\n");
    assert_eq!(r.exit_code, 0);

    let r = run_with_files(&[("/f", "abcd")], "cd / && split -b2 /f && cat xaa xab");
    assert_eq!(r.stdout, "abcd");
    assert_eq!(r.exit_code, 0);

    let r = run_with_files(&[("/f", "a\nb\n")], "cd / && split -a1 -l1 /f && cat xa xb");
    assert_eq!(r.stdout, "a\nb\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn split_double_dash_is_silently_ignored() {
    // "--" matches the starts_with('-') arm (a no-op for it), so the file
    // operand still lands in input_file.
    let r = run_with_files(&[("/f", "a\nb\n")], "cd / && split -- /f && cat xaa");
    assert_eq!(r.stdout, "a\nb\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn split_missing_input_file_fails() {
    let r = run("split /nonexistent");
    assert_eq!(r.stdout, "");
    assert_eq!(
        r.stderr,
        "split: /nonexistent: No such file or directory: /nonexistent\n"
    );
    assert_eq!(r.exit_code, 1);
}

#[test]
fn split_rejects_zero_byte_and_line_counts() {
    let r = run_with_files(&[("/f", "abc")], "split -b 0 /f");
    assert_eq!(r.stderr, "split: invalid number of bytes: 0\n");
    assert_eq!(r.exit_code, 1);

    let r = run_with_files(&[("/f", "a\nb\n")], "split -l 0 /f");
    assert_eq!(r.stderr, "split: invalid number of lines: 0\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn split_exhausts_single_letter_suffixes() {
    // 27 one-line chunks with -a 1: suffixes xa..xz cover 26, the 27th fails.
    let r = run("seq 1 27 > /f && cd / && split -a 1 -l 1 /f");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "split: output file suffixes exhausted\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn split_write_error_for_unwritable_prefix() {
    let r = run_with_files(&[("/f", "a\nb\n")], "split -l 1 /f /nodir/x");
    assert_eq!(r.stdout, "");
    assert_eq!(
        r.stderr,
        "split: /nodir/xaa: No such file or directory: /nodir\n\
         split: /nodir/xab: No such file or directory: /nodir\n"
    );
    assert_eq!(r.exit_code, 1);
}
