# Sticky Notes — Feature Backlog Working Spec

> Status: **working document.** Purpose is to let the team see the dependencies between proposed
> features and sequence work without getting stuck. The design forks that were blocking clean
> sequencing are now **resolved in the Decisions section** below; tiers and cross-references reflect
> those decisions. Revisit any decision if the team's constraints change.

## Implementation status (first pass)

Backend logic verified with `cargo test -p note-store` (27 unit + 4 integration tests, all green) and
the full shell type-checks (`cargo check`, clean). Frontend + Tauri wiring is written to match existing
patterns but needs a `npm run dev` pass to verify at runtime (marked ⚠︎).

| Feature | Status | Notes |
| --- | --- | --- |
| 1. Timestamps | ✅ done | Surfaced existing `created_at`/`updated_at` in a footer; relative time + absolute tooltip. |
| 7. Focus Mode | ✅ done ⚠︎ | Dims/blurs a note while its window is unfocused; always-on (a toggle is the natural follow-up). |
| 2. Opacity slider | ✅ done, verified | New `opacity` field + `update_note_opacity`. Translucency applied via Windows layered-window alpha (`set_window_opacity` → `SetLayeredWindowAttributes`) — WebView2's per-pixel `transparent` mode painted an opaque white backing on this setup, so the note went milky instead of see-through. Verified genuinely see-through at 35%. Slider owns baseline, Focus Mode multiplies. |
| 3. Note grouping | ✅ core ⚠︎ | `group_id` field + `update_note_group`/`assign_group`; group badge on notes. Window *stacking* not built (see #3). |
| 5. Image attachments | ✅ done ⚠︎ | Sidecar files in `attachments/`; `attach_image`/`remove_attachment`; dialog plugin + asset protocol. Verify rendering. |
| 8. Surprise Me | ✅ done ⚠︎ | Local template composer in `note-store::similarity`; lives in the new Library Hub window. |
| 9. Smart Organization | ✅ done ⚠︎ | Offline TF-IDF clustering; "Suggest groups" in the Hub → file into a collection. |
| 10. Smart Duplicate Detection | ✅ done ⚠︎ | On blur (commit moment); Merge / Keep both / Show existing modal. |
| 4. Password/PIN | ✅ done ⚠︎ | Encrypt-at-rest (Argon2id + AES-256-GCM) for note text **and attachment files**; master-password verifier; locked session. Tests prove plaintext never hits disk. Lock UI needs a runtime pass. |
| 6. Voice-to-text | ⛔ deferred | Decision 2/3 requires a bundled local STT engine — not buildable/testable in this environment, and WebView2 has no speech API. |

New files: `src-tauri/note-store/src/similarity.rs` (offline analysis engine),
`src-tauri/note-store/src/crypto.rs` (at-rest encryption), `src/hub.{html,js,css}` (Library Hub).
New dependencies: `tauri-plugin-dialog` + `base64` (shell), and `argon2` / `aes-gcm` / `getrandom` /
`base64` in `note-store`. Attachments are served to the webview as decrypted `data:` URLs (via a
`read_attachment` command), so the asset protocol was dropped — one fewer moving part, and it handles
plaintext and encrypted attachments uniformly.
Verified: `note-store` 40 tests green (36 unit + 4 integration); full shell `cargo check` clean
(0 warnings); app boots and runs (smoke-tested). Only Voice-to-text remains deferred, with reason.

## How to read this

Priorities are grounded in the actual codebase, not guesses:

- **Data layer** — `note-store` crate. The `Note` struct ([`note-store/src/lib.rs:49`](src-tauri/note-store/src/lib.rs)) is the single source of truth. Every mutation calls `save()`, which **serializes _all_ notes and rewrites the whole `notes.json`** via atomic temp-file + rename ([`lib.rs:242`](src-tauri/note-store/src/lib.rs)).
- **Shell** — `sticky-notes` crate ([`src-tauri/src/lib.rs`](src-tauri/src/lib.rs)). One frameless webview **per note**, tray, global shortcut. Commands are thin wrappers over `note-store`, registered in `invoke_handler` ([`lib.rs:295`](src-tauri/src/lib.rs)).
- **Frontend** — vanilla JS, one instance per note window, `window.__TAURI__` global, **no bundler/build step** ([`src/main.js`](src/main.js)). Autosave is a 350ms debounce on `input` ([`main.js:48`](src/main.js)).

Four facts drive most of the sequencing below:

1. **Timestamps already exist** in the model — surfacing them is frontend-only.
2. **Whole-file rewrite on every autosave** — heavy per-note payloads (images) don't belong inline.
3. **No "whole library" surface exists** — `list_notes` is defined but unused; Surprise Me + both AI features all need a new hub or a backend pass. Build that substrate **once**.
4. **"Never leaves your machine" is the product's headline promise** — voice-to-text and the AI features are therefore **locked to offline-first** (Decision 2), so nothing on this list erodes that guarantee.

---

## Recommended build order (dependency-first)

```
1. Timestamps                 ── quick win, data already present
2. Focus Mode                 ── frontend-only, proves the opacity plumbing
3. Opacity slider             ── small model change; coordinate with Focus Mode
4. Note grouping ("clipboards")── FOUNDATIONAL: unblocks Smart Organization
5. Library Hub surface        ── shared prerequisite for Surprise Me + both AI features
   ├── Surprise Me
   ├── Smart Duplicate Detection
   └── Smart Organization      (also needs #4)
—— higher-complexity track (forks now resolved — see Decisions) ——
   Password/PIN protection
   Image attachments          (sidecar files — Decision 1)
   Voice-to-text              (local engine — ship LAST — Decisions 2–3)
```

Grouping (#4) and the Library Hub (#5) are the two pieces of shared infrastructure. Building them early makes four downstream features cheaper; skipping them means each downstream feature reinvents the same plumbing. Voice-to-text sits last by decision, not by accident — it carries the largest app-size and compute cost even after going local.

---

## A note on the "Essential" tier

The task defines **Essential = blocks shipping.** Being candid: the core note-taking loop already works and already ships. **No feature on this list is a true ship-blocker.** The real decision the team is making is *enhancement value vs. added complexity*, not *what's required to launch*. So the tiers below use:

- **High Priority** — strong user value at a reasonable, low-risk lift.
- **Nice-to-Have** — worth doing, but complexity, dependencies, or design decisions mean it shouldn't jump the queue.

If a hard launch date forces a "minimum credible v2," the Essential-by-proxy set is **Timestamps + Focus Mode + Opacity** — all low-risk, all shippable without touching the privacy story or the storage model.

---

# Core Functionality

## 1. Timestamps

1. **Description** — Show each note's creation and/or last-edited time in its window.
2. **User benefit** — Context on when a thought was captured; helps triage stale vs. fresh notes.
3. **Technical considerations** — **No data model change needed.** `created_at` and `updated_at` already exist and are already maintained on every mutation ([`lib.rs:70-75`](src-tauri/note-store/src/lib.rs), set in `create`/`set_content`/`set_color`/`set_geometry`). Work is: expose them via the existing `get_note` payload (already returns the full `Note`) and render in the frontend. Only decision is display format (relative "2h ago" vs. absolute) and where it sits in the cramped title bar.
4. **Priority tier** — **High Priority.** Near-zero backend cost, immediate value; good first ticket.
5. *(not an AI feature)*
6. **Conflicts / integration points** — None. No storage-size or performance impact. Mild UI-layout pressure in the title bar (already holds swatches, `+`, `×`).

---

## 2. Opacity / transparency slider

1. **Description** — Per-note control to make a note window more or less see-through.
2. **User benefit** — Keep a note visible over other work without fully blocking what's behind it.
3. **Technical considerations** — **Data model change: add an `opacity` field to `Note`** (f64, default 1.0; use `#[serde(default)]` so old files load — the loader already relies on this pattern, see [`lib.rs:437` test](src-tauri/note-store/src/lib.rs)). Needs a new `set_opacity` command + `note-store` method mirroring `set_color`. Rendering: true window transparency requires `"transparent": true` on the window and a matching capability; CSS `opacity` on `body` alone won't make the frame translucent since the window surface is opaque. Verify against `tauri.conf.json` window config and `capabilities/`. Persist via the existing autosave path.
4. **Priority tier** — **High Priority.** Small, well-scoped model + command addition following an established pattern.
5. *(not an AI feature)*
6. **Conflicts / integration points** — **Direct interaction with Focus Mode (#7).** Focus Mode dims idle notes and restores them to "normal" — "normal" must mean *this note's slider value*, not a hardcoded 1.0, or the two features fight. **Ownership rule to adopt: slider sets the baseline; Focus Mode multiplies against it.** Building Focus Mode first (frontend-only) lets you prove the opacity mechanism before persisting a value.

---

## 3. Note grouping / stacking ("clipboards" / collections)

1. **Description** — Organize notes into named collections that can be stacked or shown/hidden together.
2. **User benefit** — Manage many notes without desktop clutter; group by project or context.
3. **Technical considerations** — **Significant, foundational data model change.** Two shapes were on the table:
   - *Lightweight:* add `group_id: Option<String>` to `Note` (backward-compatible via serde default). Cheap to persist, but group names/metadata have nowhere to live.
   - *Structured:* introduce a separate `Group`/`Collection` entity, which changes the on-disk file from a flat `Vec<Note>` to a richer document. This touches load/save, the corruption-recovery path, and every persistence test.

   **Decision 5 — ship the lightweight `group_id: Option<String>` first;** only pay for a structured `Group` entity if group-level metadata becomes a real requirement (and if so, make that schema change *before* Smart Organization builds on it).

   Beyond storage, grouping touches **window management** heavily: `show_all_notes` ([`src/lib.rs:72`](src-tauri/src/lib.rs)) and the startup restore loop ([`lib.rs:288`](src-tauri/src/lib.rs)) currently open *one window per note* with no notion of stacking. "Stacking" implies collapsing N windows into one surface or z-ordering them — a real shift from the current model.
4. **Priority tier** — **High Priority, but treat as a foundational epic, not a quick win.** It's a prerequisite for Smart Organization (#9), so its shape (settled above as `group_id`) is locked before that feature is scoped.
5. *(not an AI feature)*
6. **Conflicts / integration points** — **Prerequisite for Smart Organization** (auto-grouping needs a group concept to assign into). Interacts with the per-window architecture (stacking vs. one-window-per-note). Secondary UI question: how does a note advertise which group it's in? Low storage impact for the chosen `group_id` option.

---

## 4. Password / PIN protection (single master password for all protected notes)

1. **Description** — Mark notes as protected; a single master password gates viewing them.
2. **User benefit** — Keep sensitive notes private on a shared or unlocked machine.
3. **Technical considerations** — **Data model change: `protected: bool` on `Note`**, plus **new storage for the master secret** (store a *hash/verifier*, e.g. Argon2, never the plaintext — this is a new dependency in `note-store` and its first real crypto surface). The pivotal design fork:
   - *Gate-only (hide content behind an unlock prompt):* simpler, but the plaintext still sits in `notes.json`. Anyone opening the file reads everything. Weak, and arguably misleading given the "private" framing.
   - *Encrypt-at-rest (encrypt protected notes' `content`):* real protection, but means encrypt/decrypt on the save/load path, key derivation from the master password, and a locked state where content genuinely isn't readable.

   Either way it adds a new command surface (`set_master_password`, `unlock`, `verify`) and app-level locked/unlocked state that the current stateless command handlers don't have.
4. **Priority tier** — **Nice-to-Have.** Meaningful value, but the security decisions (crypto, key handling, threat model) make it higher-risk than its "single password" framing suggests. Don't ship gate-only and call it protection.
5. *(not an AI feature)*
6. **Conflicts / integration points** — **Sharp conflict with every library-analysis feature.** If protected content is encrypted at rest, Surprise Me, Smart Duplicate Detection, and Smart Organization **cannot read it while locked** — so **Decision 6 (adopted): library-analysis features exclude protected notes unless explicitly unlocked.** Even in gate-only mode, "Surprise Me" surfacing text from a note the user hid would be a trust violation, so the same rule holds regardless of which crypto path is chosen.

---

## 5. Image attachments

1. **Description** — Attach one or more images to a note.
2. **User benefit** — Capture screenshots, references, and visual snippets alongside text.
3. **Technical considerations** — **The storage strategy is the whole ballgame, because of the whole-file-rewrite architecture.** `save()` re-serializes *every* note into one `notes.json` on *every* autosave. Storing images as base64 **inline** means each keystroke-triggered save rewrites megabytes — a direct performance and file-bloat problem, and it inflates the corruption blast radius (one bad byte quarantines the entire library).
   - **Decision 1 — sidecar files (adopted):** store attachments as **separate files** in the app-data dir (e.g. `attachments/<id>.png`) and keep only a **reference/filename** in the `Note`. Keeps `notes.json` small and the autosave path fast. Requires a new fs capability, a copy-in-on-attach command, and cleanup-on-delete (today `delete` just drops the map entry — [`lib.rs:231`](src-tauri/note-store/src/lib.rs) — it would need to also remove orphaned files).
   - Data model: add `attachments: Vec<String>` (serde-default to empty).
   - *Rejected:* inline base64, for the whole-file-rewrite reasons above.
4. **Priority tier** — **Nice-to-Have.** Real value, but the sidecar-file approach is non-trivial and introduces the app's first non-JSON on-disk assets (backup, delete-cleanup, and recovery all need thought).
5. *(not an AI feature)*
6. **Conflicts / integration points** — **Secondary impact on local storage size** (now unbounded by user image choices) and on the delete path (orphan cleanup). Note that with sidecar files, "my notes" is no longer a single portable `notes.json` — backup/export now spans a folder. Storage strategy itself is **settled (Decision 1)**.

---

## 6. Voice-to-text input

1. **Description** — Dictate note content by speaking instead of typing.
2. **User benefit** — Faster capture, hands-free / accessibility.
3. **Technical considerations** — The integration point *was* a product-defining fork; **Decision 2 settles it as local (on-device).**
   - *Local (on-device model, e.g. a Whisper-class engine) — chosen:* preserves the "never leaves your machine" promise, but ships a large model/binary, raises app size substantially, and adds real compute cost. No bundler exists on the frontend, so a heavy JS/WASM path is awkward; a **Rust-side sidecar is the natural home** and is the recommended implementation.
   - *External cloud STT — rejected:* small and accurate, but **breaks the headline privacy guarantee** and adds network + an API key + per-use cost.

   Either path needs **microphone capture + OS permission**, new to this app. Output just flows into the existing `update_note_content` command — the transcription is the hard part, not the persistence.
4. **Priority tier** — **Nice-to-Have, sequenced last (Decision 3).** Even after going local, it carries the largest app-size and compute cost on the list, so it ships after the cheaper wins and the offline substrate exist.
5. *(not an AI feature per se, but governed by the same offline-first policy)*
6. **Conflicts / integration points** — Governed by the app-wide **offline-first policy (Decision 2)**, shared with Surprise Me and both AI features. Adds a new OS-permission surface (mic).

---

# UX Enhancements

## 7. Focus Mode

1. **Description** — Notes not being edited blur and reduce opacity; the one being edited snaps back to full clarity.
2. **User benefit** — Cuts visual noise on a crowded desktop; directs attention to the active note.
3. **Technical considerations** — **Frontend-only, no data model change** (unless the on/off toggle is persisted, which would be one small global setting). Each note window already knows its own focus/blur state; drive CSS `filter: blur()` + opacity off the webview's focus events. The cross-window coordination is naturally handled because only one window is focused at a time.
4. **Priority tier** — **High Priority.** Low-risk, high-perceived-polish, and it's the cheapest way to exercise the opacity mechanism before the persistent slider (#2) is built.
5. *(not an AI feature)*
6. **Conflicts / integration points** — **Tightly coupled to the Opacity slider (#2)** — "return to normal opacity" must resolve to the note's persisted slider value, not a constant (the ownership rule in #2). Define that contract before building #2. No storage impact.

---

## 8. "Surprise Me" button

1. **Description** — A user-triggered button that reads the whole note library and returns a greeting, motivational quote, or activity suggestion.
2. **User benefit** — A light, delightful moment; makes the app feel alive without nagging.
3. **Technical considerations** — **Needs the Library Hub surface that doesn't exist yet.** Each note window only knows its own note; "analyze entire library" requires either a new aggregate window or a backend command that reads the whole store (the `list_notes` / `store.all()` path already exists to build on — [`src/lib.rs:100`](src-tauri/src/lib.rs)). Output style is settled by **Decision 2: local template/rule-based** (pick from canned quotes, fill in note-derived context) — not a cloud LLM. For a "surprise," local template-based is entirely credible for v1.
4. **Priority tier** — **Nice-to-Have.** Fun, not core; gated on the hub surface.
5. *(Runs locally per Decision 2; no external model.)*
6. **Conflicts / integration points** — **Shares the Library Hub prerequisite with both AI features** — build that once and all three get cheaper. **Weighting is settled (Decision 4): ignore empty notes, exclude protected notes (Decision 6), mild recency bias.**

---

# AI-Powered Features

> Both run **user-triggered via an optional button** — never real-time, never scheduled. That
> constraint is a gift: it removes latency-on-keystroke pressure and lets the heavy work happen
> on demand. Both also need the **Library Hub** surface and a **similarity substrate** — build
> those two things once and share them. Both are **offline (Decision 2): lexical similarity, not
> cloud embeddings.**

## 9. Smart Organization (auto-group similar notes)

1. **Description** — On demand, cluster similar notes and propose collections to file them into.
2. **User benefit** — Tames a sprawling pile of notes without manual sorting.
3. **Technical considerations** — **Hard dependency on Note grouping (#3)** — there must be a group concept (settled as `group_id`, Decision 5) to assign notes into before auto-grouping means anything. Needs the similarity substrate (shared with #10) and the Library Hub to present proposed clusters for approval. Because it's user-triggered over the full store, an O(n²) pairwise pass is acceptable at realistic note counts; no need to precompute or index for v1.
4. **Priority tier** — **Nice-to-Have** — and specifically *after* grouping ships. It's a complexity multiplier layered on a foundational feature.
5. **Plain-language logic outline (how it works under the hood):**
   - For each note, build a compact **representation of its meaning**. Per **Decision 2 this is lexical** — normalize text, drop stop-words, weight distinctive terms (TF-IDF-style), fully offline, no model. (Embeddings stay as a future upgrade *only* if the privacy promise is later relaxed.)
   - **Compare every note against every other** to get a similarity score.
   - **Cluster** notes whose similarity clears a threshold into candidate groups.
   - **Propose, don't impose:** show the user the suggested groups and let them accept/reject/rename. On accept, write `group_id` onto the notes via the normal persistence path.
6. **Conflicts / integration points** — Depends on #3 (grouping) and shares substrate with #10. **Excludes empty + protected notes (Decisions 4 & 6).** Secondary impact: a bulk group-assignment triggers a full-store rewrite — fine as a one-shot user action, would not be fine on a timer (it isn't on one).

---

## 10. Smart Duplicate Detection (detect similar notes on save; offer Merge / Keep Both / Show Existing)

1. **Description** — When a new note is committed, detect similarity to existing notes and offer three actions: **Merge**, **Keep Both**, or **Show Existing Note** with an explanation of why they're related.
2. **User benefit** — Prevents accidental fragmentation of the same thought across many notes.
3. **Technical considerations** — **The trigger point needs defining, because there is no discrete "save" event today.** Persistence is a 350ms autosave on every keystroke pause ([`main.js:48`](src/main.js)); firing duplicate-detection on that cadence would spam the user constantly. **Adopt an explicit commit moment** — e.g. on note blur/close, or a deliberate "done" action — as the detection trigger. Reuses the similarity substrate from #9 (**lexical, Decision 2**) and needs a comparison against the existing store (the `store.all()` path). The **Merge** action is new destructive-ish logic: combine two notes' content into one and remove/redirect the other (touches `delete` + `set_content`); define the merge rule (concatenate? de-dupe lines?).
4. **Priority tier** — **Nice-to-Have.** Genuinely useful, but depends on the shared substrate and on nailing the trigger + merge semantics.
5. **Plain-language logic outline (how it works under the hood):**
   - At the **commit moment** (not on every keystroke), take the note just written and build the same **lexical meaning representation** used in #9.
   - **Score it against every existing note.** Keep the best match.
   - If the top match clears a **similarity threshold**, surface it with a **plain reason** ("both mention X, Y, Z" — derived from the overlapping distinctive terms).
   - Offer three actions:
     - **Merge** — combine the two into one note (per the defined merge rule), then remove the redundant one.
     - **Keep Both** — dismiss; record nothing beyond not asking again for this pair.
     - **Show Existing** — open the matched note's window so the user can decide.
   - Threshold tuning matters: too low = nag, too high = misses. **Thresholds are tuned during testing (Decision 4)**, not fixed now.
6. **Conflicts / integration points** — Shares substrate + Library-read plumbing with #9. **Excludes empty + protected notes (Decisions 4 & 6).** The **Merge** action interacts with the window model — deleting the redundant note must close its window (the existing `delete_note` already does this — [`src/lib.rs:151`](src-tauri/src/lib.rs)). Needs a defined commit trigger, which the current autosave loop does not provide.

---

# Resolved Decisions

The forks that were blocking clean sequencing are now settled — each adopts the recommended default so engineers can scope without waiting. Rationale is kept short; revisit any of these if the team's constraints change.

1. **Image attachment storage → sidecar files.** Store attachments as `attachments/<id>.ext` in the app-data dir with only a filename reference in the `Note`. Inline base64 is rejected: the whole-file autosave rewrite would re-serialize every image on every keystroke pause and widen the corruption blast radius. Cost accepted — new fs capability, copy-in-on-attach, and orphan cleanup on delete. *(Feature #5.)*

2. **All "intelligence" is offline-first (policy, not per-feature).** Surprise Me, both AI features, and voice-to-text stay on-device. This protects the headline "never leaves your machine" guarantee. Concretely: **lexical** similarity (not cloud embeddings), **template-based** Surprise Me (not a cloud LLM), and a **local STT** engine for voice. An upgrade path to embeddings/LLM stays open **only** if the team later decides to relax the privacy promise — a product decision, not an engineering one. *(Features #6, #8, #9, #10.)*

3. **Voice-to-text → local engine, sequenced last.** Given Decision 2, cloud STT is out. A local engine (Whisper-class sidecar on the Rust side) is the path, but it carries the largest app-size and compute cost on the list, so it ships **after** the cheaper wins and the offline substrate exist. *(Feature #6.)*

4. **Library-analysis weighting → exclude empty notes, mild recency bias, thresholds tuned in testing.** Surprise Me, Smart Organization, and Smart Duplicate Detection all skip empty/near-empty notes and apply a mild recency bias so results feel current. Similarity thresholds are tuned during testing rather than fixed now. *(Features #8, #9, #10.)*

5. **Note grouping → start with a `group_id` field.** Add `group_id: Option<String>` (serde-default, backward-compatible) rather than restructuring the on-disk schema up front. Pay the structured-`Group`-entity cost later *only* if group-level metadata/settings become a real requirement — and if the team already knows that's coming, make that schema change once, before Smart Organization is built on top of it. *(Feature #3.)*

6. **Protected notes are excluded from all library-analysis features.** Whether protection ends up gate-only or encrypted-at-rest, Surprise Me / Smart Organization / Smart Duplicate Detection never surface or cluster protected content while locked. *(Features #4, #8, #9, #10.)*

> The two remaining implementation choices left deliberately open — because they should be settled *in code review with a running prototype*, not on paper — are: **(a)** the Password protection crypto path (gate-only vs. encrypt-at-rest, Feature #4), and **(b)** the exact Merge rule for duplicate detection (concatenate vs. de-dupe lines, Feature #10). Neither blocks sequencing.

---

## One-glance summary

| # | Feature | Tier | Model change? | Depends on | Biggest remaining risk |
|---|---------|------|---------------|------------|------------------------|
| 1 | Timestamps | High | No (fields exist) | — | UI space only |
| 7 | Focus Mode | High | No | pairs with #2 | — |
| 2 | Opacity slider | High | `opacity` | coordinate w/ #7 | window transparency config |
| 3 | Note grouping | High (epic) | `group_id` (Decision 5) | — | window/stacking model |
| 4 | Password/PIN | Nice | `protected` + secret store | — | crypto path (gate vs. encrypt) |
| 5 | Image attachments | Nice | `attachments` | — | storage settled (sidecar); orphan cleanup |
| 6 | Voice-to-text | Nice (last) | No | offline policy (Decision 2) | app size / compute (local engine) |
| 8 | Surprise Me | Nice | No | Library Hub | — (local, weighting settled) |
| 9 | Smart Organization | Nice | uses `group_id` | **#3** + substrate + Hub | needs grouping first |
| 10 | Smart Duplicate Detection | Nice | No | substrate + Hub | no discrete "save" trigger today |

**Two pieces of shared infrastructure unlock four features: Note grouping (#3) and the Library Hub surface (#5-hub). Build those deliberately and early.**
