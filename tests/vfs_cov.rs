//! Coverage tests for the VFS layer (`src/vfs/`): edge cases in InMemoryFs,
//! MountableFs, OverlayFs, and the shared helpers in `vfs/mod.rs` that the
//! main test suites do not reach.
//!
//! Tests drive the `VirtualFs` trait implementations directly, or go through
//! `rust_bash::Interpreter` where shell state (shopt glob options) is needed.
//! `std::fs` is used only to build on-disk fixtures for OverlayFs' lower
//! layer (the established pattern from `tests/filesystem_backends.rs`); all
//! operations under test go through the VFS.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rust_bash::platform::SystemTime;
use rust_bash::{DirEntry, InMemoryFs, Metadata, MountableFs, NodeType, VfsError, VirtualFs};

// ── Helpers ────────────────────────────────────────────────────────────────

fn p(s: &str) -> &Path {
    Path::new(s)
}

/// An InMemoryFs seeded with files (parent dirs created as needed).
fn mem_fs(files: &[(&str, &[u8])]) -> Arc<InMemoryFs> {
    let fs = InMemoryFs::new();
    for (path, content) in files {
        let path = p(path);
        if let Some(parent) = path.parent()
            && parent != p("/")
        {
            fs.mkdir_p(parent).unwrap();
        }
        fs.write_file(path, content).unwrap();
    }
    Arc::new(fs)
}

/// A backend where nothing exists and every operation fails. Used to reach
/// MountableFs' synthetic fallbacks that built-in backends never trigger
/// (they always report their mount root `/` as existing).
#[derive(Debug)]
struct EmptyBackend;

impl VirtualFs for EmptyBackend {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        Err(VfsError::NotFound(path.to_path_buf()))
    }
    fn write_file(&self, path: &Path, _: &[u8]) -> Result<(), VfsError> {
        Err(VfsError::NotFound(path.to_path_buf()))
    }
    fn append_file(&self, path: &Path, _: &[u8]) -> Result<(), VfsError> {
        Err(VfsError::NotFound(path.to_path_buf()))
    }
    fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        Err(VfsError::NotFound(path.to_path_buf()))
    }
    fn mkdir(&self, path: &Path) -> Result<(), VfsError> {
        Err(VfsError::NotFound(path.to_path_buf()))
    }
    fn mkdir_p(&self, path: &Path) -> Result<(), VfsError> {
        Err(VfsError::NotFound(path.to_path_buf()))
    }
    fn readdir(&self, path: &Path) -> Result<Vec<DirEntry>, VfsError> {
        Err(VfsError::NotFound(path.to_path_buf()))
    }
    fn remove_dir(&self, path: &Path) -> Result<(), VfsError> {
        Err(VfsError::NotFound(path.to_path_buf()))
    }
    fn remove_dir_all(&self, path: &Path) -> Result<(), VfsError> {
        Err(VfsError::NotFound(path.to_path_buf()))
    }
    fn exists(&self, _: &Path) -> bool {
        false
    }
    fn stat(&self, path: &Path) -> Result<Metadata, VfsError> {
        Err(VfsError::NotFound(path.to_path_buf()))
    }
    fn lstat(&self, path: &Path) -> Result<Metadata, VfsError> {
        Err(VfsError::NotFound(path.to_path_buf()))
    }
    fn chmod(&self, path: &Path, _: u32) -> Result<(), VfsError> {
        Err(VfsError::NotFound(path.to_path_buf()))
    }
    fn utimes(&self, path: &Path, _: SystemTime) -> Result<(), VfsError> {
        Err(VfsError::NotFound(path.to_path_buf()))
    }
    fn symlink(&self, _: &Path, link: &Path) -> Result<(), VfsError> {
        Err(VfsError::NotFound(link.to_path_buf()))
    }
    fn hardlink(&self, src: &Path, _: &Path) -> Result<(), VfsError> {
        Err(VfsError::NotFound(src.to_path_buf()))
    }
    fn readlink(&self, path: &Path) -> Result<PathBuf, VfsError> {
        Err(VfsError::NotFound(path.to_path_buf()))
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, VfsError> {
        Err(VfsError::NotFound(path.to_path_buf()))
    }
    fn copy(&self, src: &Path, _: &Path) -> Result<(), VfsError> {
        Err(VfsError::NotFound(src.to_path_buf()))
    }
    fn rename(&self, src: &Path, _: &Path) -> Result<(), VfsError> {
        Err(VfsError::NotFound(src.to_path_buf()))
    }
    fn glob(&self, _: &str, _: &Path) -> Result<Vec<PathBuf>, VfsError> {
        Ok(Vec::new())
    }
    fn deep_clone(&self) -> Arc<dyn VirtualFs> {
        Arc::new(EmptyBackend)
    }
}

// ── vfs/mod.rs: shared path helpers ────────────────────────────────────────

#[test]
fn write_through_dangling_relative_symlink_creates_target() {
    let fs = InMemoryFs::new();
    fs.mkdir_p(p("/dir")).unwrap();
    // Dangling symlink with a relative target: POSIX open(O_CREAT) semantics
    // resolve the target against the link's parent directory.
    fs.symlink(p("rel_target.txt"), p("/dir/link")).unwrap();

    fs.write_file(p("/dir/link"), b"payload").unwrap();

    assert_eq!(fs.read_file(p("/dir/rel_target.txt")).unwrap(), b"payload");
    // The link itself is preserved, not replaced.
    assert_eq!(
        fs.lstat(p("/dir/link")).unwrap().node_type,
        NodeType::Symlink
    );
}

