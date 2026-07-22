//! Tauri shell for the Sticky Notes app.
//!
//! This module is intentionally thin: all note data and disk I/O live in the
//! [`note_store`] crate (which is unit-tested on its own). Here we wire that
//! store to the desktop: one window per note, a tray icon, a global shortcut,
//! auto-launch on login, and a single-instance guard.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;

use note_store::similarity::{self, Cluster, Match};
use note_store::{Clip, Note, NoteStore};

/// File name (inside the OS app-data directory) that notes are persisted to.
const NOTES_FILE: &str = "notes.json";
/// Sub-directory (inside the OS app-data directory) holding attachment files.
const ATTACHMENTS_DIR: &str = "attachments";
/// Window label for the single Library Hub window.
const HUB_LABEL: &str = "hub";
/// Extra WebView2 args on every window. In practice WebView2 already shares one
/// renderer across the app's same-origin windows, so these are conservative
/// defaults rather than a big memory win: `--process-per-site` keeps that sharing
/// explicit and the V8 heap cap bounds each renderer. The disable-features list
/// preserves Tauri's default (dropping it would re-enable those features).
const WEBVIEW_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --process-per-site --js-flags=--max-old-space-size=48";
/// Similarity threshold above which two notes are treated as near-duplicates.
const DUPLICATE_THRESHOLD: f64 = 0.35;
/// Similarity threshold for grouping notes in Smart Organization.
const CLUSTER_THRESHOLD: f64 = 0.18;

/// Milliseconds since the unix epoch — used to name attachment files uniquely.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The `attachments/` directory inside app-data, created on demand.
fn attachments_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join(ATTACHMENTS_DIR);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Shared application state: the note store behind a mutex so command handlers
/// (which may run on different threads) serialize their access to disk.
pub struct AppState {
    pub store: Mutex<NoteStore>,
}

/// Window label used for a note, e.g. `note-1a2b-0`.
fn window_label(id: &str) -> String {
    format!("note-{id}")
}

/// Open (or focus, if it already exists) the window for a single note.
///
/// The window is built on the event loop via `run_on_main_thread`. Creating a
/// WebView2 window synchronously from a command handler blocks the message pump
/// the webview needs to finish initializing, which leaves it rendered blank/white
/// until the next app launch. Deferring the build to the main loop avoids that.
fn open_note_window(app: &AppHandle, note: &Note) -> tauri::Result<()> {
    let app = app.clone();
    let note = note.clone();
    app.clone().run_on_main_thread(move || {
        let label = window_label(&note.id);
        if let Some(existing) = app.get_webview_window(&label) {
            let _ = existing.show();
            let _ = existing.set_focus();
            return;
        }
        let _ = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App("index.html".into()))
            .title("Sticky Note")
            .inner_size(note.width, note.height)
            .position(note.x, note.y)
            .min_inner_size(180.0, 160.0)
            .decorations(false)
            .resizable(true)
            .skip_taskbar(true)
            .additional_browser_args(WEBVIEW_ARGS)
            .build();
    })
}

/// Open (or focus) the single Library Hub window — the aggregate surface for
/// Surprise Me and Smart Organization, which need the whole note library.
/// Built on the event loop for the same reason as [`open_note_window`].
fn open_hub_window(app: &AppHandle) -> tauri::Result<()> {
    let app = app.clone();
    app.clone().run_on_main_thread(move || {
        if let Some(existing) = app.get_webview_window(HUB_LABEL) {
            let _ = existing.show();
            let _ = existing.set_focus();
            return;
        }
        let _ = WebviewWindowBuilder::new(&app, HUB_LABEL, WebviewUrl::App("hub.html".into()))
            .title("Sticky Notes — Library")
            .inner_size(460.0, 560.0)
            .min_inner_size(360.0, 400.0)
            .resizable(true)
            .skip_taskbar(false)
            .additional_browser_args(WEBVIEW_ARGS)
            .build();
    })
}

/// Window label for a clip's stack window, e.g. `clip-1a2b-0`.
fn clip_window_label(id: &str) -> String {
    format!("clip-{id}")
}

