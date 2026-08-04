//! Document storage abstraction over the `.allod/` directory tree.
//!
//! The `DocStore` trait provides a unified interface for reading, writing,
//! listing, and removing documents regardless of the underlying storage
//! mechanism. Concrete implementations include `FsStore` for filesystem-backed
//! storage and `MemStore` for in-memory storage with optional persistence.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::fs;

/// Storage abstraction over the `.allod/` document tree. Paths are
/// relative to the `.allod/` root and always use `/` separators, e.g.
/// `"changesets/ab12.yaml"`, `"HEAD"`. Implementations are synchronous;
/// asynchronous hosts (the WASM bridge) persist around the trait.
pub trait DocStore: Send {
    /// Read a document; `Ok(None)` when absent.
    fn read(&self, path: &str) -> Result<Option<String>, String>;
    /// Write (create or replace) a document, creating parents.
    fn write(&self, path: &str, text: &str) -> Result<(), String>;
    /// Names (not paths) of documents directly under `dir`, sorted.
    fn list(&self, dir: &str) -> Result<Vec<String>, String>;
    /// Remove a document; removing an absent document is Ok.
    fn remove(&self, path: &str) -> Result<(), String>;
}

/// Filesystem-backed document store, rooted at `.allod/` within a graph directory.
pub struct FsStore {
    root: PathBuf,
}

impl FsStore {
    /// Create a new FsStore, initializing the root directory.
    /// `dir` is the graph directory; the store roots at `dir/.allod`.
    pub fn create(dir: &Path) -> Result<FsStore, String> {
        let root = dir.join(".allod");
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        Ok(FsStore { root })
    }

    /// Open an existing FsStore without checking for existence beyond root.
    /// `dir` is the graph directory; the store roots at `dir/.allod`.
    pub fn open(dir: &Path) -> Result<FsStore, String> {
        let root = dir.join(".allod");
        Ok(FsStore { root })
    }
}

impl DocStore for FsStore {
    fn read(&self, path: &str) -> Result<Option<String>, String> {
        let full_path = self.root.join(path);
        match fs::read_to_string(&full_path) {
            Ok(content) => Ok(Some(content)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", full_path.display())),
        }
    }

    fn write(&self, path: &str, text: &str) -> Result<(), String> {
        let full_path = self.root.join(path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&full_path, text).map_err(|e| format!("{}: {e}", full_path.display()))
    }

    fn list(&self, dir: &str) -> Result<Vec<String>, String> {
        let full_path = self.root.join(dir);
        match fs::read_dir(&full_path) {
            Ok(entries) => {
                let mut names = Vec::new();
                for entry in entries {
                    let entry = entry.map_err(|e| e.to_string())?;
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(name) = path.file_name() {
                            if let Some(name_str) = name.to_str() {
                                names.push(name_str.to_string());
                            }
                        }
                    }
                }
                names.sort();
                Ok(names)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn remove(&self, path: &str) -> Result<(), String> {
        let full_path = self.root.join(path);
        match fs::remove_file(&full_path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", full_path.display())),
        }
    }
}

/// In-memory document store with optional persistence support.
pub struct MemStore {
    docs: Mutex<BTreeMap<String, String>>,
}

impl MemStore {
    /// Create a new, empty MemStore.
    pub fn new() -> MemStore {
        MemStore {
            docs: Mutex::new(BTreeMap::new()),
        }
    }

    /// Dump all documents as a sorted vector of (path, text) pairs.
    pub fn dump(&self) -> Vec<(String, String)> {
        let docs = self.docs.lock().unwrap();
        docs.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// Bulk-load documents from a vector of (path, text) pairs.
    ///
    /// Documents are merged into the store (not replaced): existing paths
    /// whose keys are absent from `docs` are preserved. Duplicate keys in
    /// `docs` are last-write-wins in iteration order.
    pub fn load(&self, docs: Vec<(String, String)>) {
        let mut store = self.docs.lock().unwrap();
        for (path, text) in docs {
            store.insert(path, text);
        }
    }
}

impl Default for MemStore {
    fn default() -> Self {
        Self::new()
    }
}

impl DocStore for MemStore {
    fn read(&self, path: &str) -> Result<Option<String>, String> {
        let docs = self.docs.lock().unwrap();
        Ok(docs.get(path).cloned())
    }

    fn write(&self, path: &str, text: &str) -> Result<(), String> {
        let mut docs = self.docs.lock().unwrap();
        docs.insert(path.to_string(), text.to_string());
        Ok(())
    }

    fn list(&self, dir: &str) -> Result<Vec<String>, String> {
        let docs = self.docs.lock().unwrap();
        let prefix = format!("{}/", dir);
        let mut names = Vec::new();
        for key in docs.keys() {
            if let Some(rest) = key.strip_prefix(&prefix) {
                if !rest.contains('/') {
                    names.push(rest.to_string());
                }
            }
        }
        names.sort();
        Ok(names)
    }

    fn remove(&self, path: &str) -> Result<(), String> {
        let mut docs = self.docs.lock().unwrap();
        docs.remove(path);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conformance(store: &dyn DocStore) {
        assert_eq!(store.read("HEAD").unwrap(), None);
        store.write("HEAD", "sha256:abc").unwrap();
        assert_eq!(store.read("HEAD").unwrap().as_deref(), Some("sha256:abc"));
        store.write("changesets/b.yaml", "b: 1\n").unwrap();
        store.write("changesets/a.yaml", "a: 1\n").unwrap();
        assert_eq!(
            store.list("changesets").unwrap(),
            vec!["a.yaml".to_string(), "b.yaml".to_string()]
        );
        assert_eq!(store.list("proposals").unwrap(), Vec::<String>::new());
        store.remove("changesets/a.yaml").unwrap();
        store.remove("changesets/a.yaml").unwrap(); // idempotent
        assert_eq!(store.list("changesets").unwrap(), vec!["b.yaml".to_string()]);
    }

    #[test]
    fn memstore_conforms() {
        conformance(&MemStore::new());
    }

    #[test]
    fn fsstore_conforms() {
        let dir = std::env::temp_dir().join(format!("allod-docstore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = FsStore::create(&dir).unwrap();
        conformance(&store);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn memstore_dump_load_round_trips() {
        let a = MemStore::new();
        a.write("HEAD", "h").unwrap();
        a.write("keys/o.yaml", "k: 1\n").unwrap();
        let b = MemStore::new();
        b.load(a.dump());
        assert_eq!(b.dump(), a.dump());
    }
}