#[test]
fn write_through_chained_dangling_symlinks_creates_final_target() {
    let fs = InMemoryFs::new();
    fs.mkdir_p(p("/dir")).unwrap();
    fs.symlink(p("link2"), p("/dir/link1")).unwrap();
    fs.symlink(p("real.txt"), p("/dir/link2")).unwrap();

    fs.write_file(p("/dir/link1"), b"deep").unwrap();

    assert_eq!(fs.read_file(p("/dir/real.txt")).unwrap(), b"deep");
    assert_eq!(
        fs.lstat(p("/dir/link1")).unwrap().node_type,
        NodeType::Symlink
    );
    assert_eq!(
        fs.lstat(p("/dir/link2")).unwrap().node_type,
        NodeType::Symlink
    );
}

/// A non-UTF8 symlink target cannot be resolved to a string path; the write
/// through it must fail with InvalidPath rather than panic or lossy-convert.
#[cfg(unix)]
#[test]
fn write_through_dangling_symlink_with_non_utf8_target_errors() {
    use std::os::unix::ffi::OsStrExt;

    let fs = InMemoryFs::new();
    let bad = std::ffi::OsStr::from_bytes(&[0xff, b'b', b'a', b'd']);
    fs.symlink(Path::new(bad), p("/link")).unwrap();

    let r = fs.write_file(p("/link"), b"x");
    assert!(
        matches!(r, Err(VfsError::InvalidPath(_))),
        "expected InvalidPath, got {r:?}"
    );
}

/// Windows counterpart: an unpaired-surrogate (non-UTF8) target.
#[cfg(windows)]
#[test]
fn write_through_dangling_symlink_with_non_utf8_target_errors() {
    use std::os::windows::ffi::OsStringExt;

    let fs = InMemoryFs::new();
    let bad = std::ffi::OsString::from_wide(&[0xD800, 0x0061]);
    fs.symlink(Path::new(&bad), p("/link")).unwrap();

    let r = fs.write_file(p("/link"), b"x");
    assert!(
        matches!(r, Err(VfsError::InvalidPath(_))),
        "expected InvalidPath, got {r:?}"
    );
}

// ── InMemoryFs ─────────────────────────────────────────────────────────────

#[test]
fn memory_default_impl_creates_empty_fs() {
    let fs = InMemoryFs::default();
    assert!(fs.exists(p("/")));
    assert!(fs.readdir(p("/")).unwrap().is_empty());
}

#[test]
fn memory_ops_on_root_path_error() {
    let fs = mem_fs(&[]);
    assert!(matches!(
        fs.remove_file(p("/")),
        Err(VfsError::InvalidPath(_))
    ));
    assert!(matches!(fs.mkdir(p("/")), Err(VfsError::InvalidPath(_))));
    assert!(matches!(
        fs.remove_dir(p("/")),
        Err(VfsError::InvalidPath(_))
    ));
    assert!(matches!(
        fs.remove_dir_all(p("/")),
        Err(VfsError::InvalidPath(_))
    ));
    assert!(matches!(
        fs.symlink(p("/t"), p("/")),
        Err(VfsError::InvalidPath(_))
    ));
}

#[test]
fn memory_canonicalize_through_file_is_not_a_directory() {
    let fs = mem_fs(&[("/f", b"x")]);
    let r = fs.canonicalize(p("/f/child"));
    assert!(
        matches!(r, Err(VfsError::NotADirectory(_))),
        "expected NotADirectory, got {r:?}"
    );
}

#[test]
fn memory_canonicalize_symlink_loop_errors() {
    let fs = mem_fs(&[]);
    fs.symlink(p("/b"), p("/a")).unwrap();
    fs.symlink(p("/a"), p("/b")).unwrap();
    let r = fs.canonicalize(p("/a"));
    assert!(
        matches!(r, Err(VfsError::SymlinkLoop(_))),
        "expected SymlinkLoop, got {r:?}"
    );
}

#[test]
fn memory_append_through_symlink_loop_errors() {
    let fs = mem_fs(&[]);
    fs.symlink(p("/b"), p("/a")).unwrap();
    fs.symlink(p("/a"), p("/b")).unwrap();
    let r = fs.append_file(p("/a"), b"x");
    assert!(
        matches!(r, Err(VfsError::SymlinkLoop(_))),
        "expected SymlinkLoop, got {r:?}"
    );
}

#[test]
fn memory_glob_through_symlink_loop_returns_no_match() {
    let fs = mem_fs(&[]);
    fs.symlink(p("/b"), p("/a")).unwrap();
    fs.symlink(p("/a"), p("/b")).unwrap();
    // The loop is detected internally and treated as "no entries" for glob.
    assert!(fs.glob("/a/*", p("/")).unwrap().is_empty());
}

#[test]
fn memory_append_to_directory_errors() {
    let fs = mem_fs(&[]);
    fs.mkdir(p("/d")).unwrap();
    let r = fs.append_file(p("/d"), b"x");
    assert!(
        matches!(r, Err(VfsError::IsADirectory(_))),
        "expected IsADirectory, got {r:?}"
    );
}

#[test]
fn memory_ops_with_file_as_parent_error() {
    let fs = mem_fs(&[("/f", b"x")]);
    assert!(matches!(
        fs.remove_file(p("/f/x")),
        Err(VfsError::NotADirectory(_))
    ));
    assert!(matches!(
        fs.mkdir(p("/f/x")),
        Err(VfsError::NotADirectory(_))
    ));
    assert!(matches!(
        fs.remove_dir(p("/f/x")),
        Err(VfsError::NotADirectory(_))
    ));
    assert!(matches!(
        fs.remove_dir_all(p("/f/x")),
        Err(VfsError::NotADirectory(_))
    ));
    assert!(matches!(
        fs.symlink(p("/t"), p("/f/x")),
        Err(VfsError::NotADirectory(_))
    ));
    assert!(matches!(
        fs.hardlink(p("/f"), p("/f/x")),
        Err(VfsError::NotADirectory(_))
    ));
}

#[test]
fn memory_mkdir_p_through_symlink_component_errors() {
    let fs = mem_fs(&[]);
    fs.mkdir(p("/real")).unwrap();
    fs.symlink(p("/real"), p("/s")).unwrap();
    // mkdir_p deliberately does not follow symlinks in created path components.
    let r = fs.mkdir_p(p("/s/x"));
    assert!(
        matches!(r, Err(VfsError::NotADirectory(_))),
        "expected NotADirectory, got {r:?}"
    );
}