/// Close a note's individual window if it is open (it now lives inside a stack).
fn close_note_window(app: &AppHandle, note_id: &str) {
    if let Some(win) = app.get_webview_window(&window_label(note_id)) {
        let _ = win.close();
    }
}

/// Open (or focus) the stack window for a clip. Same async/`run_on_main_thread`
/// creation as [`open_note_window`] — building synchronously renders it blank.
fn open_clip_window(app: &AppHandle, clip: &Clip) -> tauri::Result<()> {
    let app = app.clone();
    let clip = clip.clone();
    app.clone().run_on_main_thread(move || {
        let label = clip_window_label(&clip.id);
        if let Some(existing) = app.get_webview_window(&label) {
            let _ = existing.show();
            let _ = existing.set_focus();
            return;
        }
        let _ = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App("stack.html".into()))
            .title("Sticky Notes — Clip")
            .inner_size(clip.width, clip.height)
            .position(clip.x, clip.y)
            .min_inner_size(200.0, 180.0)
            .decorations(false)
            .resizable(true)
            .skip_taskbar(true)
            .additional_browser_args(WEBVIEW_ARGS)
            .build();
    })
}

/// Open every window the current state calls for: an individual window per
/// ungrouped note, and one stack window per clip that still has members.
fn open_all_windows(app: &AppHandle) {
    let (notes, clips) = {
        let state = app.state::<AppState>();
        let store = match state.store.lock() {
            Ok(store) => store,
            Err(_) => return,
        };
        (store.all(), store.all_clips())
    };
    for note in &notes {
        if note.group_id.is_some() {
            continue; // clipped notes live inside a stack window, not their own
        }
        if let Some(win) = app.get_webview_window(&window_label(&note.id)) {
            let _ = win.show();
            let _ = win.set_focus();
        } else {
            let _ = open_note_window(app, note);
        }
    }
    for clip in &clips {
        let has_members = notes
            .iter()
            .any(|n| n.group_id.as_deref() == Some(clip.id.as_str()));
        if !has_members {
            continue;
        }
        if let Some(win) = app.get_webview_window(&clip_window_label(&clip.id)) {
            let _ = win.show();
            let _ = win.set_focus();
        } else {
            let _ = open_clip_window(app, clip);
        }
    }
}

/// Fraction of the smaller rectangle covered by the intersection of two rects.
fn overlap_frac(
    ax: f64,
    ay: f64,
    aw: f64,
    ah: f64,
    bx: f64,
    by: f64,
    bw: f64,
    bh: f64,
) -> f64 {
    let ix = (ax + aw).min(bx + bw) - ax.max(bx);
    let iy = (ay + ah).min(by + bh) - ay.max(by);
    if ix <= 0.0 || iy <= 0.0 {
        return 0.0;
    }
    (ix * iy) / (aw * ah).min(bw * bh).max(1.0)
}

/// Create a brand-new note and open its window. Shared by the tray, the global
/// shortcut, the single-instance handler, and the `create_note` command.
fn spawn_new_note(app: &AppHandle) -> Result<Note, String> {
    let note = {
        let state = app.state::<AppState>();
        let mut store = state.store.lock().map_err(|e| e.to_string())?;
        // Cascade new notes so they don't stack perfectly on top of each other.
        let n = store.len();
        let x = 120.0 + (n % 12) as f64 * 26.0;
        let y = 120.0 + (n % 12) as f64 * 26.0;
        store.create(x, y).map_err(|e| e.to_string())?
    };
    open_note_window(app, &note).map_err(|e| e.to_string())?;
    Ok(note)
}

/// Show (and focus) every note and clip window, opening any that aren't open.
fn show_all_notes(app: &AppHandle) {
    open_all_windows(app);
}

// ---------------------------------------------------------------------------
// Commands (callable from the note window frontend via `invoke`)
// ---------------------------------------------------------------------------

/// Return a single note by id (or `null` if it no longer exists).
#[tauri::command]
fn get_note(id: String, state: tauri::State<AppState>) -> Result<Option<Note>, String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    Ok(store.get(&id).cloned())
}

