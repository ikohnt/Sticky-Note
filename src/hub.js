// Library Hub window logic. Aggregates the whole note library for the two
// features that need it: "Surprise Me" and Smart Organization. Uses the global
// Tauri API; everything runs locally in the Rust backend.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const dialog = window.__TAURI__.dialog;

const searchInput = document.getElementById("search-input");
const searchResults = document.getElementById("search-results");
const trashList = document.getElementById("trash-list");
const trashEmpty = document.getElementById("trash-empty");
const emptyTrashBtn = document.getElementById("empty-trash");
const exportBtn = document.getElementById("export-btn");

const surpriseBtn = document.getElementById("surprise-btn");
const surpriseOut = document.getElementById("surprise-out");
const organizeBtn = document.getElementById("organize-btn");
const clustersEl = document.getElementById("clusters");
const organizeEmpty = document.getElementById("organize-empty");
const noteCountEl = document.getElementById("note-count");
const notePicker = document.getElementById("note-picker");
const clipSelectedBtn = document.getElementById("clip-selected");

// Cache id -> content so cluster previews can show a snippet.
let notesById = new Map();
// Ids ticked in the "Clip notes into a stack" picker.
const selected = new Set();

async function refreshNotes() {
  try {
    const notes = await invoke("list_notes");
    notesById = new Map(notes.map((n) => [n.id, n]));
    noteCountEl.textContent = notes.length === 1 ? "1 note" : `${notes.length} notes`;
    renderPicker(notes);
  } catch (err) {
    console.error("Failed to list notes:", err);
  }
}

// ---- Clip picker (multi-select) -------------------------------------------
function updateClipButton() {
  clipSelectedBtn.disabled = selected.size < 2;
}

function renderPicker(notes) {
  notePicker.innerHTML = "";
  // Only un-clipped notes can start a new clip.
  const available = notes.filter((n) => !n.group_id);
  for (const id of [...selected]) {
    if (!available.some((n) => n.id === id)) selected.delete(id);
  }
  if (available.length === 0) {
    notePicker.innerHTML = '<p class="empty">No un-clipped notes to clip right now.</p>';
    updateClipButton();
    return;
  }
  for (const n of available) {
    const row = document.createElement("label");
    row.className = "pick-row";
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.checked = selected.has(n.id);
    cb.addEventListener("change", () => {
      if (cb.checked) selected.add(n.id);
      else selected.delete(n.id);
      updateClipButton();
    });
    const text = document.createElement("span");
    text.className = "pick-text";
    text.textContent = snippet(n.content);
    row.appendChild(cb);
    row.appendChild(text);
    notePicker.appendChild(row);
  }
  updateClipButton();
}

clipSelectedBtn.addEventListener("click", async () => {
  if (selected.size < 2) return;
  clipSelectedBtn.disabled = true;
  try {
    await invoke("create_clip_from", { noteIds: [...selected], name: "Clip" });
    selected.clear();
    await refreshNotes();
  } catch (err) {
    console.error("Failed to clip notes:", err);
    updateClipButton();
  }
});

function snippet(content, max = 60) {
  const line = (content || "")
    .split("\n")
    .map((l) => l.trim())
    .find((l) => l.length > 0) || "(empty note)";
  return line.length > max ? line.slice(0, max) + "…" : line;
}

// ---- Surprise Me -----------------------------------------------------------
surpriseBtn.addEventListener("click", async () => {
  surpriseBtn.disabled = true;
  try {
    const message = await invoke("surprise_me", { hour: new Date().getHours() });
    surpriseOut.textContent = message;
    surpriseOut.hidden = false;
  } catch (err) {
    console.error("Surprise failed:", err);
    surpriseOut.textContent = "Couldn't come up with something just now.";
    surpriseOut.hidden = false;
  } finally {
    surpriseBtn.disabled = false;
  }
});

