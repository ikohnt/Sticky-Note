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
// The note body is a contenteditable div so formatting (bold / italic /
// headings / lists) renders live while you type. Content is stored as HTML.
const editor = document.getElementById("content");
const timestampEl = document.getElementById("timestamp");
const opacityInput = document.getElementById("opacity");
const attachmentsEl = document.getElementById("attachments");
const lockBtn = document.getElementById("lock-note");
const pinBtn = document.getElementById("pin-note");
const moreBtn = document.getElementById("more-note");
const deleteBtn = document.getElementById("delete-note");
const ctxMenu = document.getElementById("ctx-menu");
const onboardTip = document.getElementById("onboard-tip");
const formatBar = document.getElementById("format-bar");
const lockOverlay = document.getElementById("lock-overlay");
const unlockInput = document.getElementById("unlock-input");
const unlockError = document.getElementById("unlock-error");
const masterOverlay = document.getElementById("master-overlay");
const masterNew = document.getElementById("master-new");
const masterConfirm = document.getElementById("master-confirm");
const masterError = document.getElementById("master-error");

const COLORS = ["yellow", "pink", "blue", "green", "purple"];

// Inline SVG for the lock button (swapped by protection state).
const SVG_ATTRS =
  'viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"';
const ICON_LOCK_OPEN = `<svg ${SVG_ATTRS}><rect width="18" height="11" x="3" y="11" rx="2"/><path d="M7 11V7a5 5 0 0 1 9.9-1"/></svg>`;
const ICON_LOCK_CLOSED = `<svg ${SVG_ATTRS}><rect width="18" height="11" x="3" y="11" rx="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/></svg>`;

