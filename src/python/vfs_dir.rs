//! WASI preview1 filesystem bridge: a directory handle over `VirtualFs`.
//!
//! `VfsDir` implements wasi-common's `WasiDir` trait, letting a sandboxed
//! CPython guest operate on our virtual filesystem (typically a shared
//! `Arc<OverlayFs>`) as its preopened root. All paths seen by this trait are
//! relative to the directory the handle points at; they are resolved to
//! absolute VFS paths via `vfs_resolve`.
//!
//! Symlink policy: VFS semantics. The preopen *is* the whole workspace, so
//! WASI's escape-prevention is moot; symlinks resolve exactly as they do for
//! bash (see docs/design/python-sandbox-shared-fs.md §3.4).

use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use wasi_common::dir::{OpenResult, ReaddirCursor, ReaddirEntity, WasiDir};
use wasi_common::file::{FdFlags, FileType, Filestat, OFlags};
use wasi_common::snapshots::preview_1::error::Errno;
use wasi_common::{Error, ErrorExt, SystemTimeSpec};

use crate::error::VfsError;
use crate::vfs::{Metadata, NodeType, VirtualFs, vfs_resolve};

use super::vfs_file::VfsFile;

/// Map a `VfsError` onto the closest WASI errno.
pub(crate) fn map_err(e: VfsError) -> Error {
    let errno = match &e {
        VfsError::NotFound(_) => Errno::Noent,
        VfsError::AlreadyExists(_) => Errno::Exist,
        VfsError::NotADirectory(_) | VfsError::NotAFile(_) => Errno::Notdir,
        VfsError::IsADirectory(_) => Errno::Isdir,
        VfsError::PermissionDenied(_) => Errno::Acces,
        VfsError::DirectoryNotEmpty(_) => Errno::Notempty,
        VfsError::SymlinkLoop(_) => Errno::Loop,
        VfsError::InvalidPath(_) => Errno::Inval,
        VfsError::IoError(_) => Errno::Io,
    };
    Error::from(errno).context(e.to_string())
}

pub(crate) fn map_filetype(t: NodeType) -> FileType {
    match t {
        NodeType::File => FileType::RegularFile,
        NodeType::Directory => FileType::Directory,
        NodeType::Symlink => FileType::SymbolicLink,
    }
}

/// Resolve a WASI `SystemTimeSpec` to a concrete `SystemTime` (`SymbolicNow`
/// = wall-clock now). Returns `None` when no time was specified.
pub(crate) fn resolve_time_spec(spec: Option<SystemTimeSpec>) -> Option<std::time::SystemTime> {
    match spec {
        Some(SystemTimeSpec::SymbolicNow) => Some(std::time::SystemTime::now()),
        Some(SystemTimeSpec::Absolute(t)) => Some(t.into_std()),
        None => None,
    }
}

pub(crate) fn map_metadata(m: &Metadata, filetype: FileType) -> Filestat {
    Filestat {
        device_id: 0,
        inode: m.file_id,
        filetype,
        nlink: 1,
        size: m.size,
        atim: Some(m.mtime),
        mtim: Some(m.mtime),
        ctim: Some(m.mtime),
    }
}

/// A `WasiDir` rooted at an absolute path inside a `VirtualFs`.
pub(crate) struct VfsDir {
    fs: Arc<dyn VirtualFs>,
    /// Absolute VFS path of this directory.
    path: PathBuf,
    /// Optional caller-set file size cap (see `VfsFile`).
    max_file_size: Option<u64>,
}

impl VfsDir {
    /// A directory handle at `path` (absolute VFS path, typically `/` for the
    /// workspace preopen).
    pub fn new(fs: Arc<dyn VirtualFs>, path: &Path, max_file_size: Option<u64>) -> Self {
        Self {
            fs,
            path: path.to_path_buf(),
            max_file_size,
        }
    }

    fn resolve(&self, relative: &str) -> PathBuf {
        let base = self
            .path
            .to_str()
            .expect("VFS paths are built from UTF-8 strings");
        vfs_resolve(base, relative)
    }

