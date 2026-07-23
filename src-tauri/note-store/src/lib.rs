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

pub mod crypto;
pub mod similarity;

/// Default note width in logical pixels.
pub const DEFAULT_WIDTH: f64 = 260.0;
/// Default note height in logical pixels.
pub const DEFAULT_HEIGHT: f64 = 260.0;
/// Default note color key (matches a CSS theme in the frontend).
pub const DEFAULT_COLOR: &str = "yellow";
/// Default note opacity (fully opaque).
pub const DEFAULT_OPACITY: f64 = 1.0;
/// Legible lower bound for note opacity — below this a note is unreadable.
pub const MIN_OPACITY: f64 = 0.3;

fn default_color() -> String {
    DEFAULT_COLOR.to_string()
}
fn default_width() -> f64 {
    DEFAULT_WIDTH
}
fn default_height() -> f64 {
    DEFAULT_HEIGHT
}
fn default_opacity() -> f64 {
    DEFAULT_OPACITY
}

/// A custom per-note colour palette (overrides the preset `color`), typically
/// derived from a background image. Values are CSS colours (e.g. `"#33290f"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    /// Note surface colour.
    pub note: String,
    /// Title-bar colour.
    pub header: String,
    /// Text colour.
    pub ink: String,
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
    /// Window opacity in `MIN_OPACITY..=1.0` (1.0 = fully opaque).
    #[serde(default = "default_opacity")]
    pub opacity: f64,
    /// Id of the group/collection ("clipboard") this note belongs to, if any.
    #[serde(default)]
    pub group_id: Option<String>,
    /// Whether this note is hidden behind the master password.
    #[serde(default)]
    pub protected: bool,
    /// File names (relative to the `attachments/` dir) of images on this note.
    #[serde(default)]
    pub attachments: Vec<String>,
    /// Custom colour palette from a template/background image (overrides `color`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette: Option<Palette>,
    /// Background image file (in `attachments/`) shown behind the note, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg_image: Option<String>,
    /// Font family key for the text (e.g. `"sans"`, `"serif"`, `"mono"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Font size in px for the text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<u32>,
    /// Line-height multiplier for the text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f64>,
    /// Whether the note stays above other windows (always-on-top).
    #[serde(default)]
    pub pinned: bool,
    /// When the note was moved to trash (unix ms); `None` while it's live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<u64>,
    /// Sealed content for a protected note when it is locked / stored at rest.
    /// Invariant: `enc.is_some()` iff `content` is *not* the plaintext (the note
    /// is locked); when unlocked in memory, `enc` is `None` and `content` is the
    /// decrypted text. Non-protected notes always have `enc == None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enc: Option<crypto::Sealed>,
}

impl Note {
    /// Whether the note is currently in the trash.
    pub fn is_trashed(&self) -> bool {
        self.deleted_at.is_some()
    }
}

/// Default width/height for a clip's stack window.
pub const DEFAULT_CLIP_WIDTH: f64 = 300.0;
pub const DEFAULT_CLIP_HEIGHT: f64 = 340.0;
fn default_clip_width() -> f64 {
    DEFAULT_CLIP_WIDTH
}
fn default_clip_height() -> f64 {
    DEFAULT_CLIP_HEIGHT
}

/// A "clip": a stack that collapses several notes into a single window. Which
/// notes belong to it is tracked on the notes themselves (a note is in clip `C`
/// iff `note.group_id == Some(C.id)`); this struct only holds the stack window's
/// own name, geometry and colour. Persisted to a sibling `clips.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// Stable unique id. Also used as the window label (`clip-<id>`).
    pub id: String,
    /// User-facing name of the clip.
    #[serde(default)]
    pub name: String,
    /// Stack window x position (logical pixels).
    #[serde(default)]
    pub x: f64,
    /// Stack window y position (logical pixels).
    #[serde(default)]
    pub y: f64,
    /// Stack window width (logical pixels).
    #[serde(default = "default_clip_width")]
    pub width: f64,
    /// Stack window height (logical pixels).
    #[serde(default = "default_clip_height")]
    pub height: f64,
    /// Colour theme key for the stack chrome.
    #[serde(default = "default_color")]
    pub color: String,
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
    /// A cryptographic operation failed.
    #[error("crypto error: {0}")]
    Crypto(#[from] crypto::CryptoError),
    /// The operation needs the store unlocked, but it is locked.
    #[error("store is locked")]
    Locked,
    /// The operation needs a master password to have been set, but none exists.
    #[error("no master password set")]
    NoMaster,
}