function isDark() {
  return localStorage.getItem("theme") === "dark";
}
function applyColor(color) {
  const chosen = COLORS.includes(color) ? color : "yellow";
  // Swap only the colour class (preserve has-bg etc.).
  for (const c of COLORS) body.classList.remove("color-" + c);
  body.classList.add("color-" + chosen);
  body.classList.toggle("dark", isDark());
}
function toggleTheme() {
  localStorage.setItem("theme", isDark() ? "light" : "dark");
  applyColor(currentNote ? currentNote.color : "yellow");
  if (currentNote) applyStyle(currentNote);
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
  // note). Focus Mode dims + blurs the note while it isn't focused — but only
  // when the (global, remembered) toggle is on.
  const dim = focusModeEnabled() && !isFocused;
  const effective = dim ? baselineOpacity * 0.55 : baselineOpacity;
  invoke("set_window_opacity", { opacity: effective }).catch((err) =>
    console.error("Failed to set window opacity:", err),
  );
  body.style.filter = dim ? "blur(1.5px)" : "none";
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
      img.title = "Edit image";
      img.style.cursor = "pointer";
      img.addEventListener("click", () => openImageEditor(file, dataUrl));
      const del = document.createElement("button");
      del.className = "attachment-del";
      del.title = "Remove image";
      del.innerHTML =
        '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" aria-hidden="true"><path d="M18 6 6 18M6 6l12 12"/></svg>';
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

async function attachImage() {
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
}

function openLibrary() {
  invoke("open_hub").catch((err) => console.error("Failed to open library:", err));
}

// ---- Pin (always on top) ---------------------------------------------------
function renderPin() {
  const pinned = !!(currentNote && currentNote.pinned);
  pinBtn.classList.toggle("active", pinned);
  pinBtn.title = pinned ? "Unpin (stop keeping on top)" : "Keep on top";
}
pinBtn.addEventListener("click", async () => {
  if (!currentNote) return;
  const pinned = !currentNote.pinned;
  currentNote.pinned = pinned;
  renderPin();
  try {
    await invoke("set_note_pinned", { id: noteId, pinned });
  } catch (err) {
    console.error("Failed to pin:", err);
  }
});

// ---- Focus Mode toggle (global, remembered) --------------------------------
// Stored in localStorage (shared across note windows); the `storage` event lets
// other open notes react when it's toggled.
function focusModeEnabled() {
  return localStorage.getItem("focusMode") !== "off";
}
function toggleFocusMode() {
  localStorage.setItem("focusMode", focusModeEnabled() ? "off" : "on");
  renderOpacity();
}
window.addEventListener("storage", (e) => {
  if (e.key === "focusMode") renderOpacity();
  if (e.key === "theme") {
    applyColor(currentNote ? currentNote.color : "yellow");
    if (currentNote) applyStyle(currentNote);
  }
});

// ---- Style: custom palette, background image, typography -------------------
const stylePanel = document.getElementById("style-panel");
const fontSelect = document.getElementById("font-select");
const sizeSelect = document.getElementById("size-select");
const spacingSelect = document.getElementById("spacing-select");

const FONTS = {
  sans: '-apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif',
  serif: 'Georgia, "Times New Roman", Times, serif',
  mono: '"Consolas", "Courier New", monospace',
  casual: '"Comic Sans MS", "Segoe Print", "Segoe UI", cursive',
};
// Preset palettes (solid + gradients + a dark slate). `note` may be a gradient.
const PRESETS = [
  { note: "linear-gradient(135deg,#ff9a56,#ff6f9c)", header: "#c94e72", ink: "#3a0f1e" },
  { note: "linear-gradient(135deg,#7ee8fa,#4f9dff)", header: "#2f6fb0", ink: "#0a2a44" },
  { note: "linear-gradient(135deg,#a8e6a1,#4fb06a)", header: "#2e7d46", ink: "#0e3d1e" },
  { note: "linear-gradient(135deg,#c4a3ff,#8b6bd9)", header: "#553f9e", ink: "#241640" },
  { note: "linear-gradient(135deg,#ffd3a5,#fd9db1)", header: "#c76d84", ink: "#3d1522" },
  { note: "linear-gradient(160deg,#2b2f3a,#4a5568)", header: "#20242e", ink: "#e7ebf3" },
];

function clamp255(v) {
  return Math.max(0, Math.min(255, Math.round(v)));
}
function toHex(r, g, b) {
  return "#" + [r, g, b].map((v) => clamp255(v).toString(16).padStart(2, "0")).join("");
}
function shade(r, g, b, amt) {
  // amt in -1..1: negative = darker, positive = lighter.
  if (amt < 0) {
    const f = 1 + amt;
    return [r * f, g * f, b * f];
  }
  return [r + (255 - r) * amt, g + (255 - g) * amt, b + (255 - b) * amt];
}

// Derive a {note, header, ink} palette from an image. Average luminance decides
// light vs dark; the dominant colour drives the header, shaded to contrast.
function analyzeImage(dataUrl) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => {
      const s = 24;
      const c = document.createElement("canvas");
      c.width = s;
      c.height = s;
      const ctx = c.getContext("2d", { willReadFrequently: true });
      ctx.drawImage(img, 0, 0, s, s);
      let d;
      try {
        d = ctx.getImageData(0, 0, s, s).data;
      } catch (e) {
        reject(e);
        return;
      }
      let r = 0, g = 0, b = 0, n = 0;
      const buckets = {};
      for (let i = 0; i < d.length; i += 4) {
        if (d[i + 3] < 128) continue;
        r += d[i]; g += d[i + 1]; b += d[i + 2]; n++;
        const key = (d[i] >> 5) + "," + (d[i + 1] >> 5) + "," + (d[i + 2] >> 5);
        buckets[key] = (buckets[key] || 0) + 1;
      }
      if (!n) { reject(new Error("empty image")); return; }
      r /= n; g /= n; b /= n;
      const lum = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
      let best = null, bestN = 0;
      for (const k in buckets) if (buckets[k] > bestN) { bestN = buckets[k]; best = k; }
      const bp = best.split(",").map((v) => (parseInt(v, 10) << 5) + 16);
      const isLight = lum > 0.5;
      const note = toHex(...shade(r, g, b, isLight ? 0.35 : -0.55));
      const header = toHex(...shade(bp[0], bp[1], bp[2], isLight ? -0.35 : 0.4));
      const ink = isLight ? "#1c1a10" : "#f2eee0";
      resolve({ note, header, ink });
    };
    img.onerror = () => reject(new Error("image load failed"));
    img.src = dataUrl;
  });
}