// ---- Smart Organization ----------------------------------------------------
function renderClusters(clusters) {
  clustersEl.innerHTML = "";
  if (!clusters || clusters.length === 0) {
    organizeEmpty.textContent =
      "No clusters of similar notes found. Write a few more related notes and try again.";
    organizeEmpty.hidden = false;
    return;
  }
  organizeEmpty.hidden = true;

  for (const cluster of clusters) {
    const card = document.createElement("div");
    card.className = "cluster";

    const head = document.createElement("div");
    head.className = "cluster-head";
    const title = document.createElement("span");
    title.className = "cluster-count";
    title.textContent = `${cluster.note_ids.length} similar notes`;
    head.appendChild(title);
    card.appendChild(head);

    const list = document.createElement("ul");
    list.className = "cluster-notes";
    for (const id of cluster.note_ids) {
      const li = document.createElement("li");
      const note = notesById.get(id);
      li.textContent = snippet(note ? note.content : "");
      list.appendChild(li);
    }
    card.appendChild(list);

    const row = document.createElement("div");
    row.className = "cluster-actions";
    const input = document.createElement("input");
    input.type = "text";
    input.className = "group-name";
    input.value = cluster.label || "Group";
    input.setAttribute("aria-label", "Group name");
    const accept = document.createElement("button");
    accept.className = "hub-btn small";
    accept.textContent = "Clip these";
    accept.addEventListener("click", async () => {
      accept.disabled = true;
      const name = input.value.trim() || cluster.label || "Clip";
      try {
        // Clip the cluster into a stack (this closes the notes' own windows and
        // opens one stack window for them).
        await invoke("create_clip_from", { noteIds: cluster.note_ids, name });
        card.classList.add("filed");
        row.innerHTML = "";
        const done = document.createElement("span");
        done.className = "filed-label";
        done.textContent = "Clipped into " + name;
        row.appendChild(done);
        await refreshNotes();
      } catch (err) {
        console.error("Failed to clip cluster:", err);
        accept.disabled = false;
      }
    });
    row.appendChild(input);
    row.appendChild(accept);
    card.appendChild(row);

    clustersEl.appendChild(card);
  }
}

organizeBtn.addEventListener("click", async () => {
  organizeBtn.disabled = true;
  organizeEmpty.hidden = true;
  try {
    await refreshNotes();
    const clusters = await invoke("suggest_groups");
    renderClusters(clusters);
  } catch (err) {
    console.error("Organize failed:", err);
  } finally {
    organizeBtn.disabled = false;
  }
});

// ---- Search ----------------------------------------------------------------
function runSearch() {
  const q = searchInput.value.trim().toLowerCase();
  searchResults.innerHTML = "";
  if (!q) return;
  const hits = [...notesById.values()]
    .filter((n) => (n.content || "").toLowerCase().includes(q))
    .slice(0, 20);
  if (hits.length === 0) {
    searchResults.innerHTML = '<p class="empty">No matches.</p>';
    return;
  }
  for (const n of hits) {
    const row = document.createElement("button");
    row.className = "result-row";
    row.textContent = snippet(n.content, 70);
    row.title = "Show this note";
    row.addEventListener("click", () => {
      invoke("focus_note", { id: n.id }).catch((err) => console.error("focus failed:", err));
    });
    searchResults.appendChild(row);
  }
}
searchInput.addEventListener("input", runSearch);

// ---- Trash -----------------------------------------------------------------
async function refreshTrash() {
  let trash = [];
  try {
    trash = await invoke("list_trash");
  } catch (err) {
    console.error("Failed to list trash:", err);
  }
  trashList.innerHTML = "";
  emptyTrashBtn.disabled = trash.length === 0;
  trashEmpty.hidden = trash.length !== 0;
  for (const n of trash) {
    const row = document.createElement("div");
    row.className = "trash-row";
    const text = document.createElement("span");
    text.className = "trash-text";
    text.textContent = snippet(n.content, 48);
    const restore = document.createElement("button");
    restore.className = "hub-btn small";
    restore.textContent = "Restore";
    restore.addEventListener("click", async () => {
      try {
        await invoke("restore_note", { id: n.id });
      } catch (err) {
        console.error("Restore failed:", err);
      }
    });
    const purge = document.createElement("button");
    purge.className = "hub-btn small danger";
    purge.textContent = "Delete forever";
    purge.addEventListener("click", async () => {
      try {
        await invoke("purge_note", { id: n.id });
      } catch (err) {
        console.error("Purge failed:", err);
      }
    });
    row.appendChild(text);
    row.appendChild(restore);
    row.appendChild(purge);
    trashList.appendChild(row);
  }
}
emptyTrashBtn.addEventListener("click", async () => {
  emptyTrashBtn.disabled = true;
  try {
    await invoke("empty_trash");
  } catch (err) {
    console.error("Empty trash failed:", err);
    emptyTrashBtn.disabled = false;
  }
});

// ---- Export ----------------------------------------------------------------
exportBtn.addEventListener("click", async () => {
  try {
    const path = await dialog.save({
      defaultPath: "sticky-notes.md",
      filters: [{ name: "Markdown", extensions: ["md"] }],
    });
    if (!path) return;
    const count = await invoke("export_notes_to", { path });
    const label = document.getElementById("export-label");
    label.textContent = `Exported ${count} notes`;
    setTimeout(() => {
      label.textContent = "Export all notes to Markdown";
    }, 2500);
  } catch (err) {
    console.error("Export failed:", err);
  }
});

// Keep everything fresh when notes are added/removed/restored anywhere.
listen("notes-changed", () => {
  refreshNotes();
  refreshTrash();
  runSearch();
});

refreshNotes();
refreshTrash();
