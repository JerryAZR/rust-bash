//! OverlayFs — copy-on-write filesystem backed by a real directory (lower)
//! and an in-memory write layer (upper).
//!
//! The upper layer is an explicit tree ([`OverlayTree`]) holding content
//! nodes and whiteout leaf nodes, so the load-bearing invariants are
//! structural (a path is either content or whiteout; whiteouts never nest;
//! resurrection re-hides deleted lower children inside `ensure_dirs`).
//!
//! Reads resolve through: upper tree → lower (whiteout nodes block
//! fall-through). Writes always go to the upper tree. The lower directory
//! is never modified.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::platform::SystemTime;

use parking_lot::RwLock;

use super::overlay_tree::{Descend, OverlayTree, UpperNode};
use super::{DirEntry, Metadata, NodeType, VirtualFs};
use crate::error::VfsError;
use crate::interpreter::pattern::glob_match;

const MAX_SYMLINK_DEPTH: u32 = 40;

/// A copy-on-write filesystem: reads from a real directory, writes to memory.
///
/// The lower layer (a real directory on disk) is treated as read-only.
/// All mutations go to the upper tree. Deletions are whiteout leaf nodes.
///
/// # Example
///
/// ```ignore
/// use rust_bash::{RustBashBuilder, OverlayFs};
/// use std::sync::Arc;
///
/// let overlay = OverlayFs::new("./my_project").unwrap();
/// let mut shell = RustBashBuilder::new()
///     .fs(Arc::new(overlay))
///     .cwd("/")
///     .build()
///     .unwrap();
///
/// let result = shell.exec("cat /src/main.rs").unwrap(); // reads from disk
/// shell.exec("echo new > /src/main.rs").unwrap();       // writes to memory only
/// ```
pub struct OverlayFs {
    lower: PathBuf,
    tree: RwLock<OverlayTree>,
}

/// Where a path resolved to during layer lookup.
enum LayerResult {
    Whiteout,
    Upper,
    Lower,
    NotFound,
}

/// A single write captured in the overlay's in-memory upper layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayWrite {
    /// VFS path of the written entry.
    pub path: PathBuf,
    /// Kind of entry written.
    pub node_type: NodeType,
    /// File content; the symlink target (as UTF-8 bytes) for symlinks; empty
    /// for directories.
    pub content: Vec<u8>,
    /// Unix-style permission mode to apply when materializing on the host.
    pub mode: u32,
}

/// All changes recorded in an overlay relative to its lower directory:
/// the upper-layer write set plus the deletions (whiteouts) of lower paths.
///
/// Hosts embedding rust-bash use this to apply sandboxed writes to the real
/// project directory after execution (or prompt about them) — the overlay
/// itself never modifies disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayDiff {
    /// Every entry created or modified in the upper layer, sorted by path.
    pub writes: Vec<OverlayWrite>,
    /// Lower-layer paths deleted during execution (top-most whiteouts only),
    /// sorted by path.
    pub deletions: Vec<PathBuf>,
}