/// Return every note (ordered by creation).
#[tauri::command]
fn list_notes(state: tauri::State<AppState>) -> Result<Vec<Note>, String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    Ok(store.all())
}

/// Create a new note and open its window.
///
/// Async so it runs off the main thread: building a WebView2 window while the
/// main event loop is blocked (as a sync command does) leaves the new webview
/// blank/white. An async command doesn't hold up the loop, so the webview loads.
#[tauri::command]
async fn create_note(app: AppHandle) -> Result<Note, String> {
    spawn_new_note(&app)
}

/// Persist an edit to a note's text.
#[tauri::command]
fn update_note_content(
    id: String,
    content: String,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|e| e.to_string())?;
    store.set_content(&id, content).map_err(|e| e.to_string())
}

/// Persist a note's color theme.
#[tauri::command]
fn update_note_color(
    id: String,
    color: String,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|e| e.to_string())?;
    store.set_color(&id, color).map_err(|e| e.to_string())
}

/// Persist a note window's position and size.
#[tauri::command]
fn update_note_geometry(
    id: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|e| e.to_string())?;
    store
        .set_geometry(&id, x, y, width, height)
        .map_err(|e| e.to_string())
}

/// Delete a note, remove its attachment files, and close its window. If the note
/// was in a clip that now has fewer than two members, the clip is dissolved.
/// Async because dissolving may reopen the freed member's window.
#[tauri::command]
async fn delete_note(app: AppHandle, id: String) -> Result<(), String> {
    let (removed, dissolved_clip) = {
        let state = app.state::<AppState>();
        let mut store = state.store.lock().map_err(|e| e.to_string())?;
        let removed = store.delete(&id).map_err(|e| e.to_string())?;
        let dissolved = match &removed.group_id {
            Some(cid) if store.clip_notes(cid).len() < 2 => {
                store.delete_clip(cid).map_err(|e| e.to_string())?;
                Some(cid.clone())
            }
            _ => None,
        };
        (removed, dissolved)
    };
    // Best-effort cleanup of the note's sidecar attachment files.
    if !removed.attachments.is_empty() {
        if let Ok(dir) = attachments_dir(&app) {
            for file in &removed.attachments {
                let _ = fs::remove_file(dir.join(file));
            }
        }
    }
    close_note_window(&app, &id);
    if let Some(cid) = dissolved_clip {
        if let Some(win) = app.get_webview_window(&clip_window_label(&cid)) {
            let _ = win.close();
        }
        open_all_windows(&app); // reopen the now-standalone last member(s)
    }
    let _ = app.emit("clips-changed", ());
    Ok(())
}

/// Persist a note's window opacity (clamped to a legible range in the store).
#[tauri::command]
fn update_note_opacity(
    id: String,
    opacity: f64,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|e| e.to_string())?;
    store.set_opacity(&id, opacity).map_err(|e| e.to_string())
}

/// Apply a whole-window translucency to the calling note window so lowering the
/// opacity slider genuinely shows the desktop through the note. Uses a Windows
/// layered window (`SetLayeredWindowAttributes`), which — unlike WebView2's
/// per-pixel `transparent` mode — reliably composites the whole window over the
/// desktop. `opacity` is `0.2..=1.0` (1.0 = fully opaque).
#[tauri::command]
fn set_window_opacity(window: tauri::WebviewWindow, opacity: f64) -> Result<(), String> {
    #[cfg(windows)]
    {
        // The HWND as a plain integer so the closure is `Send`.
        let raw = window.hwnd().map_err(|e| e.to_string())?.0 as isize;
        let alpha = (opacity.clamp(0.2, 1.0) * 255.0).round() as u8;
        // Win32 window calls MUST run on the UI thread. Doing them from this
        // command's worker thread sends synchronous messages to the UI thread,
        // which deadlocks if it's mid-window-creation (e.g. a focus change fired
        // while "New note"/"Library" builds a window). Marshal onto the main loop.
        window
            .run_on_main_thread(move || {
                use windows_sys::Win32::Foundation::HWND;
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, GWL_EXSTYLE,
                    LWA_ALPHA, WS_EX_LAYERED,
                };
                let hwnd = raw as HWND;
                unsafe {
                    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                    if alpha >= 255 {
                        // Fully opaque: keep it a normal (non-layered) window.
                        // Applying WS_EX_LAYERED to a freshly-created WebView2
                        // window makes it render blank/white, and a full-opacity
                        // note doesn't need layering — so only layer when actually
                        // translucent (by which point the note is already painted).
                        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex & !(WS_EX_LAYERED as isize));
                    } else {
                        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED as isize);
                        SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(windows))]
    {
        let _ = (window, opacity);
    }
    Ok(())
}