// Apply a note's custom appearance (palette / background / typography).
async function applyStyle(note) {
  if (note.palette) {
    body.style.setProperty("--note", note.palette.note);
    body.style.setProperty("--header", note.palette.header);
    body.style.setProperty("--ink", note.palette.ink);
  } else {
    body.style.removeProperty("--note");
    body.style.removeProperty("--header");
    body.style.removeProperty("--ink");
  }
  if (note.bg_image) {
    try {
      const url = await invoke("read_attachment", { id: noteId, file: note.bg_image });
      body.style.backgroundImage = `url("${url}")`;
      body.classList.add("has-bg");
    } catch (err) {
      console.error("Failed to load background:", err);
    }
  } else {
    body.style.backgroundImage = "";
    body.classList.remove("has-bg");
  }
  editor.style.fontFamily = note.font ? FONTS[note.font] || "" : "";
  editor.style.fontSize = note.font_size ? note.font_size + "px" : "";
  editor.style.lineHeight = note.line_height ? String(note.line_height) : "";
}

function renderPresets() {
  const row = document.getElementById("preset-row");
  row.innerHTML = "";
  for (const p of PRESETS) {
    const b = document.createElement("button");
    b.className = "preset";
    b.style.background = p.note;
    b.title = "Apply this style";
    b.addEventListener("click", () => applyPalette(p, null));
    row.appendChild(b);
  }
}

async function applyPalette(palette, bgImage) {
  if (!currentNote) return;
  currentNote.palette = palette;
  currentNote.bg_image = bgImage;
  await applyStyle(currentNote);
  try {
    await invoke("set_note_palette", { id: noteId, palette, bgImage });
  } catch (err) {
    console.error("Failed to save palette:", err);
  }
}

async function pickImage() {
  const selected = await dialog.open({
    multiple: false,
    directory: false,
    filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "gif", "webp", "bmp"] }],
  });
  if (!selected) return null;
  return Array.isArray(selected) ? selected[0] : selected;
}

document.getElementById("theme-from-image").addEventListener("click", async () => {
  try {
    const path = await pickImage();
    if (!path) return;
    const dataUrl = await invoke("read_image_data", { sourcePath: path });
    const palette = await analyzeImage(dataUrl);
    await applyPalette(palette, null);
  } catch (err) {
    console.error("Theme-from-image failed:", err);
  }
});

document.getElementById("bg-image").addEventListener("click", async () => {
  try {
    const path = await pickImage();
    if (!path) return;
    const file = await invoke("set_background_image", { id: noteId, sourcePath: path });
    const dataUrl = await invoke("read_image_data", { sourcePath: path });
    const palette = await analyzeImage(dataUrl);
    await applyPalette(palette, file);
  } catch (err) {
    console.error("Background image failed:", err);
  }
});

document.getElementById("reset-style").addEventListener("click", async () => {
  await applyPalette(null, null);
  await setTypography(null, null, null);
  fontSelect.value = "";
  sizeSelect.value = "";
  spacingSelect.value = "";
});

async function setTypography(font, size, line) {
  if (!currentNote) return;
  currentNote.font = font;
  currentNote.font_size = size;
  currentNote.line_height = line;
  await applyStyle(currentNote);
  try {
    await invoke("set_note_typography", {
      id: noteId,
      font,
      fontSize: size,
      lineHeight: line,
    });
  } catch (err) {
    console.error("Failed to save typography:", err);
  }
}

function readTypographyControls() {
  const font = fontSelect.value || null;
  const size = sizeSelect.value ? Number(sizeSelect.value) : null;
  const line = spacingSelect.value ? Number(spacingSelect.value) : null;
  setTypography(font, size, line);
}
fontSelect.addEventListener("change", readTypographyControls);
sizeSelect.addEventListener("change", readTypographyControls);
spacingSelect.addEventListener("change", readTypographyControls);

function openStylePanel() {
  renderPresets();
  fontSelect.value = currentNote && currentNote.font ? currentNote.font : "";
  sizeSelect.value = currentNote && currentNote.font_size ? String(currentNote.font_size) : "";
  spacingSelect.value = currentNote && currentNote.line_height ? String(currentNote.line_height) : "";
  stylePanel.hidden = false;
}
document.getElementById("style-close").addEventListener("click", () => {
  stylePanel.hidden = true;
});

