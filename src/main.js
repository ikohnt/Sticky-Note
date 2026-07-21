// Sticky note window logic. Uses the global Tauri API (`withGlobalTauri: true`)
// so no bundler/build step is required for the frontend.

const { invoke } = window.__TAURI__.core;
const { getCurrentWindow } = window.__TAURI__.window;
const { listen } = window.__TAURI__.event;
const dialog = window.__TAURI__.dialog;
const ResizeDirection = window.__TAURI__.window.ResizeDirection;

const win = getCurrentWindow();
// Window labels are "note-<id>"; recover the note id from the label.
const noteId = win.label.startsWith("note-") ? win.label.slice("note-".length) : win.label;

const body = document.body;
const textarea = document.getElementById("content");
const timestampEl = document.getElementById("timestamp");
const opacityInput = document.getElementById("opacity");
const groupBadge = document.getElementById("group-badge");
const attachmentsEl = document.getElementById("attachments");
const lockBtn = document.getElementById("lock-note");
const lockOverlay = document.getElementById("lock-overlay");
const unlockInput = document.getElementById("unlock-input");
const unlockError = document.getElementById("unlock-error");
const masterOverlay = document.getElementById("master-overlay");
const masterNew = document.getElementById("master-new");
const masterConfirm = document.getElementById("master-confirm");
const masterError = document.getElementById("master-error");

const COLORS = ["yellow", "pink", "blue", "green", "purple"];

function applyColor(color) {
  const chosen = COLORS.includes(color) ? color : "yellow";
  body.className = "color-" + chosen;
}

function debounce(fn, ms) {
  let timer;
  return (...args) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), ms);
  };
}

// ---- Opacity + Focus Mode --------------------------------------------------
// The slider sets the note's *baseline* opacity; Focus Mode dims and blurs the
// note while it isn't the focused window, multiplying against that baseline
// (Decision: slider owns the baseline, Focus Mode multiplies).
let baselineOpacity = 1.0;
let isFocused = true;

function renderOpacity() {
  // Apply translucency at the OS window level (shows the desktop through the
  // note); a slight blur when unfocused is the Focus Mode cue.
  const effective = isFocused ? baselineOpacity : baselineOpacity * 0.55;
  invoke("set_window_opacity", { opacity: effective }).catch((err) =>
    console.error("Failed to set window opacity:", err),
  );
  body.style.filter = isFocused ? "none" : "blur(1.5px)";
}

function setBaselineOpacity(value, persist) {
  baselineOpacity = Math.min(1, Math.max(0.3, value));
  opacityInput.value = String(Math.round(baselineOpacity * 100));
  renderOpacity();
  if (persist) {
    invoke("update_note_opacity", { id: noteId, opacity: baselineOpacity }).catch((err) =>
      console.error("Failed to save opacity:", err),
    );
  }
}

opacityInput.addEventListener("input", () => {
  setBaselineOpacity(Number(opacityInput.value) / 100, true);
});

// Focus Mode: react to this window gaining/losing focus.
win.onFocusChanged(({ payload: focused }) => {
  isFocused = focused;
  renderOpacity();
});

// ---- Timestamps ------------------------------------------------------------
function relativeTime(ms) {
  if (!ms) return "";
  const diff = Date.now() - ms;
  const min = Math.round(diff / 60000);
  if (min < 1) return "just now";
  if (min < 60) return `${min}m ago`;
  const hr = Math.round(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.round(hr / 24);
  if (day < 30) return `${day}d ago`;
  return new Date(ms).toLocaleDateString();
}

function renderTimestamp(note) {
  if (!note) return;
  timestampEl.textContent = "Edited " + relativeTime(note.updated_at);
  const created = note.created_at ? new Date(note.created_at).toLocaleString() : "";
  const updated = note.updated_at ? new Date(note.updated_at).toLocaleString() : "";
  timestampEl.title = `Created ${created}\nUpdated ${updated}`;
}

// ---- Group badge -----------------------------------------------------------
function renderGroup(groupId) {
  if (groupId) {
    groupBadge.textContent = "▤ " + groupId;
    groupBadge.hidden = false;
  } else {
    groupBadge.hidden = true;
  }
}

// ---- Attachments -----------------------------------------------------------
async function renderAttachments(files) {
  attachmentsEl.innerHTML = "";
  // Don't render attachments for a locked protected note — their bytes are sealed.
  const lockedNote = currentNote && currentNote.protected && vaultLocked;
  if (!files || files.length === 0 || lockedNote) {
    attachmentsEl.hidden = true;
    return;
  }
  attachmentsEl.hidden = false;
  for (const file of files) {
    try {
      const dataUrl = await invoke("read_attachment", { id: noteId, file });
      const wrap = document.createElement("div");
      wrap.className = "attachment";
      const img = document.createElement("img");
      img.src = dataUrl;
      img.alt = file;
      const del = document.createElement("button");
      del.className = "attachment-del";
      del.title = "Remove image";
      del.textContent = "×";
      del.addEventListener("click", async () => {
        try {
          await invoke("remove_attachment", { id: noteId, file });
          const note = await invoke("get_note", { id: noteId });
          if (note) currentNote = note;
          renderAttachments(note ? note.attachments : []);
        } catch (err) {
          console.error("Failed to remove attachment:", err);
        }
      });
      wrap.appendChild(img);
      wrap.appendChild(del);
      attachmentsEl.appendChild(wrap);
    } catch (err) {
      console.error("Failed to render attachment:", err);
    }
  }
}

document.getElementById("attach-note").addEventListener("click", async () => {
  try {
    const selected = await dialog.open({
      multiple: false,
      directory: false,
      filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "gif", "webp", "bmp"] }],
    });
    if (!selected) return;
    const sourcePath = Array.isArray(selected) ? selected[0] : selected;
    await invoke("attach_image", { id: noteId, sourcePath });
    const note = await invoke("get_note", { id: noteId });
    if (note) currentNote = note;
    renderAttachments(note ? note.attachments : []);
  } catch (err) {
    console.error("Failed to attach image:", err);
  }
});

