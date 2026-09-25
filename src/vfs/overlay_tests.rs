//! Tests for OverlayFs.

use std::path::{Path, PathBuf};

use crate::platform::SystemTime;

use tempfile::TempDir;

use crate::vfs::{NodeType, OverlayFs, VfsError, VirtualFs};

/// Helper: create a temp directory with some files for use as the lower layer.
fn setup_lower() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path();

    // /src/main.rs
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(base.join("src/main.rs"), b"fn main() {}").unwrap();

    // /README.md
    std::fs::write(base.join("README.md"), b"# Hello").unwrap();

    // /data/config.toml
    std::fs::create_dir_all(base.join("data")).unwrap();
    std::fs::write(base.join("data/config.toml"), b"key = \"value\"").unwrap();

    tmp
}

/// Helper: build an OverlayFs with the lower rooted at virtual "/".
fn make_overlay(lower: &Path) -> OverlayFs {
    OverlayFs::new(lower).unwrap()
}

// -----------------------------------------------------------------------
// 3l.1 Read-through from lower
// -----------------------------------------------------------------------

#[test]
fn read_through_from_lower() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    let content = ov.read_file(Path::new("/src/main.rs")).unwrap();
    assert_eq!(content, b"fn main() {}");

    let content = ov.read_file(Path::new("/README.md")).unwrap();
    assert_eq!(content, b"# Hello");
}

// -----------------------------------------------------------------------
// 3l.2 Write isolation
// -----------------------------------------------------------------------

#[test]
fn write_isolation() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // Write a new file via overlay
    ov.write_file(Path::new("/new_file.txt"), b"overlay data")
        .unwrap();

    // Readable through overlay
    assert_eq!(
        ov.read_file(Path::new("/new_file.txt")).unwrap(),
        b"overlay data"
    );

    // NOT on disk
    assert!(!tmp.path().join("new_file.txt").exists());
}

#[test]
fn overwrite_lower_file_does_not_touch_disk() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.write_file(Path::new("/README.md"), b"overwritten")
        .unwrap();
    assert_eq!(
        ov.read_file(Path::new("/README.md")).unwrap(),
        b"overwritten"
    );

    // Lower file is unchanged
    let on_disk = std::fs::read(tmp.path().join("README.md")).unwrap();
    assert_eq!(on_disk, b"# Hello");
}

// -----------------------------------------------------------------------
// 3l.3 Whiteout
// -----------------------------------------------------------------------

#[test]
fn whiteout_hides_lower_file() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    assert!(ov.exists(Path::new("/README.md")));
    ov.remove_file(Path::new("/README.md")).unwrap();
    assert!(!ov.exists(Path::new("/README.md")));

    // Still on disk
    assert!(tmp.path().join("README.md").exists());
}

// -----------------------------------------------------------------------
// 3l.4 Copy-up on modify
// -----------------------------------------------------------------------

#[test]
fn copy_up_on_append() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.append_file(Path::new("/README.md"), b"\nAppended")
        .unwrap();
    let content = ov.read_file(Path::new("/README.md")).unwrap();
    assert_eq!(content, b"# Hello\nAppended");

    // Lower unchanged
    let on_disk = std::fs::read(tmp.path().join("README.md")).unwrap();
    assert_eq!(on_disk, b"# Hello");
}

// -----------------------------------------------------------------------
// 3l.5 Merged readdir
// -----------------------------------------------------------------------

#[test]
fn merged_readdir() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // Write a new file in the same dir as a lower file
    ov.write_file(Path::new("/data/extra.txt"), b"extra")
        .unwrap();

    let mut entries = ov.readdir(Path::new("/data")).unwrap();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["config.toml", "extra.txt"]);
}

#[test]
fn readdir_excludes_whiteouts() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.remove_file(Path::new("/data/config.toml")).unwrap();
    let entries = ov.readdir(Path::new("/data")).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(!names.contains(&"config.toml"));
}

// -----------------------------------------------------------------------
// 3l.6 Rename across layers
// -----------------------------------------------------------------------

#[test]
fn rename_lower_only_file() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.rename(Path::new("/README.md"), Path::new("/RENAMED.md"))
        .unwrap();

    // New name exists
    assert_eq!(ov.read_file(Path::new("/RENAMED.md")).unwrap(), b"# Hello");

    // Old name gone
    assert!(!ov.exists(Path::new("/README.md")));

    // Lower unchanged
    assert!(tmp.path().join("README.md").exists());
}

// -----------------------------------------------------------------------
// 3l.7 Glob merging
// -----------------------------------------------------------------------

#[test]
fn glob_merging() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.write_file(Path::new("/src/lib.rs"), b"pub mod lib;")
        .unwrap();

    let mut matches = ov.glob("*.rs", Path::new("/src")).unwrap();
    matches.sort();
    assert_eq!(
        matches,
        vec![PathBuf::from("lib.rs"), PathBuf::from("main.rs")]
    );
}

// -----------------------------------------------------------------------
// 3l.8 deep_clone isolation
// -----------------------------------------------------------------------

#[test]
fn deep_clone_isolation() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.write_file(Path::new("/cloneable.txt"), b"original")
        .unwrap();

    let clone = ov.deep_clone();

    // Mutate the clone
    clone
        .write_file(Path::new("/cloneable.txt"), b"mutated")
        .unwrap();
    clone
        .write_file(Path::new("/clone_only.txt"), b"only in clone")
        .unwrap();

    // Original unaffected
    assert_eq!(
        ov.read_file(Path::new("/cloneable.txt")).unwrap(),
        b"original"
    );
    assert!(!ov.exists(Path::new("/clone_only.txt")));

    // Clone sees its changes
    assert_eq!(
        clone.read_file(Path::new("/cloneable.txt")).unwrap(),
        b"mutated"
    );

    // Both can read from lower
    assert_eq!(
        clone.read_file(Path::new("/README.md")).unwrap(),
        b"# Hello"
    );
}

// -----------------------------------------------------------------------
// 3l.9 Non-existent lower → constructor error
// -----------------------------------------------------------------------

#[test]
fn constructor_error_for_nonexistent_lower() {
    let result = OverlayFs::new("/nonexistent/directory/that/does/not/exist");
    assert!(result.is_err());
}

// -----------------------------------------------------------------------
// 3l.10 Ancestor whiteout hides descendants
// -----------------------------------------------------------------------

#[test]
fn ancestor_whiteout_hides_descendants() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // Remove the entire /src directory
    ov.remove_dir_all(Path::new("/src")).unwrap();

    // /src/main.rs should be gone
    assert!(!ov.exists(Path::new("/src/main.rs")));
    assert!(!ov.exists(Path::new("/src")));

    // Reading should fail
    assert!(ov.read_file(Path::new("/src/main.rs")).is_err());
}

// -----------------------------------------------------------------------
// 3l.11 mkdir_p through lower-only directories
// -----------------------------------------------------------------------

#[test]
fn mkdir_p_through_lower_dirs() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // /src exists only in lower. mkdir_p should recognize it and create only build/release
    ov.mkdir_p(Path::new("/src/build/release")).unwrap();
    assert!(ov.exists(Path::new("/src/build/release")));

    // /src is still from lower (not duplicated into upper unnecessarily)
    // The important thing is that it works correctly
    let entries = ov.readdir(Path::new("/src")).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"main.rs"));
    assert!(names.contains(&"build"));
}

// -----------------------------------------------------------------------
// 3l.12 stat / lstat / chmod / utimes
// -----------------------------------------------------------------------