    fn stat_follow(&self, path: &Path, follow: bool) -> Result<Metadata, VfsError> {
        if follow {
            self.fs.stat(path)
        } else {
            self.fs.lstat(path)
        }
    }
}

#[async_trait::async_trait]
impl WasiDir for VfsDir {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn open_file(
        &self,
        symlink_follow: bool,
        path: &str,
        oflags: OFlags,
        read: bool,
        write: bool,
        fdflags: FdFlags,
    ) -> Result<OpenResult, Error> {
        if oflags.contains(OFlags::DIRECTORY)
            && oflags.intersects(OFlags::CREATE | OFlags::EXCLUSIVE | OFlags::TRUNCATE)
        {
            return Err(Error::invalid_argument()
                .context("DIRECTORY combined with CREATE/EXCLUSIVE/TRUNCATE"));
        }
        if fdflags.intersects(FdFlags::DSYNC | FdFlags::SYNC | FdFlags::RSYNC) {
            return Err(Error::not_supported().context("SYNC family of FdFlags"));
        }
        let full = self.resolve(path);
        let meta = self.stat_follow(&full, symlink_follow);

        match meta {
            Ok(m) if m.node_type == NodeType::Symlink => {
                // Only reachable with symlink_follow=false (lstat): opening a
                // symlink with O_NOFOLLOW must fail, never open the target.
                Err(Error::from(Errno::Loop).context(full.display().to_string()))
            }
            Ok(m) if m.node_type == NodeType::Directory => {
                if oflags.contains(OFlags::EXCLUSIVE) {
                    return Err(Error::exist().context(full.display().to_string()));
                }
                if write {
                    return Err(Error::from(Errno::Isdir).context(full.display().to_string()));
                }
                Ok(OpenResult::Dir(Box::new(VfsDir::new(
                    self.fs.clone(),
                    &full,
                    self.max_file_size,
                ))))
            }
            Ok(m) => {
                if oflags.contains(OFlags::DIRECTORY) {
                    return Err(Error::not_dir().context(full.display().to_string()));
                }
                if oflags.contains(OFlags::EXCLUSIVE) {
                    return Err(Error::exist().context(full.display().to_string()));
                }
                let mut cursor = 0;
                if oflags.contains(OFlags::TRUNCATE) && write {
                    self.fs.write_file(&full, b"").map_err(map_err)?;
                } else if fdflags.contains(FdFlags::APPEND) {
                    cursor = m.size;
                }
                Ok(OpenResult::File(Box::new(VfsFile::new(
                    self.fs.clone(),
                    full,
                    read,
                    write,
                    fdflags.contains(FdFlags::APPEND),
                    cursor,
                    self.max_file_size,
                ))))
            }
            Err(VfsError::NotFound(_)) => {
                if !oflags.contains(OFlags::CREATE) {
                    return Err(map_err(VfsError::NotFound(full)));
                }
                self.fs.write_file(&full, b"").map_err(map_err)?;
                Ok(OpenResult::File(Box::new(VfsFile::new(
                    self.fs.clone(),
                    full,
                    read,
                    write,
                    fdflags.contains(FdFlags::APPEND),
                    0,
                    self.max_file_size,
                ))))
            }
            Err(e) => Err(map_err(e)),
        }
    }

    async fn create_dir(&self, path: &str) -> Result<(), Error> {
        self.fs.mkdir(&self.resolve(path)).map_err(map_err)
    }