/// Assign a note to a group, or clear it with `null`.
#[tauri::command]
fn update_note_group(
    id: String,
    group_id: Option<String>,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|e| e.to_string())?;
    store.set_group(&id, group_id).map_err(|e| e.to_string())
}

/// Assign many notes to a group at once (Smart Organization's "accept").
#[tauri::command]
fn assign_group(
    note_ids: Vec<String>,
    group_id: Option<String>,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|e| e.to_string())?;
    for id in &note_ids {
        store
            .set_group(id, group_id.clone())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Clips (stacks) — collapse several related notes into one tidy stack window
// ---------------------------------------------------------------------------

/// All clip definitions.
#[tauri::command]
fn list_clips(state: tauri::State<AppState>) -> Result<Vec<Clip>, String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    Ok(store.all_clips())
}

/// A single clip by id.
#[tauri::command]
fn get_clip(clip_id: String, state: tauri::State<AppState>) -> Result<Option<Clip>, String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    Ok(store.get_clip(&clip_id).cloned())
}

/// The notes inside a clip, in stack order — used by the stack window.
#[tauri::command]
fn clip_notes(clip_id: String, state: tauri::State<AppState>) -> Result<Vec<Note>, String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    Ok(store.clip_notes(&clip_id))
}

/// Rename a clip.
#[tauri::command]
fn rename_clip(
    clip_id: String,
    name: String,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|e| e.to_string())?;
    store.rename_clip(&clip_id, name).map_err(|e| e.to_string())
}

/// Persist the stack window's geometry onto its clip.
#[tauri::command]
fn update_clip_geometry(
    clip_id: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|e| e.to_string())?;
    store
        .set_clip_geometry(&clip_id, x, y, width, height)
        .map_err(|e| e.to_string())
}

/// Create a clip from a set of notes: closes their individual windows and opens
/// the stack window. Async because it opens a window.
#[tauri::command]
async fn create_clip_from(
    app: AppHandle,
    note_ids: Vec<String>,
    name: String,
) -> Result<Clip, String> {
    let clip = {
        let state = app.state::<AppState>();
        let mut store = state.store.lock().map_err(|e| e.to_string())?;
        // Anchor the stack window where the first note sits (falls back to a
        // reasonable default).
        let (x, y) = note_ids
            .first()
            .and_then(|id| store.get(id))
            .map(|n| (n.x, n.y))
            .unwrap_or((160.0, 160.0));
        let clip = store
            .create_clip(
                name,
                x,
                y,
                note_store::DEFAULT_CLIP_WIDTH,
                note_store::DEFAULT_CLIP_HEIGHT,
            )
            .map_err(|e| e.to_string())?;
        for id in &note_ids {
            store.clip_note(id, &clip.id).map_err(|e| e.to_string())?;
        }
        clip
    };
    for id in &note_ids {
        close_note_window(&app, id);
    }
    open_clip_window(&app, &clip).map_err(|e| e.to_string())?;
    let _ = app.emit("clips-changed", ());
    Ok(clip)
}

/// Add notes to an existing clip (closing their individual windows).
#[tauri::command]
async fn add_to_clip(
    app: AppHandle,
    note_ids: Vec<String>,
    clip_id: String,
) -> Result<(), String> {
    let clip = {
        let state = app.state::<AppState>();
        let mut store = state.store.lock().map_err(|e| e.to_string())?;
        for id in &note_ids {
            store.clip_note(id, &clip_id).map_err(|e| e.to_string())?;
        }
        store.get_clip(&clip_id).cloned()
    };
    for id in &note_ids {
        close_note_window(&app, id);
    }
    if let Some(clip) = clip {
        open_clip_window(&app, &clip).map_err(|e| e.to_string())?;
    }
    let _ = app.emit("clips-changed", ());
    Ok(())
}

