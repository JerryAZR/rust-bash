//! WASI preview1 file handle over `VirtualFs`: per-fd cursor and flags on top
//! of the VFS's whole-file operations.
//!
//! Positional reads/writes are read-modify-write at the VFS level. At sandbox
//! scale (agent-sized files, sequential agent turns) this is fine; it is
//! documented as non-atomic for concurrent writers in
//! docs/design/python-sandbox-shared-fs.md §3.4.

use std::any::Any;
use std::io::{IoSlice, IoSliceMut};
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use wasi_common::file::{FdFlags, FileType, Filestat, WasiFile};
use wasi_common::snapshots::preview_1::error::Errno;
use wasi_common::{Error, ErrorExt, SystemTimeSpec};

use crate::error::VfsError;

use crate::vfs::VirtualFs;

use super::vfs_dir::{map_err, map_metadata, resolve_time_spec};

/// An open regular file on a `VirtualFs`, with its own cursor.
pub(crate) struct VfsFile {
    fs: Arc<dyn VirtualFs>,
    path: PathBuf,
    writable: bool,
    append: bool,
    cursor: Mutex<u64>,
    /// Optional caller-set size cap; `None` = bounded only by available
    /// memory (allocation failures surface as `EFBIG`, never host aborts).
    max_file_size: Option<u64>,
}

impl VfsFile {
    pub(crate) fn new(
        fs: Arc<dyn VirtualFs>,
        path: PathBuf,
        writable: bool,
        append: bool,
        cursor: u64,
        max_file_size: Option<u64>,
    ) -> Self {
        Self {
            fs,
            path,
            writable,
            append,
            cursor: Mutex::new(cursor),
            max_file_size,
        }
    }

    fn read_all(&self) -> Result<Vec<u8>, Error> {
        self.fs.read_file(&self.path).map_err(map_err)
    }

    /// Copy `src` from `offset` into `bufs` (std `Read::read_vectored` on
    /// the remaining slice: fills buffers sequentially until exhausted).
    fn fill_bufs(src: &[u8], offset: u64, bufs: &mut [IoSliceMut<'_>]) -> u64 {
        use std::io::Read;
        let mut rest = src.get(offset as usize..).unwrap_or(&[]);
        rest.read_vectored(bufs)
            .expect("slice reads are infallible") as u64
    }

    /// Splice `bufs` into the file at `offset`, extending with zeros if the
    /// offset is past EOF (POSIX semantics), then write back.
    fn splice(&self, offset: u64, bufs: &[IoSlice<'_>]) -> Result<u64, Error> {
        let total: u64 = bufs.iter().map(|b| b.len() as u64).sum();
        let end = offset
            .checked_add(total)
            .filter(|end| self.max_file_size.is_none_or(|cap| *end <= cap))
            .ok_or_else(|| Error::from(Errno::Fbig).context("write beyond maximum file size"))?;
        // A missing file reads as empty; every other read error must
        // propagate (treating it as empty would silently truncate).
        let mut content = match self.fs.read_file(&self.path) {
            Ok(c) => c,
            Err(VfsError::NotFound(_)) => Vec::new(),
            Err(e) => return Err(map_err(e)),
        };
        // Reserve up front: an allocation failure is an `EFBIG` error here,
        // never a host abort.
        content
            .try_reserve(end as usize)
            .map_err(|_| Error::from(Errno::Fbig).context("file too large to materialize"))?;
        let mut pos = offset as usize;
        for buf in bufs {
            let buf_end = pos + buf.len();
            if buf_end > content.len() {
                content.resize(buf_end, 0);
            }
            content[pos..buf_end].copy_from_slice(buf);
            pos = buf_end;
        }
        debug_assert_eq!(pos as u64, end);
        self.fs.write_file(&self.path, &content).map_err(map_err)?;
        Ok(total)
    }
}

#[async_trait::async_trait]
impl WasiFile for VfsFile {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn get_filetype(&self) -> Result<FileType, Error> {
        Ok(FileType::RegularFile)
    }

    async fn get_fdflags(&self) -> Result<FdFlags, Error> {
        let mut flags = FdFlags::empty();
        if self.append {
            flags |= FdFlags::APPEND;
        }
        Ok(flags)
    }

    async fn set_fdflags(&mut self, flags: FdFlags) -> Result<(), Error> {
        if flags.intersects(FdFlags::DSYNC | FdFlags::SYNC | FdFlags::RSYNC) {
            // Same policy as open_file: never silently ignore a
            // guest-requested durability flag.
            return Err(Error::not_supported().context("SYNC family of FdFlags"));
        }
        // Only APPEND is mutable after open (POSIX fcntl F_SETFL semantics).
        self.append = flags.contains(FdFlags::APPEND);
        Ok(())
    }

    async fn get_filestat(&self) -> Result<Filestat, Error> {
        // Divergence from POSIX: fstat on an open-but-since-unlinked file
        // fails NOENT here (POSIX fstat works on the open fd). Harmless for
        // stdlib glue; the VFS is path-addressed, not inode-addressed.
        let m = self.fs.stat(&self.path).map_err(map_err)?;
        Ok(map_metadata(&m, FileType::RegularFile))
    }

    async fn set_filestat_size(&self, size: u64) -> Result<(), Error> {
        if !self.writable {
            return Err(Error::badf().context("file not opened for writing"));
        }
        if self.max_file_size.is_some_and(|cap| size > cap) {
            return Err(Error::from(Errno::Fbig).context("truncate beyond maximum file size"));
        }
        let mut content = self.fs.read_file(&self.path).map_err(map_err)?;
        content
            .try_reserve(size as usize)
            .map_err(|_| Error::from(Errno::Fbig).context("file too large to materialize"))?;
        content.resize(size as usize, 0);
        self.fs.write_file(&self.path, &content).map_err(map_err)
    }

    async fn advise(
        &self,
        _offset: u64,
        _len: u64,
        _advice: wasi_common::file::Advice,
    ) -> Result<(), Error> {
        Ok(())
    }

    async fn set_times(
        &self,
        atime: Option<SystemTimeSpec>,
        mtime: Option<SystemTimeSpec>,
    ) -> Result<(), Error> {
        let _ = atime;
        let Some(mtime) = resolve_time_spec(mtime) else {
            return Ok(());
        };
        self.fs.utimes(&self.path, mtime).map_err(map_err)
    }

    async fn read_vectored<'a>(&self, bufs: &mut [IoSliceMut<'a>]) -> Result<u64, Error> {
        let content = self.read_all()?;
        let mut cursor = self.cursor.lock();
        let n = Self::fill_bufs(&content, *cursor, bufs);
        *cursor += n;
        Ok(n)
    }