// ---- Attachment editor: crop / zoom / rotate ------------------------------
// Click a thumbnail to reframe it. The canvas IS the crop frame: the image is
// drawn beneath it (scaled / rotated / panned) and we export just the frame.
const editorModal = document.getElementById("img-editor");
const editorCanvas = document.getElementById("editor-canvas");
const editorZoom = document.getElementById("editor-zoom");
let editorImg = null;
let editingFile = null;
let editorBaseZoom = 1; // scale that makes the image cover the frame at zoom 100%
let editorRotation = 0; // radians, in 90° steps
let editorPanX = 0;
let editorPanY = 0;

// Scale that makes the (possibly rotated) image just cover the crop frame.
function coverZoom(imgW, imgH, rotation) {
  const swapped = Math.abs(Math.sin(rotation)) > 0.5; // 90° / 270°
  const iw = swapped ? imgH : imgW;
  const ih = swapped ? imgW : imgH;
  return Math.max(editorCanvas.width / iw, editorCanvas.height / ih);
}

function editorRender() {
  if (!editorImg) return;
  const ctx = editorCanvas.getContext("2d");
  const w = editorCanvas.width;
  const h = editorCanvas.height;
  ctx.clearRect(0, 0, w, h);
  const zoom = editorBaseZoom * (Number(editorZoom.value) / 100);
  ctx.save();
  ctx.translate(w / 2 + editorPanX, h / 2 + editorPanY);
  ctx.rotate(editorRotation);
  ctx.scale(zoom, zoom);
  ctx.drawImage(editorImg, -editorImg.width / 2, -editorImg.height / 2);
  ctx.restore();
}

function openImageEditor(file, dataUrl) {
  editingFile = file;
  editorRotation = 0;
  editorPanX = 0;
  editorPanY = 0;
  editorZoom.value = "100";
  const img = new Image();
  img.onload = () => {
    editorImg = img;
    editorBaseZoom = coverZoom(img.width, img.height, editorRotation);
    editorModal.hidden = false;
    editorRender();
  };
  img.onerror = () => console.error("Failed to load image for editing");
  img.src = dataUrl;
}

function closeImageEditor() {
  editorModal.hidden = true;
  editorImg = null;
  editingFile = null;
}

editorZoom.addEventListener("input", editorRender);

document.getElementById("editor-rotate").addEventListener("click", () => {
  editorRotation = (editorRotation + Math.PI / 2) % (Math.PI * 2);
  editorPanX = 0;
  editorPanY = 0;
  if (editorImg) editorBaseZoom = coverZoom(editorImg.width, editorImg.height, editorRotation);
  editorRender();
});

// Drag to pan the image within the frame (delta scaled to canvas pixels).
let editorDragging = false;
let editorLastX = 0;
let editorLastY = 0;
editorCanvas.addEventListener("mousedown", (e) => {
  editorDragging = true;
  editorLastX = e.clientX;
  editorLastY = e.clientY;
  editorCanvas.style.cursor = "grabbing";
});
window.addEventListener("mousemove", (e) => {
  if (!editorDragging) return;
  const rect = editorCanvas.getBoundingClientRect();
  editorPanX += (e.clientX - editorLastX) * (editorCanvas.width / rect.width);
  editorPanY += (e.clientY - editorLastY) * (editorCanvas.height / rect.height);
  editorLastX = e.clientX;
  editorLastY = e.clientY;
  editorRender();
});
window.addEventListener("mouseup", () => {
  if (!editorDragging) return;
  editorDragging = false;
  editorCanvas.style.cursor = "grab";
});

document.getElementById("editor-cancel").addEventListener("click", closeImageEditor);

document.getElementById("editor-save").addEventListener("click", async () => {
  if (!editingFile) return;
  const file = editingFile;
  try {
    const dataUrl = editorCanvas.toDataURL("image/png");
    await invoke("save_edited_attachment", { id: noteId, file, dataUrl });
    closeImageEditor();
    const note = await invoke("get_note", { id: noteId });
    if (note) currentNote = note;
    renderAttachments(note ? note.attachments : []);
  } catch (err) {
    console.error("Failed to save edited image:", err);
  }
});