#[test]
fn stat_follows_through_layers() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    let meta = ov.stat(Path::new("/README.md")).unwrap();
    assert_eq!(meta.node_type, NodeType::File);
    assert_eq!(meta.size, 7); // "# Hello"
}

#[test]
fn lstat_on_lower_file() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    let meta = ov.lstat(Path::new("/README.md")).unwrap();
    assert_eq!(meta.node_type, NodeType::File);
}

#[test]
fn chmod_lower_file_copies_up() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.chmod(Path::new("/README.md"), 0o755).unwrap();
    let meta = ov.stat(Path::new("/README.md")).unwrap();
    assert_eq!(meta.mode, 0o755);

    // Content preserved
    assert_eq!(ov.read_file(Path::new("/README.md")).unwrap(), b"# Hello");

    // Lower untouched (mode-bit check is Unix-only; Windows synthesizes modes)
    let disk_meta = std::fs::metadata(tmp.path().join("README.md")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(
            disk_meta.permissions().mode() & 0o777,
            0o755,
            "lower should not be modified"
        );
    }
    let _ = disk_meta;
}

#[test]
fn utimes_lower_file_copies_up() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    let new_time = SystemTime::UNIX_EPOCH;
    ov.utimes(Path::new("/README.md"), new_time).unwrap();
    let meta = ov.stat(Path::new("/README.md")).unwrap();
    assert_eq!(meta.mtime, new_time);
}

// -----------------------------------------------------------------------
// Additional edge cases
// -----------------------------------------------------------------------

#[test]
fn exists_root() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    assert!(ov.exists(Path::new("/")));
}

#[test]
fn readdir_root_merges_both_layers() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.write_file(Path::new("/upper_only.txt"), b"hi").unwrap();

    let entries = ov.readdir(Path::new("/")).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"README.md")); // lower
    assert!(names.contains(&"src")); // lower dir
    assert!(names.contains(&"upper_only.txt")); // upper
}

#[test]
fn copy_from_lower_to_upper() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.copy(Path::new("/README.md"), Path::new("/README_copy.md"))
        .unwrap();
    assert_eq!(
        ov.read_file(Path::new("/README_copy.md")).unwrap(),
        b"# Hello"
    );
    // Lower untouched
    assert!(!tmp.path().join("README_copy.md").exists());
}

#[test]
fn remove_file_then_recreate() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.remove_file(Path::new("/README.md")).unwrap();
    assert!(!ov.exists(Path::new("/README.md")));

    ov.write_file(Path::new("/README.md"), b"new content")
        .unwrap();
    assert_eq!(
        ov.read_file(Path::new("/README.md")).unwrap(),
        b"new content"
    );
}

#[test]
fn hardlink_from_lower() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.hardlink(Path::new("/README.md"), Path::new("/link.md"))
        .unwrap();
    assert_eq!(ov.read_file(Path::new("/link.md")).unwrap(), b"# Hello");

    // Modifying one doesn't affect the other (no real hardlink in overlay)
    ov.write_file(Path::new("/link.md"), b"changed").unwrap();
    assert_eq!(ov.read_file(Path::new("/README.md")).unwrap(), b"# Hello");
}

#[test]
fn symlink_in_upper() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.symlink(Path::new("/README.md"), Path::new("/link_to_readme"))
        .unwrap();
    let target = ov.readlink(Path::new("/link_to_readme")).unwrap();
    assert_eq!(target, PathBuf::from("/README.md"));

    // Reading through the symlink should work
    let content = ov.read_file(Path::new("/link_to_readme")).unwrap();
    assert_eq!(content, b"# Hello");
}

#[test]
fn glob_absolute_pattern() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    let mut matches = ov.glob("/data/*", Path::new("/")).unwrap();
    matches.sort();
    assert_eq!(matches, vec![PathBuf::from("/data/config.toml")]);
}

#[test]
fn deep_clone_whiteout_isolation() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    let clone = ov.deep_clone();
    clone.remove_file(Path::new("/README.md")).unwrap();

    // Original still has the file
    assert!(ov.exists(Path::new("/README.md")));
    // Clone does not
    assert!(!clone.exists(Path::new("/README.md")));
}

#[cfg(unix)]
#[test]
fn chmod_lower_preserves_original_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = setup_lower();
    // Set specific permissions on the lower file
    let lower_path = tmp.path().join("README.md");
    std::fs::set_permissions(&lower_path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let ov = make_overlay(tmp.path());
    ov.chmod(Path::new("/README.md"), 0o700).unwrap();

    // Overlay reports new mode
    assert_eq!(ov.stat(Path::new("/README.md")).unwrap().mode, 0o700);

    // Lower file still has old mode
    let lower_meta = std::fs::metadata(&lower_path).unwrap();
    assert_eq!(lower_meta.permissions().mode() & 0o777, 0o644);
}

#[test]
fn remove_dir_empty_upper_dir() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.mkdir(Path::new("/empty_dir")).unwrap();
    assert!(ov.exists(Path::new("/empty_dir")));

    ov.remove_dir(Path::new("/empty_dir")).unwrap();
    assert!(!ov.exists(Path::new("/empty_dir")));
}

#[test]
fn mkdir_after_rmdir() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // Remove a lower-only directory (must be empty first)
    ov.remove_file(Path::new("/data/config.toml")).unwrap();
    ov.remove_dir(Path::new("/data")).unwrap();
    assert!(!ov.exists(Path::new("/data")));

    // Re-create it
    ov.mkdir(Path::new("/data")).unwrap();
    assert!(ov.exists(Path::new("/data")));
}

#[test]
fn canonicalize_lower_path() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    let canon = ov.canonicalize(Path::new("/src/main.rs")).unwrap();
    assert_eq!(canon, PathBuf::from("/src/main.rs"));
}

#[test]
fn canonicalize_upper_path() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.write_file(Path::new("/upper.txt"), b"hi").unwrap();
    let canon = ov.canonicalize(Path::new("/upper.txt")).unwrap();
    assert_eq!(canon, PathBuf::from("/upper.txt"));
}

#[test]
fn canonicalize_nonexistent_fails() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    assert!(ov.canonicalize(Path::new("/no/such/path")).is_err());
}

#[test]
fn stat_directory_from_lower() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    let meta = ov.stat(Path::new("/src")).unwrap();
    assert_eq!(meta.node_type, NodeType::Directory);
}

#[test]
fn write_to_nested_new_dir() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.mkdir_p(Path::new("/a/b/c")).unwrap();
    ov.write_file(Path::new("/a/b/c/file.txt"), b"deep")
        .unwrap();
    assert_eq!(ov.read_file(Path::new("/a/b/c/file.txt")).unwrap(), b"deep");
}

// -----------------------------------------------------------------------
// Additional edge cases suggested by review
// -----------------------------------------------------------------------

#[test]
fn mkdir_p_after_remove_dir_all_does_not_resurrect_siblings() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // Lower has /data/config.toml
    assert!(ov.exists(Path::new("/data/config.toml")));

    // Remove everything under /data
    ov.remove_dir_all(Path::new("/data")).unwrap();
    assert!(!ov.exists(Path::new("/data")));
    assert!(!ov.exists(Path::new("/data/config.toml")));

    // Re-create /data/sub — config.toml must NOT reappear
    ov.mkdir_p(Path::new("/data/sub")).unwrap();
    assert!(ov.exists(Path::new("/data/sub")));
    assert!(!ov.exists(Path::new("/data/config.toml")));

    let entries = ov.readdir(Path::new("/data")).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["sub"]);
}

