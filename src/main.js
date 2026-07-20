// Sticky note window logic. Uses the global Tauri API (`withGlobalTauri: true`)
// so no bundler/build step is required for the frontend.

const { invoke } = window.__TAURI__.core;
const { getCurrentWindow } = window.__TAURI__.window;
const ResizeDirection = window.__TAURI__.window.ResizeDirection;

const win = getCurrentWindow();
// Window labels are "note-<id>"; recover the note id from the label.
const noteId = win.label.startsWith("note-") ? win.label.slice("note-".length) : win.label;

const body = document.body;
const textarea = document.getElementById("content");

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

// ---- Load the note --------------------------------------------------------
async function boot() {
  try {
    const note = await invoke("get_note", { id: noteId });
    if (note) {
      textarea.value = note.content ?? "";
      applyColor(note.color);
    }
  } catch (err) {
    console.error("Failed to load note:", err);
  }
  // Focus and place the caret at the end.
  textarea.focus();
  const end = textarea.value.length;
  textarea.setSelectionRange(end, end);
}

// ---- Autosave text (debounced) -------------------------------------------
const saveContent = debounce(async (value) => {
  try {
    await invoke("update_note_content", { id: noteId, content: value });
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