// ---- Context menu ----------------------------------------------------------
function ctxLabel(action, text) {
  const el = ctxMenu.querySelector(`[data-action="${action}"] .ct`);
  if (el) el.textContent = text; // updates the text span, keeps the icon
}
function openCtxMenu(x, y) {
  // Reflect current state in the toggle labels.
  ctxLabel("pin", currentNote && currentNote.pinned ? "Unpin" : "Keep on top");
  ctxLabel("protect", currentNote && currentNote.protected ? "Remove protection" : "Protect this note");
  ctxLabel("focus", "Focus Mode: " + (focusModeEnabled() ? "on" : "off"));
  ctxLabel("theme", "Dark theme: " + (isDark() ? "on" : "off"));
  ctxMenu.hidden = false;
  // Keep it on-screen.
  const w = ctxMenu.offsetWidth || 180;
  const h = ctxMenu.offsetHeight || 200;
  ctxMenu.style.left = Math.min(x, window.innerWidth - w - 4) + "px";
  ctxMenu.style.top = Math.min(y, window.innerHeight - h - 4) + "px";
}
function closeCtxMenu() {
  ctxMenu.hidden = true;
}
body.addEventListener("contextmenu", (e) => {
  // Keep the native copy/paste menu inside the editor; our menu everywhere else.
  if (editor.contains(e.target)) return;
  e.preventDefault();
  openCtxMenu(e.clientX, e.clientY);
});
moreBtn.addEventListener("click", (e) => {
  e.stopPropagation();
  const r = moreBtn.getBoundingClientRect();
  openCtxMenu(r.left, r.bottom);
});
document.addEventListener("click", (e) => {
  if (!ctxMenu.hidden && !ctxMenu.contains(e.target)) closeCtxMenu();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeCtxMenu();
});
ctxMenu.querySelectorAll(".ctx-item").forEach((item) => {
  item.addEventListener("click", () => {
    closeCtxMenu();
    switch (item.dataset.action) {
      case "pin": pinBtn.click(); break;
      case "attach": attachImage(); break;
      case "protect": lockBtn.click(); break;
      case "focus": toggleFocusMode(); break;
      case "theme": toggleTheme(); break;
      case "style": openStylePanel(); break;
      case "library": openLibrary(); break;
      case "delete": deleteBtn.click(); break;
    }
  });
});

