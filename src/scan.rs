//! Parallel recursive directory scanner and the in-memory size tree.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rayon::prelude::*;

/// Live counters the UI reads while a scan is running.
#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub errors: AtomicU64,
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

pub type NodeId = u32;

pub struct Node {
    pub name: String,
    pub parent: Option<NodeId>,
    /// Children sorted by size, largest first.
    pub children: Vec<NodeId>,
    pub size: u64,
    pub files: u64,
    pub is_dir: bool,
    /// The folder could not be read (permissions, etc.); its size is a lower bound.
    pub error: bool,
    pub expanded: bool,
}

pub struct Tree {
    pub root_path: PathBuf,
    pub nodes: Vec<Node>,
}

pub const ROOT: NodeId = 0;

impl Tree {
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id as usize]
    }

    /// Full filesystem path of a node.
    pub fn path_of(&self, id: NodeId) -> PathBuf {
        let mut parts = Vec::new();
        let mut cur = id;
        while let Some(parent) = self.node(cur).parent {
            parts.push(self.node(cur).name.as_str());
            cur = parent;
        }
        let mut path = self.root_path.clone();
        for part in parts.iter().rev() {
            path.push(part);
        }
        path
    }

    /// Removes a node (after it was deleted on disk) and subtracts its totals
    /// from every ancestor, re-sorting siblings so ordering stays correct.
    pub fn remove(&mut self, id: NodeId) {
        let Some(parent) = self.node(id).parent else {
            return; // never remove the root
        };
        let (size, files) = (self.node(id).size, self.node(id).files);
        self.node_mut(parent).children.retain(|&c| c != id);

        let mut cur = Some(parent);
        while let Some(p) = cur {
            let n = self.node_mut(p);
            n.size = n.size.saturating_sub(size);
            n.files = n.files.saturating_sub(files);
            cur = n.parent;
        }
        // Ancestors shrank, so their position among siblings may have changed.
        let mut cur = self.node(parent).parent;
        while let Some(p) = cur {
            self.sort_children(p);
            cur = self.node(p).parent;
        }
    }

    fn sort_children(&mut self, id: NodeId) {
        let mut children = std::mem::take(&mut self.node_mut(id).children);
        children.sort_by(|&a, &b| self.node(b).size.cmp(&self.node(a).size));
        self.node_mut(id).children = children;
    }
}

/// Intermediate result produced by the parallel walk, flattened into a `Tree` afterwards.
struct RawDir {
    name: String,
    size: u64,
    files: u64,
    error: bool,
    dirs: Vec<RawDir>,
    file_entries: Vec<(String, u64)>,
}

fn scan_dir(path: &Path, name: String, progress: &Progress) -> RawDir {
    let mut raw = RawDir {
        name,
        size: 0,
        files: 0,
        error: false,
        dirs: Vec::new(),
        file_entries: Vec::new(),
    };
    if progress.cancelled() {
        return raw;
    }
    progress.dirs.fetch_add(1, Ordering::Relaxed);

    let entries = match fs::read_dir(path) {
        Ok(e) => e,
        Err(_) => {
            progress.errors.fetch_add(1, Ordering::Relaxed);
            raw.error = true;
            return raw;
        }
    };

    let mut subdirs = Vec::new();
    let mut bytes = 0u64;
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        // Skip symlinks and junctions so we never loop or double count.
        if ft.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if ft.is_dir() {
            subdirs.push((entry.path(), name));
        } else {
            // On Windows this comes from the directory listing itself, no extra syscall.
            let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
            bytes += len;
            raw.file_entries.push((name, len));
        }
    }
    progress
        .files
        .fetch_add(raw.file_entries.len() as u64, Ordering::Relaxed);
    progress.bytes.fetch_add(bytes, Ordering::Relaxed);

    raw.dirs = subdirs
        .into_par_iter()
        .map(|(p, n)| scan_dir(&p, n, progress))
        .collect();

    raw.size = bytes + raw.dirs.iter().map(|d| d.size).sum::<u64>();
    raw.files = raw.file_entries.len() as u64 + raw.dirs.iter().map(|d| d.files).sum::<u64>();
    raw
}

/// Scans `root` recursively using all cores. Blocks until done or cancelled.
pub fn scan(root: &Path, progress: &Progress) -> Tree {
    let raw = scan_dir(root, root.display().to_string(), progress);
    let mut tree = Tree {
        root_path: root.to_path_buf(),
        nodes: Vec::with_capacity((raw.files + 1) as usize),
    };
    flatten(&mut tree, raw, None);
    tree.node_mut(ROOT).expanded = true;
    tree
}

fn flatten(tree: &mut Tree, raw: RawDir, parent: Option<NodeId>) -> NodeId {
    let id = tree.nodes.len() as NodeId;
    tree.nodes.push(Node {
        name: raw.name,
        parent,
        children: Vec::with_capacity(raw.dirs.len() + raw.file_entries.len()),
        size: raw.size,
        files: raw.files,
        is_dir: true,
        error: raw.error,
        expanded: false,
    });

    let mut children = Vec::with_capacity(raw.dirs.len() + raw.file_entries.len());
    for dir in raw.dirs {
        children.push(flatten(tree, dir, Some(id)));
    }
    for (name, size) in raw.file_entries {
        let fid = tree.nodes.len() as NodeId;
        tree.nodes.push(Node {
            name,
            parent: Some(id),
            children: Vec::new(),
            size,
            files: 1,
            is_dir: false,
            error: false,
            expanded: false,
        });
        children.push(fid);
    }
    tree.node_mut(id).children = children;
    tree.sort_children(id);
    id
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

pub fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_sizes_and_removes() {
        let dir = std::env::temp_dir().join(format!("dirscope-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("big/inner")).unwrap();
        fs::create_dir_all(dir.join("small")).unwrap();
        fs::write(dir.join("big/inner/a.bin"), vec![0u8; 5000]).unwrap();
        fs::write(dir.join("big/b.bin"), vec![0u8; 1000]).unwrap();
        fs::write(dir.join("small/c.bin"), vec![0u8; 10]).unwrap();
        fs::write(dir.join("root.txt"), vec![0u8; 3]).unwrap();

        let progress = Progress::default();
        let mut tree = scan(&dir, &progress);
        let root = tree.node(ROOT);
        assert_eq!(root.size, 6013);
        assert_eq!(root.files, 4);
        // Largest child first.
        let first = root.children[0];
        assert_eq!(tree.node(first).name, "big");
        assert_eq!(tree.path_of(first), dir.join("big"));

        tree.remove(first);
        assert_eq!(tree.node(ROOT).size, 13);
        assert_eq!(tree.node(ROOT).files, 2);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn formats() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_count(1234567), "1,234,567");
    }
}