/// An in-memory collection of notes backed by a JSON file on disk.
///
/// Every mutating method persists to disk before returning, so the on-disk file
/// always reflects the last successful operation.
#[derive(Debug)]
pub struct NoteStore {
    path: PathBuf,
    /// Sibling file holding the master-password credential, if one is set.
    master_path: PathBuf,
    /// Sibling file holding the clip (stack) definitions.
    clips_path: PathBuf,
    notes: HashMap<String, Note>,
    /// Clip (stack) definitions, keyed by clip id.
    clips: HashMap<String, Clip>,
    /// The master credential loaded from disk, if a master password was set.
    master: Option<crypto::MasterCred>,
    /// The derived session key once unlocked this run; `None` means locked.
    key: Option<[u8; 32]>,
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

        // The master credential lives beside the notes file. A missing or
        // unreadable credential simply means "no password protection set up".
        let master_path = master_path_for(&path);
        let master = match fs::read_to_string(&master_path) {
            Ok(text) => serde_json::from_str::<crypto::MasterCred>(&text).ok(),
            Err(_) => None,
        };

        // Clip definitions live in a sibling `clips.json`. A missing/unreadable
        // file just means "no clips" — never blocks startup.
        let clips_path = clips_path_for(&path);
        let clips = match fs::read_to_string(&clips_path) {
            Ok(text) => serde_json::from_str::<Vec<Clip>>(&text)
                .map(|list| list.into_iter().map(|c| (c.id.clone(), c)).collect())
                .unwrap_or_default(),
            Err(_) => HashMap::new(),
        };