    async fn read_vectored_at<'a>(
        &self,
        bufs: &mut [IoSliceMut<'a>],
        offset: u64,
    ) -> Result<u64, Error> {
        let content = self.read_all()?;
        Ok(Self::fill_bufs(&content, offset, bufs))
    }

    async fn write_vectored<'a>(&self, bufs: &[IoSlice<'a>]) -> Result<u64, Error> {
        let offset = if self.append {
            self.fs.stat(&self.path).map_err(map_err)?.size
        } else {
            *self.cursor.lock()
        };
        let n = self.splice(offset, bufs)?;
        // POSIX: the file offset is EOF after every O_APPEND write.
        *self.cursor.lock() = offset + n;
        Ok(n)
    }

    async fn write_vectored_at<'a>(&self, bufs: &[IoSlice<'a>], offset: u64) -> Result<u64, Error> {
        self.splice(offset, bufs)
    }

    async fn seek(&self, pos: std::io::SeekFrom) -> Result<u64, Error> {
        let mut cursor = self.cursor.lock();
        let new: i128 = match pos {
            std::io::SeekFrom::Start(n) => n as i128,
            std::io::SeekFrom::Current(d) => *cursor as i128 + d as i128,
            std::io::SeekFrom::End(d) => {
                let size = self.fs.stat(&self.path).map_err(map_err)?.size;
                size as i128 + d as i128
            }
        };
        if new < 0 || new > i64::MAX as i128 {
            return Err(Error::invalid_argument().context("seek offset out of range"));
        }
        *cursor = new as u64;
        Ok(*cursor)
    }

    fn num_ready_bytes(&self) -> Result<u64, Error> {
        let size = self.fs.stat(&self.path).map_err(map_err)?.size;
        Ok(size.saturating_sub(*self.cursor.lock()))
    }

    async fn readable(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn writable(&self) -> Result<(), Error> {
        Ok(())
    }
}