#[test]
fn remove_dir_nonempty_merged_directory() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // /data has config.toml from lower — remove_dir should fail
    let result = ov.remove_dir(Path::new("/data"));
    assert!(result.is_err());
}

#[test]
fn append_file_creates_missing_file() {
    // POSIX O_APPEND|O_CREAT: appending to a nonexistent file creates it.
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.append_file(Path::new("/no_such_file.txt"), b"data")
        .unwrap();
    assert_eq!(
        ov.read_file(Path::new("/no_such_file.txt")).unwrap(),
        b"data"
    );
    // The create is an upper-layer write, so it shows up in the diff.
    assert!(
        ov.diff()
            .writes
            .iter()
            .any(|w| w.path == Path::new("/no_such_file.txt"))
    );
}

#[test]
fn append_file_recreates_whiteouted_file() {
    // rm + >> recreates the file fresh (POSIX), without resurrecting any
    // lower content of the same path.
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.remove_file(Path::new("/data/config.toml")).unwrap();
    ov.append_file(Path::new("/data/config.toml"), b"new")
        .unwrap();
    assert_eq!(
        ov.read_file(Path::new("/data/config.toml")).unwrap(),
        b"new"
    );
}

#[test]
fn glob_excludes_whiteouts() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.remove_file(Path::new("/data/config.toml")).unwrap();

    // Glob should not return the whiteout-ed file
    let matches = ov.glob("*", Path::new("/data")).unwrap();
    assert!(matches.is_empty());
}

#[test]
fn rename_directory_with_mixed_layer_children() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // Add an upper-only file alongside the lower-only file
    ov.write_file(Path::new("/src/upper_file.rs"), b"upper")
        .unwrap();

    // Rename the directory
    ov.rename(Path::new("/src"), Path::new("/source")).unwrap();

    // Both children should be under the new name
    assert_eq!(
        ov.read_file(Path::new("/source/main.rs")).unwrap(),
        b"fn main() {}"
    );
    assert_eq!(
        ov.read_file(Path::new("/source/upper_file.rs")).unwrap(),
        b"upper"
    );

    // Old name should be gone
    assert!(!ov.exists(Path::new("/src")));
    assert!(!ov.exists(Path::new("/src/main.rs")));
}

// -----------------------------------------------------------------------
// FIX 1: ensure_upper_dir_path clears ancestor whiteouts
// -----------------------------------------------------------------------

#[test]
fn write_file_under_removed_dir_all_is_visible() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // /data exists in lower with config.toml
    assert!(ov.exists(Path::new("/data/config.toml")));

    // Remove the entire /data tree
    ov.remove_dir_all(Path::new("/data")).unwrap();
    assert!(!ov.exists(Path::new("/data")));

    // Write a new file under /data — ensure_upper_dir_path must clear the
    // whiteout on /data for this to become visible.
    ov.write_file(Path::new("/data/new.txt"), b"hello").unwrap();
    assert!(ov.exists(Path::new("/data")));
    assert!(ov.exists(Path::new("/data/new.txt")));
    assert_eq!(ov.read_file(Path::new("/data/new.txt")).unwrap(), b"hello");

    // The old file must NOT reappear
    assert!(!ov.exists(Path::new("/data/config.toml")));
}

#[test]
fn mkdir_under_whiteout_ancestor() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // Remove /data entirely
    ov.remove_dir_all(Path::new("/data")).unwrap();
    assert!(!ov.exists(Path::new("/data")));

    // mkdir (not mkdir_p) a new dir under a re-created parent
    ov.mkdir_p(Path::new("/data")).unwrap();
    ov.mkdir(Path::new("/data/sub")).unwrap();
    assert!(ov.exists(Path::new("/data/sub")));

    // Old children still gone
    assert!(!ov.exists(Path::new("/data/config.toml")));
}

// -----------------------------------------------------------------------
// FIX 5: append_file / chmod / utimes follow symlinks through overlay
// -----------------------------------------------------------------------

#[test]
fn append_through_symlink_to_lower_file() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // Create a symlink in upper pointing to a lower-layer file
    ov.symlink(Path::new("/README.md"), Path::new("/link_to_readme"))
        .unwrap();

    // Append through the symlink
    ov.append_file(Path::new("/link_to_readme"), b" world")
        .unwrap();

    // The target file should have the appended content
    assert_eq!(
        ov.read_file(Path::new("/README.md")).unwrap(),
        b"# Hello world"
    );
    // Reading through the symlink should also work
    assert_eq!(
        ov.read_file(Path::new("/link_to_readme")).unwrap(),
        b"# Hello world"
    );
}

#[test]
fn chmod_through_symlink() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.symlink(Path::new("/README.md"), Path::new("/link_readme"))
        .unwrap();
    ov.chmod(Path::new("/link_readme"), 0o700).unwrap();

    // The target file should have the new mode
    assert_eq!(ov.stat(Path::new("/README.md")).unwrap().mode, 0o700);
}

#[test]
fn utimes_through_symlink() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.symlink(Path::new("/README.md"), Path::new("/link_readme"))
        .unwrap();
    let new_time = SystemTime::UNIX_EPOCH;
    ov.utimes(Path::new("/link_readme"), new_time).unwrap();

    assert_eq!(ov.stat(Path::new("/README.md")).unwrap().mtime, new_time);
}

// -----------------------------------------------------------------------
// diff() export for harnesses applying sandboxed writes to disk
// -----------------------------------------------------------------------

#[test]
fn diff_empty_when_nothing_written() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    // Reads alone produce no diff
    let _ = ov.read_file(Path::new("/README.md")).unwrap();
    let d = ov.diff();
    assert!(d.writes.is_empty());
    assert!(d.deletions.is_empty());
}

#[test]
fn diff_reports_modified_and_created_files() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.write_file(Path::new("/README.md"), b"changed").unwrap();
    ov.write_file(Path::new("/src/new.rs"), b"new").unwrap();
    ov.mkdir(Path::new("/build")).unwrap();

    let d = ov.diff();
    assert_eq!(d.deletions, Vec::<PathBuf>::new());
    let paths: Vec<&PathBuf> = d.writes.iter().map(|w| &w.path).collect();
    assert!(paths.contains(&&PathBuf::from("/README.md")));
    assert!(paths.contains(&&PathBuf::from("/src/new.rs")));
    assert!(paths.contains(&&PathBuf::from("/build")));
    let readme = d
        .writes
        .iter()
        .find(|w| w.path == Path::new("/README.md"))
        .unwrap();
    assert_eq!(readme.content, b"changed");
    assert_eq!(readme.node_type, NodeType::File);
    let build = d
        .writes
        .iter()
        .find(|w| w.path == Path::new("/build"))
        .unwrap();
    assert_eq!(build.node_type, NodeType::Directory);
    assert!(build.content.is_empty());
}

#[test]
fn diff_reports_deletion_of_lower_file() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.remove_file(Path::new("/README.md")).unwrap();
    let d = ov.diff();
    assert!(d.writes.is_empty());
    assert_eq!(d.deletions, vec![PathBuf::from("/README.md")]);
}

#[test]
fn diff_remove_dir_all_yields_top_most_deletion_only() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.remove_dir_all(Path::new("/data")).unwrap();
    let d = ov.diff();
    // The whole subtree is gone but only the top-most path is reported.
    assert_eq!(d.deletions, vec![PathBuf::from("/data")]);
}