        Ok(Self {
            path,
            master_path,
            clips_path,
            notes,
            clips,
            master,
            // Always start locked; the user must enter the password this run.
            key: None,
        })
    }

    /// Path of the backing JSON file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Number of live (non-trashed) notes.
    pub fn len(&self) -> usize {
        self.notes.values().filter(|n| !n.is_trashed()).count()
    }

    /// Whether the store holds no live notes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Borrow a single note by id (live or trashed).
    pub fn get(&self, id: &str) -> Option<&Note> {
        self.notes.get(id)
    }

    /// All live notes, ordered by creation time (stable for window restore order).
    pub fn all(&self) -> Vec<Note> {
        let mut list: Vec<Note> = self
            .notes
            .values()
            .filter(|n| !n.is_trashed())
            .cloned()
            .collect();
        list.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
        list
    }

    /// All trashed notes, most-recently-deleted first.
    pub fn trash(&self) -> Vec<Note> {
        let mut list: Vec<Note> = self
            .notes
            .values()
            .filter(|n| n.is_trashed())
            .cloned()
            .collect();
        list.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at).then_with(|| a.id.cmp(&b.id)));
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
            opacity: DEFAULT_OPACITY,
            group_id: None,
            protected: false,
            attachments: Vec::new(),
            palette: None,
            bg_image: None,
            font: None,
            font_size: None,
            line_height: None,
            pinned: false,
            deleted_at: None,
            enc: None,
        };
        self.notes.insert(note.id.clone(), note.clone());
        self.save()?;
        Ok(note)
    }

    /// Replace a note's text. Refuses to edit a protected note while it is locked,
    /// which would otherwise clobber its sealed content with plaintext.
    pub fn set_content(&mut self, id: &str, content: String) -> Result<(), StoreError> {
        let locked = self.key.is_none();
        {
            let note = self
                .notes
                .get_mut(id)
                .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
            if note.protected && locked {
                return Err(StoreError::Locked);
            }
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

    /// Set a note's window opacity, clamped to the legible `MIN_OPACITY..=1.0`
    /// range so a note can never be made invisible and unrecoverable.
    pub fn set_opacity(&mut self, id: &str, opacity: f64) -> Result<(), StoreError> {
        {
            let note = self
                .notes
                .get_mut(id)
                .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
            note.opacity = opacity.clamp(MIN_OPACITY, 1.0);
            note.updated_at = now_ms();
        }
        self.save()
    }

    /// Set (or clear) a note's custom palette and background image. Clearing both
    /// reverts the note to its preset `color`.
    pub fn set_palette(
        &mut self,
        id: &str,
        palette: Option<Palette>,
        bg_image: Option<String>,
    ) -> Result<(), StoreError> {
        {
            let note = self
                .notes
                .get_mut(id)
                .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
            note.palette = palette;
            note.bg_image = bg_image;
            note.updated_at = now_ms();
        }
        self.save()
    }

    /// Set (or clear, with `None`) a note's typography. Any field left `None`
    /// falls back to the app default in the frontend.
    pub fn set_typography(
        &mut self,
        id: &str,
        font: Option<String>,
        font_size: Option<u32>,
        line_height: Option<f64>,
    ) -> Result<(), StoreError> {
        {
            let note = self
                .notes
                .get_mut(id)
                .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
            note.font = font;
            note.font_size = font_size.map(|s| s.clamp(9, 48));
            note.line_height = line_height.map(|h| h.clamp(1.0, 3.0));
            note.updated_at = now_ms();
        }
        self.save()
    }

    /// Assign the note to a group, or clear its group with `None`.
    pub fn set_group(&mut self, id: &str, group_id: Option<String>) -> Result<(), StoreError> {
        {
            let note = self
                .notes
                .get_mut(id)
                .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
            note.group_id = group_id;
            note.updated_at = now_ms();
        }
        self.save()
    }

    // ---- Clips (stacks) --------------------------------------------------

    /// Borrow a clip by id.
    pub fn get_clip(&self, id: &str) -> Option<&Clip> {
        self.clips.get(id)
    }

    /// All clips, ordered by id (stable for restore order).
    pub fn all_clips(&self) -> Vec<Clip> {
        let mut list: Vec<Clip> = self.clips.values().cloned().collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        list
    }

    /// Notes belonging to `clip_id`, ordered by creation (the stack order).
    pub fn clip_notes(&self, clip_id: &str) -> Vec<Note> {
        let mut list: Vec<Note> = self
            .notes
            .values()
            .filter(|n| n.group_id.as_deref() == Some(clip_id) && !n.is_trashed())
            .cloned()
            .collect();
        list.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
        list
    }

    /// Create a new (empty) clip with the given name and stack-window geometry.
    pub fn create_clip(
        &mut self,
        name: String,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<Clip, StoreError> {
        let clip = Clip {
            id: new_id(),
            name: if name.trim().is_empty() {
                "Clip".to_string()
            } else {
                name
            },
            x,
            y,
            width: width.max(200.0),
            height: height.max(180.0),
            color: DEFAULT_COLOR.to_string(),
        };
        self.clips.insert(clip.id.clone(), clip.clone());
        self.save_clips()?;
        Ok(clip)
    }

    /// Update a clip's stack-window geometry.
    pub fn set_clip_geometry(
        &mut self,
        id: &str,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<(), StoreError> {
        {
            let clip = self
                .clips
                .get_mut(id)
                .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
            clip.x = x;
            clip.y = y;
            clip.width = width.max(200.0);
            clip.height = height.max(180.0);
        }
        self.save_clips()
    }

    /// Rename a clip.
    pub fn rename_clip(&mut self, id: &str, name: String) -> Result<(), StoreError> {
        {
            let clip = self
                .clips
                .get_mut(id)
                .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
            clip.name = name;
        }
        self.save_clips()
    }

    /// Delete a clip and un-group all of its members (they become standalone).
    pub fn delete_clip(&mut self, id: &str) -> Result<(), StoreError> {
        let mut changed = false;
        for note in self.notes.values_mut() {
            if note.group_id.as_deref() == Some(id) {
                note.group_id = None;
                note.updated_at = now_ms();
                changed = true;
            }
        }
        self.clips.remove(id);
        if changed {
            self.save()?;
        }
        self.save_clips()
    }

    /// Put a note into an existing clip.
    pub fn clip_note(&mut self, note_id: &str, clip_id: &str) -> Result<(), StoreError> {
        if !self.clips.contains_key(clip_id) {
            return Err(StoreError::NotFound(clip_id.to_string()));
        }
        self.set_group(note_id, Some(clip_id.to_string()))
    }

    /// Remove a note from its clip. If the clip is left with fewer than two
    /// members it is dissolved (the last member becomes standalone). Returns the
    /// clip the note left and whether that clip was dissolved, or `None` if the
    /// note wasn't clipped.
    pub fn unclip_note(&mut self, note_id: &str) -> Result<Option<(String, bool)>, StoreError> {
        let clip_id = match self.notes.get(note_id).and_then(|n| n.group_id.clone()) {
            Some(c) => c,
            None => return Ok(None),
        };
        self.set_group(note_id, None)?;

        let dissolved = self.clip_notes(&clip_id).len() < 2;
        if dissolved {
            self.delete_clip(&clip_id)?;
        }
        Ok(Some((clip_id, dissolved)))
    }

    /// Serialize clips and write them to their sibling file atomically.
    fn save_clips(&self) -> Result<(), StoreError> {
        let list = self.all_clips();
        let json = serde_json::to_string_pretty(&list)?;
        let tmp = tmp_path(&self.clips_path);
        fs::write(&tmp, json.as_bytes())?;
        fs::rename(&tmp, &self.clips_path)?;
        Ok(())
    }

    /// Mark a note as protected (behind the master password) or not.
    ///
    /// Protecting requires a master password to exist and the store to be
    /// unlocked (we must be able to seal the content). Un-protecting requires the
    /// note to be currently decrypted — you can't un-protect a note you can't read.
    pub fn set_protected(&mut self, id: &str, protected: bool) -> Result<(), StoreError> {
        if protected {
            if self.master.is_none() {
                return Err(StoreError::NoMaster);
            }
            if self.key.is_none() {
                return Err(StoreError::Locked);
            }
        }
        {
            let note = self
                .notes
                .get_mut(id)
                .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
            if !protected && note.enc.is_some() {
                // Still sealed => locked => can't safely un-protect.
                return Err(StoreError::Locked);
            }
            note.protected = protected;
            note.updated_at = now_ms();
        }
        self.save()
    }

    /// Whether a master password has been configured.
    pub fn has_master(&self) -> bool {
        self.master.is_some()
    }

    /// Whether the store is currently locked: a master exists but no key is held.
    pub fn is_locked(&self) -> bool {
        self.master.is_some() && self.key.is_none()
    }

    /// Set the master password for the first time and start the session unlocked.
    /// Returns [`StoreError::NoMaster`]-adjacent behavior is avoided: it errors if
    /// a master password already exists (changing it is a separate concern).
    pub fn set_master_password(&mut self, password: &str) -> Result<(), StoreError> {
        if self.master.is_some() {
            // A master already exists; refuse to silently overwrite it.
            return Err(StoreError::Crypto(crypto::CryptoError::Locked));
        }
        let (cred, key) = crypto::MasterCred::create(password)?;
        self.master = Some(cred);
        self.key = Some(key);
        self.save_master()
    }

    /// Try to unlock the store with `password`. On success the session key is held
    /// and all protected notes are decrypted in memory. Returns `false` (without
    /// error) if the password is wrong.
    pub fn unlock(&mut self, password: &str) -> Result<bool, StoreError> {
        let master = self.master.as_ref().ok_or(StoreError::NoMaster)?;
        let key = match master.unlock(password)? {
            Some(k) => k,
            None => return Ok(false),
        };
        // Decrypt every protected note's sealed content into memory.
        for note in self.notes.values_mut() {
            if note.protected {
                if let Some(sealed) = note.enc.take() {
                    let plain = crypto::open(&key, &sealed)?;
                    note.content = String::from_utf8(plain)
                        .map_err(|_| StoreError::Crypto(crypto::CryptoError::Malformed))?;
                }
            }
        }
        self.key = Some(key);
        Ok(true)
    }

    /// Lock the vault now: re-seal every unlocked protected note in memory and
    /// drop the session key, so protected content becomes unreadable (and its
    /// windows show the locked state) until the next successful [`unlock`]. Safe
    /// to call when already locked or when no master password is set.
    pub fn lock(&mut self) -> Result<(), StoreError> {
        if let Some(key) = self.key.take() {
            for note in self.notes.values_mut() {
                if note.protected && note.enc.is_none() {
                    let sealed = crypto::seal(&key, note.content.as_bytes())?;
                    note.content = String::new();
                    note.enc = Some(sealed);
                }
            }
        }
        Ok(())
    }

    /// Encrypt arbitrary bytes (an attachment file) with the session key.
    /// Errors with [`StoreError::Locked`] if the store is locked.
    pub fn encrypt_bytes(&self, plaintext: &[u8]) -> Result<Vec<u8>, StoreError> {
        let key = self.key.as_ref().ok_or(StoreError::Locked)?;
        Ok(crypto::seal_bytes(key, plaintext)?)
    }

    /// Decrypt bytes sealed by [`encrypt_bytes`] with the session key.
    pub fn decrypt_bytes(&self, data: &[u8]) -> Result<Vec<u8>, StoreError> {
        let key = self.key.as_ref().ok_or(StoreError::Locked)?;
        Ok(crypto::open_bytes(key, data)?)
    }

    /// Write the master credential to its sibling file atomically.
    fn save_master(&self) -> Result<(), StoreError> {
        let cred = match &self.master {
            Some(c) => c,
            None => return Ok(()),
        };
        let json = serde_json::to_string_pretty(cred)?;
        let tmp = tmp_path(&self.master_path);
        fs::write(&tmp, json.as_bytes())?;
        fs::rename(&tmp, &self.master_path)?;
        Ok(())
    }

    /// Produce the on-disk form of a note: protected notes are sealed so their
    /// plaintext never touches disk. Fails closed if a protected note is somehow
    /// plaintext while the store is locked.
    fn persist_form(&self, note: &Note) -> Result<Note, StoreError> {
        if !note.protected {
            return Ok(note.clone());
        }
        if note.enc.is_some() {
            // Already sealed (loaded locked and never opened) — keep as-is.
            return Ok(note.clone());
        }
        match &self.key {
            Some(key) => {
                let sealed = crypto::seal(key, note.content.as_bytes())?;
                let mut p = note.clone();
                p.content = String::new();
                p.enc = Some(sealed);
                Ok(p)
            }
            None => Err(StoreError::Locked),
        }
    }

    /// Record an attachment file name on a note (idempotent — no duplicates).
    pub fn add_attachment(&mut self, id: &str, file: String) -> Result<(), StoreError> {
        {
            let note = self
                .notes
                .get_mut(id)
                .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
            if !note.attachments.contains(&file) {
                note.attachments.push(file);
            }
            note.updated_at = now_ms();
        }
        self.save()
    }

    /// Remove an attachment file name from a note. The caller is responsible for
    /// deleting the underlying file on disk.
    pub fn remove_attachment(&mut self, id: &str, file: &str) -> Result<(), StoreError> {
        {
            let note = self
                .notes
                .get_mut(id)
                .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
            note.attachments.retain(|f| f != file);
            note.updated_at = now_ms();
        }
        self.save()
    }

    /// Merge the `source` note into `target` (Smart Duplicate Detection's "Merge"
    /// action): append the source's text under a divider unless it's identical or
    /// empty, carry over any attachments, then remove the source. Returns the
    /// removed source note so the caller can close its window. Attachment files are
    /// *not* deleted — they now belong to `target`.
    pub fn merge(&mut self, source_id: &str, target_id: &str) -> Result<Note, StoreError> {
        if source_id == target_id {
            return Err(StoreError::NotFound(source_id.to_string()));
        }
        // Both must exist before we mutate anything.
        let source = self
            .notes
            .get(source_id)
            .ok_or_else(|| StoreError::NotFound(source_id.to_string()))?
            .clone();
        if !self.notes.contains_key(target_id) {
            return Err(StoreError::NotFound(target_id.to_string()));
        }

        {
            let target = self.notes.get_mut(target_id).expect("checked above");
            let src = source.content.trim();
            if !src.is_empty() && target.content.trim() != src {
                if target.content.trim().is_empty() {
                    target.content = source.content.clone();
                } else {
                    target.content = format!("{}\n\n---\n\n{}", target.content.trim_end(), src);
                }
            }
            for a in &source.attachments {
                if !target.attachments.contains(a) {
                    target.attachments.push(a.clone());
                }
            }
            target.updated_at = now_ms();
        }

        let removed = self.notes.remove(source_id).expect("checked above");
        self.save()?;
        Ok(removed)
    }

    /// Delete a note. Returns [`StoreError::NotFound`] if it does not exist.
    /// Returns the deleted note so the caller can clean up its attachment files.
    pub fn delete(&mut self, id: &str) -> Result<Note, StoreError> {
        {
            let note = self
                .notes
                .get_mut(id)
                .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
            note.deleted_at = Some(now_ms());
        }
        self.save()?;
        Ok(self.notes.get(id).cloned().expect("just soft-deleted"))
    }

    /// Restore a note from the trash.
    pub fn restore(&mut self, id: &str) -> Result<(), StoreError> {
        {
            let note = self
                .notes
                .get_mut(id)
                .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
            note.deleted_at = None;
            note.updated_at = now_ms();
        }
        self.save()
    }

    /// Permanently remove a note. Returns it so the caller can clean up its
    /// attachment files.
    pub fn purge(&mut self, id: &str) -> Result<Note, StoreError> {
        let removed = self
            .notes
            .remove(id)
            .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        self.save()?;
        Ok(removed)
    }

    /// Permanently remove every trashed note. Returns them for attachment cleanup.
    pub fn empty_trash(&mut self) -> Result<Vec<Note>, StoreError> {
        let ids: Vec<String> = self
            .notes
            .values()
            .filter(|n| n.is_trashed())
            .map(|n| n.id.clone())
            .collect();
        let removed: Vec<Note> = ids.iter().filter_map(|id| self.notes.remove(id)).collect();
        self.save()?;
        Ok(removed)
    }

    /// Pin or unpin a note (always-on-top).
    pub fn set_pinned(&mut self, id: &str, pinned: bool) -> Result<(), StoreError> {
        {
            let note = self
                .notes
                .get_mut(id)
                .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
            note.pinned = pinned;
            note.updated_at = now_ms();
        }
        self.save()
    }

    /// Serialize all notes and write them to disk atomically.
    ///
    /// Writes to `<file>.tmp` and renames over the target so readers never see
    /// a partially written file.
    pub fn save(&self) -> Result<(), StoreError> {
        // Persist *every* note, including trashed ones (so the trash survives a
        // restart) — ordered stably for a clean diff.
        let mut list: Vec<&Note> = self.notes.values().collect();
        list.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
        // Seal protected notes so their plaintext is never written to disk.
        let persisted: Vec<Note> = list
            .iter()
            .map(|n| self.persist_form(n))
            .collect::<Result<_, _>>()?;
        let json = serde_json::to_string_pretty(&persisted)?;

        let tmp = tmp_path(&self.path);
        fs::write(&tmp, json.as_bytes())?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// `<dir>/master.json` — the master-password credential beside the notes file.
fn master_path_for(notes_path: &Path) -> PathBuf {
    match notes_path.parent() {
        Some(dir) => dir.join("master.json"),
        None => PathBuf::from("master.json"),
    }
}

/// `<dir>/clips.json` — the clip (stack) definitions beside the notes file.
fn clips_path_for(notes_path: &Path) -> PathBuf {
    match notes_path.parent() {
        Some(dir) => dir.join("clips.json"),
        None => PathBuf::from("clips.json"),
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
    fn palette_and_typography_persist_and_clamp() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let id = store.create(0.0, 0.0).unwrap().id;

        let pal = Palette {
            note: "#332a0f".into(),
            header: "#4a4212".into(),
            ink: "#f4edc4".into(),
        };
        store
            .set_palette(&id, Some(pal.clone()), Some("bg-1.png".into()))
            .unwrap();
        store.set_typography(&id, Some("serif".into()), Some(99), Some(5.0)).unwrap();

        let reloaded = store_in(dir.path());
        let n = reloaded.get(&id).unwrap();
        assert_eq!(n.palette.as_ref().unwrap().ink, "#f4edc4");
        assert_eq!(n.bg_image.as_deref(), Some("bg-1.png"));
        assert_eq!(n.font.as_deref(), Some("serif"));
        assert_eq!(n.font_size, Some(48)); // clamped from 99
        assert_eq!(n.line_height, Some(3.0)); // clamped from 5.0

        // Clearing reverts to the preset colour + defaults.
        let mut store = reloaded;
        store.set_palette(&id, None, None).unwrap();
        store.set_typography(&id, None, None, None).unwrap();
        let n = store.get(&id).unwrap();
        assert!(n.palette.is_none() && n.bg_image.is_none() && n.font.is_none());
    }

    #[test]
    fn opacity_persists_and_clamps_to_legible_range() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let id = store.create(0.0, 0.0).unwrap().id;

        store.set_opacity(&id, 0.05).unwrap(); // below floor
        assert_eq!(store.get(&id).unwrap().opacity, MIN_OPACITY);

        store.set_opacity(&id, 2.0).unwrap(); // above ceiling
        assert_eq!(store.get(&id).unwrap().opacity, 1.0);

        store.set_opacity(&id, 0.6).unwrap();
        let reloaded = store_in(dir.path());
        assert_eq!(reloaded.get(&id).unwrap().opacity, 0.6);
    }

    #[test]
    fn clip_membership_and_geometry_round_trip() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let a = store.create(0.0, 0.0).unwrap().id;
        let b = store.create(0.0, 0.0).unwrap().id;
        let clip = store
            .create_clip("Work".into(), 10.0, 20.0, 300.0, 340.0)
            .unwrap();
        store.clip_note(&a, &clip.id).unwrap();
        store.clip_note(&b, &clip.id).unwrap();

        assert_eq!(store.clip_notes(&clip.id).len(), 2);
        assert_eq!(store.get(&a).unwrap().group_id.as_deref(), Some(clip.id.as_str()));

        store.set_clip_geometry(&clip.id, 50.0, 60.0, 280.0, 300.0).unwrap();

        // clips.json + notes.json both round-trip through a reload.
        let reloaded = store_in(dir.path());
        let rc = reloaded.get_clip(&clip.id).unwrap();
        assert_eq!(rc.name, "Work");
        assert_eq!(rc.x, 50.0);
        assert_eq!(rc.width, 280.0);
        assert_eq!(reloaded.clip_notes(&clip.id).len(), 2);
    }

    #[test]
    fn unclip_dissolves_clip_with_one_member_left() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let a = store.create(0.0, 0.0).unwrap().id;
        let b = store.create(0.0, 0.0).unwrap().id;
        let clip = store.create_clip("C".into(), 0.0, 0.0, 300.0, 340.0).unwrap();
        store.clip_note(&a, &clip.id).unwrap();
        store.clip_note(&b, &clip.id).unwrap();

        let res = store.unclip_note(&a).unwrap();
        assert_eq!(res, Some((clip.id.clone(), true)));
        assert!(store.get_clip(&clip.id).is_none());
        assert_eq!(store.get(&a).unwrap().group_id, None);
        assert_eq!(store.get(&b).unwrap().group_id, None, "last member freed too");
    }

    #[test]
    fn delete_clip_ungroups_its_members() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let a = store.create(0.0, 0.0).unwrap().id;
        let clip = store.create_clip("C".into(), 0.0, 0.0, 300.0, 340.0).unwrap();
        store.clip_note(&a, &clip.id).unwrap();

        store.delete_clip(&clip.id).unwrap();
        assert!(store.get_clip(&clip.id).is_none());
        assert_eq!(store.get(&a).unwrap().group_id, None);
    }

    #[test]
    fn clip_note_requires_existing_clip() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let a = store.create(0.0, 0.0).unwrap().id;
        assert!(matches!(
            store.clip_note(&a, "no-such-clip"),
            Err(StoreError::NotFound(_))
        ));
    }

    #[test]
    fn group_assignment_and_clear_persist() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let id = store.create(0.0, 0.0).unwrap().id;

        store.set_group(&id, Some("work".into())).unwrap();
        assert_eq!(store.get(&id).unwrap().group_id.as_deref(), Some("work"));

        store.set_group(&id, None).unwrap();
        let reloaded = store_in(dir.path());
        assert_eq!(reloaded.get(&id).unwrap().group_id, None);
    }

    #[test]
    fn attachments_add_dedupe_and_remove() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let id = store.create(0.0, 0.0).unwrap().id;

        store.add_attachment(&id, "a.png".into()).unwrap();
        store.add_attachment(&id, "a.png".into()).unwrap(); // idempotent
        store.add_attachment(&id, "b.png".into()).unwrap();
        assert_eq!(store.get(&id).unwrap().attachments, vec!["a.png", "b.png"]);

        store.remove_attachment(&id, "a.png").unwrap();
        let reloaded = store_in(dir.path());
        assert_eq!(reloaded.get(&id).unwrap().attachments, vec!["b.png"]);
    }

    #[test]
    fn merge_appends_content_moves_attachments_and_removes_source() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let source = store.create(0.0, 0.0).unwrap().id;
        let target = store.create(0.0, 0.0).unwrap().id;
        store.set_content(&source, "buy milk".into()).unwrap();
        store.set_content(&target, "grocery list".into()).unwrap();
        store.add_attachment(&source, "receipt.png".into()).unwrap();

        let removed = store.merge(&source, &target).unwrap();
        assert_eq!(removed.id, source);
        assert!(store.get(&source).is_none());

        let merged = store.get(&target).unwrap();
        assert!(merged.content.contains("grocery list"));
        assert!(merged.content.contains("buy milk"));
        assert!(merged.attachments.contains(&"receipt.png".to_string()));
    }

    #[test]
    fn merge_skips_identical_content() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let source = store.create(0.0, 0.0).unwrap().id;
        let target = store.create(0.0, 0.0).unwrap().id;
        store.set_content(&source, "same text".into()).unwrap();
        store.set_content(&target, "same text".into()).unwrap();

        store.merge(&source, &target).unwrap();
        // Not duplicated into "same text\n\n---\n\nsame text".
        assert_eq!(store.get(&target).unwrap().content, "same text");
    }

    #[test]
    fn delete_trashes_then_restore_and_purge() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let id = store.create(0.0, 0.0).unwrap().id;
        store.set_content(&id, "keep me".into()).unwrap();

        // Soft-delete: gone from live views, still recoverable from the trash.
        store.delete(&id).unwrap();
        assert_eq!(store.len(), 0);
        assert!(store.all().is_empty());
        assert_eq!(store.trash().len(), 1);
        assert!(store.get(&id).unwrap().is_trashed());

        // Survives a reload while trashed.
        let mut reloaded = store_in(dir.path());
        assert_eq!(reloaded.trash().len(), 1);

        // Restore brings it back to life.
        reloaded.restore(&id).unwrap();
        assert_eq!(reloaded.len(), 1);
        assert!(reloaded.trash().is_empty());
        assert_eq!(reloaded.get(&id).unwrap().content, "keep me");

        // Purge removes it for good.
        reloaded.delete(&id).unwrap();
        let removed = reloaded.purge(&id).unwrap();
        assert_eq!(removed.id, id);
        assert!(reloaded.get(&id).is_none());
    }

    #[test]
    fn empty_trash_removes_only_trashed() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let keep = store.create(0.0, 0.0).unwrap().id;
        let a = store.create(0.0, 0.0).unwrap().id;
        let b = store.create(0.0, 0.0).unwrap().id;
        store.delete(&a).unwrap();
        store.delete(&b).unwrap();

        let purged = store.empty_trash().unwrap();
        assert_eq!(purged.len(), 2);
        assert!(store.trash().is_empty());
        assert_eq!(store.len(), 1);
        assert!(store.get(&keep).is_some());
    }

    #[test]
    fn pin_persists() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let id = store.create(0.0, 0.0).unwrap().id;
        store.set_pinned(&id, true).unwrap();
        assert!(store.get(&id).unwrap().pinned);

        let reloaded = store_in(dir.path());
        assert!(reloaded.get(&id).unwrap().pinned);
    }

    #[test]
    fn delete_returns_the_removed_note() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let id = store.create(0.0, 0.0).unwrap().id;
        store.add_attachment(&id, "pic.png".into()).unwrap();

        let removed = store.delete(&id).unwrap();
        assert_eq!(removed.id, id);
        assert_eq!(removed.attachments, vec!["pic.png"]);
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
    fn protected_note_seals_on_disk_and_unlocks_in_memory() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let id = store.create(0.0, 0.0).unwrap().id;
        store.set_content(&id, "launch codes: 0000".into()).unwrap();

        store.set_master_password("correct horse").unwrap();
        assert!(store.has_master());
        assert!(!store.is_locked(), "just-set master starts unlocked");

        store.set_protected(&id, true).unwrap();

        // The plaintext must not appear anywhere in the on-disk file.
        let on_disk = fs::read_to_string(dir.path().join("notes.json")).unwrap();
        assert!(!on_disk.contains("launch codes"), "plaintext leaked to disk");
        assert!(on_disk.contains("nonce"), "expected a sealed blob on disk");

        // Reload: the store is locked and the protected note is unreadable.
        let mut reloaded = store_in(dir.path());
        assert!(reloaded.is_locked());
        let locked = reloaded.get(&id).unwrap();
        assert!(locked.protected);
        assert_eq!(locked.content, "", "locked content must be empty in memory");
        assert!(locked.enc.is_some());

        // Wrong password fails without error; right password reveals the content.
        assert!(!reloaded.unlock("nope").unwrap());
        assert!(reloaded.is_locked());
        assert!(reloaded.unlock("correct horse").unwrap());
        assert!(!reloaded.is_locked());
        assert_eq!(reloaded.get(&id).unwrap().content, "launch codes: 0000");
        assert!(reloaded.get(&id).unwrap().enc.is_none());
    }

    #[test]
    fn encrypt_bytes_round_trips_when_unlocked_and_fails_when_locked() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        store.set_master_password("pw").unwrap();

        let blob = store.encrypt_bytes(b"image bytes").unwrap();
        assert_ne!(blob, b"image bytes");
        assert_eq!(store.decrypt_bytes(&blob).unwrap(), b"image bytes");

        // A freshly reloaded (locked) store cannot encrypt or decrypt.
        let locked = store_in(dir.path());
        assert!(matches!(locked.encrypt_bytes(b"x"), Err(StoreError::Locked)));
        assert!(matches!(locked.decrypt_bytes(&blob), Err(StoreError::Locked)));
    }

    #[test]
    fn lock_reseals_protected_notes_and_hides_content() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let id = store.create(0.0, 0.0).unwrap().id;
        store.set_content(&id, "my secret".into()).unwrap();
        store.set_master_password("pw").unwrap();
        store.set_protected(&id, true).unwrap();

        // Unlocked right after protecting: content is readable.
        assert!(!store.is_locked());
        assert_eq!(store.get(&id).unwrap().content, "my secret");

        // Lock now: content is hidden and the note reports as locked.
        store.lock().unwrap();
        assert!(store.is_locked());
        let n = store.get(&id).unwrap();
        assert_eq!(n.content, "");
        assert!(n.enc.is_some());

        // The same password unlocks it again.
        assert!(store.unlock("pw").unwrap());
        assert_eq!(store.get(&id).unwrap().content, "my secret");
    }

    #[test]
    fn cannot_edit_or_protect_while_locked() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let id = store.create(0.0, 0.0).unwrap().id;
        store.set_content(&id, "sensitive".into()).unwrap();
        store.set_master_password("pw").unwrap();
        store.set_protected(&id, true).unwrap();

        // Reload -> locked. Editing and un-protecting must both be refused.
        let mut reloaded = store_in(dir.path());
        assert!(matches!(
            reloaded.set_content(&id, "hack".into()),
            Err(StoreError::Locked)
        ));
        assert!(matches!(
            reloaded.set_protected(&id, false),
            Err(StoreError::Locked)
        ));
    }

    #[test]
    fn protecting_without_master_is_rejected() {
        let dir = tempdir().unwrap();
        let mut store = store_in(dir.path());
        let id = store.create(0.0, 0.0).unwrap().id;
        assert!(matches!(
            store.set_protected(&id, true),
            Err(StoreError::NoMaster)
        ));
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
