//! Integration tests exercising the public `note-store` API end to end,
//! the same way the Tauri layer uses it: load -> mutate -> reload.

use std::fs;

use note_store::{NoteStore, StoreError, DEFAULT_COLOR};
use tempfile::tempdir;

#[test]
fn full_lifecycle_survives_restart() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("notes.json");

    // First "session": create three notes and edit them.
    let (id1, id2, id3);
    {
        let mut store = NoteStore::load(&path).unwrap();
        id1 = store.create(100.0, 100.0).unwrap().id;
        id2 = store.create(130.0, 130.0).unwrap().id;
        id3 = store.create(160.0, 160.0).unwrap().id;

        store.set_content(&id1, "buy milk".into()).unwrap();
        store.set_color(&id2, "blue".into()).unwrap();
        store.set_geometry(&id3, 500.0, 220.0, 320.0, 300.0).unwrap();
    }

    // Second "session": everything restored exactly.
    {
        let store = NoteStore::load(&path).unwrap();
        assert_eq!(store.len(), 3);
        assert_eq!(store.get(&id1).unwrap().content, "buy milk");
        assert_eq!(store.get(&id1).unwrap().color, DEFAULT_COLOR);
        assert_eq!(store.get(&id2).unwrap().color, "blue");
        let n3 = store.get(&id3).unwrap();
        assert_eq!((n3.x, n3.y, n3.width, n3.height), (500.0, 220.0, 320.0, 300.0));
    }

    // Third "session": delete one, confirm persistence.
    {
        let mut store = NoteStore::load(&path).unwrap();
        store.delete(&id2).unwrap();
    }
    {
        let store = NoteStore::load(&path).unwrap();
        assert_eq!(store.len(), 2);
        assert!(store.get(&id2).is_none());
    }
}

#[test]
fn concurrent_style_reopen_sees_latest_write() {
    // Simulates the single-instance guarantee: one writer at a time, each new
    // load observes all prior committed writes.
    let dir = tempdir().unwrap();
    let path = dir.path().join("notes.json");

    let id = {
        let mut a = NoteStore::load(&path).unwrap();
        a.create(0.0, 0.0).unwrap().id
    };

    let mut b = NoteStore::load(&path).unwrap();
    assert_eq!(b.len(), 1);
    b.set_content(&id, "written by second handle".into()).unwrap();

    let c = NoteStore::load(&path).unwrap();
    assert_eq!(c.get(&id).unwrap().content, "written by second handle");
}

#[test]
fn corrupted_store_recovers_without_data_loss_of_backup() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("notes.json");
    let garbage = b"<<< not json at all >>>";
    fs::write(&path, garbage).unwrap();

    let store = NoteStore::load(&path).unwrap();
    assert!(store.is_empty());

    // The original bytes are preserved in a quarantine file.
    let backup = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().contains("corrupt"))
        .expect("quarantine file exists");
    assert_eq!(fs::read(backup.path()).unwrap(), garbage);
}

#[test]
fn not_found_error_reports_id() {
    let dir = tempdir().unwrap();
    let mut store = NoteStore::load(dir.path().join("notes.json")).unwrap();
    match store.delete("abc123") {
        Err(StoreError::NotFound(id)) => assert_eq!(id, "abc123"),
        other => panic!("expected NotFound, got {other:?}"),
    }
}