#[test]
fn diff_recreated_path_is_write_not_deletion() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.remove_file(Path::new("/README.md")).unwrap();
    ov.write_file(Path::new("/README.md"), b"reborn").unwrap();

    let d = ov.diff();
    assert!(d.deletions.is_empty());
    // Mode is inherited from the lower file (POSIX O_TRUNC semantics), so
    // it is platform-dependent — compare everything else.
    assert_eq!(d.writes.len(), 1);
    assert_eq!(d.writes[0].path, PathBuf::from("/README.md"));
    assert_eq!(d.writes[0].node_type, NodeType::File);
    assert_eq!(d.writes[0].content, b"reborn".to_vec());
}

#[test]
fn diff_upper_only_delete_is_not_a_deletion() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.write_file(Path::new("/scratch.txt"), b"temp").unwrap();
    ov.remove_file(Path::new("/scratch.txt")).unwrap();

    let d = ov.diff();
    assert!(d.writes.is_empty());
    assert!(d.deletions.is_empty());
}

#[test]
fn diff_reports_symlink_target() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.symlink(Path::new("/README.md"), Path::new("/link.md"))
        .unwrap();
    let d = ov.diff();
    let link = d
        .writes
        .iter()
        .find(|w| w.path == Path::new("/link.md"))
        .unwrap();
    assert_eq!(link.node_type, NodeType::Symlink);
    assert_eq!(link.content, b"/README.md");
}

#[test]
fn diff_reflects_writes_through_shell_exec() {
    use crate::RustBashBuilder;
    use std::sync::Arc;

    let tmp = setup_lower();
    let overlay = Arc::new(make_overlay(tmp.path()));
    let mut shell = RustBashBuilder::new()
        .fs(overlay.clone())
        .cwd("/")
        .build()
        .unwrap();

    shell
        .exec("echo patched > /README.md; rm /data/config.toml; mkdir -p /out")
        .unwrap();

    let d = overlay.diff();
    assert_eq!(d.deletions, vec![PathBuf::from("/data/config.toml")]);
    let paths: Vec<&PathBuf> = d.writes.iter().map(|w| &w.path).collect();
    assert!(paths.contains(&&PathBuf::from("/README.md")));
    assert!(paths.contains(&&PathBuf::from("/out")));
    let readme = d
        .writes
        .iter()
        .find(|w| w.path == Path::new("/README.md"))
        .unwrap();
    assert_eq!(readme.content, b"patched\n");
    // Disk is never modified
    assert_eq!(
        std::fs::read(tmp.path().join("README.md")).unwrap(),
        b"# Hello"
    );
    assert!(tmp.path().join("data/config.toml").exists());
}

#[test]
fn stat_resolves_dotdot_segments() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    let meta = ov.stat(Path::new("/data/../README.md")).unwrap();
    assert_eq!(meta.node_type, NodeType::File);
    let content = ov.read_file(Path::new("/./data/../README.md")).unwrap();
    assert_eq!(content, b"# Hello");
}

#[test]
fn upper_layer_writes_persist_across_exec_calls() {
    use crate::RustBashBuilder;
    use std::sync::Arc;

    let tmp = setup_lower();
    let overlay = Arc::new(make_overlay(tmp.path()));
    let mut shell = RustBashBuilder::new()
        .fs(overlay.clone())
        .cwd("/")
        .build()
        .unwrap();

    // First exec writes a scratch file; it is NOT applied to disk.
    let r1 = shell
        .exec("echo 'intermediate data' > /tmp/result.txt")
        .unwrap();
    assert_eq!(r1.exit_code, 0);
    assert!(!tmp.path().join("tmp/result.txt").exists());

    // Second exec (a later "tool call") reads it back through the overlay.
    let r2 = shell.exec("cat /tmp/result.txt | tr a-z A-Z").unwrap();
    assert_eq!(r2.stdout, "INTERMEDIATE DATA\n");

    // And it is still reported by diff() as a pending write.
    let d = overlay.diff();
    assert!(
        d.writes
            .iter()
            .any(|w| w.path == Path::new("/tmp/result.txt"))
    );
}

// -----------------------------------------------------------------------
// sync()/reset() — reconciling the overlay with disk state
// -----------------------------------------------------------------------

#[test]
fn sync_drops_applied_writes_and_keeps_unapplied() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.write_file(Path::new("/README.md"), b"applied").unwrap();
    ov.write_file(Path::new("/src/main.rs"), b"still pending")
        .unwrap();
    // Harness applies only the first write to disk.
    std::fs::write(tmp.path().join("README.md"), b"applied").unwrap();

    ov.sync();
    let d = ov.diff();
    let paths: Vec<&PathBuf> = d.writes.iter().map(|w| &w.path).collect();
    assert!(!paths.contains(&&PathBuf::from("/README.md")));
    assert!(paths.contains(&&PathBuf::from("/src/main.rs")));
    // Reads fall through to disk for the dropped entry.
    assert_eq!(ov.read_file(Path::new("/README.md")).unwrap(), b"applied");
}

#[test]
fn sync_keeps_write_when_disk_differs() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.write_file(Path::new("/README.md"), b"sandbox version")
        .unwrap();
    // A native run rewrote the disk file differently.
    std::fs::write(tmp.path().join("README.md"), b"native version").unwrap();

    ov.sync();
    let d = ov.diff();
    let w = d
        .writes
        .iter()
        .find(|w| w.path == Path::new("/README.md"))
        .unwrap();
    assert_eq!(w.content, b"sandbox version");
}

#[test]
fn sync_drops_byte_identical_disk_rewrite() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.write_file(Path::new("/README.md"), b"same content")
        .unwrap();
    // A native tool rewrote the file with identical bytes (new mtime).
    std::fs::write(tmp.path().join("README.md"), b"same content").unwrap();

    ov.sync();
    assert!(ov.diff().writes.is_empty());
}

#[test]
fn sync_clears_applied_deletion_and_keeps_pending_one() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.remove_file(Path::new("/README.md")).unwrap(); // deletion applied below
    ov.remove_file(Path::new("/src/main.rs")).unwrap(); // deletion not applied
    std::fs::remove_file(tmp.path().join("README.md")).unwrap();

    ov.sync();
    assert_eq!(ov.diff().deletions, vec![PathBuf::from("/src/main.rs")]);
}

#[test]
fn sync_drops_fully_applied_directory_tree() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.mkdir(Path::new("/out")).unwrap();
    ov.write_file(Path::new("/out/a.txt"), b"a").unwrap();
    ov.write_file(Path::new("/out/b.txt"), b"b").unwrap();
    // Harness applies the whole tree.
    std::fs::create_dir_all(tmp.path().join("out")).unwrap();
    std::fs::write(tmp.path().join("out/a.txt"), b"a").unwrap();
    std::fs::write(tmp.path().join("out/b.txt"), b"b").unwrap();

    ov.sync();
    let d = ov.diff();
    assert!(
        d.writes.iter().all(|w| w.path != Path::new("/out")),
        "got {:?}",
        d.writes
    );
    assert!(d.writes.iter().all(|w| w.path != Path::new("/out/a.txt")));
}

