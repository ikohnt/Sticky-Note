# Sticky Notes

> Lightweight desktop sticky notes for Windows — draggable, resizable, and always exactly where you left them. Now with per‑note transparency, image attachments, password‑protected notes and offline smart‑organisation. Built with [Tauri 2](https://v2.tauri.app/) and Rust.

![Platform](https://img.shields.io/badge/platform-Windows-0078D6)
![Built with Tauri 2](https://img.shields.io/badge/built%20with-Tauri%202-24C8DB)
![Rust](https://img.shields.io/badge/Rust-1.77%2B-000000?logo=rust)
![License: MIT](https://img.shields.io/badge/license-MIT-green)

A tiny sticky‑notes app that lives in your system tray. Spin up a note with a global hotkey or a click, scribble on it, drag it anywhere on your desktop, and it saves itself automatically — restoring every note's text, position, size and colour each time you log in. Attach images, dial in how see‑through a note is, lock the private ones behind a master password, and let the built‑in Library tidy related notes into collections. **No accounts, no cloud, no Electron** — the smart features run entirely on your machine.

<!-- Add a screenshot or GIF here to make the repo shine, e.g. put one at docs/screenshot.png:
![Sticky Notes screenshot](docs/screenshot.png)
-->

## Features

**Writing & appearance**

- **Unlimited notes**, each in its own frameless, draggable, resizable window.
- **Instant capture** — global shortcut `Ctrl + Alt + N`, a tray‑icon click, or the `+` button on any note.
- **Five colour themes**, plus a **per‑note opacity slider** that makes a note genuinely see‑through to whatever's behind it.
- **Focus Mode** — notes you aren't editing dim and blur, so the active one stands out; the one you click back into snaps to full clarity.
- **Timestamps** — each note shows when it was last edited (hover for the exact created/updated times).
- **Image attachments** — attach images to a note; they're stored alongside it, not pasted into the text.

**Organising**

- **Collections** — file related notes into named groups.
- **Library** window (tray → *Library*, or the ☰ button on a note):
  - **Surprise Me** — a friendly greeting / nudge composed from your own notes.
  - **Smart Organization** — clusters similar notes so you can file a whole group at once.
- **Smart Duplicate Detection** — when a note looks a lot like an existing one, it offers **Merge**, **Keep both**, or **Show existing** (with a plain reason why they matched). All of this is computed **on‑device** — no note ever leaves your machine.

**Private & safe**

- **Password protection** — lock sensitive notes behind a single master password. Protected notes — **their text and their image attachments** — are **encrypted at rest** with AES‑256‑GCM using a key derived from your password via Argon2id. The password itself is never stored.
- **Local only** — no accounts, no cloud, no telemetry.
- **Crash‑safe storage** — atomic writes plus automatic recovery if the data file is ever corrupted.
- **Launches at login** and lives in the tray; closing the last note doesn't quit the app.
- **Small footprint** — a native Tauri app, not a bundled browser.

## Download

Grab the latest installer from the [Releases](https://github.com/ikohnt/Sticky-Note/releases) page and run `Sticky Notes_x.y.z_x64-setup.exe`. Install once and it starts with Windows from then on.

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
| Adjust transparency | Drag the opacity slider in the note's footer |
| Attach an image | The 🖼 button in the title bar |
| Protect a note | The 🔒 button (you'll set a master password the first time) |
| Unlock protected notes | Enter the master password on any locked note — it unlocks them all for the session |
| Open the Library | The ☰ button on a note, or right‑click the tray icon → *Library…* |
| Group similar notes | *Library* → *Suggest groups* → name a cluster and file it |
| Delete | The `×` button on the note |
| Show all notes | Right‑click the tray icon → *Show All Notes* |
| Quit | Right‑click the tray icon → *Quit* |

## Data & privacy

Everything lives locally under `%APPDATA%\com.stickynotes.desktop\` and **never leaves your machine**:

- `notes.json` — your notes (JSON). Writes are atomic (temp file + rename), so a crash can't truncate them; if the file is ever unreadable it's moved aside to `notes.json.corrupt-<timestamp>` and the app starts fresh instead of failing to launch.
- `attachments/` — image attachments, stored as separate files (keeps `notes.json` small and fast to save).
- `master.json` — only present once you set a master password. It stores a **verifier**, never the password or the key.

**Protected notes** are sealed at rest: the note's text and its attachment files are encrypted with **AES‑256‑GCM**, using a 32‑byte key derived from your master password with **Argon2id**. The vault starts locked each run; unlocking once reveals every protected note for that session. Because the password is never stored, **there is no way to recover a protected note if you forget it.**

The **Library** features (Surprise Me, Smart Organization) and **duplicate detection** analyse your notes with a small, offline lexical model (TF‑IDF similarity) — there is no network call and nothing is uploaded. Protected notes are excluded from this analysis while locked.

## How it works

Two Rust crates keep the important logic testable:

- **`note-store`** (`src-tauri/note-store`) — the pure data layer, with no Tauri dependency so its tests run in seconds:
  - `lib.rs` — the `Note` model, JSON load/save, atomic writes, corruption recovery, CRUD, plus opacity/group/attachment/protection updates and note merging.
  - `similarity.rs` — offline TF‑IDF cosine similarity + clustering that powers Smart Organization, Smart Duplicate Detection and Surprise Me. Dependency‑free; nothing is downloaded or sent anywhere.
  - `crypto.rs` — Argon2id key derivation and AES‑256‑GCM sealing for protected notes and their attachments.
- **`sticky-notes`** (`src-tauri`) — the Tauri shell: one window per note plus the Library window, the tray icon, the global shortcut, launch‑at‑login, a single‑instance guard, and thin (async) command handlers that call into `note-store`.

The frontend (`src/`) is vanilla HTML/CSS/JS using the global `window.__TAURI__` API — no bundler or build step.

```
.
├─ src/                       # frontend (vanilla HTML/CSS/JS, no bundler)
│  ├─ index.html / main.js / styles.css   # a single note window
│  └─ hub.html  / hub.js  / hub.css        # the Library window
└─ src-tauri/
   ├─ src/                    # Tauri shell (main.rs, lib.rs)
   ├─ note-store/             # pure-logic crate + tests
   │  └─ src/                 # lib.rs, similarity.rs, crypto.rs
   ├─ capabilities/           # window permissions
   ├─ icons/                  # app + tray icons
   └─ tauri.conf.json
```

## Auto‑launch

The app registers itself under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` on first run via [`tauri-plugin-autostart`](https://v2.tauri.app/plugin/autostart/). Disable it any time from **Task Manager → Startup apps**. (Launch‑at‑login only applies to the installed build, not `npm run dev`.)

## Testing

```powershell
npm test
```

Covers the full `note-store` surface: note creation, editing, geometry updates, deletion and persistence round‑trips (including a multi‑note restart scenario); corrupted‑file recovery, empty/missing files, default‑field fallback and unique id generation; opacity clamping, grouping, attachment add/remove and note merging; the offline similarity engine (tokenising, matching, clustering, protected/empty‑note exclusion); and the encryption path — a protect → lock → unlock round‑trip that asserts a protected note's plaintext **never reaches disk**.

## Troubleshooting

- **`link.exe` not found** — install the Visual Studio C++ Build Tools ("Desktop development with C++"); VS Code alone is not enough.
- **`dlltool ... Invalid bfd target`** — you're on the GNU Rust toolchain with an old MinGW; switch with `rustup default stable-msvc`.
- **Blank/white window** — update the WebView2 Runtime.
- **`npm run dev` seems to do nothing** — an instance is already running (the installed build launches at login) and holds the single‑instance lock, so your launch hands off to it. Quit that instance first (tray → *Quit*, or end `sticky-notes.exe`), then run again.
- **`Ctrl+Alt+N` does nothing** — another app already claimed that hotkey; change it in `src-tauri/src/lib.rs`.

## Tech stack

Tauri 2 · Rust · vanilla HTML/CSS/JS · JSON persistence · Argon2id + AES‑256‑GCM (protected notes) · offline TF‑IDF similarity · `tauri-plugin-dialog` (image picker) · Windows layered‑window transparency · Windows (NSIS/MSI installers).

## Contributing

Issues and pull requests are welcome. For code changes, please keep note logic in the `note-store` crate and add a test for it.

## License

[MIT](LICENSE) © 2026 Hendriko