#[test]
fn memory_remove_dir_nonexistent_errors() {
    let fs = mem_fs(&[]);
    let r = fs.remove_dir(p("/nope"));
    assert!(
        matches!(r, Err(VfsError::NotFound(_))),
        "expected NotFound, got {r:?}"
    );
}

#[test]
fn memory_utimes_on_directory() {
    let fs = mem_fs(&[]);
    fs.mkdir(p("/d")).unwrap();
    let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    fs.utimes(p("/d"), t).unwrap();
    assert_eq!(fs.stat(p("/d")).unwrap().mtime, t);
}

#[test]
fn memory_rename_root_errors() {
    let fs = mem_fs(&[("/f", b"x")]);
    assert!(matches!(
        fs.rename(p("/"), p("/x")),
        Err(VfsError::InvalidPath(_))
    ));
    assert!(matches!(
        fs.rename(p("/f"), p("/")),
        Err(VfsError::InvalidPath(_))
    ));
    // Both root-rename attempts are no-ops.
    assert!(fs.exists(p("/f")));
}

#[test]
fn memory_rename_nested_file() {
    let fs = mem_fs(&[("/a/b/c.txt", b"nested")]);
    fs.rename(p("/a/b/c.txt"), p("/a/b/d.txt")).unwrap();
    assert_eq!(fs.read_file(p("/a/b/d.txt")).unwrap(), b"nested");
    assert!(!fs.exists(p("/a/b/c.txt")));
}

#[test]
fn memory_rename_error_paths() {
    let fs = mem_fs(&[("/f", b"x"), ("/g", b"y")]);
    // Missing intermediate component on the source side.
    assert!(matches!(
        fs.rename(p("/missing/x"), p("/y")),
        Err(VfsError::NotFound(_))
    ));
    // File as intermediate component on the source side.
    assert!(matches!(
        fs.rename(p("/f/sub/x"), p("/y")),
        Err(VfsError::NotADirectory(_))
    ));
    // File as immediate source parent.
    assert!(matches!(
        fs.rename(p("/f/x"), p("/y")),
        Err(VfsError::NotADirectory(_))
    ));
    // Nothing was moved by the failed renames.
    assert!(fs.exists(p("/g")));
    assert!(fs.exists(p("/f")));
}

#[test]
fn memory_rename_dst_parent_errors() {
    // SUSPECTED DIVERGENCE (pinned, not fixed): rename extracts the source
    // node BEFORE validating the destination, so when destination navigation
    // fails the source is lost entirely. POSIX rename never unlinks the
    // source on failure. Each sub-case uses a fresh source file.
    let fs = mem_fs(&[("/f", b"x"), ("/g1", b"1"), ("/g2", b"2")]);
    // File as intermediate component on the destination side.
    assert!(matches!(
        fs.rename(p("/g1"), p("/f/sub/y")),
        Err(VfsError::NotADirectory(_))
    ));
    // File as immediate destination parent.
    assert!(matches!(
        fs.rename(p("/g2"), p("/f/y")),
        Err(VfsError::NotADirectory(_))
    ));
    // Pinned: the sources are gone even though the renames failed.
    assert!(!fs.exists(p("/g1")));
    assert!(!fs.exists(p("/g2")));
    assert!(fs.exists(p("/f")));
}

#[test]
fn memory_glob_root_pattern_returns_root() {
    let fs = mem_fs(&[("/f", b"x")]);
    assert_eq!(fs.glob("/", p("/")).unwrap(), vec![PathBuf::from("/")]);
}

// ── Glob shopt options through the interpreter (InMemoryFs::glob_with_opts) ──

fn shell_with_files(files: &[(&str, &[u8])]) -> rust_bash::RustBash {
    let map: std::collections::HashMap<String, Vec<u8>> = files
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_vec()))
        .collect();
    rust_bash::RustBashBuilder::new()
        .files(map)
        .cwd("/w")
        .build()
        .unwrap()
}

#[test]
fn glob_nocaseglob_matches_case_insensitively() {
    let mut sh = shell_with_files(&[("/w/a.txt", b"x")]);
    // extglob defaults ON in the shell; unset it to hit the nocase-only arm.
    let r = sh
        .exec("shopt -u extglob && shopt -s nocaseglob && echo *.TXT")
        .unwrap();
    assert_eq!(r.stdout, "a.txt\n");
}

#[test]
fn glob_extglob_with_nocaseglob_matches_case_insensitively() {
    let mut sh = shell_with_files(&[("/w/a.txt", b"x"), ("/w/b.md", b"x")]);
    let r = sh
        .exec("shopt -s extglob nocaseglob && echo @(A.TXT)")
        .unwrap();
    assert_eq!(r.stdout, "a.txt\n");
}

#[test]
fn glob_globskipdots_off_includes_dot_entries() {
    let mut sh = shell_with_files(&[("/w/.hidden", b"x"), ("/w/visible", b"x")]);
    // extglob defaults ON; unset it to hit the plain-matching arm.
    let r = sh
        .exec("shopt -u extglob && shopt -u globskipdots && echo .*")
        .unwrap();
    assert_eq!(r.stdout, ". .. .hidden\n");
}

#[test]
fn glob_globskipdots_off_with_nocaseglob() {
    let mut sh = shell_with_files(&[("/w/.HIDDEN", b"x"), ("/w/visible", b"x")]);
    // Case-insensitive matching must find the uppercase dot-file (and the
    // synthetic-dot matcher runs on the nocase-only arm).
    let r = sh
        .exec("shopt -u extglob && shopt -s nocaseglob && shopt -u globskipdots && echo .hidde?")
        .unwrap();
    assert_eq!(r.stdout, ".HIDDEN\n");
}