#[test]
fn sync_keeps_directory_when_a_child_is_still_pending() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.mkdir(Path::new("/out")).unwrap();
    ov.write_file(Path::new("/out/a.txt"), b"applied").unwrap();
    ov.write_file(Path::new("/out/b.txt"), b"pending").unwrap();
    std::fs::create_dir_all(tmp.path().join("out")).unwrap();
    std::fs::write(tmp.path().join("out/a.txt"), b"applied").unwrap();

    ov.sync();
    let d = ov.diff();
    let paths: Vec<&PathBuf> = d.writes.iter().map(|w| &w.path).collect();
    assert!(!paths.contains(&&PathBuf::from("/out/a.txt")));
    assert!(paths.contains(&&PathBuf::from("/out/b.txt")));
    // The directory stays as the container of the pending child.
    assert!(paths.contains(&&PathBuf::from("/out")));
}

#[test]
fn sync_clears_whiteout_tree_when_disk_directory_removed() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.remove_dir_all(Path::new("/data")).unwrap(); // recursive whiteouts
    std::fs::remove_dir_all(tmp.path().join("data")).unwrap(); // deletion applied

    ov.sync();
    assert!(ov.diff().deletions.is_empty());
}

#[test]
fn sync_keeps_unapplied_symlink() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.symlink(Path::new("/README.md"), Path::new("/link.md"))
        .unwrap();
    // Nothing applied; disk has no symlink.
    ov.sync();
    let d = ov.diff();
    assert!(d.writes.iter().any(|w| w.path == Path::new("/link.md")));
}

#[test]
fn reset_clears_pending_writes_and_deletions() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.write_file(Path::new("/README.md"), b"changed").unwrap();
    ov.write_file(Path::new("/scratch.txt"), b"new").unwrap();
    ov.remove_file(Path::new("/data/config.toml")).unwrap();

    ov.reset();
    let d = ov.diff();
    assert!(d.writes.is_empty());
    assert!(d.deletions.is_empty());
    // Reads fall through to current disk state.
    assert_eq!(ov.read_file(Path::new("/README.md")).unwrap(), b"# Hello");
    assert!(ov.exists(Path::new("/data/config.toml")));
    // New pending writes can be tracked again afterwards.
    ov.write_file(Path::new("/again.txt"), b"x").unwrap();
    assert!(
        ov.diff()
            .writes
            .iter()
            .any(|w| w.path == Path::new("/again.txt"))
    );
}

// -----------------------------------------------------------------------
// Standalone use: shared overlay across shells and direct VFS access
// -----------------------------------------------------------------------

#[test]
fn overlay_usable_without_a_shell_and_across_shell_recreations() {
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::RustBashBuilder;

    let tmp = setup_lower();
    let overlay = Arc::new(make_overlay(tmp.path()));

    // 1. Direct use with no shell at all — the harness's write tool writes
    //    through the VirtualFs trait into the upper layer.
    VirtualFs::write_file(&*overlay, Path::new("/notes.md"), b"from harness tool").unwrap();
    assert!(
        overlay
            .diff()
            .writes
            .iter()
            .any(|w| w.path == Path::new("/notes.md"))
    );

    // 2. A shell built over the same overlay sees the harness's write.
    let mut shell1 = RustBashBuilder::new()
        .fs(overlay.clone())
        .cwd("/")
        .env(HashMap::from([("SESSION".to_string(), "one".to_string())]))
        .build()
        .unwrap();
    let r = shell1
        .exec("cat /notes.md; cd /src; export LEAKED=1")
        .unwrap();
    assert_eq!(r.stdout, "from harness tool");

    // 3. Drop the shell and build a fresh one: clean env, reset cwd —
    //    but the overlay (harness write + sandbox change) persists.
    drop(shell1);
    let mut shell2 = RustBashBuilder::new()
        .fs(overlay.clone())
        .cwd("/")
        .build()
        .unwrap();
    let r = shell2
        .exec("echo $LEAKED; pwd; echo sandbox >> /notes.md; cat /notes.md")
        .unwrap();
    assert_eq!(r.stdout, "\n/\nfrom harness toolsandbox\n");
    let d = overlay.diff();
    let notes = d
        .writes
        .iter()
        .find(|w| w.path == Path::new("/notes.md"))
        .unwrap();
    assert_eq!(notes.content, b"from harness toolsandbox\n");
}

// -----------------------------------------------------------------------
// Lazy resurrection whiteouts + upper shadowing (perf rework)
// -----------------------------------------------------------------------

#[test]
fn rm_dir_all_then_mkdir_rehides_lower_children_lazily() {
    // rm -rf is a single top-most whiteout; recreating the directory must
    // lazily re-hide the lower children (one level at a time).
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.remove_dir_all(Path::new("/data")).unwrap();
    ov.mkdir(Path::new("/data")).unwrap();
    ov.write_file(Path::new("/data/new.txt"), b"x").unwrap();

    let names: Vec<String> = ov
        .readdir(Path::new("/data"))
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, vec!["new.txt"]);
    assert!(!ov.exists(Path::new("/data/config.toml")));

    // The diff reports the deletions of the hidden lower children plus the
    // recreated directory and new file — applying it to the real directory
    // reproduces exactly the sandbox state.
    let diff = ov.diff();
    assert_eq!(diff.deletions, vec![PathBuf::from("/data/config.toml")]);
    let write_paths: Vec<&Path> = diff.writes.iter().map(|w| w.path.as_path()).collect();
    assert!(write_paths.contains(&Path::new("/data")));
    assert!(write_paths.contains(&Path::new("/data/new.txt")));
}

#[test]
fn rm_dir_all_then_write_inside_rehides_lower_children() {
    // Resurrection through write_file's parent handling (no explicit mkdir).
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());

    ov.remove_dir_all(Path::new("/data")).unwrap();
    ov.write_file(Path::new("/data/new.txt"), b"x").unwrap();

    let names: Vec<String> = ov
        .readdir(Path::new("/data"))
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, vec!["new.txt"]);
    assert!(!ov.exists(Path::new("/data/config.toml")));
}

#[test]
fn rm_dir_all_deep_resurrection_populates_level_by_level() {
    // rm -rf /data, then mkdir -p /data/deep/nested: each resurrected level
    // re-hides its own lower children; untouched lower subtrees stay hidden.
    let tmp = setup_lower();
    let base = tmp.path();
    std::fs::create_dir_all(base.join("data/sub/leaf")).unwrap();
    std::fs::write(base.join("data/sub/leaf/old.txt"), b"old").unwrap();
    let ov = make_overlay(tmp.path());

    ov.remove_dir_all(Path::new("/data")).unwrap();
    ov.mkdir_p(Path::new("/data/sub/newdir")).unwrap();

    assert!(ov.exists(Path::new("/data/sub/newdir")));
    // /data/sub was resurrected by mkdir_p: its lower children are hidden…
    assert!(!ov.exists(Path::new("/data/sub/leaf")));
    assert!(!ov.exists(Path::new("/data/config.toml")));
    // …until /data/sub/leaf is itself resurrected, revealing a fresh empty dir.
    ov.mkdir_p(Path::new("/data/sub/leaf")).unwrap();
    assert!(ov.exists(Path::new("/data/sub/leaf")));
    assert!(!ov.exists(Path::new("/data/sub/leaf/old.txt")));
}

