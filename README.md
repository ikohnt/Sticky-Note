# Sticky Notes

> Lightweight desktop sticky notes for Windows — draggable, resizable, and always exactly where you left them. Now with per‑note transparency, custom styling (presets, image‑based themes, fonts), live text formatting, an in‑place image editor, image attachments, password‑protected notes, clip‑together stacks and offline smart‑organisation. Built with [Tauri 2](https://v2.tauri.app/) and Rust.

![Platform](https://img.shields.io/badge/platform-Windows-0078D6)
![Built with Tauri 2](https://img.shields.io/badge/built%20with-Tauri%202-24C8DB)
![Rust](https://img.shields.io/badge/Rust-1.77%2B-000000?logo=rust)
![License: MIT](https://img.shields.io/badge/license-MIT-green)

A tiny sticky‑notes app that lives in your system tray. Spin up a note with a global hotkey or a click, scribble on it, drag it anywhere on your desktop, and it saves itself automatically — restoring every note's text, position, size, colour and style each time you log in. Style each note with presets or a background image that themes it from its own colours, pick a font and spacing, format text live (bold, headings, lists), crop and zoom attached images, dial in how see‑through a note is, lock the private ones behind a master password, clip related notes into a single stack to declutter, and let the built‑in Library surface duplicates and suggest groups — all through a clean, icon‑based interface (no emoji) with a light or dark theme. **No accounts, no cloud, no Electron** — the smart features run entirely on your machine.

<!-- Add a screenshot or GIF here to make the repo shine, e.g. put one at docs/screenshot.png:
![Sticky Notes screenshot](docs/screenshot.png)
-->

## Features

**Writing & appearance**

- **Unlimited notes**, each in its own frameless, draggable, resizable window.
- **Instant capture** — global shortcut `Ctrl + Alt + N`, a tray‑icon click, or the `+` button on any note.
- **Five colour themes**, plus a **per‑note opacity slider** that makes a note genuinely see‑through to whatever's behind it.
- **Custom styling** (*Style & background* panel) — pick a **preset** (solid, gradient or dark), set a **background image**, or **theme from an image**: the app reads the image's dominant colours and picks a matching note background with a contrasting top‑bar shade (light image → darker bar, dark image → lighter bar).
- **Typography** — choose a **font** (sans / serif / mono / casual), **font size** and **line spacing** per note.
- **Live formatting** — a small toolbar (or `Ctrl+B` / `Ctrl+I` / `Ctrl+U`) applies **bold**, *italic*, underline, headings, bullet lists and tick‑able checklist items that render **as you type**, right in the note.
- **Image editor** — click an attached image to **crop, zoom and rotate** it in place; the reframed result is saved back (and re‑sealed if the note is protected).
- **Focus Mode** — notes you aren't editing dim and blur, so the active one stands out; the one you click back into snaps to full clarity.
- **Timestamps** — each note shows when it was last edited (hover for the exact created/updated times).
- **Image attachments** — attach images to a note; they're stored alongside it, not pasted into the text.

**Organising**

- **Clips** — collapse several related notes into a single **stack** window: drag one note onto another (or multi‑select in the Library) to clip them, flip through the pile, rename it, and un‑clip to fan them back out into separate windows.
- **Library** window (tray → *Library*, or the ☰ button on a note):
  - **Surprise Me** — a friendly greeting / nudge composed from your own notes.
  - **Smart Organization** — clusters similar notes so you can clip a whole cluster into a stack at once.
  - **Clip selected** — tick a few notes and stack them into a clip in one click.
- **Smart Duplicate Detection** — when a note looks a lot like an existing one, it offers **Merge**, **Keep both**, or **Show existing** (with a plain reason why they matched). All of this is computed **on‑device** — no note ever leaves your machine.

**Private & safe**

- **Password protection** — lock sensitive notes behind a single master password. Protected notes — **their text and their image attachments** — are **encrypted at rest** with AES‑256‑GCM using a key derived from your password via Argon2id. The password itself is never stored. A locked note shows a **sign‑in‑style lock screen** (modelled on the Windows login): the note's own surface is blurred behind a centred prompt — a lock badge, the label *Protected note*, and a rounded password box — while the real text stays encrypted and off‑screen.
- **Local only** — no accounts, no cloud, no telemetry.
- **Crash‑safe storage** — atomic writes plus automatic recovery if the data file is ever corrupted.
- **Launches at login** and lives in the tray; closing the last note doesn't quit the app.
- **Small footprint** — a native Tauri app (a ~3.5 MB binary), not a bundled browser. Memory is dominated by the shared WebView2 runtime and stays roughly flat no matter how many notes you open; render‑neutral WebView2 flags keep it lean without sacrificing the transparency/blur effects.

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
| New note | `Ctrl + Alt + N`, left‑click the tray icon, or the **new‑note** (`+`) button on a note |
| Edit | Just type — text autosaves ~0.3 s after you stop |
| Format text | The formatting bar under the title bar (bold, italic, underline, heading, list, checklist), or `Ctrl+B` / `Ctrl+I` / `Ctrl+U` |
| Move | Drag the coloured title bar |
| Resize | Drag the grip in the bottom‑right corner |
| Change colour | Click a swatch in the title bar |
| Style a note | Right‑click the note → *Style & background* — presets, *Theme from image*, *Use as background*, font, size and spacing |
| Adjust transparency | Drag the opacity slider in the note's footer |
| Attach an image | Right‑click the note → *Attach image* |
| Edit an attached image | Click the thumbnail to crop / zoom / rotate, then *Save* |
| Protect a note | The **lock** button in the title bar, or right‑click → *Protect this note* (you'll set a master password the first time) |
| Unlock protected notes | Enter the master password on any locked note — it unlocks them all for the session |
| Open the Library | Right‑click the note → *Open Library*, or right‑click the tray icon → *Library…* |
| Auto-group similar notes | *Library* → *Suggest groups* → name a cluster and *Clip these* |
| Clip notes into a stack | Drag one note onto another, or *Library* → tick notes → *Clip selected* |
| Flip through a clip | The previous / next arrows on the stack window |
| Un‑clip | On the stack: take the current note out, or fan the whole clip back into separate windows |
| Delete | The **delete** (trash) button, or right‑click → *Delete note* |
| Show all notes | Right‑click the tray icon → *Show All Notes* |
| Quit | Right‑click the tray icon → *Quit* |

## Data & privacy

Everything lives locally under `%APPDATA%\com.stickynotes.desktop\` and **never leaves your machine**:

- `notes.json` — your notes (JSON). Writes are atomic (temp file + rename), so a crash can't truncate them; if the file is ever unreadable it's moved aside to `notes.json.corrupt-<timestamp>` and the app starts fresh instead of failing to launch. Style metadata (colour palette, background‑image filename, font/size/spacing) lives here as plain fields — like geometry and colour, it isn't secret, so it's never encrypted; only a protected note's **text** and **attachment bytes** are.
- `attachments/` — image attachments **and** background images, stored as separate files (keeps `notes.json` small and fast to save).
- `clips.json` — your clip (stack) definitions: each clip's name and window geometry. Which notes belong to a clip is tracked on the notes themselves, so `notes.json` stays a plain list.
- `master.json` — only present once you set a master password. It stores a **verifier**, never the password or the key.

**Protected notes** are sealed at rest: the note's text and its attachment files are encrypted with **AES‑256‑GCM**, using a 32‑byte key derived from your master password with **Argon2id**. The vault starts locked each run; unlocking once reveals every protected note for that session. Because the password is never stored, **there is no way to recover a protected note if you forget it.**

The **Library** features (Surprise Me, Smart Organization) and **duplicate detection** analyse your notes with a small, offline lexical model (TF‑IDF similarity) — there is no network call and nothing is uploaded. Protected notes are excluded from this analysis while locked.

## How it works

Two Rust crates keep the important logic testable:

- **`note-store`** (`src-tauri/note-store`) — the pure data layer, with no Tauri dependency so its tests run in seconds:
  - `lib.rs` — the `Note` and `Clip` models, JSON load/save, atomic writes, corruption recovery, CRUD, plus opacity/attachment/protection updates, per‑note styling (palette, background image, typography), note merging, and clip (stack) membership.
  - `similarity.rs` — offline TF‑IDF cosine similarity + clustering that powers Smart Organization, Smart Duplicate Detection and Surprise Me. Dependency‑free; nothing is downloaded or sent anywhere.
  - `crypto.rs` — Argon2id key derivation and AES‑256‑GCM sealing for protected notes and their attachments.
- **`sticky-notes`** (`src-tauri`) — the Tauri shell: a window per note, a stack window per clip and the Library window, the tray icon, the global shortcut, launch‑at‑login, a single‑instance guard, and thin (async) command handlers that call into `note-store`. Windows are built on the event loop so their webviews always initialise.

The frontend (`src/`) is vanilla HTML/CSS/JS using the global `window.__TAURI__` API — no bundler or build step. Each window type is its own page; two lightweight events (`vault-changed`, `clips-changed`) keep the separate webviews in sync.

```
.
├─ src/                       # frontend (vanilla HTML/CSS/JS, no bundler)
│  ├─ index.html / main.js / styles.css   # a single note window
│  ├─ hub.html  / hub.js  / hub.css        # the Library window
│  └─ stack.html / stack.js                # a clip's stack window
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

Covers the full `note-store` surface: note creation, editing, geometry updates, deletion and persistence round‑trips (including a multi‑note restart scenario); corrupted‑file recovery, empty/missing files, default‑field fallback and unique id generation; opacity clamping, attachment add/remove and note merging; per‑note styling — palette/background persistence and font‑size/line‑height clamping; clips — membership, `clips.json` round‑trips and auto‑dissolving a clip that drops below two notes; the offline similarity engine (tokenising, matching, clustering, protected/empty‑note exclusion); and the encryption path — a protect → lock → unlock round‑trip that asserts a protected note's plaintext **never reaches disk**.

## Troubleshooting

- **`link.exe` not found** — install the Visual Studio C++ Build Tools ("Desktop development with C++"); VS Code alone is not enough.
- **`dlltool ... Invalid bfd target`** — you're on the GNU Rust toolchain with an old MinGW; switch with `rustup default stable-msvc`.
- **Blank/white window** — update the WebView2 Runtime.
- **`npm run dev` seems to do nothing** — an instance is already running (the installed build launches at login) and holds the single‑instance lock, so your launch hands off to it. Quit that instance first (tray → *Quit*, or end `sticky-notes.exe`), then run again.
- **`Ctrl+Alt+N` does nothing** — another app already claimed that hotkey; change it in `src-tauri/src/lib.rs`.

## Tech stack

Tauri 2 · Rust · vanilla HTML/CSS/JS · inline‑SVG icons (no emoji) · light/dark themes · JSON persistence · Argon2id + AES‑256‑GCM (protected notes) · offline TF‑IDF similarity · `tauri-plugin-dialog` (image picker) · Windows layered‑window transparency · Windows (NSIS/MSI installers).

## Contributing

Issues and pull requests are welcome. For code changes, please keep note logic in the `note-store` crate and add a test for it.

## License

[MIT](LICENSE) © 2026 Hendriko