#[test]
fn glob_globskipdots_off_with_extglob_and_nocaseglob() {
    let mut sh = shell_with_files(&[("/w/.hidden", b"x"), ("/w/visible", b"x")]);
    let r = sh
        .exec("shopt -u globskipdots && shopt -s extglob nocaseglob && echo .*")
        .unwrap();
    assert_eq!(r.stdout, ". .. .hidden\n");
}

// ── MountableFs ────────────────────────────────────────────────────────────

#[test]
fn mountable_default_impl() {
    let mfs = MountableFs::default().mount("/", mem_fs(&[("/f", b"x")]));
    assert_eq!(mfs.read_file(p("/f")).unwrap(), b"x");
}

#[test]
fn mountable_mkdir_and_remove_dir_all() {
    let mfs = MountableFs::new().mount("/", mem_fs(&[]));
    mfs.mkdir(p("/newdir")).unwrap();
    assert!(mfs.exists(p("/newdir")));
    mfs.mkdir_p(p("/newdir/sub")).unwrap();
    mfs.remove_dir_all(p("/newdir")).unwrap();
    assert!(!mfs.exists(p("/newdir")));
}

#[test]
fn mountable_utimes_through_mount() {
    let mfs = MountableFs::new().mount("/", mem_fs(&[("/f", b"x")]));
    let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
    mfs.utimes(p("/f"), t).unwrap();
    assert_eq!(mfs.stat(p("/f")).unwrap().mtime, t);
}

#[test]
fn mountable_lstat_through_mount() {
    let fs = mem_fs(&[("/f", b"x")]);
    let mfs = MountableFs::new().mount("/", fs);
    let meta = mfs.lstat(p("/f")).unwrap();
    assert_eq!(meta.node_type, NodeType::File);
}

#[test]
fn mountable_lstat_nonexistent_errors() {
    let mfs = MountableFs::new().mount("/", mem_fs(&[]));
    let r = mfs.lstat(p("/missing"));
    assert!(
        matches!(r, Err(VfsError::NotFound(_))),
        "expected NotFound, got {r:?}"
    );
}

#[test]
fn mountable_stat_lstat_synthetic_ancestors() {
    // No root mount — only a deep mount. Ancestors are synthetic directories.
    let mfs = MountableFs::new().mount("/a/b/c", mem_fs(&[("/f", b"x")]));

    for path in ["/", "/a", "/a/b"] {
        let meta = mfs.stat(p(path)).unwrap();
        assert_eq!(meta.node_type, NodeType::Directory, "stat {path}");
        let meta = mfs.lstat(p(path)).unwrap();
        assert_eq!(meta.node_type, NodeType::Directory, "lstat {path}");
        assert!(mfs.exists(p(path)), "exists {path}");
    }
    // The mounted backend's own metadata still wins at and below the mount.
    let meta = mfs.stat(p("/a/b/c/f")).unwrap();
    assert_eq!(meta.node_type, NodeType::File);
}

#[test]
fn mountable_exists_at_mount_point_with_empty_backend() {
    // A backend that reports even its root as nonexistent: the mount point
    // itself must still exist as a synthetic directory.
    let mfs = MountableFs::new().mount("/m", Arc::new(EmptyBackend));
    assert!(mfs.exists(p("/m")));

    let meta = mfs.stat(p("/m")).unwrap();
    assert_eq!(meta.node_type, NodeType::Directory);
    let meta = mfs.lstat(p("/m")).unwrap();
    assert_eq!(meta.node_type, NodeType::Directory);
}

#[test]
fn mountable_symlink_absolute_target_on_other_mount_kept() {
    let project_fs = mem_fs(&[]);
    let mfs = MountableFs::new()
        .mount("/", mem_fs(&[]))
        .mount("/project", project_fs.clone());

    // Target lives on a different mount than the link: it cannot be remapped
    // into the link's backend namespace, so it is stored unchanged.
    mfs.symlink(p("/etc/hostname"), p("/project/link")).unwrap();
    assert_eq!(
        project_fs.readlink(p("/link")).unwrap(),
        PathBuf::from("/etc/hostname")
    );
}

#[test]
fn mountable_symlink_absolute_target_beyond_all_mounts_kept() {
    let project_fs = mem_fs(&[]);
    let mfs = MountableFs::new().mount("/project", project_fs.clone());

    // No mount covers the target at all — stored unchanged.
    mfs.symlink(p("/elsewhere/t"), p("/project/link")).unwrap();
    assert_eq!(
        project_fs.readlink(p("/link")).unwrap(),
        PathBuf::from("/elsewhere/t")
    );
}

#[test]
fn mountable_symlink_relative_target_kept() {
    let project_fs = mem_fs(&[]);
    let mfs = MountableFs::new().mount("/project", project_fs.clone());

    mfs.symlink(p("sibling.txt"), p("/project/link")).unwrap();
    assert_eq!(
        project_fs.readlink(p("/link")).unwrap(),
        PathBuf::from("sibling.txt")
    );
}

#[test]
fn mountable_readlink_absolute_target_at_root_mount_not_remapped() {
    let mfs = MountableFs::new()
        .mount("/", mem_fs(&[]))
        .mount("/project", mem_fs(&[]));

    mfs.symlink(p("/etc/hostname"), p("/rootlink")).unwrap();
    // Link lives on the root mount: the absolute target needs no remapping.
    assert_eq!(
        mfs.readlink(p("/rootlink")).unwrap(),
        PathBuf::from("/etc/hostname")
    );
}

#[test]
fn mountable_readlink_target_of_slash_maps_back_to_mount_point() {
    // Only /project is mounted. A link whose absolute target is "/" (the
    // backend's own root) reads back as the mount point in the global view.
    let mfs = MountableFs::new().mount("/project", mem_fs(&[]));
    mfs.symlink(p("/"), p("/project/rootlink")).unwrap();
    assert_eq!(
        mfs.readlink(p("/project/rootlink")).unwrap(),
        PathBuf::from("/project")
    );
}