// ---- Library hub -----------------------------------------------------------
document.getElementById("open-library").addEventListener("click", () => {
  invoke("open_hub").catch((err) => console.error("Failed to open library:", err));
});

// ---- Password protection ---------------------------------------------------
// A single master password protects all protected notes. The vault is locked at
// startup; unlocking once (from any note) reveals every protected note (the
// backend emits "vault-changed", which reloads all windows).
let currentNote = null;
let vaultLocked = false;

function applyLockState() {
  const lockedNote = currentNote && currentNote.protected && vaultLocked;
  lockOverlay.hidden = !lockedNote;
  textarea.disabled = !!lockedNote;
  if (lockedNote) {
    unlockInput.value = "";
    unlockError.hidden = true;
    setTimeout(() => unlockInput.focus(), 30);
  }
  if (currentNote && currentNote.protected) {
    lockBtn.textContent = "🔒";
    lockBtn.title = vaultLocked ? "Locked — unlock to edit" : "Remove protection";
  } else {
    lockBtn.textContent = "🔓";
    lockBtn.title = "Protect this note";
  }
}

async function refreshVaultState() {
  try {
    vaultLocked = await invoke("is_locked");
  } catch (err) {
    console.error("Failed to read lock state:", err);
    vaultLocked = false;
  }
}

lockBtn.addEventListener("click", async () => {
  if (!currentNote) return;
  try {
    if (currentNote.protected) {
      if (vaultLocked) {
        lockOverlay.hidden = false;
        unlockInput.focus();
        return;
      }
      await invoke("set_note_protected", { id: noteId, protected: false });
    } else {
      const hasMaster = await invoke("has_master");
      if (!hasMaster) {
        masterNew.value = "";
        masterConfirm.value = "";
        masterError.hidden = true;
        masterOverlay.hidden = false;
        masterNew.focus();
        return;
      }
      if (vaultLocked) {
        lockOverlay.hidden = false;
        unlockInput.focus();
        return;
      }
      await invoke("set_note_protected", { id: noteId, protected: true });
    }
  } catch (err) {
    console.error("Protection toggle failed:", err);
  }
});

async function doUnlock() {
  const pw = unlockInput.value;
  if (!pw) return;
  try {
    const ok = await invoke("unlock_vault", { password: pw });
    if (!ok) {
      unlockError.hidden = false;
      unlockInput.select();
      return;
    }
    lockOverlay.hidden = true; // vault-changed will also reload this window
  } catch (err) {
    console.error("Unlock failed:", err);
  }
}
document.getElementById("unlock-btn").addEventListener("click", doUnlock);
unlockInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter") doUnlock();
});

document.getElementById("master-create").addEventListener("click", async () => {
  const pw = masterNew.value;
  if (pw.length < 4) {
    masterError.textContent = "Use at least 4 characters.";
    masterError.hidden = false;
    return;
  }
  if (pw !== masterConfirm.value) {
    masterError.textContent = "Passwords don't match.";
    masterError.hidden = false;
    return;
  }
  try {
    await invoke("set_master_password", { password: pw });
    await invoke("set_note_protected", { id: noteId, protected: true });
    masterOverlay.hidden = true;
  } catch (err) {
    console.error("Failed to set master password:", err);
    masterError.textContent = "Something went wrong.";
    masterError.hidden = false;
  }
});
document.getElementById("master-cancel").addEventListener("click", () => {
  masterOverlay.hidden = true;
});

// Reload when protection changes or the vault is unlocked anywhere.
listen("vault-changed", () => boot());