/// Called after a note is dropped: if it now overlaps another ungrouped note or a
/// clip window, clip them together. Returns whether a clip happened.
#[tauri::command]
async fn try_clip_on_drop(app: AppHandle, id: String) -> Result<bool, String> {
    enum Target {
        Note(String),
        Clip(String),
    }
    const THRESHOLD: f64 = 0.4;
    let target = {
        let state = app.state::<AppState>();
        let store = state.store.lock().map_err(|e| e.to_string())?;
        let me = match store.get(&id) {
            Some(n) if n.group_id.is_none() => n.clone(),
            _ => return Ok(false), // gone, or already clipped
        };
        let mut found: Option<Target> = None;
        // Prefer dropping onto an existing clip window.
        for clip in store.all_clips() {
            if store.clip_notes(&clip.id).is_empty() {
                continue;
            }
            if overlap_frac(
                me.x, me.y, me.width, me.height, clip.x, clip.y, clip.width, clip.height,
            ) >= THRESHOLD
            {
                found = Some(Target::Clip(clip.id.clone()));
                break;
            }
        }
        // Otherwise onto another standalone note.
        if found.is_none() {
            for other in store.all() {
                if other.id == id || other.group_id.is_some() {
                    continue;
                }
                if overlap_frac(
                    me.x, me.y, me.width, me.height, other.x, other.y, other.width, other.height,
                ) >= THRESHOLD
                {
                    found = Some(Target::Note(other.id.clone()));
                    break;
                }
            }
        }
        found
    };
    match target {
        Some(Target::Clip(clip_id)) => {
            add_to_clip(app.clone(), vec![id], clip_id).await?;
            Ok(true)
        }
        Some(Target::Note(other_id)) => {
            create_clip_from(app.clone(), vec![id, other_id], "Clip".into()).await?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Take a single note out of its clip and give it back its own window. Dissolves
/// the clip if fewer than two notes remain.
#[tauri::command]
async fn unclip_note(app: AppHandle, note_id: String) -> Result<(), String> {
    let (note, dissolved) = {
        let state = app.state::<AppState>();
        let mut store = state.store.lock().map_err(|e| e.to_string())?;
        let res = store.unclip_note(&note_id).map_err(|e| e.to_string())?;
        (store.get(&note_id).cloned(), res)
    };
    if let Some(note) = note {
        open_note_window(&app, &note).map_err(|e| e.to_string())?;
    }
    if let Some((clip_id, was_dissolved)) = dissolved {
        if was_dissolved {
            if let Some(win) = app.get_webview_window(&clip_window_label(&clip_id)) {
                let _ = win.close();
            }
            open_all_windows(&app); // the freed last member needs a window too
        }
    }
    let _ = app.emit("clips-changed", ());
    Ok(())
}

/// Dissolve a whole clip: every member becomes a standalone note window again.
#[tauri::command]
async fn unclip_all(app: AppHandle, clip_id: String) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        let mut store = state.store.lock().map_err(|e| e.to_string())?;
        store.delete_clip(&clip_id).map_err(|e| e.to_string())?;
    }
    if let Some(win) = app.get_webview_window(&clip_window_label(&clip_id)) {
        let _ = win.close();
    }
    open_all_windows(&app);
    let _ = app.emit("clips-changed", ());
    Ok(())
}

/// Focus (opening if needed) a single note's window — the "Show Existing" action.
/// Async for the same reason as [`create_note`] (it may build a window).
#[tauri::command]
async fn focus_note(
    app: AppHandle,
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let (note, clip) = {
        let store = state.store.lock().map_err(|e| e.to_string())?;
        let note = store.get(&id).cloned();
        // If the note is clipped, its window is the clip's stack window.
        let clip = note
            .as_ref()
            .and_then(|n| n.group_id.as_deref())
            .and_then(|cid| store.get_clip(cid).cloned());
        (note, clip)
    };
    if let Some(clip) = clip {
        open_clip_window(&app, &clip).map_err(|e| e.to_string())?;
    } else if let Some(note) = note {
        open_note_window(&app, &note).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Open the Library Hub window. Async for the same reason as [`create_note`].
#[tauri::command]
async fn open_hub(app: AppHandle) -> Result<(), String> {
    open_hub_window(&app).map_err(|e| e.to_string())
}

/// Compose a local, offline "Surprise Me" message from the library. `hour` is the
/// user's local hour-of-day (supplied by the frontend) for the greeting.
#[tauri::command]
fn surprise_me(hour: u32, state: tauri::State<AppState>) -> Result<String, String> {
    let notes = {
        let store = state.store.lock().map_err(|e| e.to_string())?;
        store.all()
    };
    Ok(similarity::surprise_message(&notes, hour, now_ms()))
}

/// Find the note most similar to `id` above the duplicate threshold, if any.
/// Called at a note's commit moment (blur) for Smart Duplicate Detection.
#[tauri::command]
fn find_duplicate(id: String, state: tauri::State<AppState>) -> Result<Option<Match>, String> {
    let notes = {
        let store = state.store.lock().map_err(|e| e.to_string())?;
        store.all()
    };
    Ok(similarity::most_similar(&id, &notes, DUPLICATE_THRESHOLD))
}

/// Propose groups of similar notes for Smart Organization.
#[tauri::command]
fn suggest_groups(state: tauri::State<AppState>) -> Result<Vec<Cluster>, String> {
    let notes = {
        let store = state.store.lock().map_err(|e| e.to_string())?;
        store.all()
    };
    Ok(similarity::cluster(&notes, CLUSTER_THRESHOLD))
}

/// Merge `source` into `target`, then close the source window ("Merge" action).
#[tauri::command]
fn merge_notes(
    app: AppHandle,
    source_id: String,
    target_id: String,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    {
        let mut store = state.store.lock().map_err(|e| e.to_string())?;
        store
            .merge(&source_id, &target_id)
            .map_err(|e| e.to_string())?;
    }
    // Attachments moved to the target, so do NOT delete files here — just close.
    if let Some(win) = app.get_webview_window(&window_label(&source_id)) {
        let _ = win.close();
    }
    Ok(())
}

/// Copy an image chosen by the user into the note's attachments and record it.
/// If the note is protected, the file is sealed at rest so its bytes never sit
/// on disk in the clear. Returns the stored file name.
#[tauri::command]
fn attach_image(
    app: AppHandle,
    id: String,
    source_path: String,
    state: tauri::State<AppState>,
) -> Result<String, String> {
    let src = PathBuf::from(&source_path);
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();
    let raw = fs::read(&src).map_err(|e| e.to_string())?;
    let file_name = format!("{id}-{}.{ext}", now_ms());
    let dest = attachments_dir(&app)?.join(&file_name);

    let mut store = state.store.lock().map_err(|e| e.to_string())?;
    let protected = store.get(&id).map(|n| n.protected).unwrap_or(false);
    let data = if protected {
        store.encrypt_bytes(&raw).map_err(|e| e.to_string())?
    } else {
        raw
    };
    fs::write(&dest, &data).map_err(|e| e.to_string())?;
    store
        .add_attachment(&id, file_name.clone())
        .map_err(|e| e.to_string())?;
    Ok(file_name)
}

/// Guess an image MIME type from a file name's extension.
fn mime_for(file: &str) -> &'static str {
    let ext = std::path::Path::new(file)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "application/octet-stream",
    }
}

/// Remove an attachment from a note and delete its file (best-effort).
#[tauri::command]
fn remove_attachment(
    app: AppHandle,
    id: String,
    file: String,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    {
        let mut store = state.store.lock().map_err(|e| e.to_string())?;
        store
            .remove_attachment(&id, &file)
            .map_err(|e| e.to_string())?;
    }
    let _ = fs::remove_file(attachments_dir(&app)?.join(&file));
    Ok(())
}

/// Read an attachment as a `data:` URL, decrypting it first if the note is
/// protected. Serving bytes this way (rather than a file path) means an
/// encrypted attachment is never written to disk in the clear to be displayed.
#[tauri::command]
fn read_attachment(
    app: AppHandle,
    id: String,
    file: String,
    state: tauri::State<AppState>,
) -> Result<String, String> {
    let path = attachments_dir(&app)?.join(&file);
    let raw = fs::read(&path).map_err(|e| e.to_string())?;
    let store = state.store.lock().map_err(|e| e.to_string())?;
    let protected = store.get(&id).map(|n| n.protected).unwrap_or(false);
    let bytes = if protected {
        store.decrypt_bytes(&raw).map_err(|e| e.to_string())?
    } else {
        raw
    };
    Ok(format!("data:{};base64,{}", mime_for(&file), BASE64.encode(bytes)))
}

// ---------------------------------------------------------------------------
// Password protection
// ---------------------------------------------------------------------------

/// Whether a master password has been configured.
#[tauri::command]
fn has_master(state: tauri::State<AppState>) -> Result<bool, String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    Ok(store.has_master())
}