#[test]
fn mountable_canonicalize_at_mount_point() {
    let mfs = MountableFs::new().mount("/data", mem_fs(&[("/f.txt", b"x")]));
    assert_eq!(
        mfs.canonicalize(p("/data")).unwrap(),
        PathBuf::from("/data")
    );
}

#[test]
fn mountable_copy_from_mount_point_errors() {
    let mfs = MountableFs::new()
        .mount("/a", mem_fs(&[("/f", b"x")]))
        .mount("/b", mem_fs(&[]));
    // The source path IS the mount point: resolves to the backend's root,
    // which is a directory and cannot be read as a file.
    let r = mfs.copy(p("/a"), p("/b/copy"));
    assert!(
        matches!(r, Err(VfsError::IsADirectory(_))),
        "expected IsADirectory, got {r:?}"
    );
    assert!(!mfs.exists(p("/b/copy")));
}

#[test]
fn mountable_copy_from_unmounted_path_errors() {
    let mfs = MountableFs::new()
        .mount("/a", mem_fs(&[]))
        .mount("/b", mem_fs(&[]));
    let r = mfs.copy(p("/outside/x"), p("/b/y"));
    assert!(
        matches!(r, Err(VfsError::NotFound(_))),
        "expected NotFound, got {r:?}"
    );
}

#[test]
fn mountable_glob_root_pattern_returns_root() {
    let mfs = MountableFs::new().mount("/", mem_fs(&[("/f", b"x")]));
    assert_eq!(mfs.glob("/", p("/")).unwrap(), vec![PathBuf::from("/")]);
}

#[test]
fn mountable_glob_doublestar_recurses_and_skips_hidden() {
    let root = mem_fs(&[
        ("/x/1.txt", b"1"),
        ("/x/y/2.txt", b"2"),
        ("/.hidden.txt", b"h"),
    ]);
    let mfs = MountableFs::new().mount("/", root);
    let matches = mfs.glob("/**/*.txt", p("/")).unwrap();
    let strs: Vec<String> = matches
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    assert!(strs.contains(&"/x/1.txt".to_string()), "got: {strs:?}");
    assert!(strs.contains(&"/x/y/2.txt".to_string()), "got: {strs:?}");
    assert!(
        !strs.iter().any(|s| s.contains("hidden")),
        "hidden file must be skipped: {strs:?}"
    );
}

#[test]
fn mountable_glob_skips_hidden_in_plain_pattern() {
    let root = mem_fs(&[("/visible.txt", b"v"), ("/.hidden.txt", b"h")]);
    let mfs = MountableFs::new().mount("/", root);
    let matches = mfs.glob("/*.txt", p("/")).unwrap();
    assert_eq!(matches, vec![PathBuf::from("/visible.txt")]);
}

#[test]
fn mountable_glob_through_symlink_dir() {
    let root = mem_fs(&[("/dir/f.txt", b"x")]);
    root.symlink(p("/dir"), p("/s")).unwrap();
    let mfs = MountableFs::new().mount("/", root);
    let matches = mfs.glob("/s/*.txt", p("/")).unwrap();
    assert_eq!(matches, vec![PathBuf::from("/s/f.txt")]);
}

#[test]
fn mountable_glob_without_root_mount_lists_mount_points() {
    let mfs = MountableFs::new().mount("/project", mem_fs(&[("/f.txt", b"x")]));
    // "/" has no owning mount; the synthetic "project" entry still matches.
    let matches = mfs.glob("/*", p("/")).unwrap();
    assert_eq!(matches, vec![PathBuf::from("/project")]);
    // Globbing inside the mount works.
    let matches = mfs.glob("/project/*.txt", p("/")).unwrap();
    assert_eq!(matches, vec![PathBuf::from("/project/f.txt")]);
}

#[test]
fn mountable_shell_glob_uses_default_glob_with_opts() {
    // MountableFs does not override `glob_with_opts`; the trait default
    // delegates to `glob()`. Drive it through the shell's expansion layer.
    let mfs = MountableFs::new().mount("/", mem_fs(&[("/g1.txt", b"1"), ("/g2.txt", b"2")]));
    let mut shell = rust_bash::RustBashBuilder::new()
        .fs(Arc::new(mfs))
        .cwd("/")
        .build()
        .unwrap();
    let r = shell.exec("echo /*.txt").unwrap();
    assert_eq!(r.stdout, "/g1.txt /g2.txt\n");
}

// ── OverlayFs (requires native-fs) ─────────────────────────────────────────

#[cfg(feature = "native-fs")]
mod overlay {
    use super::*;
    use rust_bash::OverlayFs;
    use tempfile::TempDir;

