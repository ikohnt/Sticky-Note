# Sticky Notes

> Lightweight desktop sticky notes for Windows — draggable, resizable, and always exactly where you left them. Built with [Tauri 2](https://v2.tauri.app/) and Rust.

![Platform](https://img.shields.io/badge/platform-Windows-0078D6)
![Built with Tauri 2](https://img.shields.io/badge/built%20with-Tauri%202-24C8DB)
![Rust](https://img.shields.io/badge/Rust-1.77%2B-000000?logo=rust)
![License: MIT](https://img.shields.io/badge/license-MIT-green)

A tiny sticky‑notes app that lives in your system tray. Spin up a note with a global hotkey or a click, scribble on it, drag it anywhere on your desktop, and it saves itself automatically — restoring every note's text, position, size and colour each time you log in. No accounts, no cloud, no Electron.

<!-- Add a screenshot or GIF here to make the repo shine, e.g. put one at docs/screenshot.png:
![Sticky Notes screenshot](docs/screenshot.png)
-->

## Features

- **Unlimited notes**, each in its own frameless, draggable, resizable window.
- **Instant capture** — global shortcut `Ctrl + Alt + N`, a tray‑icon click, or the `+` button on any note.
- **Automatic persistence** — every edit, move, resize and colour change is saved to disk; all notes are restored on restart.
- **Launches at login** and lives in the tray; closing the last note doesn't quit the app.
- **Five colour themes.**
- **Crash‑safe storage** — atomic writes plus automatic recovery if the data file is ever corrupted.
- **Small footprint** — a native Tauri app, not a bundled browser.

## Download

Grab the latest installer from the [Releases](https://github.com/YOUR_USERNAME/sticky-notes/releases) page and run `Sticky Notes_x.y.z_x64-setup.exe`. Install once and it starts with Windows from then on.

> Not signed yet, so Windows SmartScreen may warn on first run — choose *More info → Run anyway*, or [sign the build](https://v2.tauri.app/distribute/sign/windows/) yourself.

## Build from source

**Prerequisites**

- [Rust](https://rustup.rs/) 1.77.2+ on the **MSVC** toolchain (`rustup default stable-msvc`)
- [Node.js](https://nodejs.org/) 18+
- **Visual Studio C++ Build Tools** with the "Desktop development with C++" workload (provides `link.exe` and the Windows SDK)
- **WebView2 Runtime** (preinstalled on Windows 10/11)

**Commands**

```powershell
npm install       # install the Tauri CLI
npm run dev        # run in development
npm run build      # produce installers in src-tauri/target/release/bundle/
npm test           # run the note-store unit + integration tests
```

## Usage

| Action | How |
| ------ | --- |
| New note | `Ctrl + Alt + N`, left‑click the tray icon, or the `+` button on a note |
| Edit | Just type — text autosaves ~0.3 s after you stop |
| Move | Drag the coloured title bar |
| Resize | Drag the grip in the bottom‑right corner |
| Change colour | Click a swatch in the title bar |
| Delete | The `×` button on the note |
| Show all notes | Right‑click the tray icon → *Show All Notes* |
| Quit | Right‑click the tray icon → *Quit* |

## Data & privacy

Notes are stored locally as JSON at `%APPDATA%\com.stickynotes.desktop\notes.json` and **never leave your machine**. Writes are atomic (temp file + rename), so a crash can't truncate your notes. If the file is ever unreadable it's moved aside to `notes.json.corrupt-<timestamp>` and the app starts fresh instead of failing to launch.

## How it works

Two Rust crates keep the important logic testable:

- **`note-store`** (`src-tauri/note-store`) — the pure data layer: the `Note` model, JSON load/save, atomic writes, corruption recovery and CRUD. No Tauri dependency, so its unit + integration tests run in seconds.
- **`sticky-notes`** (`src-tauri`) — the Tauri shell: one window per note, the tray icon, the global shortcut, launch‑at‑login, a single‑instance guard, and thin command handlers that call into `note-store`.

The frontend (`src/`) is vanilla HTML/CSS/JS using the global `window.__TAURI__` API — no bundler or build step.

```
.
├─ src/                    # frontend (index.html, main.js, styles.css)
└─ src-tauri/
   ├─ src/                 # Tauri shell (main.rs, lib.rs)
   ├─ note-store/          # pure-logic crate + tests
   ├─ capabilities/        # window permissions
   ├─ icons/               # app + tray icons
   └─ tauri.conf.json
```

## Auto‑launch

The app registers itself under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` on first run via [`tauri-plugin-autostart`](https://v2.tauri.app/plugin/autostart/). Disable it any time from **Task Manager → Startup apps**. (Launch‑at‑login only applies to the installed build, not `npm run dev`.)

## Testing

```powershell
npm test
```

Covers note creation, editing, geometry updates, deletion, persistence round‑trips (including a multi‑note restart scenario), corrupted‑file recovery, empty/missing files, default‑field fallback and unique id generation.

## Troubleshooting

- **`link.exe` not found** — install the Visual Studio C++ Build Tools ("Desktop development with C++"); VS Code alone is not enough.
- **`dlltool ... Invalid bfd target`** — you're on the GNU Rust toolchain with an old MinGW; switch with `rustup default stable-msvc`.
- **Blank/white window** — update the WebView2 Runtime.
- **`Ctrl+Alt+N` does nothing** — another app already claimed that hotkey; change it in `src-tauri/src/lib.rs`.

## Tech stack

Tauri 2 · Rust · vanilla HTML/CSS/JS · JSON persistence · Windows (NSIS/MSI installers).

## Contributing

Issues and pull requests are welcome. For code changes, please keep note logic in the `note-store` crate and add a test for it.

## License

[MIT](LICENSE) © 2026 Hendriko
