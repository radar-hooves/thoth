//! The database layer shares one connection (WAL, a busy timeout) across
//! threads. Its own binary: the connection is process-global and opened once.

use tempfile::TempDir;
use thoth_lib::database::{self, DatabaseError, Transcription};

#[test]
fn one_connection_serves_concurrent_writers() {
    let temp = TempDir::new().expect("create temp dir");
    // SAFETY: set before any thread starts; nothing else reads it yet.
    unsafe { std::env::set_var("THOTH_DATA_DIR", temp.path()) };

    assert!(matches!(
        database::connection(),
        Err(DatabaseError::NotInitialised)
    ));
    database::initialise_database().expect("initialise");
    database::initialise_database().expect("a second initialise is harmless");

    {
        let conn = database::connection().expect("connection");
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .expect("journal_mode");
        let busy: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .expect("busy_timeout");
        assert_eq!(mode, "wal");
        assert_eq!(busy, 5000);
    }

    const WRITERS: usize = 8;
    let handles: Vec<_> = (0..WRITERS)
        .map(|i| {
            std::thread::spawn(move || {
                database::create_transcription(&Transcription::new(format!("writer {i}")))?;
                database::list_transcriptions(Some(50), None).map(|_| ())
            })
        })
        .collect();
    for h in handles {
        h.join().expect("thread").expect("write and read");
    }
    assert_eq!(
        database::count_transcriptions(None).expect("count"),
        WRITERS
    );
}
