//! Tauri shell for the Sticky Notes app.
//!
//! This module is intentionally thin: all note data and disk I/O live in the
//! [`note_store`] crate (which is unit-tested on its own). Here we wire that
//! store to the desktop: one window per note, a tray icon, a global shortcut,
//! auto-launch on login, and a single-instance guard.

use std::sync::Mutex;

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use note_store::{Note, NoteStore};

/// File name (inside the OS app-data directory) that notes are persisted to.
const NOTES_FILE: &str = "notes.json";

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
fn open_note_window(app: &AppHandle, note: &Note) -> tauri::Result<()> {
    let label = window_label(&note.id);
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }

    WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
        .title("Sticky Note")
        .inner_size(note.width, note.height)
        .position(note.x, note.y)
        .min_inner_size(180.0, 160.0)
        .decorations(false)
        .resizable(true)
        .skip_taskbar(true)
        .build()?;
    Ok(())
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

/// Show (and focus) every note window, opening any that aren't currently open.
fn show_all_notes(app: &AppHandle) {
    let notes = match app.state::<AppState>().store.lock() {
        Ok(store) => store.all(),
        Err(_) => return,
    };
    for note in &notes {
        if let Some(win) = app.get_webview_window(&window_label(&note.id)) {
            let _ = win.show();
            let _ = win.set_focus();
        } else {
            let _ = open_note_window(app, note);
        }
    }
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
#[tauri::command]
fn create_note(app: AppHandle) -> Result<Note, String> {
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

/// Delete a note and close its window.
#[tauri::command]
fn delete_note(app: AppHandle, id: String, state: tauri::State<AppState>) -> Result<(), String> {
    {
        let mut store = state.store.lock().map_err(|e| e.to_string())?;
        store.delete(&id).map_err(|e| e.to_string())?;
    }
    if let Some(win) = app.get_webview_window(&window_label(&id)) {
        let _ = win.close();
    }
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
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&new_item, &show_item, &separator, &quit_item])?;

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
                for note in &notes {
                    let _ = open_note_window(app.handle(), note);
                }
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
            delete_note,
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
