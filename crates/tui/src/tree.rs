//! The sidebar's folder tree: changed files nested under collapsible folders.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::app::FileEntry;

/// One tree entry, stored in display order: a folder is followed by everything inside it.
#[derive(Debug)]
pub enum Node {
    /// A folder; `label` spans merged single-child folders, like `core/src`.
    Dir {
        path: PathBuf,
        label: Box<str>,
        depth: usize,
        /// Node index just past this folder's last descendant.
        end: usize,
        added: u32,
        removed: u32,
    },
    File {
        depth: usize,
        entry: FileEntry,
    },
}

impl Node {
    pub fn depth(&self) -> usize {
        match self {
            Self::Dir { depth, .. } | Self::File { depth, .. } => *depth,
        }
    }

    pub fn path(&self) -> &Path {
        match self {
            Self::Dir { path, .. } => path,
            Self::File { entry, .. } => &entry.path,
        }
    }
}

/// Changed files grouped into folders, with the rows left visible by closed folders.
#[derive(Debug, Default)]
pub struct Tree {
    nodes: Vec<Node>,
    /// Node index of each visible row.
    rows: Vec<usize>,
    /// Folders the user closed; carried into the next build.
    collapsed: HashSet<PathBuf>,
}

/// Build-time grouping only; flattened into [`Node`]s and dropped.
#[derive(Default)]
struct Folder {
    dirs: BTreeMap<OsString, Folder>,
    files: Vec<FileEntry>,
}

impl Tree {
    /// Builds the tree; folders in `collapsed` start closed, and missing ones are forgotten.
    pub fn new(files: Vec<FileEntry>, mut collapsed: HashSet<PathBuf>) -> Self {
        let mut root = Folder::default();
        for file in files {
            let mut folder = &mut root;
            for part in file.path.parent().into_iter().flatten() {
                folder = folder.dirs.entry(part.to_owned()).or_default();
            }
            folder.files.push(file);
        }
        let mut nodes = Vec::new();
        flatten(root, Path::new(""), 0, &mut nodes);
        collapsed.retain(|p| {
            nodes
                .iter()
                .any(|n| matches!(n, Node::Dir { path, .. } if path == p))
        });
        let mut tree = Self {
            nodes,
            rows: Vec::new(),
            collapsed,
        };
        tree.update_rows();
        tree
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Visible nodes, top to bottom.
    pub fn visible(&self) -> impl Iterator<Item = &Node> {
        self.rows.iter().map(|&i| &self.nodes[i])
    }

    pub fn node(&self, row: usize) -> Option<&Node> {
        self.nodes.get(self.index(row)?)
    }

    fn index(&self, row: usize) -> Option<usize> {
        self.rows.get(row).copied()
    }

    /// Whether the folder at `row` is open; `None` for files.
    pub fn is_open(&self, row: usize) -> Option<bool> {
        match self.node(row)? {
            Node::Dir { path, .. } => Some(!self.collapsed.contains(path)),
            Node::File { .. } => None,
        }
    }

    /// Opens or closes the folder at `row`; returns whether anything changed.
    pub fn set_open(&mut self, row: usize, open: bool) -> bool {
        let Some(Node::Dir { path, .. }) = self.index(row).map(|i| &self.nodes[i]) else {
            return false;
        };
        let changed = if open {
            self.collapsed.remove(path)
        } else {
            self.collapsed.insert(path.clone())
        };
        if changed {
            self.update_rows();
        }
        changed
    }

    /// Row of the folder containing `row`.
    pub fn parent(&self, row: usize) -> Option<usize> {
        let depth = self.node(row)?.depth();
        (0..row)
            .rev()
            .find(|&r| self.node(r).is_some_and(|n| n.depth() < depth))
    }

    /// Visible row of the folder or file at `path`.
    pub fn find(&self, path: &Path) -> Option<usize> {
        self.visible().position(|n| n.path() == path)
    }

    /// Node index of the file at `row`.
    pub fn file_at(&self, row: usize) -> Option<usize> {
        let i = self.index(row)?;
        matches!(self.nodes[i], Node::File { .. }).then_some(i)
    }

    /// Node index of the file at `path`, even inside a closed folder.
    pub fn find_file(&self, path: &Path) -> Option<usize> {
        self.nodes
            .iter()
            .position(|n| matches!(n, Node::File { entry, .. } if entry.path == path))
    }

    /// The file at node index `node`, as returned by [`Tree::file_at`] or [`Tree::find_file`].
    pub fn file(&self, node: usize) -> Option<&FileEntry> {
        match self.nodes.get(node)? {
            Node::File { entry, .. } => Some(entry),
            Node::Dir { .. } => None,
        }
    }

    /// Every file with its node index, including files inside closed folders.
    pub fn files(&self) -> impl Iterator<Item = (usize, &FileEntry)> {
        self.nodes.iter().enumerate().filter_map(|(i, n)| match n {
            Node::File { entry, .. } => Some((i, entry)),
            Node::Dir { .. } => None,
        })
    }

    /// Opens every folder above `path`; returns the row it is then shown at.
    pub fn reveal(&mut self, path: &Path) -> Option<usize> {
        let before = self.collapsed.len();
        self.collapsed.retain(|dir| !path.starts_with(dir));
        if self.collapsed.len() != before {
            self.update_rows();
        }
        self.find(path)
    }

    pub fn into_collapsed(self) -> HashSet<PathBuf> {
        self.collapsed
    }

    fn update_rows(&mut self) {
        self.rows.clear();
        let mut i = 0;
        while let Some(node) = self.nodes.get(i) {
            self.rows.push(i);
            i = match node {
                Node::Dir { path, end, .. } if self.collapsed.contains(path) => *end,
                _ => i + 1,
            };
        }
    }
}

/// Appends `folder`'s subfolders, then its files, at `depth`; returns their `(added, removed)`.
fn flatten(folder: Folder, path: &Path, depth: usize, nodes: &mut Vec<Node>) -> (u32, u32) {
    let mut totals = (0, 0);
    for (name, mut dir) in folder.dirs {
        let mut path = path.join(&name);
        let mut label = name.to_string_lossy().into_owned();
        while dir.files.is_empty() && dir.dirs.len() == 1 {
            let (name, only) = dir.dirs.pop_first().expect("exactly one subfolder");
            path.push(&name);
            label.push('/');
            label.push_str(&name.to_string_lossy());
            dir = only;
        }
        // Placeholder until the subtree's totals and end are known.
        let at = nodes.len();
        nodes.push(Node::Dir {
            path: PathBuf::new(),
            label: Box::default(),
            depth,
            end: 0,
            added: 0,
            removed: 0,
        });
        let (added, removed) = flatten(dir, &path, depth + 1, nodes);
        nodes[at] = Node::Dir {
            path,
            label: label.into(),
            depth,
            end: nodes.len(),
            added,
            removed,
        };
        totals = (totals.0 + added, totals.1 + removed);
    }
    let mut files = folder.files;
    files.sort_by(|a, b| a.path.file_name().cmp(&b.path.file_name()));
    for entry in files {
        totals = (totals.0 + entry.added, totals.1 + entry.removed);
        nodes.push(Node::File { depth, entry });
    }
    totals
}

#[cfg(test)]
mod tests {
    use zdiff_core::Status;

