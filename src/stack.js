// Clip "stack" window: renders one clip (a group of notes) in a single window,
// showing one note at a time. It collapses a set of related notes into one tidy
// stack you can flip through. Uses the global Tauri API; no bundler.

const { invoke } = window.__TAURI__.core;
const { getCurrentWindow } = window.__TAURI__.window;
const { listen } = window.__TAURI__.event;
const ResizeDirection = window.__TAURI__.window.ResizeDirection;

const win = getCurrentWindow();
// Window labels are "clip-<id>"; recover the clip id from the label.
const clipId = win.label.startsWith("clip-") ? win.label.slice("clip-".length) : win.label;

const body = document.body;
const textarea = document.getElementById("content");
const nameInput = document.getElementById("clip-name");
const posEl = document.getElementById("clip-pos");

const COLORS = ["yellow", "pink", "blue", "green", "purple"];
function applyColor(color) {
  const chosen = COLORS.includes(color) ? color : "yellow";
  const dark = localStorage.getItem("theme") === "dark" ? " dark" : "";
  body.className = "color-" + chosen + " stack" + dark;
}
window.addEventListener("storage", (e) => {
  if (e.key === "theme") {
    const n = current();
    applyColor(n ? n.color : "yellow");
  }
});
function debounce(fn, ms) {
  let t;
  return (...a) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...a), ms);
  };
}

let notes = []; // notes in this clip, in stack order
let index = 0; // which note is on top

function current() {
  return notes[index];
}

function showCurrent() {
  posEl.textContent = notes.length ? `${index + 1} / ${notes.length}` : "empty";
  const n = current();
  if (!n) {
    textarea.value = "";
    textarea.disabled = true;
    return;
  }
  textarea.disabled = false;
  textarea.value = n.content ?? "";
  applyColor(n.color);
}

async function load() {
  try {
    const clip = await invoke("get_clip", { clipId });
    if (!clip) {
      // The clip was dissolved elsewhere; this window is going away.
      win.close();
      return;
    }
    nameInput.value = clip.name || "Clip";
    notes = await invoke("clip_notes", { clipId });
    if (index >= notes.length) index = Math.max(0, notes.length - 1);
    showCurrent();
  } catch (err) {
    console.error("Failed to load clip:", err);
  }
}

// ---- Edit the current note (autosave) -------------------------------------
const saveContent = debounce(async (value) => {
  const n = current();
  if (!n) return;
  try {
    await invoke("update_note_content", { id: n.id, content: value });
    n.content = value;
  } catch (err) {
    console.error("Failed to save content:", err);
  }
}, 350);
textarea.addEventListener("input", () => saveContent(textarea.value));

// ---- Colour (applies to the current note) ---------------------------------
document.querySelectorAll(".swatch").forEach((btn) => {
  btn.addEventListener("click", async () => {
    const n = current();
    if (!n) return;
    const color = btn.dataset.color;
    applyColor(color);
    n.color = color;
    try {
      await invoke("update_note_color", { id: n.id, color });
    } catch (err) {
      console.error("Failed to save colour:", err);
    }
  });
});

// ---- Flip through the stack -----------------------------------------------
document.getElementById("prev").addEventListener("click", () => {
  if (!notes.length) return;
  index = (index - 1 + notes.length) % notes.length;
  showCurrent();
});
document.getElementById("next").addEventListener("click", () => {
  if (!notes.length) return;
  index = (index + 1) % notes.length;
  showCurrent();
});

// ---- Rename the clip -------------------------------------------------------
const saveName = debounce(async (name) => {
  try {
    await invoke("rename_clip", { clipId, name });
  } catch (err) {
    console.error("Failed to rename clip:", err);
  }
}, 400);
nameInput.addEventListener("input", () => saveName(nameInput.value.trim() || "Clip"));

// ---- Un-clip current / all / delete ---------------------------------------
document.getElementById("unclip-one").addEventListener("click", async () => {
  const n = current();
  if (!n) return;
  try {
    // Backend gives this note its own window and reloads us via "clips-changed"
    // (or closes this window if the clip drops below two notes).
    await invoke("unclip_note", { noteId: n.id });
  } catch (err) {
    console.error("Un-clip failed:", err);
  }
});

document.getElementById("unclip-all").addEventListener("click", async () => {
  try {
    await invoke("unclip_all", { clipId });
  } catch (err) {
    console.error("Un-clip all failed:", err);
  }
});

document.getElementById("delete-note").addEventListener("click", async () => {
  const n = current();
  if (!n) return;
  try {
    await invoke("delete_note", { id: n.id });
  } catch (err) {
    console.error("Delete failed:", err);
  }
});

// ---- Reload when clips change anywhere ------------------------------------
listen("clips-changed", () => load());

// ---- Persist the stack window's geometry onto the clip --------------------
async function persistGeometry() {
  try {
    const scale = await win.scaleFactor();
    const pos = await win.outerPosition();
    const size = await win.innerSize();
    await invoke("update_clip_geometry", {
      clipId,
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

// ---- Resize grip (frameless window) ---------------------------------------
document.getElementById("resize-handle").addEventListener("mousedown", (e) => {
  e.preventDefault();
  const dir = ResizeDirection ? ResizeDirection.SouthEast : "SouthEast";
  win.startResizeDragging(dir).catch((err) => console.error("resize failed:", err));
});

load();