impl OverlayFs {
    /// Create an overlay filesystem with `lower` as the read-only base.
    ///
    /// The lower directory must exist and be a directory. It is canonicalized
    /// on construction so symlinks in the lower path itself are resolved once.
    pub fn new(lower: impl Into<PathBuf>) -> std::io::Result<Self> {
        let lower = lower.into();
        if !lower.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                format!("{} is not a directory", lower.display()),
            ));
        }
        let lower = lower.canonicalize()?;
        Ok(Self {
            lower,
            tree: RwLock::new(OverlayTree::new()),
        })
    }

    /// Test-only: is there an upper node (content or whiteout) at `path`?
    #[cfg(test)]
    pub(crate) fn tree_contains(&self, path: &Path) -> bool {
        matches!(self.tree.read().descend(path), Descend::Found(_))
    }

    /// Return all changes recorded in this overlay since construction: every
    /// write captured in the upper layer and every deletion of a lower-layer
    /// path.
    ///
    /// Deletions are the whiteout leaf set (top-most by construction —
    /// whiteouts never nest) minus stale markers whose disk path is gone.
    /// Writes to paths that only ever existed in the upper layer are
    /// reported as writes; their removal does not appear in `deletions`
    /// since there is nothing on disk to delete.
    pub fn diff(&self) -> OverlayDiff {
        let mut writes = Vec::new();
        let mut deletions = Vec::new();
        // Materialize owned entries under a scoped guard: the disk probes
        // below must not run with a read guard held (a writer would block
        // for the whole disk scan, and the lock discipline stays
        // structural rather than true-by-luck).
        let entries: Vec<(PathBuf, UpperNode)> = {
            let tree = self.tree.read();
            tree.walk()
                .into_iter()
                .map(|(p, n)| (p, n.clone()))
                .collect()
        };
        for (path, node) in &entries {
            match node {
                UpperNode::File { content, mode, .. } => writes.push(OverlayWrite {
                    path: path.clone(),
                    node_type: NodeType::File,
                    content: content.clone(),
                    mode: *mode,
                }),
                UpperNode::Dir { children, mode, .. } => {
                    // A directory is a write iff it is user-created content:
                    // completely empty (mkdir result) or holding content
                    // beneath it. A dir whose subtree is ALL whiteouts is
                    // put_whiteout scaffolding — nothing to materialize.
                    let scaffold = !children.is_empty() && !subtree_has_content(children);
                    if !scaffold {
                        writes.push(OverlayWrite {
                            path: path.clone(),
                            node_type: NodeType::Directory,
                            content: Vec::new(),
                            mode: *mode,
                        });
                    }
                }
                UpperNode::Symlink { target, .. } => writes.push(OverlayWrite {
                    path: path.clone(),
                    node_type: NodeType::Symlink,
                    content: target.to_string_lossy().into_owned().into_bytes(),
                    mode: 0o777,
                }),
                // Top-most by construction; only lower paths are real
                // deletions (a stale whiteout whose disk path vanished hides
                // nothing).
                UpperNode::Whiteout => {
                    // Stale marker: the disk path vanished out-of-band.
                    if self.lower_exists(path) {
                        deletions.push(path.clone());
                    }
                }
            }
        }
        writes.sort_by(|a, b| a.path.cmp(&b.path));
        deletions.sort();
        OverlayDiff { writes, deletions }
    }

    /// Reconcile the overlay with current disk state: drop every upper-layer
    /// shadow that now matches the lower directory byte-for-byte, and clear
    /// whiteouts whose disk paths no longer exist. Entries that still differ
    /// remain pending and keep appearing in [`OverlayFs::diff`].
    ///
    /// After the host applies writes (or a native run rewrites files),
    /// `sync()` leaves the overlay holding "exactly the differences from
    /// disk right now": applied writes and deletions disappear, failed or
    /// conflicting ones stay visible — no per-path bookkeeping needed. Mode
    /// bits are not compared (they are advisory on Windows).
    pub fn sync(&self) {
        // NOTE: single-writer assumption — the probe phase and the detach
        // phase are not atomic against a concurrent mutation (a path that
        // stops matching disk between phases would be wrongly detached).
        // Callers are agent harnesses applying diffs between runs.
        //
        // Phase 1 (scoped read guard): snapshot the tree.
        let snapshot: Vec<(PathBuf, UpperNode)> = {
            let tree = self.tree.read();
            tree.walk_post()
                .into_iter()
                .map(|(p, n)| (p, n.clone()))
                .collect()
        };
        // Phase 2 (guard-free): disk probes.
        let entries: Vec<(PathBuf, bool, bool)> = snapshot
            .iter()
            .map(|(p, n)| {
                let is_dir = matches!(n, UpperNode::Dir { .. });
                let matches_disk = match n {
                    UpperNode::File { content, .. } => self.file_matches_disk(p, content),
                    UpperNode::Dir { .. } => self.dir_exists_on_disk(p),
                    UpperNode::Symlink { target, .. } => self.symlink_matches_disk(p, target),
                    // Whiteout: stale (deletion applied or path vanished
                    // out-of-band) when the disk path is gone.
                    UpperNode::Whiteout => !self.lower_exists(p),
                };
                (p.clone(), is_dir, matches_disk)
            })
            .collect();

        let mut tree = self.tree.write();
        for (path, is_dir, matches_disk) in entries {
            if !matches_disk {
                continue;
            }
            if is_dir {
                // A directory drops only once it is empty: post-order means
                // matching children were already detached, and a surviving
                // descendant implies a surviving direct child. O(depth)
                // descend instead of a full-tree scan per directory.
                let has_children = matches!(
                    tree.descend(&path),
                    Descend::Found(UpperNode::Dir { children, .. }) if !children.is_empty()
                );
                if has_children {
                    continue;
                }
            }
            tree.detach(&path);
        }
    }

    /// Discard all pending state: clear the upper tree so the overlay
    /// re-baselines on current disk state.
    ///
    /// Unlike [`Self::sync`], which keeps entries that genuinely differ from
    /// disk, `reset` deliberately forgets them — e.g. after a native run
    /// whose disk changes should win over the sandbox's pending opinions.
    pub fn reset(&self) {
        self.tree.write().clear();
    }

    /// True when the disk file at `path` is byte-identical to `content`.
    fn file_matches_disk(&self, path: &Path, content: &[u8]) -> bool {
        match self.lstat_lower(path) {
            Ok(meta) if meta.node_type == NodeType::File => {}
            _ => return false,
        }
        match self.read_lower_file(path) {
            Ok(disk) => disk == content,
            // Read failure after a successful lstat: permission errors or an
            // OS race where the file disappears mid-check.
            Err(_) => false,
        }
    }

    /// True when a directory exists at `path` on disk.
    fn dir_exists_on_disk(&self, path: &Path) -> bool {
        self.lstat_lower(path)
            .is_ok_and(|m| m.node_type == NodeType::Directory)
    }

    /// True when the disk symlink at `path` has the same target.
    fn symlink_matches_disk(&self, path: &Path, target: &Path) -> bool {
        match self.lstat_lower(path) {
            Ok(meta) if meta.node_type == NodeType::Symlink => {}
            _ => return false,
        }
        match self.readlink_lower(path) {
            Ok(disk) => disk == target,
            Err(_) => false,
        }
    }

    // ------------------------------------------------------------------
    // Layer checks (no symlink following)
    // ------------------------------------------------------------------

    /// Content node exists in the upper tree at exactly `path`.
    fn upper_has_entry(&self, path: &Path) -> bool {
        self.tree.read().descend(path).content().is_some()
    }

    /// The path is hidden by a whiteout (at or above it).
    fn is_whiteout(&self, path: &Path) -> bool {
        self.tree.read().descend(path).is_blocked_or_whiteout()
    }

    /// Determine which layer `path` lives in (after normalization).
    fn resolve_layer(&self, path: &Path) -> LayerResult {
        let upper = {
            let tree = self.tree.read();
            match tree.descend(path) {
                Descend::Found(node) if node.is_content() => Some(LayerResult::Upper),
                Descend::Found(UpperNode::Whiteout) | Descend::Blocked => {
                    Some(LayerResult::Whiteout)
                }
                // A file/symlink mid-path hides everything beneath it,
                // whatever the lower holds at the literal path.
                Descend::NotDir => Some(LayerResult::NotFound),
                Descend::Found(_) => None, // unreachable: whiteout covered
                Descend::Missing => None,
            }
        };
        match upper {
            Some(layer) => layer,
            None => {
                if self.lower_exists(path) {
                    LayerResult::Lower
                } else {
                    LayerResult::NotFound
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Lower-layer reading helpers
    // ------------------------------------------------------------------

    /// Map a VFS absolute path to the corresponding real path under `lower`.
    fn lower_path(&self, vfs_path: &Path) -> PathBuf {
        let rel = vfs_path.strip_prefix("/").unwrap_or(vfs_path.as_ref());
        self.lower.join(rel)
    }

    /// Read a file from the lower layer.
    fn read_lower_file(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        let real = self.lower_path(path);
        std::fs::read(&real).map_err(|e| map_io_error(e, path))
    }

    /// Get metadata for a path in the lower layer (follows symlinks).
    fn stat_lower(&self, path: &Path) -> Result<Metadata, VfsError> {
        let real = self.lower_path(path);
        let meta = std::fs::metadata(&real).map_err(|e| map_io_error(e, path))?;
        Ok(map_std_metadata(&meta))
    }

    /// Get metadata for a path in the lower layer (does NOT follow symlinks).
    fn lstat_lower(&self, path: &Path) -> Result<Metadata, VfsError> {
        let real = self.lower_path(path);
        let meta = std::fs::symlink_metadata(&real).map_err(|e| map_io_error(e, path))?;
        Ok(map_std_metadata(&meta))
    }

    /// List entries in a lower-layer directory.
    fn readdir_lower(&self, path: &Path) -> Result<Vec<DirEntry>, VfsError> {
        let real = self.lower_path(path);
        let entries = std::fs::read_dir(&real).map_err(|e| map_io_error(e, path))?;
        let mut result = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| map_io_error(e, path))?;
            let ft = entry.file_type().map_err(|e| map_io_error(e, path))?;
            let node_type = if ft.is_dir() {
                NodeType::Directory
            } else if ft.is_symlink() {
                NodeType::Symlink
            } else {
                NodeType::File
            };
            result.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                node_type,
            });
        }
        Ok(result)
    }

    /// Read a symlink target from the lower layer.
    fn readlink_lower(&self, path: &Path) -> Result<PathBuf, VfsError> {
        let real = self.lower_path(path);
        std::fs::read_link(&real).map_err(|e| map_io_error(e, path))
    }

    /// Check whether a path exists in the lower layer (symlink_metadata).
    fn lower_exists(&self, path: &Path) -> bool {
        let real = self.lower_path(path);
        real.symlink_metadata().is_ok()
    }

    // ------------------------------------------------------------------
    // Tree mutation helpers
    // ------------------------------------------------------------------

    /// Closures for the tree's resurrection machinery.
    fn lower_entries_of(&self, dir: &Path) -> Option<Vec<String>> {
        self.readdir_lower(dir)
            .ok()
            .map(|es| es.into_iter().map(|e| e.name).collect())
    }

    fn lower_mode_of(&self, path: &Path) -> Option<u32> {
        self.stat_lower(path).ok().map(|m| m.mode)
    }

    /// Ensure all components of `path` exist as directories in the upper
    /// tree, resurrecting whiteouts (deleted lower children stay hidden).
    ///
    /// Upper symlinks in the existing prefix are followed first: with
    /// `mkdir /real; ln -s /real /l`, writing `/l/f` must create `/real/f`
    /// (POSIX). Whiteout components stop the following (they are left for
    /// ensure_dirs to resurrect).
    fn ensure_upper_dir_path(&self, path: &Path) -> Result<(), VfsError> {
        let resolved = self.resolve_write_target(path, true)?;
        self.tree
            .write()
            .ensure_dirs(&resolved, &|d| self.lower_entries_of(d), &|p| {
                self.lower_mode_of(p)
            })
    }

    /// Resolve a write target through upper-layer symlinks, POSIX style:
    /// symlinks in every existing component are followed (including the
    /// final component when `follow_final`), chained links are re-scanned,
    /// and `..` in relative targets is resolved lexically so the result is
    /// always a normalized absolute path. Whiteouted or missing components
    /// pass through literally (nothing to follow). Callers MUST attach at
    /// the returned path, not the original.
    fn resolve_write_target(&self, path: &Path, follow_final: bool) -> Result<PathBuf, VfsError> {
        let norm = normalize(path)?;
        let mut segs: Vec<String> = path_components(&norm)
            .into_iter()
            .map(str::to_string)
            .collect();
        let mut resolved: Vec<String> = Vec::new();
        let mut hops = 0u32;
        let mut i = 0;
        while i < segs.len() {
            let candidate = format!(
                "/{}",
                resolved
                    .iter()
                    .chain(std::iter::once(&segs[i]))
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("/")
            );
            let target = {
                let tree = self.tree.read();
                match tree.descend(Path::new(&candidate)) {
                    Descend::Found(UpperNode::Symlink { target, .. }) => Some(target.clone()),
                    _ => None,
                }
            };
            match target {
                Some(target) if follow_final || i + 1 < segs.len() => {
                    hops += 1;
                    if hops > MAX_SYMLINK_DEPTH {
                        return Err(VfsError::SymlinkLoop(path.to_path_buf()));
                    }
                    // Splice: (resolved | target segs) + remaining, lexically
                    // normalized, then re-scan from the start so chains and
                    // substituted `..` are handled. `hops` caps the loop.
                    let mut new_segs: Vec<String> = Vec::new();
                    if !super::vfs_path_is_absolute(&target) {
                        new_segs.extend(resolved.iter().cloned());
                    }
                    new_segs.extend(path_components(&target).into_iter().map(str::to_string));
                    new_segs.extend(segs[i + 1..].iter().cloned());
                    normalize_segments(&mut new_segs);
                    segs = new_segs;
                    resolved.clear();
                    i = 0;
                }
                _ => {
                    resolved.push(segs[i].clone());
                    i += 1;
                }
            }
        }
        Ok(PathBuf::from(format!("/{}", resolved.join("/"))))
    }

    /// Create a file node at `path` (replacing file/symlink/whiteout).
    /// Parents must already exist (call `ensure_upper_dir_path` first).
    fn attach_file(&self, path: &Path, content: Vec<u8>, mode: u32) -> Result<(), VfsError> {
        self.tree.write().attach(
            path,
            UpperNode::File {
                content,
                mode,
                mtime: SystemTime::now(),
            },
        )
    }

    /// Ensure a file is present in the upper tree, copying content and
    /// metadata up from the lower layer if needed.
    fn copy_up_if_needed(&self, path: &Path) -> Result<(), VfsError> {
        if self.upper_has_entry(path) {
            return Ok(());
        }
        debug_assert!(
            !self.is_whiteout(path),
            "copy_up_if_needed called on whiteout-ed path"
        );
        if let Some(parent) = path.parent()
            && parent != Path::new("/")
        {
            self.ensure_upper_dir_path(parent)?;
        }
        let content = self.read_lower_file(path)?;
        let meta = self.stat_lower(path)?;
        self.attach_file(path, content, meta.mode)?;
        self.tree.write().set_mtime(path, meta.mtime)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Merged readdir helper
    // ------------------------------------------------------------------

    /// Merge directory listings from upper and lower, hiding whiteouts.
    /// The path is resolved first: a directory reached THROUGH an upper
    /// symlink has its content at the target (glob walks hand us
    /// unresolved child paths of symlinked dirs).
    fn readdir_merged(&self, path: &Path) -> Result<Vec<DirEntry>, VfsError> {
        let resolved;
        let path = match self.resolve_path(path, true) {
            Ok(r) => {
                resolved = r;
                &resolved
            }
            Err(_) => path,
        };
        let mut entries: std::collections::BTreeMap<String, DirEntry> =
            std::collections::BTreeMap::new();

        // Lower entries first, minus children hidden by whiteout nodes.
        if self.lower_exists(path)
            && let Ok(lower_entries) = self.readdir_lower(path)
        {
            for e in lower_entries {
                let child_path = super::vfs_join(path, &e.name);
                if !self.is_whiteout(&child_path) {
                    entries.insert(e.name.clone(), e);
                }
            }
        }

        // Upper content children override lower entries (dedup by name);
        // whiteout children remove lower entries from the listing.
        {
            let tree = self.tree.read();
            if let Some(upper_entries) = tree.dir_entries(path) {
                for e in upper_entries {
                    entries.insert(e.name.clone(), e);
                }
            }
            for name in tree.whiteout_children(path) {
                entries.remove(&name);
            }
        }

        Ok(entries.into_values().collect())
    }

    // ------------------------------------------------------------------
    // Canonicalize helper
    // ------------------------------------------------------------------

    /// Step-by-step path resolution through both layers with symlink following.
    fn resolve_path(&self, path: &Path, follow_final: bool) -> Result<PathBuf, VfsError> {
        self.resolve_path_depth(path, follow_final, MAX_SYMLINK_DEPTH)
    }

    fn resolve_path_depth(
        &self,
        path: &Path,
        follow_final: bool,
        depth: u32,
    ) -> Result<PathBuf, VfsError> {
        if depth == 0 {
            return Err(VfsError::SymlinkLoop(path.to_path_buf()));
        }

        let norm = normalize(path)?;
        let parts = path_components(&norm);
        let mut resolved = PathBuf::from("/");

        for (i, name) in parts.iter().enumerate() {
            let is_last = i == parts.len() - 1;
            let candidate = super::vfs_join(&resolved, name);

            // Tree descent for this component: whiteout (at/above) hides;
            // an upper component of ANY type shadows the lower entry
            // entirely (no lower lstat when the upper has an entry — one
            // disk syscall per component saved, and a shadowed lower symlink
            // is never followed).
            // Decide under a scoped read guard, act after it drops: no lock
            // is held across recursion or disk I/O (parking_lot read locks
            // are not recursion-safe with a queued writer).
            enum Step {
                Hidden,
                NotDir,
                FollowUpper(PathBuf),
                Plain,
                CheckLower,
            }
            let step = {
                let tree = self.tree.read();
                match tree.descend(&candidate) {
                    Descend::Found(UpperNode::Whiteout) | Descend::Blocked => Step::Hidden,
                    Descend::NotDir => Step::NotDir,
                    Descend::Found(UpperNode::Symlink { target, .. }) => {
                        Step::FollowUpper(target.clone())
                    }
                    Descend::Found(_) => Step::Plain,
                    Descend::Missing => Step::CheckLower,
                }
            };
            match step {
                Step::Hidden => return Err(VfsError::NotFound(path.to_path_buf())),
                Step::NotDir => return Err(VfsError::NotADirectory(path.to_path_buf())),
                Step::FollowUpper(target) => {
                    if is_last && !follow_final {
                        resolved = candidate;
                        continue;
                    }
                    let abs_target = if super::vfs_path_is_absolute(&target) {
                        target
                    } else {
                        super::vfs_append(&resolved, &target)
                    };
                    resolved = self.resolve_path_depth(&abs_target, true, depth - 1)?;
                }
                Step::Plain => {
                    resolved = candidate;
                }
                Step::CheckLower => {
                    // Lower fall-through: follow lower symlinks.
                    let is_lower_symlink = self
                        .lstat_lower(&candidate)
                        .is_ok_and(|m| m.node_type == NodeType::Symlink);
                    if is_lower_symlink {
                        if is_last && !follow_final {
                            resolved = candidate;
                            continue;
                        }
                        let target = self.readlink_lower(&candidate)?;
                        let abs_target = if super::vfs_path_is_absolute(&target) {
                            target
                        } else {
                            super::vfs_append(&resolved, &target)
                        };
                        resolved = self.resolve_path_depth(&abs_target, true, depth - 1)?;
                    } else {
                        resolved = candidate;
                    }
                }
            }
        }
        Ok(resolved)
    }

    // ------------------------------------------------------------------
    // Glob helpers
    // ------------------------------------------------------------------

    /// Walk directories in both layers for glob matching.
    fn glob_walk(
        &self,
        dir: &Path,
        components: &[&str],
        current_path: PathBuf,
        results: &mut Vec<PathBuf>,
        max: usize,
    ) {
        if results.len() >= max || components.is_empty() {
            if components.is_empty() {
                results.push(current_path);
            }
            return;
        }

        let pattern = components[0];
        let rest = &components[1..];

        if pattern == "**" {
            // Zero directories — advance past **
            self.glob_walk(dir, rest, current_path.clone(), results, max);

            // One or more directories — recurse
            if let Ok(entries) = self.readdir_merged(dir) {
                for entry in entries {
                    // Defensive backstop (max = 100_000 from glob): only
                    // fires once 100_000 results have been collected; test
                    // trees are far smaller.
                    if results.len() >= max {
                        return;
                    }
                    if entry.name.starts_with('.') {
                        continue;
                    }
                    let child_path = super::vfs_join(&current_path, &entry.name);
                    let child_dir = super::vfs_join(dir, &entry.name);
                    if entry.node_type == NodeType::Directory
                        || entry.node_type == NodeType::Symlink
                    {
                        self.glob_walk(&child_dir, components, child_path, results, max);
                    }
                }
            }
        } else if let Ok(entries) = self.readdir_merged(dir) {
            for entry in entries {
                // Defensive backstop, see the max-cap comment above.
                if results.len() >= max {
                    return;
                }
                if entry.name.starts_with('.') && !pattern.starts_with('.') {
                    continue;
                }
                if glob_match(pattern, &entry.name) {
                    let child_path = super::vfs_join(&current_path, &entry.name);
                    let child_dir = super::vfs_join(dir, &entry.name);
                    if rest.is_empty() {
                        results.push(child_path);
                    } else if entry.node_type == NodeType::Directory
                        || entry.node_type == NodeType::Symlink
                    {
                        self.glob_walk(&child_dir, rest, child_path, results, max);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// VirtualFs implementation
// ---------------------------------------------------------------------------

impl VirtualFs for OverlayFs {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        let norm = normalize(path)?;
        let resolved = self.resolve_path(&norm, true)?;
        match self.resolve_layer(&resolved) {
            LayerResult::Whiteout => Err(VfsError::NotFound(path.to_path_buf())),
            LayerResult::Upper => {
                let tree = self.tree.read();
                match tree.descend(&resolved) {
                    Descend::Found(UpperNode::File { content, .. }) => Ok(content.clone()),
                    Descend::Found(UpperNode::Dir { .. }) => {
                        Err(VfsError::IsADirectory(path.to_path_buf()))
                    }
                    // Unreachable: resolve_layer(Upper) implies a content
                    // node; symlinks were followed by resolve_path.
                    _ => Err(VfsError::NotFound(path.to_path_buf())),
                }
            }
            LayerResult::Lower => self.read_lower_file(&resolved),
            LayerResult::NotFound => Err(VfsError::NotFound(path.to_path_buf())),
        }
    }

    fn write_file(&self, path: &Path, content: &[u8]) -> Result<(), VfsError> {
        let norm = normalize(path)?;
        // POSIX open() follows symlinks: upper links first (including
        // mid-path — attach must happen at the resolved path), then any
        // lower chain / dangling target (O_CREAT creates the link's
        // target, link preserved).
        let norm = self.resolve_write_target(&norm, true)?;
        let norm = super::resolve_through_dangling(self, &norm)?;
        // Writing onto an existing directory is EISDIR (POSIX); only a
        // whiteouted (deleted-from-view) path may be recreated as a file.
        let is_upper_dir = {
            let tree = self.tree.read();
            matches!(tree.descend(&norm), Descend::Found(UpperNode::Dir { .. }))
        };
        if is_upper_dir {
            return Err(VfsError::IsADirectory(path.to_path_buf()));
        }
        if !is_upper_dir
            && !self.upper_has_entry(&norm)
            && !self.is_whiteout(&norm)
            && self
                .lstat_lower(&norm)
                .is_ok_and(|m| m.node_type == NodeType::Directory)
        {
            return Err(VfsError::IsADirectory(path.to_path_buf()));
        }
        if let Some(parent) = norm.parent()
            && parent != Path::new("/")
        {
            self.ensure_upper_dir_path(parent)?;
        }
        // attach replaces a whiteout atomically; a resurrected lower
        // directory's children stay hidden (the whiteout was a leaf, so its
        // children are unreachable — NotDir on descent).
        // Mode: POSIX O_TRUNC keeps an existing file's mode — check the
        // upper, then the lower (overwriting a lower 0o755 script must not
        // silently turn it 0o644 in the diff); new files get the default.
        enum ModeSrc {
            Upper(u32),
            Fresh,
            Lower,
        }
        let mode_src = {
            let tree = self.tree.read();
            match tree.descend(&norm) {
                // Overwriting a live upper file keeps its mode (O_TRUNC).
                Descend::Found(UpperNode::File { mode, .. }) => ModeSrc::Upper(*mode),
                // A whiteouted (deleted) path is a fresh create: default
                // mode, NOT the deleted file's (POSIX O_CREAT).
                Descend::Found(_) | Descend::Blocked => ModeSrc::Fresh,
                // Lower-only: O_TRUNC keeps the lower file's mode.
                Descend::Missing => ModeSrc::Lower,
                Descend::NotDir => ModeSrc::Fresh, // unreachable: EISDIR above
            }
        };
        let mode = match mode_src {
            ModeSrc::Upper(m) => m,
            ModeSrc::Fresh => 0o644,
            ModeSrc::Lower => self.lstat_lower(&norm).map(|m| m.mode).unwrap_or(0o644),
        };
        self.attach_file(&norm, content.to_vec(), mode)
    }

    fn append_file(&self, path: &Path, content: &[u8]) -> Result<(), VfsError> {
        let norm = normalize(path)?;
        // POSIX O_APPEND|O_CREAT over a deleted-from-view file: recreate it
        // FRESH (the whiteouted lower content must not be copied up). The
        // tree makes this atomic: attach replaces the whiteout leaf.
        if self.is_whiteout(&norm) {
            if let Some(parent) = norm.parent()
                && parent != Path::new("/")
            {
                self.ensure_upper_dir_path(parent)?;
            }
            return self.attach_file(&norm, content.to_vec(), 0o644);
        }
        // Follow upper symlinks (mid-path and final) before layering.
        let norm = self.resolve_write_target(&norm, true)?;
        let resolved = match self.resolve_path(&norm, true) {
            Ok(resolved) => resolved,
            // Missing target: create below (POSIX O_APPEND|O_CREAT).
            Err(VfsError::NotFound(_)) => norm.clone(),
            Err(e) => return Err(e),
        };
        match self.resolve_layer(&resolved) {
            // The symlink resolved onto a whiteouted (deleted) path: POSIX
            // O_APPEND|O_CREAT recreates it fresh, same as a direct append
            // to a whiteouted path.
            LayerResult::Whiteout => {
                if let Some(parent) = resolved.parent()
                    && parent != Path::new("/")
                {
                    self.ensure_upper_dir_path(parent)?;
                }
                self.attach_file(&resolved, content.to_vec(), 0o644)
            }
            LayerResult::Upper => self.tree.write().append_file(&resolved, content),
            LayerResult::Lower => {
                self.copy_up_if_needed(&resolved)?;
                self.tree.write().append_file(&resolved, content)
            }
            LayerResult::NotFound => {
                if let Some(parent) = resolved.parent()
                    && parent != Path::new("/")
                {
                    self.ensure_upper_dir_path(parent)?;
                }
                self.attach_file(&resolved, content.to_vec(), 0o644)
            }
        }
    }

    fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        let norm = normalize(path)?;
        if self.is_whiteout(&norm) {
            return Err(VfsError::NotFound(path.to_path_buf()));
        }
        // POSIX: rm follows mid-path symlinks (upper or lower), never the
        // final component. Whiteout components in the prefix error out
        // here (a deleted ancestor means the path is gone).
        let norm = match self.resolve_path(&norm, false) {
            Ok(p) => p,
            Err(VfsError::NotFound(_)) => return Err(VfsError::NotFound(path.to_path_buf())),
            Err(e) => return Err(e),
        };
        // Probe first (the read guard must drop before any write lock).
        enum Probe {
            Dir,
            UpperContent,
            NoUpper,
            NotDir,
        }
        let probe = {
            let tree = self.tree.read();
            match tree.descend(&norm) {
                Descend::Found(UpperNode::Dir { .. }) => Probe::Dir,
                Descend::Found(_) => Probe::UpperContent,
                Descend::NotDir => Probe::NotDir,
                _ => Probe::NoUpper,
            }
        };
        match probe {
            Probe::Dir => Err(VfsError::IsADirectory(path.to_path_buf())),
            Probe::NotDir => Err(VfsError::NotADirectory(path.to_path_buf())),
            Probe::UpperContent => {
                // Upper content: whiteout if the lower also has the path
                // (something to hide), otherwise plain detach.
                if self.lower_exists(&norm) {
                    self.tree
                        .write()
                        .put_whiteout(&norm, &|p| self.lower_mode_of(p));
                } else {
                    self.tree.write().detach(&norm);
                }
                Ok(())
            }
            Probe::NoUpper => {
                // No upper entry: check the lower layer.
                match self.lstat_lower(&norm) {
                    Ok(m) if m.node_type == NodeType::Directory => {
                        Err(VfsError::IsADirectory(path.to_path_buf()))
                    }
                    Ok(_) => {
                        self.tree
                            .write()
                            .put_whiteout(&norm, &|p| self.lower_mode_of(p));
                        Ok(())
                    }
                    Err(_) => Err(VfsError::NotFound(path.to_path_buf())),
                }
            }
        }
    }

    fn mkdir(&self, path: &Path) -> Result<(), VfsError> {
        // Resolve mid-path upper symlinks first: EEXIST is judged at the
        // real location (mkdir /l/existing through /l -> /real must fail).
        let norm = self.resolve_write_target(path, false)?;
        // bash mkdir (no -p) fails if the entry exists in either layer —
        // unless it is whiteouted (deleted-from-view), which resurrects.
        if !self.is_whiteout(&norm) {
            let in_upper = self.upper_has_entry(&norm);
            let in_lower = self.lower_exists(&norm);
            if in_upper || in_lower {
                return Err(VfsError::AlreadyExists(path.to_path_buf()));
            }
        }
        // ensure_dirs resurrects whiteouts per component, re-hiding deleted
        // lower children; a mid-path file yields NotADirectory.
        self.ensure_upper_dir_path(&norm)
    }

    fn mkdir_p(&self, path: &Path) -> Result<(), VfsError> {
        // Follow upper symlinks first (POSIX mkdir -p through a symlinked
        // directory creates beneath its target).
        let norm = self.resolve_write_target(path, true)?;
        let parts = path_components(&norm);
        if parts.is_empty() {
            return Ok(());
        }

        let mut built = PathBuf::from("/");
        for name in parts {
            built = super::vfs_join(&built, name);

            if self.is_whiteout(&built) {
                // Resurrect (re-hides deleted lower children).
                self.tree
                    .write()
                    .ensure_dirs(&built, &|d| self.lower_entries_of(d), &|p| {
                        self.lower_mode_of(p)
                    })?;
                continue;
            }

            // If it exists in upper, verify it's a directory
            if let Some(meta) = self.tree.read().meta(&built) {
                if meta.node_type != NodeType::Directory {
                    return Err(VfsError::NotADirectory(path.to_path_buf()));
                }
                continue;
            }

            // If it exists in lower, verify it's a directory — no copy-up needed
            if self.lower_exists(&built) {
                let m = self.lstat_lower(&built)?;
                if m.node_type != NodeType::Directory {
                    return Err(VfsError::NotADirectory(path.to_path_buf()));
                }
                continue;
            }

            // Doesn't exist anywhere — create in upper
            self.tree
                .write()
                .ensure_dirs(&built, &|d| self.lower_entries_of(d), &|p| {
                    self.lower_mode_of(p)
                })?;
        }
        Ok(())
    }

    fn readdir(&self, path: &Path) -> Result<Vec<DirEntry>, VfsError> {
        let norm = normalize(path)?;
        // Resolve symlinks first: an upper symlink may point into the lower
        // layer, where the literal path has no upper entry.
        let resolved = self.resolve_path(&norm, true)?;
        if self.is_whiteout(&resolved) {
            return Err(VfsError::NotFound(path.to_path_buf()));
        }

        if !self.upper_has_entry(&resolved) && !self.lower_exists(&resolved) {
            return Err(VfsError::NotFound(path.to_path_buf()));
        }

        // Validate directory through the merged view
        let m = self.stat(&resolved)?;
        if m.node_type != NodeType::Directory {
            return Err(VfsError::NotADirectory(path.to_path_buf()));
        }

        self.readdir_merged(&resolved)
    }

    fn remove_dir(&self, path: &Path) -> Result<(), VfsError> {
        // POSIX: rmdir follows mid-path symlinks, never the final.
        let norm = normalize(path)?;
        if norm == Path::new("/") {
            // POSIX rmdir("/") fails EBUSY; a whiteouted root is not
            // representable in the tree (the root is not a node).
            return Err(VfsError::InvalidPath("cannot remove root".into()));
        }
        if self.is_whiteout(&norm) {
            return Err(VfsError::NotFound(path.to_path_buf()));
        }
        let norm = match self.resolve_path(&norm, false) {
            Ok(p) => p,
            Err(VfsError::NotFound(_)) => return Err(VfsError::NotFound(path.to_path_buf())),
            Err(e) => return Err(e),
        };

        // Check that it exists and is a directory
        let m = self.lstat_overlay(&norm, path)?;
        if m.node_type != NodeType::Directory {
            return Err(VfsError::NotADirectory(path.to_path_buf()));
        }

        // Check that it's empty (merged view)
        let entries = self.readdir_merged(&norm)?;
        if !entries.is_empty() {
            return Err(VfsError::DirectoryNotEmpty(path.to_path_buf()));
        }

        if self.lower_exists(&norm) {
            self.tree
                .write()
                .put_whiteout(&norm, &|p| self.lower_mode_of(p));
        } else {
            self.tree.write().detach(&norm);
        }
        Ok(())
    }

    fn remove_dir_all(&self, path: &Path) -> Result<(), VfsError> {
        let norm = normalize(path)?;
        if norm == Path::new("/") {
            return Err(VfsError::InvalidPath("cannot remove root".into()));
        }
        if self.is_whiteout(&norm) {
            return Err(VfsError::NotFound(path.to_path_buf()));
        }
        let norm = match self.resolve_path(&norm, false) {
            Ok(p) => p,
            Err(VfsError::NotFound(_)) => return Err(VfsError::NotFound(path.to_path_buf())),
            Err(e) => return Err(e),
        };

        // Check that it exists
        let m = self.lstat_overlay(&norm, path)?;
        if m.node_type != NodeType::Directory {
            return Err(VfsError::NotADirectory(path.to_path_buf()));
        }

        // One whiteout leaf hides the whole subtree (tree invariant:
        // whiteouts never nest — put_whiteout replaces the subtree). An
        // upper-only directory has nothing to hide: plain detach (like
        // remove_file/remove_dir), leaving no phantom marker.
        if self.lower_exists(&norm) {
            self.tree
                .write()
                .put_whiteout(&norm, &|p| self.lower_mode_of(p));
        } else {
            self.tree.write().detach(&norm);
        }
        Ok(())
    }

    fn exists(&self, path: &Path) -> bool {
        let norm = match normalize(path) {
            Ok(p) => p,
            Err(_) => return false,
        };
        // Resolve symlinks first: an upper symlink may point into the lower
        // layer, in which case the literal path has no upper entry.
        let norm = match self.resolve_path(&norm, true) {
            Ok(p) => p,
            Err(_) => return false,
        };
        if self.is_whiteout(&norm) {
            return false;
        }
        self.upper_has_entry(&norm) || self.lower_exists(&norm)
    }

    fn stat(&self, path: &Path) -> Result<Metadata, VfsError> {
        let norm = normalize(path)?;
        let resolved = self.resolve_path(&norm, true)?;
        match self.resolve_layer(&resolved) {
            LayerResult::Whiteout => Err(VfsError::NotFound(path.to_path_buf())),
            LayerResult::Upper => self
                .tree
                .read()
                .meta(&resolved)
                .ok_or_else(|| VfsError::NotFound(path.to_path_buf())),
            LayerResult::Lower => self.stat_lower(&resolved),
            LayerResult::NotFound => Err(VfsError::NotFound(path.to_path_buf())),
        }
    }

    fn lstat(&self, path: &Path) -> Result<Metadata, VfsError> {
        let norm = normalize(path)?;
        self.lstat_overlay(&norm, path)
    }

    fn chmod(&self, path: &Path, mode: u32) -> Result<(), VfsError> {
        let norm = normalize(path)?;
        let resolved = self.resolve_path(&norm, true)?;
        match self.resolve_layer(&resolved) {
            LayerResult::Whiteout => Err(VfsError::NotFound(path.to_path_buf())),
            LayerResult::Upper => self.tree.write().set_mode(&resolved, mode),
            LayerResult::Lower => {
                let meta = self.lstat_lower(&resolved)?;
                match meta.node_type {
                    NodeType::File => {
                        self.copy_up_if_needed(&resolved)?;
                        self.tree.write().set_mode(&resolved, mode)
                    }
                    NodeType::Directory => {
                        self.ensure_upper_dir_path(&resolved)?;
                        self.tree.write().set_mode(&resolved, mode)
                    }
                    // Unreachable: resolve_path(follow_final = true) above
                    // already followed every symlink.
                    NodeType::Symlink => {
                        Err(VfsError::IoError("cannot chmod a symlink directly".into()))
                    }
                }
            }
            LayerResult::NotFound => Err(VfsError::NotFound(path.to_path_buf())),
        }
    }

    fn utimes(&self, path: &Path, mtime: SystemTime) -> Result<(), VfsError> {
        let norm = normalize(path)?;
        let resolved = self.resolve_path(&norm, true)?;
        match self.resolve_layer(&resolved) {
            LayerResult::Whiteout => Err(VfsError::NotFound(path.to_path_buf())),
            LayerResult::Upper => self.tree.write().set_mtime(&resolved, mtime),
            LayerResult::Lower => {
                let meta = self.lstat_lower(&resolved)?;
                match meta.node_type {
                    NodeType::File => {
                        self.copy_up_if_needed(&resolved)?;
                        self.tree.write().set_mtime(&resolved, mtime)
                    }
                    NodeType::Directory => {
                        self.ensure_upper_dir_path(&resolved)?;
                        self.tree.write().set_mtime(&resolved, mtime)
                    }
                    NodeType::Symlink => {
                        Err(VfsError::IoError("cannot utimes a symlink directly".into()))
                    }
                }
            }
            LayerResult::NotFound => Err(VfsError::NotFound(path.to_path_buf())),
        }
    }

    fn symlink(&self, target: &Path, link: &Path) -> Result<(), VfsError> {
        let norm_link = normalize(link)?;
        // POSIX: the link must not exist in EITHER layer (a whiteouted
        // entry may be recreated). The EEXIST check runs against the
        // UNRESOLVED link path (checking the final name in its literal
        // directory), then the parent is resolved through upper symlinks
        // so the node lands at the real location.
        if !self.is_whiteout(&norm_link)
            && (self.upper_has_entry(&norm_link) || self.lower_exists(&norm_link))
        {
            return Err(VfsError::AlreadyExists(link.to_path_buf()));
        }
        let norm_link = self.resolve_write_target(&norm_link, false)?;
        // EEXIST must also hold at the RESOLVED path (POSIX ln -s refuses
        // to clobber through a mid-path symlink).
        if !self.is_whiteout(&norm_link)
            && (self.upper_has_entry(&norm_link) || self.lower_exists(&norm_link))
        {
            return Err(VfsError::AlreadyExists(link.to_path_buf()));
        }
        if let Some(parent) = norm_link.parent()
            && parent != Path::new("/")
        {
            self.ensure_upper_dir_path(parent)?;
        }
        self.tree.write().attach(
            &norm_link,
            UpperNode::Symlink {
                target: target.to_path_buf(),
                mtime: SystemTime::now(),
            },
        )
    }

    fn hardlink(&self, src: &Path, dst: &Path) -> Result<(), VfsError> {
        let norm_src = normalize(src)?;
        let norm_dst = normalize(dst)?;
        // POSIX: dst must not exist in either layer.
        if !self.is_whiteout(&norm_dst)
            && (self.upper_has_entry(&norm_dst) || self.lower_exists(&norm_dst))
        {
            return Err(VfsError::AlreadyExists(dst.to_path_buf()));
        }
        // Read source from whichever layer has it (content copy — see the
        // hardlink divergence note in the guidebook registry).
        let content = self.read_file(&norm_src)?;
        let meta = self.stat(&norm_src)?;
        let norm_dst = self.resolve_write_target(&norm_dst, false)?;
        // EEXIST at the resolved path too (see symlink()).
        if !self.is_whiteout(&norm_dst)
            && (self.upper_has_entry(&norm_dst) || self.lower_exists(&norm_dst))
        {
            return Err(VfsError::AlreadyExists(dst.to_path_buf()));
        }
        if let Some(parent) = norm_dst.parent()
            && parent != Path::new("/")
        {
            self.ensure_upper_dir_path(parent)?;
        }
        self.attach_file(&norm_dst, content, meta.mode)?;
        self.tree.write().set_mtime(&norm_dst, meta.mtime)?;
        Ok(())
    }

    fn readlink(&self, path: &Path) -> Result<PathBuf, VfsError> {
        let norm = normalize(path)?;
        match self.tree.read().descend(&norm) {
            Descend::Found(UpperNode::Symlink { target, .. }) => Ok(target.clone()),
            Descend::Found(UpperNode::Whiteout) | Descend::Blocked => {
                Err(VfsError::NotFound(path.to_path_buf()))
            }
            // Upper non-symlink: EINVAL-ish (no EINVAL variant; InvalidPath).
            Descend::Found(_) => Err(VfsError::InvalidPath(path.to_string_lossy().into_owned())),
            _ => {
                if self.lower_exists(&norm) {
                    self.readlink_lower(&norm)
                } else {
                    Err(VfsError::NotFound(path.to_path_buf()))
                }
            }
        }
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, VfsError> {
        let norm = normalize(path)?;
        let resolved = self.resolve_path(&norm, true)?;
        if self.is_whiteout(&resolved) {
            return Err(VfsError::NotFound(path.to_path_buf()));
        }
        if !self.upper_has_entry(&resolved) && !self.lower_exists(&resolved) {
            return Err(VfsError::NotFound(path.to_path_buf()));
        }
        Ok(resolved)
    }

    fn copy(&self, src: &Path, dst: &Path) -> Result<(), VfsError> {
        let norm_src = normalize(src)?;
        let norm_dst = normalize(dst)?;
        let content = self.read_file(&norm_src)?;
        let meta = self.stat(&norm_src)?;
        self.write_file(&norm_dst, &content)?;
        self.chmod(&norm_dst, meta.mode)?;
        Ok(())
    }

    fn rename(&self, src: &Path, dst: &Path) -> Result<(), VfsError> {
        let norm_src = normalize(src)?;
        let norm_dst = normalize(dst)?;

        if norm_src == norm_dst {
            // POSIX rename(2) onto itself is a no-op success.
            return Ok(());
        }
        if self.is_whiteout(&norm_src) {
            return Err(VfsError::NotFound(src.to_path_buf()));
        }
        // POSIX: rename follows mid-path symlinks of the SOURCE (never the
        // final component — renaming a symlink moves the link itself).
        let norm_src = match self.resolve_path(&norm_src, false) {
            Ok(p) => p,
            Err(VfsError::NotFound(_)) => return Err(VfsError::NotFound(src.to_path_buf())),
            Err(e) => return Err(e),
        };

        let in_upper = self.upper_has_entry(&norm_src);
        let in_lower = self.lower_exists(&norm_src);
        if !in_upper && !in_lower {
            return Err(VfsError::NotFound(src.to_path_buf()));
        }

        // Resolve the destination's parent through upper symlinks (the
        // final name is NOT followed: rename replaces a dst symlink).
        let norm_dst = self.resolve_write_target(&norm_dst, false)?;
        let meta = self.lstat_overlay(&norm_src, src)?;
        match meta.node_type {
            NodeType::File => {
                // POSIX rename(2): a file cannot replace a directory.
                // (Upper dirs already EISDIR via attach; check the lower.)
                if !self.is_whiteout(&norm_dst)
                    && !self.upper_has_entry(&norm_dst)
                    && self
                        .lstat_lower(&norm_dst)
                        .is_ok_and(|m| m.node_type == NodeType::Directory)
                {
                    return Err(VfsError::IsADirectory(dst.to_path_buf()));
                }
                let content = self.read_file(&norm_src)?;
                if let Some(parent) = norm_dst.parent()
                    && parent != Path::new("/")
                {
                    self.ensure_upper_dir_path(parent)?;
                }
                self.attach_file(&norm_dst, content, meta.mode)?;
                self.tree.write().set_mtime(&norm_dst, meta.mtime)?;
            }
            NodeType::Symlink => {
                // Same EISDIR rule for a symlink landing on a lower dir.
                if !self.is_whiteout(&norm_dst)
                    && !self.upper_has_entry(&norm_dst)
                    && self
                        .lstat_lower(&norm_dst)
                        .is_ok_and(|m| m.node_type == NodeType::Directory)
                {
                    return Err(VfsError::IsADirectory(dst.to_path_buf()));
                }
                let target = self.readlink(&norm_src)?;
                if let Some(parent) = norm_dst.parent()
                    && parent != Path::new("/")
                {
                    self.ensure_upper_dir_path(parent)?;
                }
                self.tree.write().attach(
                    &norm_dst,
                    UpperNode::Symlink {
                        target,
                        mtime: meta.mtime,
                    },
                )?;
            }
            NodeType::Directory => {
                // ensure_dirs resurrects a whiteouted dst (re-hiding deleted
                // lower children); recursive copies land inside it.
                self.tree
                    .write()
                    .ensure_dirs(&norm_dst, &|d| self.lower_entries_of(d), &|p| {
                        self.lower_mode_of(p)
                    })?;
                let entries = self.readdir_merged(&norm_src)?;
                for entry in entries {
                    let child_src = super::vfs_join(&norm_src, &entry.name);
                    let child_dst = super::vfs_join(&norm_dst, &entry.name);
                    self.rename(&child_src, &child_dst)?;
                }
            }
        }

        // Hide the source from lower: one whiteout leaf replaces the whole
        // source subtree (recursive renames put per-descendant whiteouts or
        // content under it — put_whiteout collapses). An upper-only source
        // has nothing to hide: plain detach (no phantom marker).
        if self.lower_exists(&norm_src) {
            self.tree
                .write()
                .put_whiteout(&norm_src, &|p| self.lower_mode_of(p));
        } else {
            self.tree.write().detach(&norm_src);
        }
        Ok(())
    }

    fn glob(&self, pattern: &str, cwd: &Path) -> Result<Vec<PathBuf>, VfsError> {
        let is_absolute = pattern.starts_with('/');
        let abs_pattern = if is_absolute {
            pattern.to_string()
        } else {
            let cwd_str = cwd.to_str().unwrap_or("/").trim_end_matches('/');
            format!("{cwd_str}/{pattern}")
        };

        let components: Vec<&str> = abs_pattern.split('/').filter(|s| !s.is_empty()).collect();
        let mut results = Vec::new();
        let max = 100_000;
        self.glob_walk(
            Path::new("/"),
            &components,
            PathBuf::from("/"),
            &mut results,
            max,
        );

        results.sort();
        results.dedup();

        if !is_absolute {
            results = results
                .into_iter()
                .filter_map(|p| p.strip_prefix(cwd).ok().map(|r| r.to_path_buf()))
                .collect();
        }

        Ok(results)
    }

    fn deep_clone(&self) -> Arc<dyn VirtualFs> {
        Arc::new(OverlayFs {
            lower: self.lower.clone(),
            tree: RwLock::new(self.tree.read().clone()),
        })
    }
}

// ---------------------------------------------------------------------------
// Private OverlayFs helpers
// ---------------------------------------------------------------------------

impl OverlayFs {
    /// lstat through the overlay (no symlink following on final component).
    fn lstat_overlay(&self, norm: &Path, orig: &Path) -> Result<Metadata, VfsError> {
        // Probe with a scoped read guard (no lock held across the return).
        enum Probe {
            Content,
            Hidden,
            Miss,
            NotDir,
        }
        let probe = {
            let tree = self.tree.read();
            match tree.descend(norm) {
                Descend::Found(n) if n.is_content() => Probe::Content,
                Descend::Found(UpperNode::Whiteout) | Descend::Blocked => Probe::Hidden,
                Descend::NotDir => Probe::NotDir,
                _ => Probe::Miss,
            }
        };
        match probe {
            Probe::NotDir => Err(VfsError::NotADirectory(orig.to_path_buf())),
            Probe::Content => self
                .tree
                .read()
                .meta(norm)
                .ok_or_else(|| VfsError::NotFound(orig.to_path_buf())),
            Probe::Hidden => Err(VfsError::NotFound(orig.to_path_buf())),
            Probe::Miss => {
                if self.lower_exists(norm) {
                    self.lstat_lower(norm)
                } else {
                    Err(VfsError::NotFound(orig.to_path_buf()))
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Lexically normalize path segments in place: drop "." and empty segments,
/// resolve ".." by popping (a ".." with nothing to pop is kept, matching
/// bash's lexical handling of absolute paths).
fn normalize_segments(segs: &mut Vec<String>) {
    let mut out: Vec<String> = Vec::with_capacity(segs.len());
    for seg in segs.drain(..) {
        match seg.as_str() {
            "." | "" => {}
            ".." => {
                // Pop if possible; at the root, ".." is the root (POSIX,
                // matching vfs_normalize_checked) — never keep a literal
                // ".." segment (it would create unreachable tree nodes and
                // escape-prone diff paths).
                if out.last().is_some_and(|s| s != "..") {
                    out.pop();
                }
            }
            _ => out.push(seg),
        }
    }
    *segs = out;
}

/// True when any node in the subtree is a content node (not a whiteout).
/// Iterative (no recursion-depth limit on deep trees).
fn subtree_has_content(children: &std::collections::BTreeMap<String, UpperNode>) -> bool {
    let mut stack: Vec<&UpperNode> = children.values().collect();
    while let Some(n) = stack.pop() {
        match n {
            UpperNode::Whiteout => {}
            UpperNode::Dir { children, .. } => stack.extend(children.values()),
            _ => return true,
        }
    }
    false
}

/// Normalize an absolute path: resolve `.` and `..`.
fn normalize(path: &Path) -> Result<PathBuf, VfsError> {
    super::vfs_normalize_checked(path)
}

/// Split a normalized absolute path into component names.
fn path_components(path: &Path) -> Vec<&str> {
    let s = path.to_str().unwrap_or("");
    s.trim_start_matches('/')
        .split('/')
        .filter(|c| !c.is_empty())
        .collect()
}

/// Map `std::io::Error` to `VfsError`.
fn map_io_error(err: std::io::Error, path: &Path) -> VfsError {
    let p = path.to_path_buf();
    match err.kind() {
        std::io::ErrorKind::NotFound => VfsError::NotFound(p),
        std::io::ErrorKind::AlreadyExists => VfsError::AlreadyExists(p),
        std::io::ErrorKind::PermissionDenied => VfsError::PermissionDenied(p),
        std::io::ErrorKind::DirectoryNotEmpty => VfsError::DirectoryNotEmpty(p),
        std::io::ErrorKind::NotADirectory => VfsError::NotADirectory(p),
        std::io::ErrorKind::IsADirectory => VfsError::IsADirectory(p),
        _ => VfsError::IoError(err.to_string()),
    }
}

/// Map `std::fs::Metadata` to our `vfs::Metadata`.
fn map_std_metadata(meta: &std::fs::Metadata) -> Metadata {
    let node_type = if meta.is_symlink() {
        NodeType::Symlink
    } else if meta.is_dir() {
        NodeType::Directory
    } else {
        NodeType::File
    };
    Metadata {
        node_type,
        size: meta.len(),
        mode: super::unix_mode_from_metadata(meta),
        mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        file_id: 0,
    }
}
