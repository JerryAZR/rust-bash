//! The overlay upper layer as an explicit tree.
//!
//! Design follows the just-bash fork's OverlayTree (its docs/design/
//! overlay-tree.md — in the sibling repository, not vendored here): content
//! nodes and whiteout nodes live in ONE tree, so
//! the load-bearing invariants are structural rather than maintained by
//! call-site discipline:
//!
//! 1. The root is always a directory.
//! 2. Child names are single path segments.
//! 3. Whiteouts are leaves and never nest (`put_whiteout` collapses a
//!    subtree into one node), so every whiteout is a top-most deletion by
//!    construction and `diff()` needs no filtering machinery.
//! 4. A node is either content or a whiteout — "whiteout coexists with an
//!    upper entry" is unrepresentable.
//! 5. Directory resurrection happens only through `ensure_dirs` (which
//!    re-hides deleted lower children by inserting whiteout children).
//!
//! The tree knows nothing about the lower layer; callers inject lower-layer
//! probes (`lower_exists`, `lower_readdir`) at the call sites that need them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::VfsError;
use crate::platform::SystemTime;
use crate::vfs::{DirEntry, Metadata, NodeType};

/// One node in the overlay upper layer.
#[derive(Debug, Clone)]
pub enum UpperNode {
    File {
        content: Vec<u8>,
        mode: u32,
        mtime: SystemTime,
    },
    Dir {
        children: BTreeMap<String, UpperNode>,
        mode: u32,
        mtime: SystemTime,
    },
    Symlink {
        target: PathBuf,
        mtime: SystemTime,
    },
    /// "This path and everything under it was deleted." Leaf, never nested.
    Whiteout,
}

impl UpperNode {
    pub fn node_type(&self) -> NodeType {
        match self {
            UpperNode::File { .. } => NodeType::File,
            UpperNode::Dir { .. } => NodeType::Directory,
            UpperNode::Symlink { .. } => NodeType::Symlink,
            // A whiteout is not a visible node; callers must not expose it.
            UpperNode::Whiteout => NodeType::File,
        }
    }

    pub fn is_content(&self) -> bool {
        !matches!(self, UpperNode::Whiteout)
    }

    /// Mutable access to a Dir node's children map.
    fn as_dir_mut(&mut self) -> &mut BTreeMap<String, UpperNode> {
        match self {
            UpperNode::Dir { children, .. } => children,
            // Unreachable: put_whiteout's scaffold loop only descends
            // through Dirs it just verified or created.
            _ => panic!("as_dir_mut on non-directory"),
        }
    }
}

/// Result of descending the tree to a path.
#[derive(Debug)]
pub enum Descend<'a> {
    /// A node exists at exactly this path (may be a Whiteout).
    Found(&'a UpperNode),
    /// No node at this path and no whiteout above it: the caller may fall
    /// through to the lower layer.
    Missing,
    /// A whiteout sits at or above this path: ENOENT, no lower fall-through.
    Blocked,
    /// A file or symlink shadows an ancestor directory: ENOTDIR.
    NotDir,
}

impl Descend<'_> {
    pub fn is_blocked_or_whiteout(&self) -> bool {
        matches!(self, Descend::Blocked) || matches!(self, Descend::Found(UpperNode::Whiteout))
    }

    /// Content node at the exact path (whiteouts excluded).
    pub fn content(&self) -> Option<&UpperNode> {
        match self {
            Descend::Found(n) if n.is_content() => Some(n),
            _ => None,
        }
    }
}