#[test]
fn upper_dir_shadows_lower_symlink_without_following_it() {
    // Overlay semantics: an upper entry shadows the lower entry at the same
    // path whatever its type. A lower symlink shadowed by an upper directory
    // must NOT be followed during path resolution.
    let tmp = setup_lower();
    let base = tmp.path();
    std::fs::create_dir_all(base.join("target")).unwrap();
    std::fs::write(base.join("target/secret.txt"), b"secret").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(base.join("target"), base.join("link")).unwrap();
        let ov = make_overlay(base);
        // Create an upper DIRECTORY at /link (shadowing the lower symlink).
        ov.write_file(Path::new("/link/file.txt"), b"new").unwrap();
        // Resolution must treat /link as the upper directory, not follow
        // the lower symlink into /target.
        assert_eq!(ov.read_file(Path::new("/link/file.txt")).unwrap(), b"new");
        assert!(!ov.exists(Path::new("/link/secret.txt")));
        assert!(ov.exists(Path::new("/target/secret.txt")));
    }
}

#[test]
fn diff_topmost_filter_with_many_whiteouts() {
    // Many individual deletions: every file is its own top-most whiteout;
    // a subsequent rm -rf collapses them to the directory whiteout.
    let tmp = setup_lower();
    let base = tmp.path();
    for i in 0..50 {
        std::fs::write(base.join(format!("data/f{i}.txt")), b"x").unwrap();
    }
    let ov = make_overlay(tmp.path());
    for i in 0..50 {
        ov.remove_file(Path::new(&format!("/data/f{i}.txt")))
            .unwrap();
    }
    assert_eq!(ov.diff().deletions.len(), 50);
    ov.remove_dir_all(Path::new("/data")).unwrap();
    assert_eq!(ov.diff().deletions, vec![PathBuf::from("/data")]);
}

#[test]
fn mkdir_with_whiteouted_parent_resurrects_child_cleanly() {
    // rm -rf /data then mkdir /data/sub: parent resurrection re-whiteouts
    // /data/sub (a deleted lower child); mkdir must resurrect it in turn —
    // a whiteout must never coexist with an upper entry.
    let tmp = setup_lower();
    std::fs::create_dir_all(tmp.path().join("data/sub")).unwrap();
    std::fs::write(tmp.path().join("data/sub/inner.txt"), b"i").unwrap();
    let ov = make_overlay(tmp.path());

    ov.remove_dir_all(Path::new("/data")).unwrap();
    ov.mkdir(Path::new("/data/sub")).unwrap();

    assert!(ov.exists(Path::new("/data/sub")));
    let names: Vec<String> = ov
        .readdir(Path::new("/data"))
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, vec!["sub"]);
    // The resurrected /data/sub is a fresh empty dir: lower content hidden.
    assert!(ov.readdir(Path::new("/data/sub")).unwrap().is_empty());
    assert!(!ov.exists(Path::new("/data/sub/inner.txt")));
}

#[test]
fn write_file_onto_existing_directory_is_eisdir() {
    // POSIX: open(O_CREAT|O_TRUNC) on a directory fails with EISDIR.
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    let err = ov.write_file(Path::new("/data"), b"x").unwrap_err();
    assert!(matches!(err, VfsError::IsADirectory(_)));
    // Same through an upper-only directory.
    ov.mkdir(Path::new("/upperdir")).unwrap();
    let err = ov.write_file(Path::new("/upperdir"), b"x").unwrap_err();
    assert!(matches!(err, VfsError::IsADirectory(_)));
}

#[test]
fn write_file_onto_whiteouted_directory_stays_hidden_children() {
    // rm -rf /data; echo x > /data — bash creates a plain file and the
    // deleted children stay deleted.
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.remove_dir_all(Path::new("/data")).unwrap();
    ov.write_file(Path::new("/data"), b"x").unwrap();
    assert_eq!(ov.read_file(Path::new("/data")).unwrap(), b"x");
    assert!(!ov.exists(Path::new("/data/config.toml")));
}

#[test]
fn exists_and_readdir_follow_upper_symlink_into_lower() {
    // An upper symlink pointing into the lower layer: exists/readdir must
    // resolve it (previously they probed the layers by literal path).
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.symlink(Path::new("/data"), Path::new("/ln")).unwrap();
    assert!(ov.exists(Path::new("/ln/config.toml")));
    let names: Vec<String> = ov
        .readdir(Path::new("/ln"))
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, vec!["config.toml"]);
}

#[test]
fn rename_collapses_source_whiteouts_to_topmost() {
    // Renaming a lower directory must record ONE top-most deletion in the
    // diff, not one per descendant.
    let tmp = setup_lower();
    std::fs::create_dir_all(tmp.path().join("data/sub")).unwrap();
    std::fs::write(tmp.path().join("data/sub/inner.txt"), b"i").unwrap();
    let ov = make_overlay(tmp.path());
    ov.rename(Path::new("/data"), Path::new("/moved")).unwrap();
    assert_eq!(ov.diff().deletions, vec![PathBuf::from("/data")]);
    assert!(ov.exists(Path::new("/moved/sub/inner.txt")));
}

#[test]
fn rename_onto_whiteouted_lower_dir_hides_its_children() {
    // mv newdir deleted-dir: the destination's deleted lower children must
    // not reappear.
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.remove_dir_all(Path::new("/data")).unwrap();
    ov.mkdir(Path::new("/new")).unwrap();
    ov.write_file(Path::new("/new/x.txt"), b"x").unwrap();
    ov.rename(Path::new("/new"), Path::new("/data")).unwrap();
    let names: Vec<String> = ov
        .readdir(Path::new("/data"))
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, vec!["x.txt"]);
    assert!(!ov.exists(Path::new("/data/config.toml")));
}

// ---------------------------------------------------------------------------
// Review-fix pins: writes through mid-path upper symlinks (resolve_write_target)
// ---------------------------------------------------------------------------

#[test]
fn write_through_mid_path_upper_symlink_creates_at_target() {
    // mkdir /real; ln -s /real /l; write /l/f — POSIX: creates /real/f.
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.mkdir(Path::new("/real")).unwrap();
    ov.symlink(Path::new("/real"), Path::new("/l")).unwrap();
    ov.write_file(Path::new("/l/f.txt"), b"hi").unwrap();
    assert_eq!(ov.read_file(Path::new("/real/f.txt")).unwrap(), b"hi");
    // The upper tree holds the file at the RESOLVED path, not behind /l.
    assert!(ov.tree_contains(Path::new("/real/f.txt")));
    assert!(!ov.tree_contains(Path::new("/l/f.txt")));
}

#[test]
fn append_and_hardlink_through_mid_path_upper_symlink() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.mkdir(Path::new("/real")).unwrap();
    ov.symlink(Path::new("/real"), Path::new("/l")).unwrap();
    ov.append_file(Path::new("/l/a.txt"), b"a").unwrap();
    ov.append_file(Path::new("/l/a.txt"), b"b").unwrap();
    assert_eq!(ov.read_file(Path::new("/real/a.txt")).unwrap(), b"ab");
    ov.hardlink(Path::new("/real/a.txt"), Path::new("/l/h.txt"))
        .unwrap();
    assert!(ov.tree_contains(Path::new("/real/h.txt")));
}

#[test]
fn write_through_chained_mid_path_symlinks() {
    // ln -s /real /l2; ln -s /l2 /l1; write /l1/f — chains are followed.
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.mkdir(Path::new("/real")).unwrap();
    ov.symlink(Path::new("/real"), Path::new("/l2")).unwrap();
    ov.symlink(Path::new("/l2"), Path::new("/l1")).unwrap();
    ov.write_file(Path::new("/l1/f.txt"), b"hi").unwrap();
    assert!(ov.tree_contains(Path::new("/real/f.txt")));
}