    use super::*;

    fn entry(path: &str) -> FileEntry {
        FileEntry {
            path: path.into(),
            status: Status::Modified,
            added: 2,
            removed: 1,
            change: 0,
        }
    }

    fn tree(paths: &[&str]) -> Tree {
        Tree::new(paths.iter().copied().map(entry).collect(), HashSet::new())
    }

    /// Visible rows as `depth:label`, folders ending in `/`.
    fn outline(tree: &Tree) -> Vec<String> {
        tree.visible()
            .map(|n| match n {
                Node::Dir { label, depth, .. } => format!("{depth}:{label}/"),
                Node::File { entry, depth } => {
                    let name = entry.path.file_name().unwrap().to_string_lossy();
                    format!("{depth}:{name}")
                }
            })
            .collect()
    }

    #[test]
    fn folders_come_first_and_nest() {
        let t = tree(&["b/x.rs", "top.rs", "a/z.rs", "a/y.rs"]);
        assert_eq!(
            outline(&t),
            ["0:a/", "1:y.rs", "1:z.rs", "0:b/", "1:x.rs", "0:top.rs"]
        );
    }

    #[test]
    fn single_child_folders_merge() {
        assert_eq!(outline(&tree(&["a/b/c/x.rs"])), ["0:a/b/c/", "1:x.rs"]);
        assert_eq!(
            outline(&tree(&["a/b/x.rs", "a/c/y.rs"])),
            ["0:a/", "1:b/", "2:x.rs", "1:c/", "2:y.rs"]
        );
    }

    #[test]
    fn folder_totals_include_every_descendant() {
        let t = tree(&["a/x.rs", "a/b/y.rs", "a/b/z.rs"]);
        let Some(Node::Dir { added, removed, .. }) = t.node(0) else {
            panic!("first row is a folder");
        };
        assert_eq!((*added, *removed), (6, 3));
    }

    #[test]
    fn closing_hides_the_subtree_and_survives_rebuilds() {
        let mut t = tree(&["a/b/x.rs", "a/c/y.rs", "top.rs"]);
        assert!(t.set_open(0, false));
        assert!(!t.set_open(0, false), "already closed");
        assert_eq!(outline(&t), ["0:a/", "0:top.rs"]);
        assert_eq!(t.is_open(0), Some(false));
        assert_eq!(t.is_open(1), None, "files have no open state");

        let files = ["a/b/x.rs", "a/new.rs", "top.rs"].map(entry).into();
        let t = Tree::new(files, t.into_collapsed());
        assert_eq!(outline(&t), ["0:a/", "0:top.rs"]);
        let t = Tree::new(vec![entry("top.rs")], t.into_collapsed());
        assert!(t.into_collapsed().is_empty(), "gone folders are forgotten");
    }

    #[test]
    fn parent_and_find() {
        let mut t = tree(&["a/b/x.rs", "a/c/y.rs"]);
        assert_eq!(t.parent(2), Some(1), "x.rs is in a/b");
        assert_eq!(t.parent(1), Some(0));
        assert_eq!(t.parent(0), None);
        assert_eq!(t.find(Path::new("a/c")), Some(3));
        let y = t.find_file(Path::new("a/c/y.rs")).unwrap();
        t.set_open(0, false);
        assert_eq!(t.find(Path::new("a/c/y.rs")), None, "hidden");
        assert_eq!(
            t.file(y).map(|f| f.path.as_path()),
            Some(Path::new("a/c/y.rs"))
        );
    }

    #[test]
    fn reveal_opens_closed_parents_and_files_sees_inside_them() {
        let mut t = tree(&["a/b/x.rs", "a/c/y.rs", "top.rs"]);
        t.set_open(0, false);
        assert_eq!(t.files().count(), 3, "hidden files still listed");
        assert_eq!(t.reveal(Path::new("a/c/y.rs")), Some(4));
        assert_eq!(t.is_open(0), Some(true));
        assert_eq!(t.reveal(Path::new("gone.rs")), None);
    }
}
