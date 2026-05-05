use nephila_store::SqliteStore;
use rusqlite::Connection;

#[test]
fn database_file_has_persistent_pragmas() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("db.sqlite");
    let _store = SqliteStore::open(&path, 384).unwrap();

    // Per-connection pragmas (cache_size, temp_store, mmap_size,
    // wal_autocheckpoint, synchronous) reset on each new Connection::open, so
    // we can only assert persistent pragmas here. journal_mode=WAL lives in
    // the file header and survives reopens.
    let conn = Connection::open(&path).unwrap();

    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "wal", "journal_mode should be WAL");
}