#[test]
fn write_through_relative_symlink_with_dotdot_normalizes() {
    // ln -s ../real /a/l: the spliced target must normalize — no literal
    // ".." nodes may enter the tree.
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.mkdir_p(Path::new("/real")).unwrap();
    ov.mkdir_p(Path::new("/a")).unwrap();
    ov.symlink(Path::new("../real"), Path::new("/a/l")).unwrap();
    ov.write_file(Path::new("/a/l/f.txt"), b"hi").unwrap();
    assert!(ov.tree_contains(Path::new("/real/f.txt")));
    assert!(!ov.tree_contains(Path::new("/a/../real/f.txt")));
    assert_eq!(ov.read_file(Path::new("/real/f.txt")).unwrap(), b"hi");
}

#[test]
fn mkdir_p_through_mid_path_upper_symlink() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.mkdir(Path::new("/real")).unwrap();
    ov.symlink(Path::new("/real"), Path::new("/l")).unwrap();
    ov.mkdir_p(Path::new("/l/deep/nested")).unwrap();
    assert!(ov.tree_contains(Path::new("/real/deep/nested")));
    assert!(!ov.tree_contains(Path::new("/l/deep")));
}

#[test]
fn rename_dst_through_mid_path_upper_symlink() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.mkdir(Path::new("/real")).unwrap();
    ov.symlink(Path::new("/real"), Path::new("/l")).unwrap();
    ov.write_file(Path::new("/src.txt"), b"s").unwrap();
    ov.rename(Path::new("/src.txt"), Path::new("/l/dst.txt"))
        .unwrap();
    assert!(ov.tree_contains(Path::new("/real/dst.txt")));
    assert!(!ov.exists(Path::new("/src.txt")));
}

#[test]
fn write_over_lower_file_inherits_lower_mode() {
    // POSIX O_TRUNC keeps the existing file's mode; an overwrite of a lower
    // 0o755 script must not turn it 0o644 in the diff.
    let tmp = setup_lower();
    let script = tmp.path().join("tool.sh");
    std::fs::write(&script, b"#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let ov = make_overlay(tmp.path());
    ov.write_file(Path::new("/tool.sh"), b"#!/bin/sh\necho new\n")
        .unwrap();
    let write = ov
        .diff()
        .writes
        .into_iter()
        .find(|w| w.path == Path::new("/tool.sh"))
        .unwrap();
    #[cfg(unix)]
    assert_eq!(write.mode, 0o755);
    #[cfg(not(unix))]
    let _ = write.mode;
}

#[test]
fn remove_through_file_mid_path_is_enotdir() {
    // Upper file at /a shadows lower /a/b/c: rm /a/b/c is ENOTDIR, not a
    // silent Ok (put_whiteout used to no-op on the file ancestor).
    let tmp = setup_lower();
    std::fs::create_dir_all(tmp.path().join("a/b")).unwrap();
    std::fs::write(tmp.path().join("a/b/c.txt"), b"c").unwrap();
    let ov = make_overlay(tmp.path());
    ov.remove_dir_all(Path::new("/a")).unwrap();
    ov.write_file(Path::new("/a"), b"shadow").unwrap();
    let err = ov.remove_file(Path::new("/a/b/c.txt")).unwrap_err();
    assert!(
        matches!(err, VfsError::NotADirectory(_)),
        "expected ENOTDIR, got {err:?}"
    );
}

#[test]
fn stat_through_file_mid_path_is_hidden() {
    let tmp = setup_lower();
    std::fs::create_dir_all(tmp.path().join("a/b")).unwrap();
    std::fs::write(tmp.path().join("a/b/c.txt"), b"c").unwrap();
    let ov = make_overlay(tmp.path());
    ov.remove_dir_all(Path::new("/a")).unwrap();
    ov.write_file(Path::new("/a"), b"shadow").unwrap();
    assert!(ov.stat(Path::new("/a/b/c.txt")).is_err());
    assert!(!ov.exists(Path::new("/a/b/c.txt")));
}

#[test]
fn remove_dir_all_of_upper_only_dir_leaves_no_whiteout() {
    // rm -rf of an upper-created directory detaches; no phantom marker.
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.mkdir_p(Path::new("/mine/sub")).unwrap();
    ov.write_file(Path::new("/mine/f"), b"x").unwrap();
    ov.remove_dir_all(Path::new("/mine")).unwrap();
    assert!(!ov.tree_contains(Path::new("/mine")));
    assert!(ov.diff().deletions.is_empty());
}

#[test]
fn readdir_never_lists_whiteout_children() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.remove_file(Path::new("/data/config.toml")).unwrap();
    let names: Vec<String> = ov
        .readdir(Path::new("/data"))
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(!names.contains(&"config.toml".to_string()));
    assert!(names.is_empty());
}

// ---------------------------------------------------------------------------
// Review-3 pins (F1-F10): the symlink seam's existence checks and
// normalization boundaries
// ---------------------------------------------------------------------------

#[test]
fn symlink_through_mid_path_link_refuses_to_clobber_existing() {
    // mkdir /real; write /real/exists; ln -s /real /l; ln -s /evil /l/exists
    // must fail EEXIST (POSIX) — not silently replace the file.
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.mkdir(Path::new("/real")).unwrap();
    ov.write_file(Path::new("/real/exists"), b"data").unwrap();
    ov.symlink(Path::new("/real"), Path::new("/l")).unwrap();
    let err = ov
        .symlink(Path::new("/evil"), Path::new("/l/exists"))
        .unwrap_err();
    assert!(matches!(err, VfsError::AlreadyExists(_)), "got {err:?}");
    assert_eq!(ov.read_file(Path::new("/real/exists")).unwrap(), b"data");
}

#[test]
fn hardlink_through_mid_path_link_refuses_to_clobber_existing() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.mkdir(Path::new("/real")).unwrap();
    ov.write_file(Path::new("/real/exists"), b"data").unwrap();
    ov.symlink(Path::new("/real"), Path::new("/l")).unwrap();
    let err = ov
        .hardlink(Path::new("/README.md"), Path::new("/l/exists"))
        .unwrap_err();
    assert!(matches!(err, VfsError::AlreadyExists(_)), "got {err:?}");
}

#[test]
fn dotdot_escaping_root_normalizes_to_root() {
    // ln -s ../../etc /a/l: ".." past the root is the root (POSIX); no
    // literal ".." node may enter the tree, and the write lands at /etc/f.
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.mkdir_p(Path::new("/a")).unwrap();
    ov.symlink(Path::new("../../etc"), Path::new("/a/l"))
        .unwrap();
    ov.write_file(Path::new("/a/l/f.txt"), b"hi").unwrap();
    assert!(ov.tree_contains(Path::new("/etc/f.txt")));
    assert!(!ov.tree_contains(Path::new("/../etc/f.txt")));
    // Readable back through the same path (write/read symmetry).
    assert_eq!(ov.read_file(Path::new("/a/l/f.txt")).unwrap(), b"hi");
}

#[test]
fn mkdir_through_mid_path_link_eexist_at_resolved() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.mkdir_p(Path::new("/real/existing")).unwrap();
    ov.symlink(Path::new("/real"), Path::new("/l")).unwrap();
    let err = ov.mkdir(Path::new("/l/existing")).unwrap_err();
    assert!(matches!(err, VfsError::AlreadyExists(_)), "got {err:?}");
    // A fresh name under the link works and lands at the target.
    ov.mkdir(Path::new("/l/newdir")).unwrap();
    assert!(ov.tree_contains(Path::new("/real/newdir")));
}