    async fn readdir(
        &self,
        cursor: ReaddirCursor,
    ) -> Result<Box<dyn Iterator<Item = Result<ReaddirEntity, Error>> + Send>, Error> {
        let entries = self.fs.readdir(&self.path).map_err(map_err)?;
        let start: u64 = cursor.into();

        // wasi-common's reference impl includes "." and ".."; wasi-libc's
        // readdir(3) expects them. Cookies are indices into this ordering.
        let parent = self.path.parent().unwrap_or(&self.path).to_path_buf();
        let mut all: Vec<(String, FileType, u64)> = Vec::with_capacity(entries.len() + 2);
        for (name, dir) in [(".".to_string(), &self.path), ("..".to_string(), &parent)] {
            // inode 0 is the getdents "deleted entry" convention; wasi-libc
            // tolerates it, and CPython never reads d_ino. Only hit if stat
            // of an entry that readdir just listed fails (shouldn't happen).
            let inode = self.fs.stat(dir).map(|m| m.file_id).unwrap_or(0);
            all.push((name, FileType::Directory, inode));
        }
        for e in entries {
            let child = crate::vfs::vfs_join(&self.path, &e.name);
            let inode = self.fs.stat(&child).map(|m| m.file_id).unwrap_or(0); // see note above
            all.push((e.name, map_filetype(e.node_type), inode));
        }

        let entities: Vec<Result<ReaddirEntity, Error>> = all
            .into_iter()
            .enumerate()
            .skip(start as usize)
            .map(|(i, (name, filetype, inode))| {
                Ok(ReaddirEntity {
                    next: ReaddirCursor::from(i as u64 + 1),
                    inode,
                    name,
                    filetype,
                })
            })
            .collect();
        Ok(Box::new(entities.into_iter()))
    }

    async fn symlink(&self, old_path: &str, new_path: &str) -> Result<(), Error> {
        // WASI path_symlink(old_path, dirfd, new_path): create a symlink AT
        // new_path (relative to this dir) pointing to old_path (kept as-is,
        // interpreted by readers; VFS symlink semantics apply).
        self.fs
            .symlink(Path::new(old_path), &self.resolve(new_path))
            .map_err(map_err)
    }

    async fn remove_dir(&self, path: &str) -> Result<(), Error> {
        self.fs.remove_dir(&self.resolve(path)).map_err(map_err)
    }

    async fn unlink_file(&self, path: &str) -> Result<(), Error> {
        self.fs.remove_file(&self.resolve(path)).map_err(map_err)
    }

    async fn read_link(&self, path: &str) -> Result<PathBuf, Error> {
        self.fs.readlink(&self.resolve(path)).map_err(map_err)
    }

    async fn get_filestat(&self) -> Result<Filestat, Error> {
        let m = self.fs.stat(&self.path).map_err(map_err)?;
        Ok(map_metadata(&m, FileType::Directory))
    }

    async fn get_path_filestat(
        &self,
        path: &str,
        follow_symlinks: bool,
    ) -> Result<Filestat, Error> {
        let full = self.resolve(path);
        let m = self.stat_follow(&full, follow_symlinks).map_err(map_err)?;
        Ok(map_metadata(&m, map_filetype(m.node_type)))
    }

    async fn rename(
        &self,
        path: &str,
        dest_dir: &dyn WasiDir,
        dest_path: &str,
    ) -> Result<(), Error> {
        let dest = dest_dir
            .as_any()
            .downcast_ref::<VfsDir>()
            .filter(|d| Arc::ptr_eq(&d.fs, &self.fs))
            .ok_or_else(|| Error::from(Errno::Xdev).context("rename across filesystems"))?;
        self.fs
            .rename(&self.resolve(path), &dest.resolve(dest_path))
            .map_err(map_err)
    }

    async fn hard_link(
        &self,
        path: &str,
        target_dir: &dyn WasiDir,
        target_path: &str,
    ) -> Result<(), Error> {
        let dest = target_dir
            .as_any()
            .downcast_ref::<VfsDir>()
            .filter(|d| Arc::ptr_eq(&d.fs, &self.fs))
            .ok_or_else(|| Error::from(Errno::Xdev).context("hard link across filesystems"))?;
        self.fs
            .hardlink(&self.resolve(path), &dest.resolve(target_path))
            .map_err(map_err)
    }

    async fn set_times(
        &self,
        path: &str,
        atime: Option<SystemTimeSpec>,
        mtime: Option<SystemTimeSpec>,
        follow_symlinks: bool,
    ) -> Result<(), Error> {
        let _ = (atime, follow_symlinks);
        let full = self.resolve(path);
        let Some(mtime) = resolve_time_spec(mtime) else {
            return Ok(());
        };
        self.fs.utimes(&full, mtime).map_err(map_err)
    }
}