/// Whether the vault is currently locked (master set, but no key held this run).
#[tauri::command]
fn is_locked(state: tauri::State<AppState>) -> Result<bool, String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    Ok(store.is_locked())
}

/// Set the master password for the first time (starts the session unlocked).
#[tauri::command]
fn set_master_password(
    app: AppHandle,
    password: String,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    {
        let mut store = state.store.lock().map_err(|e| e.to_string())?;
        store
            .set_master_password(&password)
            .map_err(|e| e.to_string())?;
    }
    let _ = app.emit("vault-changed", ());
    Ok(())
}

/// Try to unlock the vault. Returns `false` (no error) on a wrong password.
#[tauri::command]
fn unlock_vault(
    app: AppHandle,
    password: String,
    state: tauri::State<AppState>,
) -> Result<bool, String> {
    let ok = {
        let mut store = state.store.lock().map_err(|e| e.to_string())?;
        store.unlock(&password).map_err(|e| e.to_string())?
    };
    if ok {
        // Tell every note window to refresh now that protected content is readable.
        let _ = app.emit("vault-changed", ());
    }
    Ok(ok)
}

/// Lock the vault now (re-blur protected notes without restarting the app).
#[tauri::command]
fn lock_vault(app: AppHandle, state: tauri::State<AppState>) -> Result<(), String> {
    {
        let mut store = state.store.lock().map_err(|e| e.to_string())?;
        store.lock().map_err(|e| e.to_string())?;
    }
    let _ = app.emit("vault-changed", ());
    Ok(())
}

