mod memory;
mod mountable;

#[cfg(feature = "native-fs")]
mod overlay;

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "native-fs"))]
mod overlay_tests;

#[cfg(test)]
mod mountable_tests;

pub use memory::InMemoryFs;
pub use mountable::MountableFs;

#[cfg(feature = "native-fs")]
pub use overlay::{OverlayDiff, OverlayFs, OverlayWrite};

use crate::error::VfsError;
use crate::platform::SystemTime;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// VFS paths always use Unix-style `/` separators. `std::path::Path::is_absolute()`
/// is platform-dependent (Windows drive/UNC prefixes), so we roll our own check.
pub(crate) fn vfs_path_is_absolute(path: &Path) -> bool {
    path.to_str().is_some_and(|s| s.starts_with('/'))
}

/// Append one path component to a VFS path using Unix-style `/` separators.
///
/// `PathBuf::join`/`push` insert the host separator (`\` on Windows), which
/// would leak into VFS paths and user-visible output. All VFS-internal path
/// construction must go through this helper instead. `name` must be a single
/// component (never empty, never containing `/`).
pub(crate) fn vfs_join(base: &Path, name: &str) -> PathBuf {
    debug_assert!(!name.is_empty() && !name.contains('/'));
    let mut s = base.to_string_lossy().into_owned();
    if !s.ends_with('/') {
        s.push('/');
    }
    s.push_str(name);
    PathBuf::from(s)
}

/// Append a multi-component relative path to a VFS path using `/` separators.
///
/// Like [`vfs_join`] but for joining two paths instead of a single component.
/// `rel` must not be absolute.
pub(crate) fn vfs_append(base: &Path, rel: &Path) -> PathBuf {
    debug_assert!(!vfs_path_is_absolute(rel));
    let mut s = base.to_string_lossy().into_owned();
    let rel = rel.to_string_lossy();
    let rel = rel.trim_start_matches('/');
    if rel.is_empty() {
        return PathBuf::from(s);
    }
    if !s.ends_with('/') {
        s.push('/');
    }
    s.push_str(rel);
    PathBuf::from(s)
}

/// Normalize an absolute VFS path: resolve `.` and `..`, strip trailing
/// slashes, reject empty and non-absolute paths. Splits on `/` only — `\` is
/// an ordinary filename character (see [`vfs_normalize`] for why
/// `Path::components()` must not be used on VFS paths).
pub(crate) fn vfs_normalize_checked(path: &Path) -> Result<PathBuf, VfsError> {
    let s = path.to_str().unwrap_or("");
    if s.is_empty() {
        return Err(VfsError::InvalidPath("empty path".into()));
    }
    if !vfs_path_is_absolute(path) {
        return Err(VfsError::InvalidPath(format!(
            "path must be absolute: {}",
            path.display()
        )));
    }
    let mut parts: Vec<&str> = Vec::new();
    for seg in s.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    Ok(PathBuf::from(format!("/{}", parts.join("/"))))
}

/// Resolve a possibly-relative VFS path string against a cwd string, using `/`
/// separators only (never the host separator).
pub(crate) fn vfs_resolve(cwd: &str, path: &str) -> PathBuf {
    if path.starts_with('/') {
        PathBuf::from(path)
    } else {
        let mut s = cwd.to_string();
        if !s.ends_with('/') {
            s.push('/');
        }
        s.push_str(path);
        PathBuf::from(s)
    }
}

/// Resolve `.` and `..` in a VFS path without filesystem access. Splits on `/`
/// only — `\` is an ordinary filename character. Preserves a relative result
/// for relative input.
pub(crate) fn vfs_normalize(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    let absolute = s.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in s.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if !parts.is_empty() {
                    parts.pop();
                }
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    PathBuf::from(if absolute {
        format!("/{joined}")
    } else {
        joined
    })
}

/// Derive a Unix-style permission mode from host file metadata.
///
/// On Windows there is no execute bit and only a read-only attribute, so we
/// report an optimistic MSYS-like mapping: everything is readable and
/// executable, and only the read-only attribute clears the write bits.
/// Falsely-permissive modes are safe — VFS operations are never gated on mode
/// bits — while falsely-restrictive ones would mislead `test -x`/`test -w`.
#[cfg(feature = "native-fs")]
pub(crate) fn unix_mode_from_metadata(meta: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode()
    }
    #[cfg(windows)]
    {
        if meta.permissions().readonly() {
            0o555
        } else {
            0o755
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        0o755
    }
}

/// Metadata for a filesystem node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub node_type: NodeType,
    pub size: u64,
    pub mode: u32,
    pub mtime: SystemTime,
    pub file_id: u64,
}