    /// Create a symlink on disk; returns false when the OS denies symlink
    /// creation (e.g. Windows without Developer Mode), in which case the
    /// caller skips.
    fn try_disk_symlink(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            // Resolve a relative target against the link's directory to pick
            // the right symlink kind (file vs dir).
            let abs_target = link.parent().unwrap_or(link).join(target);
            let r = if abs_target.is_dir() {
                std::os::windows::fs::symlink_dir(target, link)
            } else {
                std::os::windows::fs::symlink_file(target, link)
            };
            r.is_ok()
        }
    }

    /// Lower layer: /top.txt, /sub/leaf.rs, /sub/nested/deep.rs, /.h.rs
    fn lower_tree() -> TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        std::fs::write(base.join("top.txt"), b"top").unwrap();
        std::fs::create_dir_all(base.join("sub/nested")).unwrap();
        std::fs::write(base.join("sub/leaf.rs"), b"leaf").unwrap();
        std::fs::write(base.join("sub/nested/deep.rs"), b"deep").unwrap();
        std::fs::write(base.join(".h.rs"), b"hidden").unwrap();
        tmp
    }

    #[test]
    fn append_copies_up_nested_lower_file() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        o.append_file(p("/sub/nested/deep.rs"), b"+").unwrap();
        assert_eq!(o.read_file(p("/sub/nested/deep.rs")).unwrap(), b"deep+");
        // Disk untouched.
        assert_eq!(
            std::fs::read(tmp.path().join("sub/nested/deep.rs")).unwrap(),
            b"deep"
        );
    }

    #[test]
    fn write_in_fresh_dir_uses_default_dir_mode() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        // /fresh exists nowhere: parent dirs are created with the fallback mode.
        o.write_file(p("/fresh/dir/f.txt"), b"new").unwrap();
        assert_eq!(o.read_file(p("/fresh/dir/f.txt")).unwrap(), b"new");
        let d = o.diff();
        assert!(
            d.writes
                .iter()
                .any(|w| w.path == p("/fresh") && w.node_type == NodeType::Directory),
            "expected /fresh dir in diff: {:?}",
            d.writes
        );
    }

    #[test]
    fn remove_dir_all_nested_yields_topmost_deletion_only() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        o.remove_dir_all(p("/sub")).unwrap();
        assert!(!o.exists(p("/sub/nested/deep.rs")));
        assert_eq!(o.diff().deletions, vec![PathBuf::from("/sub")]);
        // Disk untouched.
        assert!(tmp.path().join("sub/nested/deep.rs").exists());
    }

    #[test]
    fn remove_file_twice_reports_not_found() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        o.remove_file(p("/top.txt")).unwrap();
        let r = o.remove_file(p("/top.txt"));
        assert!(
            matches!(r, Err(VfsError::NotFound(_))),
            "expected NotFound, got {r:?}"
        );
    }

    #[test]
    fn remove_file_nonexistent_reports_not_found() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        let r = o.remove_file(p("/nope"));
        assert!(
            matches!(r, Err(VfsError::NotFound(_))),
            "expected NotFound, got {r:?}"
        );
    }

    #[test]
    fn remove_file_on_directory_reports_is_a_directory() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        // Lower-layer directory.
        let r = o.remove_file(p("/sub"));
        assert!(
            matches!(r, Err(VfsError::IsADirectory(_))),
            "lower dir: expected IsADirectory, got {r:?}"
        );
        // Upper-layer directory.
        o.mkdir(p("/upperdir")).unwrap();
        let r = o.remove_file(p("/upperdir"));
        assert!(
            matches!(r, Err(VfsError::IsADirectory(_))),
            "upper dir: expected IsADirectory, got {r:?}"
        );
    }

    #[test]
    fn mkdir_root_after_rmdir_root_reports_already_exists() {
        let tmp = tempfile::tempdir().unwrap(); // empty lower
        let o = OverlayFs::new(tmp.path()).unwrap();

        // SUSPECTED POSIX DIVERGENCE (pinned, not fixed): rmdir("/") on a
        // real fs fails with EBUSY; here remove_dir("/") on an empty merged
        // root succeeds and whiteouts the root.
        o.remove_dir(p("/")).unwrap();
        assert!(!o.exists(p("/")));

        // The whiteout is cleared, but the upper layer always has a root
        // directory, so mkdir("/") reports AlreadyExists.
        let r = o.mkdir(p("/"));
        assert!(
            matches!(r, Err(VfsError::AlreadyExists(_))),
            "expected AlreadyExists, got {r:?}"
        );
    }

    #[test]
    fn mkdir_p_through_file_component_errors() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        // Through a lower-layer file.
        let r = o.mkdir_p(p("/top.txt/x"));
        assert!(
            matches!(r, Err(VfsError::NotADirectory(_))),
            "lower file: expected NotADirectory, got {r:?}"
        );
        // Through an upper-layer file.
        o.write_file(p("/uf"), b"x").unwrap();
        let r = o.mkdir_p(p("/uf/x"));
        assert!(
            matches!(r, Err(VfsError::NotADirectory(_))),
            "upper file: expected NotADirectory, got {r:?}"
        );
    }

    #[test]
    fn readdir_error_paths() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        // Whiteout-ed directory.
        o.remove_dir_all(p("/sub")).unwrap();
        assert!(matches!(o.readdir(p("/sub")), Err(VfsError::NotFound(_))));
        // Nonexistent directory.
        assert!(matches!(o.readdir(p("/nope")), Err(VfsError::NotFound(_))));
        // A file is not a directory.
        assert!(matches!(
            o.readdir(p("/top.txt")),
            Err(VfsError::NotADirectory(_))
        ));
    }

    #[test]
    fn remove_dir_error_paths() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("empty")).unwrap();
        std::fs::write(tmp.path().join("f"), b"x").unwrap();
        let o = OverlayFs::new(tmp.path()).unwrap();
        // On a file.
        assert!(matches!(
            o.remove_dir(p("/f")),
            Err(VfsError::NotADirectory(_))
        ));
        // Once removed, the whiteout makes a second attempt NotFound.
        o.remove_dir(p("/empty")).unwrap();
        assert!(matches!(
            o.remove_dir(p("/empty")),
            Err(VfsError::NotFound(_))
        ));
    }

    #[test]
    fn remove_dir_all_error_paths_and_upper_removal() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        // On a file.
        assert!(matches!(
            o.remove_dir_all(p("/top.txt")),
            Err(VfsError::NotADirectory(_))
        ));
        // Upper-only directory tree is removed for real (no diff deletion).
        o.mkdir_p(p("/x/y")).unwrap();
        o.write_file(p("/x/y/f"), b"x").unwrap();
        o.remove_dir_all(p("/x")).unwrap();
        assert!(!o.exists(p("/x/y/f")));
        assert!(o.diff().deletions.is_empty());
        // A second removal of a lower dir hits the whiteout.
        o.remove_dir_all(p("/sub")).unwrap();
        assert!(matches!(
            o.remove_dir_all(p("/sub")),
            Err(VfsError::NotFound(_))
        ));
    }

    #[test]
    fn exists_edge_cases() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        // Relative paths are invalid for the VFS and report false.
        assert!(!o.exists(p("top.txt")));
        // Dangling upper symlink: the entry exists but its target does not.
        o.symlink(p("/missing"), p("/dangling")).unwrap();
        assert!(!o.exists(p("/dangling")));
        // Valid upper symlink: target existence is checked through the overlay.
        o.symlink(p("/top.txt"), p("/valid")).unwrap();
        assert!(o.exists(p("/valid")));
    }

    #[test]
    fn chmod_utimes_error_and_dir_paths() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("d")).unwrap();
        std::fs::create_dir(tmp.path().join("d2")).unwrap();
        let o = OverlayFs::new(tmp.path()).unwrap();

        assert!(matches!(
            o.chmod(p("/nope"), 0o700),
            Err(VfsError::NotFound(_))
        ));
        assert!(matches!(
            o.utimes(p("/nope"), SystemTime::UNIX_EPOCH),
            Err(VfsError::NotFound(_))
        ));

        // chmod on a lower directory copies the dir up and applies the mode.
        o.chmod(p("/d"), 0o700).unwrap();
        assert_eq!(o.stat(p("/d")).unwrap().mode & 0o777, 0o700);

        // utimes on a lower directory copies the dir up and sets mtime.
        // (Separate dir: /d is already shadowed in the upper layer by the
        // chmod above.)
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000);
        o.utimes(p("/d2"), t).unwrap();
        assert_eq!(o.stat(p("/d2")).unwrap().mtime, t);
    }

    #[test]
    fn symlink_and_hardlink_into_nested_paths() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        // Symlink whose parent only exists in the lower layer: the parent is
        // ensured in the upper layer first.
        o.symlink(p("/top.txt"), p("/sub/nested/l")).unwrap();
        assert_eq!(o.read_file(p("/sub/nested/l")).unwrap(), b"top");
        // Hardlink into a nested destination: copies content up.
        o.hardlink(p("/top.txt"), p("/sub/hl")).unwrap();
        assert_eq!(o.read_file(p("/sub/hl")).unwrap(), b"top");
        // Disk untouched by either.
        assert!(!tmp.path().join("sub/nested/l").exists());
        assert!(!tmp.path().join("sub/hl").exists());
    }

    #[test]
    fn readlink_error_paths() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        // Nonexistent.
        assert!(matches!(o.readlink(p("/nope")), Err(VfsError::NotFound(_))));
        // Whiteout-ed upper symlink.
        o.symlink(p("/top.txt"), p("/l")).unwrap();
        o.remove_file(p("/l")).unwrap();
        assert!(matches!(o.readlink(p("/l")), Err(VfsError::NotFound(_))));
    }

    #[test]
    fn rename_error_paths() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        // Nonexistent source.
        assert!(matches!(
            o.rename(p("/nope"), p("/y")),
            Err(VfsError::NotFound(_))
        ));
        // Whiteout-ed source.
        o.remove_file(p("/top.txt")).unwrap();
        assert!(matches!(
            o.rename(p("/top.txt"), p("/y")),
            Err(VfsError::NotFound(_))
        ));
    }

    #[test]
    fn rename_upper_symlink() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        o.symlink(p("/top.txt"), p("/l")).unwrap();
        o.rename(p("/l"), p("/l2")).unwrap();
        assert_eq!(o.readlink(p("/l2")).unwrap(), PathBuf::from("/top.txt"));
        assert!(matches!(o.readlink(p("/l")), Err(VfsError::NotFound(_))));
        // The symlink still resolves to the lower file's content.
        assert_eq!(o.read_file(p("/l2")).unwrap(), b"top");
    }

    #[test]
    fn rename_symlink_into_new_subdir() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        o.symlink(p("/top.txt"), p("/l")).unwrap();
        o.rename(p("/l"), p("/newsub/l")).unwrap();
        assert_eq!(
            o.readlink(p("/newsub/l")).unwrap(),
            PathBuf::from("/top.txt")
        );
        assert_eq!(o.read_file(p("/newsub/l")).unwrap(), b"top");
    }

    #[test]
    fn rename_dir_into_new_subdir() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        o.rename(p("/sub"), p("/newparent/sub")).unwrap();
        assert_eq!(
            o.read_file(p("/newparent/sub/nested/deep.rs")).unwrap(),
            b"deep"
        );
        assert!(!o.exists(p("/sub/leaf.rs")));
        // Disk untouched.
        assert!(tmp.path().join("sub/leaf.rs").exists());
    }

    #[test]
    fn stat_through_upper_relative_symlink() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        // Relative target resolves against the link's parent directory.
        o.symlink(p("top.txt"), p("/rel")).unwrap();
        let meta = o.stat(p("/rel")).unwrap();
        assert_eq!(meta.node_type, NodeType::File);
        assert_eq!(o.read_file(p("/rel")).unwrap(), b"top");
    }

    #[test]
    fn stat_through_upper_symlink_with_empty_target_resolves_to_parent() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        // An empty relative target resolves to the link's parent ("/") —
        // degenerate, but it must not panic or loop.
        o.symlink(p(""), p("/empty")).unwrap();
        let meta = o.stat(p("/empty")).unwrap();
        assert_eq!(meta.node_type, NodeType::Directory);
    }

    #[test]
    fn glob_root_pattern_returns_root() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        assert_eq!(o.glob("/", p("/")).unwrap(), vec![PathBuf::from("/")]);
    }

    #[test]
    fn glob_doublestar_recurses_both_layers_and_skips_hidden() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        o.write_file(p("/upper.rs"), b"u").unwrap();
        let matches = o.glob("/**/*.rs", p("/")).unwrap();
        let strs: Vec<String> = matches
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        assert!(strs.contains(&"/sub/leaf.rs".to_string()), "got: {strs:?}");
        assert!(
            strs.contains(&"/sub/nested/deep.rs".to_string()),
            "got: {strs:?}"
        );
        assert!(strs.contains(&"/upper.rs".to_string()), "got: {strs:?}");
        assert!(
            !strs.iter().any(|s| s.contains(".h.rs")),
            "hidden file must be skipped: {strs:?}"
        );
    }

    #[test]
    fn glob_through_upper_symlink_dirs() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        // Upper symlink whose target lives in the UPPER layer: glob follows it.
        o.mkdir_p(p("/ud")).unwrap();
        o.write_file(p("/ud/f.rs"), b"u").unwrap();
        o.symlink(p("/ud"), p("/ln")).unwrap();

        // Plain pattern through the symlink.
        let matches = o.glob("/ln/*.rs", p("/")).unwrap();
        assert_eq!(matches, vec![PathBuf::from("/ln/f.rs")]);

        // Recursive ** walk also descends into symlinked dirs.
        let matches = o.glob("/**/*.rs", p("/")).unwrap();
        assert!(
            matches.contains(&PathBuf::from("/ln/f.rs")),
            "got: {matches:?}"
        );
    }

    #[test]
    fn glob_through_upper_symlink_to_lower_dir_finds_nothing() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        // SUSPECTED DIVERGENCE (pinned, not fixed): an upper symlink pointing
        // at a LOWER-layer directory is not traversed by glob — the merged
        // listing delegates to the upper layer alone, where the symlink
        // target does not exist. POSIX/overlayfs would list the contents.
        o.symlink(p("/sub"), p("/ln")).unwrap();
        assert!(o.glob("/ln/*.rs", p("/")).unwrap().is_empty());
    }

    #[test]
    fn glob_skips_hidden_in_plain_pattern() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        let matches = o.glob("/*.rs", p("/")).unwrap();
        assert!(
            matches.is_empty(),
            "hidden .h.rs must not match: {matches:?}"
        );
    }

    #[test]
    fn read_lower_directory_error_maps_by_platform() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        let r = o.read_file(p("/sub"));
        // Reading a directory from disk fails at the OS level; the mapped
        // VfsError variant is platform-specific.
        #[cfg(windows)]
        assert!(
            matches!(r, Err(VfsError::PermissionDenied(_))),
            "windows: expected PermissionDenied, got {r:?}"
        );
        #[cfg(unix)]
        assert!(
            matches!(r, Err(VfsError::IsADirectory(_))),
            "unix: expected IsADirectory, got {r:?}"
        );
    }

    #[test]
    fn stat_beneath_lower_file_reports_not_found() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        // Internally the OS reports ENOTDIR (unix) / ERROR_PATH_NOT_FOUND
        // (windows) while probing the path; the overlay surfaces NotFound.
        let r = o.stat(p("/top.txt/child"));
        assert!(
            matches!(r, Err(VfsError::NotFound(_))),
            "expected NotFound, got {r:?}"
        );
    }

    #[test]
    fn shell_glob_through_overlay_uses_default_glob_with_opts() {
        let tmp = lower_tree();
        let o = OverlayFs::new(tmp.path()).unwrap();
        let mut shell = rust_bash::RustBashBuilder::new()
            .fs(Arc::new(o))
            .cwd("/")
            .build()
            .unwrap();
        let r = shell.exec("echo /*.txt").unwrap();
        assert_eq!(r.stdout, "/top.txt\n");
    }

    #[cfg(windows)]
    #[test]
    fn readonly_lower_file_reports_mode_0555() {
        // Windows has no Unix mode bits; the VFS maps the read-only
        // attribute to 0o555 (MSYS-like semantics).
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("ro.txt");
        std::fs::write(&f, b"x").unwrap();
        let mut perms = std::fs::metadata(&f).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&f, perms).unwrap();

        let o = OverlayFs::new(tmp.path()).unwrap();
        assert_eq!(o.stat(p("/ro.txt")).unwrap().mode, 0o555);
    }

    #[cfg(unix)]
    #[test]
    fn sync_keeps_write_when_disk_file_is_unreadable() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secret.txt");
        std::fs::write(&f, b"disk").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).unwrap();

        let o = OverlayFs::new(tmp.path()).unwrap();
        o.write_file(p("/secret.txt"), b"mem").unwrap();
        // The disk file cannot be read for comparison, so the shadow must be
        // treated as still pending (defensive mismatch) rather than dropped.
        o.sync();
        let d = o.diff();
        assert!(
            d.writes.iter().any(|w| w.path == p("/secret.txt")),
            "unreadable disk file must stay pending: {:?}",
            d.writes
        );

        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[test]
    fn lower_symlink_stat_readdir_and_sync() {
        let tmp = lower_tree();
        if !try_disk_symlink(p("top.txt"), &tmp.path().join("lk")) {
            eprintln!("skipping: OS denied symlink creation");
            return;
        }
        let o = OverlayFs::new(tmp.path()).unwrap();

        // stat follows the lower symlink to its target.
        let meta = o.stat(p("/lk")).unwrap();
        assert_eq!(meta.node_type, NodeType::File);
        assert_eq!(o.read_file(p("/lk")).unwrap(), b"top");

        // readdir reports the lower entry as a symlink.
        let entries = o.readdir(p("/")).unwrap();
        let lk = entries.iter().find(|e| e.name == "lk").unwrap();
        assert_eq!(lk.node_type, NodeType::Symlink);

        // sync(): an upper shadow identical to the disk symlink is dropped,
        // a differing one stays pending.
        o.symlink(p("top.txt"), p("/lk")).unwrap();
        o.sync();
        let d = o.diff();
        assert!(
            !d.writes.iter().any(|w| w.path == p("/lk")),
            "identical symlink shadow should be dropped: {:?}",
            d.writes
        );
        // (the identical shadow was dropped by sync, so the link can be
        // recreated) — a differing target stays pending.
        o.symlink(p("/sub/leaf.rs"), p("/lk")).unwrap();
        o.sync();
        let d = o.diff();
        assert!(
            d.writes.iter().any(|w| w.path == p("/lk")),
            "differing symlink shadow should stay pending: {:?}",
            d.writes
        );
    }
}