/// Toggle a note's protection. Errors if the vault is locked or unset (protecting).
/// Also migrates the note's attachment files: they are sealed when protecting and
/// unsealed when un-protecting, so an attachment's on-disk state always matches.
#[tauri::command]
fn set_note_protected(
    app: AppHandle,
    id: String,
    protected: bool,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    {
        let mut store = state.store.lock().map_err(|e| e.to_string())?;
        store
            .set_protected(&id, protected)
            .map_err(|e| e.to_string())?;

        let files = store.get(&id).map(|n| n.attachments.clone()).unwrap_or_default();
        if !files.is_empty() {
            let dir = attachments_dir(&app)?;
            for f in &files {
                let path = dir.join(f);
                let raw = match fs::read(&path) {
                    Ok(b) => b,
                    Err(_) => continue, // missing file: nothing to migrate
                };
                let out = if protected {
                    // Sealing must succeed; a failure here would leave plaintext.
                    store.encrypt_bytes(&raw).map_err(|e| e.to_string())?
                } else {
                    // Best-effort: skip a file that isn't actually sealed.
                    match store.decrypt_bytes(&raw) {
                        Ok(b) => b,
                        Err(_) => continue,
                    }
                };
                fs::write(&path, &out).map_err(|e| e.to_string())?;
            }
        }
    }
    let _ = app.emit("vault-changed", ());
    Ok(())
}