/// The type of a filesystem node (without content).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    File,
    Directory,
    Symlink,
}

/// An entry returned by `readdir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub node_type: NodeType,
}

/// In-memory representation of a filesystem node.
#[derive(Debug, Clone)]
pub enum FsNode {
    File {
        content: Vec<u8>,
        mode: u32,
        mtime: SystemTime,
        file_id: u64,
    },
    Directory {
        children: std::collections::BTreeMap<String, FsNode>,
        mode: u32,
        mtime: SystemTime,
    },
    Symlink {
        target: PathBuf,
        mtime: SystemTime,
    },
}

/// Options that modify glob expansion behavior.
#[derive(Debug, Clone)]
pub struct GlobOptions {
    /// Include dot-files even when the pattern doesn't start with `.`.
    pub dotglob: bool,
    /// Use case-insensitive matching for filenames.
    pub nocaseglob: bool,
    /// Treat `**` as recursive directory match (globstar).
    /// When false, `**` is treated as `*`.
    pub globstar: bool,
    /// Enable extended glob patterns: `@(...)`, `+(...)`, `*(...)`, `?(...)`, `!(...)`.
    pub extglob: bool,
    /// When true (default), `.` and `..` are excluded from glob results.
    pub globskipdots: bool,
}

impl Default for GlobOptions {
    fn default() -> Self {
        Self {
            dotglob: false,
            nocaseglob: false,
            globstar: false,
            extglob: false,
            globskipdots: true,
        }
    }
}

/// Trait abstracting all filesystem operations.
///
/// All methods take `&self` — implementations use interior mutability.
/// All paths are expected to be absolute.
pub trait VirtualFs: Send + Sync {
    // File CRUD
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, VfsError>;
    fn write_file(&self, path: &Path, content: &[u8]) -> Result<(), VfsError>;
    fn append_file(&self, path: &Path, content: &[u8]) -> Result<(), VfsError>;
    fn remove_file(&self, path: &Path) -> Result<(), VfsError>;

    // Directory operations
    fn mkdir(&self, path: &Path) -> Result<(), VfsError>;
    fn mkdir_p(&self, path: &Path) -> Result<(), VfsError>;
    fn readdir(&self, path: &Path) -> Result<Vec<DirEntry>, VfsError>;
    fn remove_dir(&self, path: &Path) -> Result<(), VfsError>;
    fn remove_dir_all(&self, path: &Path) -> Result<(), VfsError>;

    // Metadata and permissions
    fn exists(&self, path: &Path) -> bool;
    fn stat(&self, path: &Path) -> Result<Metadata, VfsError>;
    fn lstat(&self, path: &Path) -> Result<Metadata, VfsError>;
    fn chmod(&self, path: &Path, mode: u32) -> Result<(), VfsError>;
    fn utimes(&self, path: &Path, mtime: SystemTime) -> Result<(), VfsError>;

    // Links
    fn symlink(&self, target: &Path, link: &Path) -> Result<(), VfsError>;
    fn hardlink(&self, src: &Path, dst: &Path) -> Result<(), VfsError>;
    fn readlink(&self, path: &Path) -> Result<PathBuf, VfsError>;

    // Path resolution
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, VfsError>;

    // File operations
    fn copy(&self, src: &Path, dst: &Path) -> Result<(), VfsError>;
    fn rename(&self, src: &Path, dst: &Path) -> Result<(), VfsError>;

    // Glob expansion (stub for now)
    fn glob(&self, pattern: &str, cwd: &Path) -> Result<Vec<PathBuf>, VfsError>;

    /// Glob expansion with shopt-controlled options (dotglob, nocaseglob, globstar).
    ///
    /// The default implementation ignores options and delegates to `glob()`.
    /// Override in backends that can honor the options.
    fn glob_with_opts(
        &self,
        pattern: &str,
        cwd: &Path,
        _opts: &GlobOptions,
    ) -> Result<Vec<PathBuf>, VfsError> {
        self.glob(pattern, cwd)
    }

    /// Create an independent deep copy for subshell isolation.
    ///
    /// Subshells `( ... )` and command substitutions `$(...)` need an isolated
    /// filesystem so their mutations don't leak back to the parent. Each backend
    /// decides what "independent copy" means:
    /// - InMemoryFs: clones the entire tree
    /// - OverlayFs: clones the upper layer and whiteouts; lower is shared
    /// - MountableFs: recursively deep-clones each mount
    fn deep_clone(&self) -> Arc<dyn VirtualFs>;
}