/// Split a normalized absolute VFS path into segments. The root ("/")
/// yields an empty Vec (the root itself is not a node).
fn segments(path: &Path) -> Vec<String> {
    path.to_string_lossy()
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct OverlayTree {
    /// The root directory's children.
    children: BTreeMap<String, UpperNode>,
}

impl OverlayTree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read-only descent to `path` (normalized absolute VFS path).
    pub fn descend(&self, path: &Path) -> Descend<'_> {
        let segs = segments(path);
        let mut cur = &self.children;
        for (i, seg) in segs.iter().enumerate() {
            let Some(node) = cur.get(seg) else {
                return Descend::Missing;
            };
            if i == segs.len() - 1 {
                return Descend::Found(node);
            }
            match node {
                UpperNode::Whiteout => return Descend::Blocked,
                UpperNode::Dir { children, .. } => cur = children,
                UpperNode::File { .. } | UpperNode::Symlink { .. } => return Descend::NotDir,
            }
        }
        // The path is "/" — the root directory itself.
        Descend::Missing
    }

    /// Mutable handle to the children map of the parent directory of `path`.
    /// `NotDir` when an ancestor is a file/symlink, `Blocked` when an
    /// ancestor is a whiteout, `Missing` when a parent component is absent
    /// (callers create parents via `ensure_dirs` first).
    fn parent_children_mut(
        &mut self,
        path: &Path,
    ) -> Result<&mut BTreeMap<String, UpperNode>, VfsError> {
        let segs = segments(path);
        if segs.is_empty() {
            return Err(VfsError::InvalidPath("cannot operate on root".into()));
        }
        let mut cur = &mut self.children;
        for seg in &segs[..segs.len() - 1] {
            match cur.get_mut(seg) {
                None => {
                    return Err(VfsError::NotFound(PathBuf::from(path)));
                }
                Some(UpperNode::Whiteout) => {
                    return Err(VfsError::NotFound(PathBuf::from(path)));
                }
                Some(UpperNode::Dir { children, .. }) => cur = children,
                Some(UpperNode::File { .. } | UpperNode::Symlink { .. }) => {
                    return Err(VfsError::NotADirectory(PathBuf::from(path)));
                }
            }
        }
        Ok(cur)
    }

    /// Attach a content node at `path`, replacing any file/symlink/whiteout.
    /// Mirrors syscall errnos: file/symlink over a directory → EISDIR;
    /// directory over any existing node → EEXIST (resurrection goes through
    /// `ensure_dirs`). Callers create parents first (`ensure_dirs`).
    pub fn attach(&mut self, path: &Path, node: UpperNode) -> Result<(), VfsError> {
        let segs = segments(path);
        let name = segs.last().unwrap();
        let children = self.parent_children_mut(path)?;
        if let Some(existing) = children.get(name) {
            match (&node, existing) {
                (UpperNode::Dir { .. }, UpperNode::Dir { .. }) => {
                    return Err(VfsError::AlreadyExists(PathBuf::from(path)));
                }
                (UpperNode::Dir { .. }, _) => {
                    // Directory over file/symlink/whiteout: resurrection must
                    // go through ensure_dirs so lower children get re-hidden.
                    return Err(VfsError::AlreadyExists(PathBuf::from(path)));
                }
                (_, UpperNode::Dir { .. }) => {
                    return Err(VfsError::IsADirectory(PathBuf::from(path)));
                }
                _ => {}
            }
        }
        children.insert(name.clone(), node);
        Ok(())
    }

    /// Replace the subtree at `path` with a single whiteout leaf, creating
    /// scaffold directories as needed (the upper tree is sparse — a
    /// whiteout for a lower path often has no upper ancestors yet).
    /// Scaffold dirs pick up the lower layer's mode via the probe so stat
    /// on an ancestor doesn't report fabricated metadata; they are
    /// invisible to `diff()` (see its Dir rule). No-op if the path is
    /// already under a whiteout or beneath a file/symlink (already
    /// invisible through the overlay; POSIX rm would fail ENOTDIR — the
    /// remove_* callers validate existence first, so we keep the no-op).
    pub fn put_whiteout(&mut self, path: &Path, lower_mode: &dyn Fn(&Path) -> Option<u32>) {
        let segs = segments(path);
        if segs.is_empty() {
            return;
        }
        // Scaffold ancestors (never resurrecting or replacing anything).
        let mut cur = &mut self.children;
        let mut cur_path = PathBuf::from("/");
        for seg in &segs[..segs.len() - 1] {
            cur_path = super::vfs_join(&cur_path, seg);
            if matches!(cur.get(seg), Some(UpperNode::Whiteout)) {
                return; // already under a whiteout
            }
            if matches!(
                cur.get(seg),
                Some(UpperNode::File { .. } | UpperNode::Symlink { .. })
            ) {
                return; // not a directory
            }
            let mode = lower_mode(&cur_path).unwrap_or(0o755);
            cur = cur
                .entry(seg.clone())
                .or_insert_with(|| UpperNode::Dir {
                    children: BTreeMap::new(),
                    mode,
                    mtime: SystemTime::now(),
                })
                .as_dir_mut();
        }
        cur.insert(segs.last().unwrap().clone(), UpperNode::Whiteout);
    }

    /// Remove the node at `path` entirely (used for upper-only deletions and
    /// by `sync`/`drop`).
    pub fn detach(&mut self, path: &Path) {
        let segs = segments(path);
        if segs.is_empty() {
            return;
        }
        if let Ok(children) = self.parent_children_mut(path) {
            children.remove(segs.last().unwrap());
        }
    }

    /// Ensure directory nodes exist for every component of `path`,
    /// resurrecting whiteouts: a whiteouted directory becomes a fresh empty
    /// directory whose deleted lower children are re-hidden with child
    /// whiteouts (one lower readdir per resurrected level, injected).
    ///
    /// `lower_readdir(dir)` returns the lower layer's entry names for a
    /// directory, or None when the lower layer has no such directory.
    /// `lower_mode(dir)` optionally supplies a mode for created shadow dirs.
    pub fn ensure_dirs(
        &mut self,
        path: &Path,
        lower_readdir: &dyn Fn(&Path) -> Option<Vec<String>>,
        lower_mode: &dyn Fn(&Path) -> Option<u32>,
    ) -> Result<(), VfsError> {
        let segs = segments(path);
        let mut cur = &mut self.children;
        let mut cur_path = PathBuf::from("/");
        for seg in segs {
            cur_path = super::vfs_join(&cur_path, &seg);
            match cur.get(&seg) {
                Some(UpperNode::Dir { .. }) => {}
                Some(UpperNode::Whiteout) => {
                    // Resurrect: fresh directory + a whiteout child per lower
                    // entry (they were deleted; they must not reappear).
                    let lower_entries = lower_readdir(&cur_path).unwrap_or_default();
                    let mode = lower_mode(&cur_path).unwrap_or(0o755);
                    let mut children = BTreeMap::new();
                    for name in lower_entries {
                        children.insert(name, UpperNode::Whiteout);
                    }
                    cur.insert(
                        seg.clone(),
                        UpperNode::Dir {
                            children,
                            mode,
                            mtime: SystemTime::now(),
                        },
                    );
                }
                Some(UpperNode::File { .. } | UpperNode::Symlink { .. }) => {
                    return Err(VfsError::NotADirectory(cur_path));
                }
                None => {
                    let mode = lower_mode(&cur_path).unwrap_or(0o755);
                    cur.insert(
                        seg.clone(),
                        UpperNode::Dir {
                            children: BTreeMap::new(),
                            mode,
                            mtime: SystemTime::now(),
                        },
                    );
                }
            }
            // Descend (the node is a Dir by the branches above).
            let Some(UpperNode::Dir { children: next, .. }) = cur.get_mut(&seg) else {
                // Unreachable: the match above guarantees a Dir.
                return Err(VfsError::InvalidPath(
                    cur_path.to_string_lossy().into_owned(),
                ));
            };
            cur = next;
        }
        Ok(())
    }

    /// Pre-order walk: every node with its path. Materialized so callers may
    /// mutate between/after inspections.
    pub fn walk(&self) -> Vec<(PathBuf, &UpperNode)> {
        let mut out = Vec::new();
        let mut stack: Vec<(PathBuf, &UpperNode)> = self
            .children
            .iter()
            .rev()
            .map(|(n, node)| (super::vfs_join(Path::new("/"), n), node))
            .collect();
        while let Some((path, node)) = stack.pop() {
            out.push((path.clone(), node));
            if let UpperNode::Dir { children, .. } = node {
                for (name, child) in children.iter().rev() {
                    stack.push((super::vfs_join(&path, name), child));
                }
            }
        }
        out
    }

    /// Post-order walk (children before parents) — for sync()/drop().
    /// Iterative (no recursion-depth limit on deep trees).
    pub fn walk_post(&self) -> Vec<(PathBuf, &UpperNode)> {
        let mut out = Vec::new();
        // (path, node, children_expanded) stack machine.
        let mut stack: Vec<(PathBuf, &UpperNode, bool)> = self
            .children
            .iter()
            .rev()
            .map(|(n, node)| (super::vfs_join(Path::new("/"), n), node, false))
            .collect();
        while let Some((path, node, expanded)) = stack.pop() {
            if expanded {
                out.push((path, node));
                continue;
            }
            stack.push((path.clone(), node, true));
            if let UpperNode::Dir { children, .. } = node {
                for (name, child) in children.iter().rev() {
                    stack.push((super::vfs_join(&path, name), child, false));
                }
            }
        }
        out
    }

    /// All entries of a directory node at `path` (None if not a directory).
    /// Whiteout children are included — the caller filters.
    pub fn dir_entries(&self, path: &Path) -> Option<Vec<DirEntry>> {
        if segments(path).is_empty() {
            // The root directory is the top-level children map.
            return Some(
                self.children
                    .iter()
                    .map(|(name, node)| DirEntry {
                        name: name.clone(),
                        node_type: node.node_type(),
                    })
                    .collect(),
            );
        }
        match self.descend(path) {
            Descend::Found(UpperNode::Dir { children, .. }) => Some(
                children
                    .iter()
                    .map(|(name, node)| DirEntry {
                        name: name.clone(),
                        node_type: node.node_type(),
                    })
                    .collect(),
            ),
            _ => None,
        }
    }

    pub fn clear(&mut self) {
        self.children.clear();
    }

    /// Metadata for the node at `path` (content nodes only).
    pub fn meta(&self, path: &Path) -> Option<Metadata> {
        match self.descend(path) {
            Descend::Found(UpperNode::File {
                content,
                mode,
                mtime,
            }) => Some(Metadata {
                node_type: NodeType::File,
                size: content.len() as u64,
                mode: *mode,
                mtime: *mtime,
                file_id: 0,
            }),
            Descend::Found(UpperNode::Dir { mode, mtime, .. }) => Some(Metadata {
                node_type: NodeType::Directory,
                size: 0,
                mode: *mode,
                mtime: *mtime,
                file_id: 0,
            }),
            Descend::Found(UpperNode::Symlink { target, mtime }) => Some(Metadata {
                node_type: NodeType::Symlink,
                size: target.to_string_lossy().len() as u64,
                mode: 0o777,
                mtime: *mtime,
                file_id: 0,
            }),
            _ => None,
        }
    }

    /// Append to a file node's content (touching mtime). ENOENT/ENOTDIR map
    /// to NotFound; a directory target is IsADirectory.
    pub fn append_file(&mut self, path: &Path, content: &[u8]) -> Result<(), VfsError> {
        match self.node_mut(path) {
            Ok(UpperNode::File {
                content: c, mtime, ..
            }) => {
                c.extend_from_slice(content);
                *mtime = SystemTime::now();
                Ok(())
            }
            Ok(UpperNode::Dir { .. }) => Err(VfsError::IsADirectory(path.to_path_buf())),
            Ok(UpperNode::Symlink { .. }) | Err(_) => Err(VfsError::NotFound(path.to_path_buf())),
            Ok(UpperNode::Whiteout) => Err(VfsError::NotFound(path.to_path_buf())),
        }
    }

    /// Set the mode of the node at `path`.
    pub fn set_mode(&mut self, path: &Path, mode: u32) -> Result<(), VfsError> {
        match self.node_mut(path)? {
            UpperNode::File { mode: m, .. } | UpperNode::Dir { mode: m, .. } => {
                *m = mode;
                Ok(())
            }
            UpperNode::Symlink { .. } | UpperNode::Whiteout => {
                Err(VfsError::InvalidPath(path.to_string_lossy().into_owned()))
            }
        }
    }

    /// Set the mtime of the node at `path`.
    pub fn set_mtime(&mut self, path: &Path, mtime: SystemTime) -> Result<(), VfsError> {
        match self.node_mut(path)? {
            UpperNode::File { mtime: m, .. }
            | UpperNode::Dir { mtime: m, .. }
            | UpperNode::Symlink { mtime: m, .. } => {
                *m = mtime;
                Ok(())
            }
            UpperNode::Whiteout => Err(VfsError::InvalidPath(path.to_string_lossy().into_owned())),
        }
    }

    /// Mutable access to the node at exactly `path`.
    fn node_mut(&mut self, path: &Path) -> Result<&mut UpperNode, VfsError> {
        let segs = segments(path);
        if segs.is_empty() {
            return Err(VfsError::InvalidPath("root".into()));
        }
        let children = self.parent_children_mut(path)?;
        children
            .get_mut(segs.last().unwrap())
            .ok_or_else(|| VfsError::NotFound(path.to_path_buf()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file() -> UpperNode {
        UpperNode::File {
            content: b"x".to_vec(),
            mode: 0o644,
            mtime: SystemTime::now(),
        }
    }

    #[test]
    fn descend_contract() {
        let mut t = OverlayTree::new();
        t.ensure_dirs(Path::new("/a/b"), &|_| None, &|_| None)
            .unwrap();
        t.attach(Path::new("/a/b/f"), file()).unwrap();
        t.attach(Path::new("/a/g"), file()).unwrap();
        assert!(matches!(
            t.descend(Path::new("/a/b/f")),
            Descend::Found(UpperNode::File { .. })
        ));
        assert!(matches!(
            t.descend(Path::new("/a/missing")),
            Descend::Missing
        ));
        // File blocks descent
        assert!(matches!(
            t.descend(Path::new("/a/g/deeper")),
            Descend::NotDir
        ));
        // Whiteout blocks descendants
        t.put_whiteout(Path::new("/a/b"), &|_| None);
        assert!(matches!(
            t.descend(Path::new("/a/b")),
            Descend::Found(UpperNode::Whiteout)
        ));
        assert!(matches!(t.descend(Path::new("/a/b/f")), Descend::Blocked));
        assert!(t.descend(Path::new("/a/b/f")).is_blocked_or_whiteout());
    }

    #[test]
    fn attach_type_rules() {
        let mut t = OverlayTree::new();
        t.ensure_dirs(Path::new("/d"), &|_| None, &|_| None)
            .unwrap();
        // file over directory → EISDIR
        let err = t.attach(Path::new("/d"), file()).unwrap_err();
        assert!(matches!(err, VfsError::IsADirectory(_)));
        // directory over anything → EEXIST (resurrection via ensure_dirs)
        let err = t
            .attach(
                Path::new("/d"),
                UpperNode::Dir {
                    children: BTreeMap::new(),
                    mode: 0o755,
                    mtime: SystemTime::now(),
                },
            )
            .unwrap_err();
        assert!(matches!(err, VfsError::AlreadyExists(_)));
        // file over file → replace
        t.attach(Path::new("/d/f"), file()).unwrap();
        t.attach(Path::new("/d/f"), file()).unwrap();
    }

    #[test]
    fn whiteout_collapse_and_resurrection() {
        let mut t = OverlayTree::new();
        t.ensure_dirs(Path::new("/a/b"), &|_| None, &|_| None)
            .unwrap();
        t.attach(Path::new("/a/b/f"), file()).unwrap();
        // put_whiteout collapses the subtree to one leaf
        t.put_whiteout(Path::new("/a"), &|_| None);
        let walked = t.walk();
        let paths: Vec<&Path> = walked.iter().map(|(p, _)| p.as_path()).collect();
        assert_eq!(paths, vec![Path::new("/a")]);
        // Resurrect /a with two deleted lower children
        t.ensure_dirs(
            Path::new("/a/b"),
            &|p| {
                if p == Path::new("/a") {
                    Some(vec!["old1".to_string(), "old2".to_string()])
                } else {
                    None
                }
            },
            &|_| None,
        )
        .unwrap();
        assert!(t.descend(Path::new("/a/old1")).is_blocked_or_whiteout());
        assert!(t.descend(Path::new("/a/old2")).is_blocked_or_whiteout());
        assert!(matches!(
            t.descend(Path::new("/a/b")),
            Descend::Found(UpperNode::Dir { .. })
        ));
        // Fresh writes under the resurrected dir work
        t.attach(Path::new("/a/b/new"), file()).unwrap();
        assert!(matches!(
            t.descend(Path::new("/a/b/new")),
            Descend::Found(UpperNode::File { .. })
        ));
    }

    #[test]
    fn walk_orders() {
        let mut t = OverlayTree::new();
        t.ensure_dirs(Path::new("/a/b"), &|_| None, &|_| None)
            .unwrap();
        t.attach(Path::new("/a/b/f"), file()).unwrap();
        t.put_whiteout(Path::new("/a/w"), &|_| None);
        let pre: Vec<String> = t
            .walk()
            .iter()
            .map(|(p, _)| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(pre, vec!["/a", "/a/b", "/a/b/f", "/a/w"]);
        let post: Vec<String> = t
            .walk_post()
            .iter()
            .map(|(p, _)| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(post, vec!["/a/b/f", "/a/b", "/a/w", "/a"]);
    }
}