// ---- First-run tip ---------------------------------------------------------
if (!localStorage.getItem("onboarded")) {
  onboardTip.hidden = false;
}
document.getElementById("onboard-dismiss").addEventListener("click", () => {
  localStorage.setItem("onboarded", "1");
  onboardTip.hidden = true;
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
  formatBar.hidden = !!lockedNote;
  editor.contentEditable = lockedNote ? "false" : "true";
  if (lockedNote) {
    unlockInput.value = "";
    unlockError.hidden = true;
    setTimeout(() => unlockInput.focus(), 30);
  }
  if (currentNote && currentNote.protected) {
    lockBtn.innerHTML = ICON_LOCK_CLOSED;
    lockBtn.title = vaultLocked ? "Locked — unlock to edit" : "Remove protection";
  } else {
    lockBtn.innerHTML = ICON_LOCK_OPEN;
    lockBtn.title = "Protect this note";
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
      // Lock right away so the note blurs and requires the password to reopen.
      await invoke("lock_vault");
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
    // Lock right away so the note blurs and requires the password to reopen.
    await invoke("lock_vault");
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
    // One IPC round-trip for the whole boot payload.
    const { note, locked } = await invoke("note_bootstrap", { id: noteId });
    vaultLocked = !!locked;
    if (note) {
      currentNote = note;
      setEditorContent(note.content ?? "");
      applyColor(note.color);
      applyStyle(note);
      setBaselineOpacity(note.opacity ?? 1.0, false);
      renderTimestamp(note);
      renderAttachments(note.attachments);
      renderPin();
      lastCheckedContent = editor.innerText;
      applyLockState();
    }
  } catch (err) {
    console.error("Failed to load note:", err);
  }
  // Focus the note (unless it's locked — applyLockState focuses the password box).
  if (!(currentNote && currentNote.protected && vaultLocked)) {
    focusEditorEnd();
  }
}

// ---- Autosave text (debounced) -------------------------------------------
const saveContent = debounce(async () => {
  try {
    await invoke("update_note_content", { id: noteId, content: editor.innerHTML });
    // Refresh the "edited" time locally instead of a second round-trip.
    if (currentNote) {
      currentNote.updated_at = Date.now();
      renderTimestamp(currentNote);
    }
  } catch (err) {
    console.error("Failed to save content:", err);
  }
}, 350);

editor.addEventListener("input", () => {
  // Deleting everything can leave a stray <br>/<div>; reset to truly empty so the
  // placeholder returns and the note saves empty. The querySelector guard keeps
  // checklist / list / image-only notes (which have no plain text) intact.
  if (
    editor.textContent.trim() === "" &&
    !editor.querySelector(".task-box, input, img, li, hr") &&
    editor.innerHTML !== ""
  ) {
    editor.innerHTML = "";
  }
  saveContent();
});

// Paste as plain text so no arbitrary markup enters the note.
editor.addEventListener("paste", (e) => {
  e.preventDefault();
  const text = (e.clipboardData || window.clipboardData).getData("text/plain");
  document.execCommand("insertText", false, text);
});

// ---- Content (contenteditable) helpers ------------------------------------
// Tags kept when loading/sanitising stored HTML. The CSP already blocks inline
// scripts and event handlers, so this is defence-in-depth plus tidiness.
const ALLOWED_TAGS = new Set([
  "B", "STRONG", "I", "EM", "U", "H1", "H2", "H3",
  "UL", "OL", "LI", "BR", "DIV", "P", "SPAN",
]);
// Safe presentational attributes we keep (no handlers, no URLs).
const KEEP_ATTRS = new Set(["class", "contenteditable", "data-checked", "role", "aria-checked"]);

// Build a checklist box: a plain span, not an <input>. Real form controls are
// unreliable to click inside contenteditable, and a checkbox's activation
// behaviour fights any manual toggle.
function makeTaskBox(doc, checked) {
  const box = doc.createElement("span");
  box.className = "task-box";
  box.setAttribute("contenteditable", "false");
  box.setAttribute("role", "checkbox");
  box.setAttribute("data-checked", checked ? "true" : "false");
  box.setAttribute("aria-checked", checked ? "true" : "false");
  return box;
}

function sanitizeHtml(html) {
  const doc = new DOMParser().parseFromString(html, "text/html");
  const clean = (node) => {
    for (const el of [...node.children]) {
      clean(el); // depth-first: children are clean before we judge the parent
      // Migrate any earlier native checkbox to the span box, keeping its state.
      if (el.tagName === "INPUT" && el.getAttribute("type") === "checkbox") {
        el.replaceWith(makeTaskBox(doc, el.hasAttribute("checked")));
        continue;
      }
      if (!ALLOWED_TAGS.has(el.tagName)) {
        el.replaceWith(...el.childNodes); // unwrap unknown tags, keep their text
        continue;
      }
      for (const a of [...el.attributes]) {
        if (!KEEP_ATTRS.has(a.name.toLowerCase())) el.removeAttribute(a.name);
      }
    }
  };
  clean(doc.body);
  return doc.body.innerHTML;
}
// Distinguish rich content we saved (innerHTML — escapes </&/> to entities and
// uses our tags) from legacy plain-text notes (raw chars, real newlines, no
// entities). Only a known tag OR an HTML entity counts as rich, so a plain note
// containing "i<j" or "a & b" is loaded verbatim instead of being mangled.
const RICH_TAG = /<\/?(?:b|strong|i|em|u|h[1-3]|ul|ol|li|br|div|p|span)\b/i;
const HTML_ENTITY = /&(?:[a-z]+|#\d+|#x[0-9a-f]+);/i;
function setEditorContent(raw) {
  if (RICH_TAG.test(raw) || HTML_ENTITY.test(raw)) {
    editor.innerHTML = sanitizeHtml(raw); // stored rich content
  } else {
    editor.textContent = raw; // legacy plain text (pre-wrap keeps newlines)
  }
}
function focusEditorEnd() {
  editor.focus();
  const sel = window.getSelection();
  if (!sel) return;
  sel.selectAllChildren(editor);
  sel.collapseToEnd();
}

// ---- Checklists: click the box in a checklist item to tick it -------------
// The box is a span, so there's no default action to cancel — we just flip the
// attribute, which both drives the CSS tick and persists via innerHTML.
editor.addEventListener("click", (e) => {
  const target = e.target;
  if (!(target instanceof Element)) return;
  const box = target.closest(".task-box");
  if (!box || !editor.contains(box)) return;
  const checked = box.getAttribute("data-checked") !== "true";
  box.setAttribute("data-checked", checked ? "true" : "false");
  box.setAttribute("aria-checked", checked ? "true" : "false");
  saveContent();
});

// ---- Live formatting (WYSIWYG) --------------------------------------------
// Buttons apply real formatting to the contenteditable via execCommand, so
// bold / italic / headings / lists render as you type.
function toggleHeading() {
  const block = String(document.queryCommandValue("formatBlock") || "");
  document.execCommand("formatBlock", false, /h[1-6]/i.test(block) ? "<div>" : "<h2>");
}
function insertCheckItem() {
  document.execCommand(
    "insertHTML",
    false,
    '<div class="task"><span class="task-box" contenteditable="false" role="checkbox" ' +
      'data-checked="false" aria-checked="false"></span>&nbsp;</div>'
  );
}
function applyFormat(fmt) {
  if (!editor.isContentEditable) return;
  editor.focus();
  switch (fmt) {
    case "bold": document.execCommand("bold"); break;
    case "italic": document.execCommand("italic"); break;
    case "underline": document.execCommand("underline"); break;
    case "heading": toggleHeading(); break;
    case "list": document.execCommand("insertUnorderedList"); break;
    case "check": insertCheckItem(); break;
  }
  saveContent();
}

document.querySelectorAll("#format-bar .fmt-btn").forEach((btn) => {
  // Keep the editor's selection/caret when the button takes the click.
  btn.addEventListener("mousedown", (e) => e.preventDefault());
  btn.addEventListener("click", () => applyFormat(btn.dataset.fmt));
});

editor.addEventListener("keydown", (e) => {
  if (!(e.ctrlKey || e.metaKey)) return;
  const k = e.key.toLowerCase();
  if (k === "b") { e.preventDefault(); applyFormat("bold"); }
  else if (k === "i") { e.preventDefault(); applyFormat("italic"); }
  else if (k === "u") { e.preventDefault(); applyFormat("underline"); }
});

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
// Runs at a commit moment (the editor losing focus), never on every keystroke.
// Pairs the user has dismissed with "Keep both" aren't asked about again.
const dupModal = document.getElementById("dup-modal");
const dupReason = document.getElementById("dup-reason");
let lastCheckedContent = "";
const dismissedMatches = new Set();
let currentMatch = null;

async function checkForDuplicate() {
  const value = editor.innerText.trim();
  if (!value || value === lastCheckedContent.trim()) return;
  lastCheckedContent = editor.innerText;
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

editor.addEventListener("blur", checkForDuplicate);

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

// After a drag settles, persist the position and then ask the backend whether
// this note was dropped onto another note (or a clip) — if so, they get clipped
// into a stack and this window closes.
const onMoveSettled = debounce(async () => {
  await persistGeometry();
  try {
    await invoke("try_clip_on_drop", { id: noteId });
  } catch (err) {
    console.error("Clip-on-drop failed:", err);
  }
}, 500);

win.onMoved(onMoveSettled);
win.onResized(persistGeometryDebounced);

// ---- Resize grip (frameless window) --------------------------------------
document.getElementById("resize-handle").addEventListener("mousedown", (e) => {
  e.preventDefault();
  const dir = ResizeDirection ? ResizeDirection.SouthEast : "SouthEast";
  win.startResizeDragging(dir).catch((err) => console.error("resize failed:", err));
});

boot();