#[test]
fn recreate_after_rm_uses_default_mode_not_deleted_mode() {
    // POSIX: rm then create is O_CREAT (0o644 & umask), not O_TRUNC.
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.remove_file(Path::new("/README.md")).unwrap();
    ov.write_file(Path::new("/README.md"), b"reborn").unwrap();
    let d = ov.diff();
    let w = d
        .writes
        .iter()
        .find(|w| w.path == Path::new("/README.md"))
        .unwrap();
    assert_eq!(
        w.mode, 0o644,
        "recreated file must not inherit deleted mode"
    );
}

#[test]
fn remove_through_mid_path_symlink_whiteouts_the_target() {
    // rm /l/f where /l -> /real: POSIX follows mid-path links; /real/f is
    // whiteouted (and the symlink itself survives).
    let tmp = setup_lower();
    std::fs::create_dir_all(tmp.path().join("real")).unwrap();
    std::fs::write(tmp.path().join("real/f.txt"), b"f").unwrap();
    let ov = make_overlay(tmp.path());
    ov.symlink(Path::new("/real"), Path::new("/l")).unwrap();
    ov.remove_file(Path::new("/l/f.txt")).unwrap();
    assert!(!ov.exists(Path::new("/real/f.txt")));
    assert!(ov.exists(Path::new("/l")));
    assert!(ov.diff().deletions.contains(&PathBuf::from("/real/f.txt")));
}

#[test]
fn append_through_symlink_to_deleted_target_recreates() {
    // rm /f; ln -s /f /l; append /l — POSIX creates the target fresh.
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.remove_file(Path::new("/README.md")).unwrap();
    ov.symlink(Path::new("/README.md"), Path::new("/l"))
        .unwrap();
    ov.append_file(Path::new("/l"), b"new").unwrap();
    assert_eq!(ov.read_file(Path::new("/README.md")).unwrap(), b"new");
}

#[test]
fn rename_file_onto_lower_dir_is_eisdir() {
    // POSIX rename(2): a file cannot replace a directory (the lower dir
    // must not be silently shadowed by a file node).
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.write_file(Path::new("/f.txt"), b"f").unwrap();
    let err = ov
        .rename(Path::new("/f.txt"), Path::new("/data"))
        .unwrap_err();
    assert!(matches!(err, VfsError::IsADirectory(_)), "got {err:?}");
    assert!(ov.stat(Path::new("/data")).unwrap().node_type == NodeType::Directory);
}

#[test]
fn rename_of_upper_only_source_leaves_no_phantom_whiteout() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.write_file(Path::new("/mine.txt"), b"x").unwrap();
    ov.rename(Path::new("/mine.txt"), Path::new("/moved.txt"))
        .unwrap();
    assert!(!ov.tree_contains(Path::new("/mine.txt")));
    assert!(ov.diff().deletions.is_empty());
}

#[test]
fn rename_src_through_mid_path_symlink() {
    // mv /l/f /g where /l -> /real: moves /real/f.
    let tmp = setup_lower();
    std::fs::create_dir_all(tmp.path().join("real")).unwrap();
    std::fs::write(tmp.path().join("real/f.txt"), b"f").unwrap();
    let ov = make_overlay(tmp.path());
    ov.symlink(Path::new("/real"), Path::new("/l")).unwrap();
    ov.rename(Path::new("/l/f.txt"), Path::new("/g.txt"))
        .unwrap();
    assert_eq!(ov.read_file(Path::new("/g.txt")).unwrap(), b"f");
    assert!(!ov.exists(Path::new("/real/f.txt")));
}

// ---------------------------------------------------------------------------
// Review-4 pins: rename guards, lower-symlink write seam, rm -rf symlink
// ---------------------------------------------------------------------------

#[test]
fn rename_dir_into_own_descendant_is_rejected() {
    // POSIX rename(2): EINVAL ("cannot move to a subdirectory of itself").
    // Without the guard the recursive copy never terminates (stack
    // overflow) — mv /a /a/b must error, not crash.
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.mkdir_p(Path::new("/a/b")).unwrap();
    ov.write_file(Path::new("/a/f.txt"), b"f").unwrap();
    let err = ov.rename(Path::new("/a"), Path::new("/a/b/a")).unwrap_err();
    assert!(matches!(err, VfsError::InvalidPath(_)), "got {err:?}");
    // Source untouched.
    assert_eq!(ov.read_file(Path::new("/a/f.txt")).unwrap(), b"f");
}

#[test]
fn rename_dir_onto_nonempty_dir_is_enotempty() {
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.mkdir_p(Path::new("/src")).unwrap();
    ov.write_file(Path::new("/src/new.txt"), b"n").unwrap();
    // /data exists in the lower with config.toml inside.
    let err = ov
        .rename(Path::new("/src"), Path::new("/data"))
        .unwrap_err();
    assert!(matches!(err, VfsError::DirectoryNotEmpty(_)), "got {err:?}");
}

#[test]
fn rm_rf_of_symlink_unlinks_the_link() {
    // GNU rm -rf of a symlink (even to a directory) removes the LINK.
    let tmp = setup_lower();
    let ov = make_overlay(tmp.path());
    ov.symlink(Path::new("/data"), Path::new("/lk")).unwrap();
    ov.remove_dir_all(Path::new("/lk")).unwrap();
    assert!(!ov.exists(Path::new("/lk")));
    assert!(ov.exists(Path::new("/data/config.toml")));
}

#[test]
fn write_through_live_lower_symlink_writes_target() {
    // POSIX open() follows a live lower symlink: the write lands on the
    // target (copied up), the link stays a link, and the mode is the
    // target's (not the symlink's 0o777).
    let tmp = setup_lower();
    std::fs::write(tmp.path().join("target.txt"), b"old").unwrap();
    let link = tmp.path().join("lk");
    #[cfg(unix)]
    std::os::unix::fs::symlink("target.txt", &link).unwrap();
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_file("target.txt", &link).is_err() {
            eprintln!("skipping: OS denied symlink creation");
            return;
        }
    }
    let ov = make_overlay(tmp.path());
    ov.write_file(Path::new("/lk"), b"new").unwrap();
    assert_eq!(ov.read_file(Path::new("/target.txt")).unwrap(), b"new");
    assert_eq!(
        ov.readlink(Path::new("/lk")).unwrap(),
        Path::new("target.txt")
    );
    let w = ov
        .diff()
        .writes
        .iter()
        .find(|w| w.path == Path::new("/target.txt"))
        .expect("diff reports the write at the target")
        .clone();
    assert_eq!(
        w.mode & 0o777,
        0o644 & (w.mode),
        "mode from target, not link"
    );
}

#[test]
fn mkdir_p_through_lower_symlinked_dir() {
    // POSIX mkdir -p follows mid-path lower symlinks.
    let tmp = setup_lower();
    std::fs::create_dir_all(tmp.path().join("real")).unwrap();
    let link = tmp.path().join("ldir");
    #[cfg(unix)]
    std::os::unix::fs::symlink("real", &link).unwrap();
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_dir("real", &link).is_err() {
            eprintln!("skipping: OS denied symlink creation");
            return;
        }
    }
    let ov = make_overlay(tmp.path());
    ov.mkdir_p(Path::new("/ldir/x/y")).unwrap();
    assert!(ov.tree_contains(Path::new("/real/x/y")));
    assert!(!ov.tree_contains(Path::new("/ldir/x")));
}
