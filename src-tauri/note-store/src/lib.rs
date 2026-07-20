//! Pure-logic persistence layer for the Sticky Notes desktop app.
//!
//! This crate deliberately has **no dependency on Tauri**. All note data and
//! all disk I/O live here so the logic can be unit- and integration-tested
//! quickly, without spinning up a webview. The Tauri layer (`sticky_notes_lib`)
//! is a thin shell that calls into [`NoteStore`].
//!
//! ## Reliability guarantees
//! * **Atomic writes** — notes are written to a temporary file which is then
//!   renamed over the real file. A crash mid-write can never truncate the
//!   existing note file.
//! * **Corruption recovery** — if the note file cannot be parsed, it is moved
//!   aside (`*.corrupt-<timestamp>`) and the app starts with an empty store
//!   instead of refusing to launch.
//! * **Missing file / first run** — a non-existent file is treated as an empty
//!   store; the parent directory is created on demand.
//! * **Forward/backward compatibility** — missing optional fields fall back to
//!   sensible defaults instead of failing the whole load.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Default note width in logical pixels.
pub const DEFAULT_WIDTH: f64 = 260.0;
/// Default note height in logical pixels.
pub const DEFAULT_HEIGHT: f64 = 260.0;
/// Default note color key (matches a CSS theme in the frontend).
pub const DEFAULT_COLOR: &str = "yellow";

fn default_color() -> String {
    DEFAULT_COLOR.to_string()
}
fn default_width() -> f64 {
    DEFAULT_WIDTH
}
fn default_height() -> f64 {
    DEFAULT_HEIGHT
}

/// A single sticky note and the geometry of its window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// Stable unique identifier. Also used as the window label (`note-<id>`).
    pub id: String,
    /// The note text.
    #[serde(default)]
    pub content: String,
    /// Color theme key, e.g. `"yellow"`.
    #[serde(default = "default_color")]
    pub color: String,
    /// Window x position (logical pixels).
    #[serde(default)]
    pub x: f64,
    /// Window y position (logical pixels).
    #[serde(default)]
    pub y: f64,
    /// Window width (logical pixels).
    #[serde(default = "default_width")]
    pub width: f64,
    /// Window height (logical pixels).
    #[serde(default = "default_height")]
    pub height: f64,
    /// Creation time, unix milliseconds.
    #[serde(default)]
    pub created_at: u64,
    /// Last update time, unix milliseconds.
    #[serde(default)]
    pub updated_at: u64,
}

/// Errors that can occur while loading or mutating the store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// No note exists with the given id.
    #[error("note not found: {0}")]
    NotFound(String),
    /// An underlying filesystem error (permissions, disk full, etc.).
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),
    /// The note data could not be (de)serialized.
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

/// An in-memory collection of notes backed by a JSON file on disk.
///
/// Every mutating method persists to disk before returning, so the on-disk file
/// always reflects the last successful operation.
#[derive(Debug)]
pub struct NoteStore {
    path: PathBuf,
    notes: HashMap<String, Note>,
}