// ---- Load the note --------------------------------------------------------
async function boot() {
  try {
    await refreshVaultState();
    const note = await invoke("get_note", { id: noteId });
    if (note) {
      currentNote = note;
      textarea.value = note.content ?? "";
      applyColor(note.color);
      setBaselineOpacity(note.opacity ?? 1.0, false);
      renderTimestamp(note);
      renderGroup(note.group_id);
      renderAttachments(note.attachments);
      lastCheckedContent = note.content ?? "";
      applyLockState();
    }
  } catch (err) {
    console.error("Failed to load note:", err);
  }
  // Focus the note (unless it's locked — applyLockState focuses the password box).
  if (!(currentNote && currentNote.protected && vaultLocked)) {
    textarea.focus();
    const end = textarea.value.length;
    textarea.setSelectionRange(end, end);
  }
}

// ---- Autosave text (debounced) -------------------------------------------
const saveContent = debounce(async (value) => {
  try {
    await invoke("update_note_content", { id: noteId, content: value });
    const note = await invoke("get_note", { id: noteId });
    renderTimestamp(note);
  } catch (err) {
    console.error("Failed to save content:", err);
  }
}, 350);

textarea.addEventListener("input", () => saveContent(textarea.value));

// ---- Color swatches -------------------------------------------------------
document.querySelectorAll(".swatch").forEach((btn) => {
  btn.addEventListener("click", async () => {
    const color = btn.dataset.color;
    applyColor(color);
    try {
      await invoke("update_note_color", { id: noteId, color });
    } catch (err) {
      console.error("Failed to save color:", err);
    }
  });
});

// ---- New / delete ---------------------------------------------------------
document.getElementById("new-note").addEventListener("click", async () => {
  try {
    await invoke("create_note");
  } catch (err) {
    console.error("Failed to create note:", err);
  }
});

document.getElementById("delete-note").addEventListener("click", async () => {
  try {
    // Backend deletes the note and closes this window.
    await invoke("delete_note", { id: noteId });
  } catch (err) {
    console.error("Failed to delete note:", err);
  }
});

// ---- Smart Duplicate Detection --------------------------------------------
// Runs at a commit moment (the textarea losing focus), never on every keystroke.
// Pairs the user has dismissed with "Keep both" aren't asked about again.
const dupModal = document.getElementById("dup-modal");
const dupReason = document.getElementById("dup-reason");
let lastCheckedContent = "";
const dismissedMatches = new Set();
let currentMatch = null;

async function checkForDuplicate() {
  const value = textarea.value.trim();
  if (!value || value === lastCheckedContent.trim()) return;
  lastCheckedContent = textarea.value;
  try {
    const match = await invoke("find_duplicate", { id: noteId });
    if (match && !dismissedMatches.has(match.id)) {
      currentMatch = match;
      const terms = (match.shared_terms || []).slice(0, 4).join(", ");
      dupReason.textContent = terms
        ? `Both notes mention: ${terms}.`
        : "This note looks a lot like an existing one.";
      dupModal.hidden = false;
    }
  } catch (err) {
    console.error("Duplicate check failed:", err);
  }
}

textarea.addEventListener("blur", checkForDuplicate);

document.getElementById("dup-merge").addEventListener("click", async () => {
  if (!currentMatch) return;
  try {
    // Merge this (source) note into the existing (target) note; backend closes us.
    await invoke("merge_notes", { sourceId: noteId, targetId: currentMatch.id });
  } catch (err) {
    console.error("Merge failed:", err);
    dupModal.hidden = true;
  }
});

document.getElementById("dup-keep").addEventListener("click", () => {
  if (currentMatch) dismissedMatches.add(currentMatch.id);
  dupModal.hidden = true;
  currentMatch = null;
});

document.getElementById("dup-show").addEventListener("click", async () => {
  if (currentMatch) {
    try {
      await invoke("focus_note", { id: currentMatch.id });
    } catch (err) {
      console.error("Failed to show existing note:", err);
    }
  }
  dupModal.hidden = true;
  currentMatch = null;
});

// ---- Persist window geometry (position + size) ----------------------------
async function persistGeometry() {
  try {
    const scale = await win.scaleFactor();
    const pos = await win.outerPosition(); // physical pixels
    const size = await win.innerSize(); // physical pixels
    await invoke("update_note_geometry", {
      id: noteId,
      x: pos.x / scale,
      y: pos.y / scale,
      width: size.width / scale,
      height: size.height / scale,
    });
  } catch (err) {
    console.error("Failed to save geometry:", err);
  }
}
const persistGeometryDebounced = debounce(persistGeometry, 450);

win.onMoved(persistGeometryDebounced);
win.onResized(persistGeometryDebounced);

// ---- Resize grip (frameless window) --------------------------------------
document.getElementById("resize-handle").addEventListener("mousedown", (e) => {
  e.preventDefault();
  const dir = ResizeDirection ? ResizeDirection.SouthEast : "SouthEast";
  win.startResizeDragging(dir).catch((err) => console.error("resize failed:", err));
});

boot();
