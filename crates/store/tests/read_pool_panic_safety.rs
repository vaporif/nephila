//! Regression test: panics inside scopes that hold a pooled connection
//! must NOT leak the connection. Without RAII guarding, a single panic in
//! `spawn_blocking` would permanently shrink the pool by one; over time
//! the pool drains to `Exhausted` for all readers.

use nephila_store::read_pool::ReadPool;
use std::sync::Arc;

#[test]
fn panic_inside_acquire_guarded_scope_returns_connection() {
    // Build a tiny in-memory pool against a shared name, then panic while
    // holding a guarded connection. Verify the pool's available count
    // returns to its starting size after the panic unwinds.
    let share_name = format!("nephila-panic-test-{}", uuid::Uuid::new_v4());

    // Open a writer-side connection first so the in-memory db exists with
    // schema. Without this, the read-only pool would attach to an empty db.
    let _writer = rusqlite::Connection::open_with_flags(
        format!("file:{share_name}?mode=memory&cache=shared"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_URI
            | rusqlite::OpenFlags::SQLITE_OPEN_FULL_MUTEX,
    )
    .unwrap();

    let pool = Arc::new(ReadPool::open_in_memory_shared(&share_name, 4).unwrap());
    assert_eq!(pool.available(), 4);

    // Panic while holding a guarded connection.
    let pool_for_panic = pool.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = pool_for_panic.acquire_guarded().unwrap();
        // While the guard is alive, the pool sees one fewer connection.
        assert_eq!(pool_for_panic.available(), 3);
        panic!("simulated panic inside guarded scope");
    }));
    assert!(result.is_err(), "expected panic to propagate");

    // Connection must be back in the pool.
    assert_eq!(
        pool.available(),
        4,
        "pool leaked a connection after a panic inside the guarded scope",
    );
}

#[test]
fn dropped_guard_normally_returns_connection() {
    let share_name = format!("nephila-panic-test-{}", uuid::Uuid::new_v4());
    let _writer = rusqlite::Connection::open_with_flags(
        format!("file:{share_name}?mode=memory&cache=shared"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_URI
            | rusqlite::OpenFlags::SQLITE_OPEN_FULL_MUTEX,
    )
    .unwrap();

    let pool = Arc::new(ReadPool::open_in_memory_shared(&share_name, 2).unwrap());
    assert_eq!(pool.available(), 2);
    {
        let _guard = pool.acquire_guarded().unwrap();
        assert_eq!(pool.available(), 1);
    }
    assert_eq!(pool.available(), 2);
}