/// Report whether the app is registered to launch at login.
#[tauri::command]
fn is_autostart_enabled(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

/// Enable or disable launch-at-login at runtime.
#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable().map_err(|e| e.to_string())
    } else {
        manager.disable().map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Tray
// ---------------------------------------------------------------------------

fn build_tray(app: &AppHandle, icon: tauri::image::Image<'_>) -> tauri::Result<()> {
    let new_item = MenuItem::with_id(app, "new", "New Note", true, Some("Ctrl+Alt+N"))?;
    let show_item = MenuItem::with_id(app, "show_all", "Show All Notes", true, None::<&str>)?;
    let library_item = MenuItem::with_id(app, "library", "Library…", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&new_item, &show_item, &library_item, &separator, &quit_item],
    )?;

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip("Sticky Notes — click to add a note")
        .menu(&menu)
        // We handle left-click ourselves (create a note); the menu is right-click.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "new" => {
                let _ = spawn_new_note(app);
            }
            "show_all" => show_all_notes(app),
            "library" => {
                let _ = open_hub_window(app);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = spawn_new_note(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// App entry point
// ---------------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Single instance MUST be the first plugin registered. A second launch
        // routes to this callback instead of starting a competing process that
        // would fight over the notes file.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let _ = spawn_new_note(app);
        }))
        .plugin(
            // The "new note" hotkey (Ctrl+Alt+N). The handler is attached here;
            // the shortcut itself is registered in `setup`.
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() == ShortcutState::Pressed
                        && shortcut.matches(Modifiers::CONTROL | Modifiers::ALT, Code::KeyN)
                    {
                        let _ = spawn_new_note(app);
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None::<Vec<&str>>,
        ))
        // Native file picker, used to attach images to a note.
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // 1. Load persisted notes from the OS app-data directory.
            let data_dir = app.path().app_data_dir()?;
            let notes_path = data_dir.join(NOTES_FILE);
            let store = NoteStore::load(notes_path)?;
            app.manage(AppState {
                store: Mutex::new(store),
            });

            // 2. Register for launch-at-login (idempotent).
            #[cfg(desktop)]
            {
                let _ = app.autolaunch().enable();
            }

            // 3. Register the global "new note" shortcut: Ctrl+Alt+N.
            //    (Its handler is attached at plugin initialization above.)
            #[cfg(desktop)]
            {
                let shortcut = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyN);
                app.global_shortcut().register(shortcut)?;
            }

            // 4. System tray (uses the bundled app icon).
            let tray_icon = app
                .default_window_icon()
                .cloned()
                .expect("bundle icon is configured in tauri.conf.json");
            build_tray(app.handle(), tray_icon)?;

            // 5. Restore windows for saved notes, or greet a first-time user.
            let notes = {
                let state = app.state::<AppState>();
                let store = state.store.lock().expect("store lock poisoned");
                store.all()
            };
            if notes.is_empty() {
                let _ = spawn_new_note(app.handle());
            } else {
                open_all_windows(app.handle());
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_note,
            list_notes,
            create_note,
            update_note_content,
            update_note_color,
            update_note_geometry,
            update_note_opacity,
            set_window_opacity,
            update_note_group,
            assign_group,
            list_clips,
            get_clip,
            clip_notes,
            rename_clip,
            update_clip_geometry,
            create_clip_from,
            add_to_clip,
            try_clip_on_drop,
            unclip_note,
            unclip_all,
            delete_note,
            focus_note,
            open_hub,
            surprise_me,
            find_duplicate,
            suggest_groups,
            merge_notes,
            attach_image,
            remove_attachment,
            read_attachment,
            has_master,
            is_locked,
            set_master_password,
            unlock_vault,
            lock_vault,
            set_note_protected,
            is_autostart_enabled,
            set_autostart,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the Sticky Notes application")
        .run(|_app_handle, event| {
            // Closing the last note window should NOT quit the app — it lives in
            // the tray. Only an explicit `app.exit(0)` (code = Some) really exits.
            if let RunEvent::ExitRequested { code, api, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}