impl NoteStore {
    /// Load notes from `path`.
    ///
    /// * A missing file yields an empty store (and its parent dir is created).
    /// * An unparseable file is backed up to `*.corrupt-<ts>` and an empty
    ///   store is returned, so a bad file never blocks startup.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let notes = match fs::read_to_string(&path) {
            Ok(text) if text.trim().is_empty() => HashMap::new(),
            Ok(text) => match serde_json::from_str::<Vec<Note>>(&text) {
                Ok(list) => list.into_iter().map(|n| (n.id.clone(), n)).collect(),
                Err(_) => {
                    // Corrupted file: move it aside and start fresh instead of crashing.
                    let backup = backup_path(&path, now_ms());
                    let _ = fs::rename(&path, &backup);
                    HashMap::new()
                }
            },
            Err(ref e) if e.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e.into()),
        };

        Ok(Self { path, notes })
    }

    /// Path of the backing JSON file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Number of notes currently stored.
    pub fn len(&self) -> usize {
        self.notes.len()
    }

    /// Whether the store holds no notes.
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    /// Borrow a single note by id.
    pub fn get(&self, id: &str) -> Option<&Note> {
        self.notes.get(id)
    }

    /// All notes, ordered by creation time (stable for window restore order).
    pub fn all(&self) -> Vec<Note> {
        let mut list: Vec<Note> = self.notes.values().cloned().collect();
        list.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
        list
    }

    /// Create a new blank note positioned at `(x, y)`, persist, and return it.
    pub fn create(&mut self, x: f64, y: f64) -> Result<Note, StoreError> {
        let now = now_ms();
        let note = Note {
            id: new_id(),
            content: String::new(),
            color: DEFAULT_COLOR.to_string(),
            x,
            y,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            created_at: now,
            updated_at: now,
        };
        self.notes.insert(note.id.clone(), note.clone());
        self.save()?;
        Ok(note)
    }

    /// Replace a note's text.
    pub fn set_content(&mut self, id: &str, content: String) -> Result<(), StoreError> {
        {
            let note = self
                .notes
                .get_mut(id)
                .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
            note.content = content;
            note.updated_at = now_ms();
        }
        self.save()
    }

    /// Change a note's color theme.
    pub fn set_color(&mut self, id: &str, color: String) -> Result<(), StoreError> {
        {
            let note = self
                .notes
                .get_mut(id)
                .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
            note.color = color;
            note.updated_at = now_ms();
        }
        self.save()
    }

    /// Update a note's window geometry (position and size).
    pub fn set_geometry(
        &mut self,
        id: &str,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<(), StoreError> {
        {
            let note = self
                .notes
                .get_mut(id)
                .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
            note.x = x;
            note.y = y;
            // Guard against degenerate sizes from spurious resize events.
            note.width = width.max(120.0);
            note.height = height.max(100.0);
            note.updated_at = now_ms();
        }
        self.save()
    }

    /// Delete a note. Returns [`StoreError::NotFound`] if it does not exist.
    pub fn delete(&mut self, id: &str) -> Result<(), StoreError> {
        self.notes
            .remove(id)
            .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        self.save()
    }

    /// Serialize all notes and write them to disk atomically.
    ///
    /// Writes to `<file>.tmp` and renames over the target so readers never see
    /// a partially written file.
    pub fn save(&self) -> Result<(), StoreError> {
        let list = self.all();
        let json = serde_json::to_string_pretty(&list)?;

        let tmp = tmp_path(&self.path);
        fs::write(&tmp, json.as_bytes())?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// Milliseconds since the unix epoch (saturating to 0 before 1970).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Process-wide monotonic counter so ids created in the same millisecond differ.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a compact, collision-resistant id.
fn new_id() -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{:x}", now_ms(), n)
}

/// `<path>.tmp` — used for atomic writes.
fn tmp_path(path: &Path) -> PathBuf {
    let mut s: OsString = path.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}

/// `<path>.corrupt-<ts>` — used to quarantine an unreadable note file.
fn backup_path(path: &Path, ts: u64) -> PathBuf {
    let mut s: OsString = path.as_os_str().to_owned();
    s.push(format!(".corrupt-{ts}"));
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn store_in(dir: &Path) -> NoteStore {
        NoteStore::load(dir.join("notes.json")).expect("load")
    }

    #[test]
    fn missing_file_loads_empty() {
        let dir = tempdir().unwrap();
        let store = store_in(dir.path());
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn load_creates_parent_directory() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("notes.json");
        let store = NoteStore::load(&nested).expect("load nested");
        assert!(store.is_empty());
        assert!(nested.parent().unwrap().exists());
    }

    #[test]
    fn create_persists_to_disk() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let note = store.create(10.0, 20.0).unwrap();

        assert_eq!(store.len(), 1);
        assert_eq!(note.x, 10.0);
        assert_eq!(note.color, DEFAULT_COLOR);
        assert!(dir.path().join("notes.json").exists());

        // A freshly loaded store sees the same note.
        let reloaded = store_in(dir.path());
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded.get(&note.id).unwrap(), &note);
    }

    #[test]
    fn edit_content_and_color_persist() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let id = store.create(0.0, 0.0).unwrap().id;

        store.set_content(&id, "hello world".into()).unwrap();
        store.set_color(&id, "pink".into()).unwrap();

        let reloaded = store_in(dir.path());
        let note = reloaded.get(&id).unwrap();
        assert_eq!(note.content, "hello world");
        assert_eq!(note.color, "pink");
    }

    #[test]
    fn geometry_updates_persist_and_clamp() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let id = store.create(0.0, 0.0).unwrap().id;

        store.set_geometry(&id, 300.0, 400.0, 1.0, 1.0).unwrap();
        let reloaded = store_in(dir.path());
        let note = reloaded.get(&id).unwrap();
        assert_eq!(note.x, 300.0);
        assert_eq!(note.y, 400.0);
        // Degenerate sizes are clamped to sane minimums.
        assert_eq!(note.width, 120.0);
        assert_eq!(note.height, 100.0);
    }

    #[test]
    fn delete_removes_and_persists() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let id = store.create(0.0, 0.0).unwrap().id;
        assert_eq!(store.len(), 1);

        store.delete(&id).unwrap();
        assert_eq!(store.len(), 0);

        let reloaded = store_in(dir.path());
        assert!(reloaded.is_empty());
    }

    #[test]
    fn delete_missing_is_not_found() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let err = store.delete("does-not-exist").unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)));
    }

    #[test]
    fn edit_missing_is_not_found() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let err = store.set_content("nope", "x".into()).unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)));
    }

    #[test]
    fn ids_are_unique() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let a = store.create(0.0, 0.0).unwrap().id;
        let b = store.create(0.0, 0.0).unwrap().id;
        let c = store.create(0.0, 0.0).unwrap().id;
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn all_is_ordered_by_creation() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let ids: Vec<String> = (0..5).map(|_| store.create(0.0, 0.0).unwrap().id).collect();
        let ordered: Vec<String> = store.all().into_iter().map(|n| n.id).collect();
        assert_eq!(ids, ordered);
    }

    #[test]
    fn corrupted_file_is_quarantined_and_recovers() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("notes.json");
        fs::write(&path, b"{ this is not valid json ]").unwrap();

        // Load must succeed with an empty store, not error.
        let store = NoteStore::load(&path).expect("recover from corruption");
        assert!(store.is_empty());

        // The bad file was moved aside for the user to recover manually.
        let quarantined = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains("corrupt"));
        assert!(quarantined, "expected a .corrupt-* backup file");
    }

    #[test]
    fn empty_file_loads_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("notes.json");
        fs::write(&path, b"   \n  ").unwrap();
        let store = NoteStore::load(&path).expect("load");
        assert!(store.is_empty());
    }

    #[test]
    fn missing_optional_fields_use_defaults() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("notes.json");
        // Only id + content provided; everything else should default.
        fs::write(&path, br#"[{"id":"n1","content":"hi"}]"#).unwrap();
        let store = NoteStore::load(&path).expect("load");
        let note = store.get("n1").unwrap();
        assert_eq!(note.content, "hi");
        assert_eq!(note.color, DEFAULT_COLOR);
        assert_eq!(note.width, DEFAULT_WIDTH);
        assert_eq!(note.height, DEFAULT_HEIGHT);
    }

    #[test]
    fn save_leaves_no_temp_file() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        store.create(0.0, 0.0).unwrap();
        let tmp = tmp_path(&dir.path().join("notes.json"));
        assert!(!tmp.exists(), "temp file should be renamed away after save");
    }
}
