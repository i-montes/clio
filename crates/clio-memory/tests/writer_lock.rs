#![allow(clippy::unwrap_used)] // test helpers: panicking is the failure mode

mod common;

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use clio_memory::*;
use common::*;

fn clock() -> Arc<dyn Clock> {
    Arc::new(FixedClock::new(1000))
}

#[test]
fn t16_second_writer_gets_clear_error() {
    let f = fixture();
    let second = Engine::open(&f.path, OpenMode::ReadWrite, clock());
    assert!(matches!(second, Err(Error::WriterLocked)));
    let Fixture {
        path, engine, _dir, ..
    } = f;
    drop(engine);
    assert!(Engine::open(&path, OpenMode::ReadWrite, clock()).is_ok());
}

#[test]
fn read_only_engine_reads_but_cannot_write() {
    let f = fixture();
    let m = f
        .engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    let ro = Engine::open(&f.path, OpenMode::ReadOnly, clock()).unwrap();
    assert_eq!(ro.get(m.memory).unwrap().content, "vive en Medellín");
    assert_eq!(
        ro.search(f.scope, "Medellín", SearchOpts::default())
            .unwrap()
            .len(),
        1
    );
    assert!(matches!(
        ro.propose(prop(f.scope, f.person, "otro")),
        Err(Error::ReadOnly)
    ));
    assert!(matches!(ro.create_scope("x", "es"), Err(Error::ReadOnly)));
}

// S-23: file permissions must be 0600
#[test]
fn s23_open_rejects_0644_file_and_new_file_is_0600() {
    let dir = tempfile::tempdir().unwrap();
    let loose = dir.path().join("loose.db");
    std::fs::write(&loose, b"").unwrap();
    std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        Engine::open(&loose, OpenMode::ReadWrite, clock()),
        Err(Error::InsecurePermissions(_))
    ));

    let fresh = dir.path().join("fresh.db");
    Engine::open(&fresh, OpenMode::ReadWrite, clock()).unwrap();
    let mode = std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn engine_survives_reopen() {
    let f = fixture();
    let m = f
        .engine
        .propose(prop(f.scope, f.person, "dato persistente"))
        .unwrap();
    let Fixture {
        path, engine, _dir, ..
    } = f;
    drop(engine);
    let again = Engine::open(&path, OpenMode::ReadWrite, clock()).unwrap();
    assert_eq!(again.get(m.memory).unwrap().content, "dato persistente");
}

fn file_with_mode(dir: &std::path::Path, name: &str, mode: u32) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, b"").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    path
}

#[test]
fn read_only_open_of_a_missing_file_fails_and_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("typo.db");
    assert!(matches!(
        Engine::open(&path, OpenMode::ReadOnly, clock()),
        Err(Error::NotFound)
    ));
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "nothing may be created"
    );
}

#[test]
fn read_only_open_of_an_empty_file_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = file_with_mode(dir.path(), "empty.db", 0o600);
    assert!(matches!(
        Engine::open(&path, OpenMode::ReadOnly, clock()),
        Err(Error::NotFound)
    ));
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        0,
        "the file must be left untouched"
    );
}

#[test]
fn read_only_open_of_a_newer_schema_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.db");
    rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        Engine::open(&path, OpenMode::ReadOnly, clock()),
        Err(Error::SchemaTooNew(99))
    ));
}

#[test]
fn read_only_open_still_rejects_loose_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let path = file_with_mode(dir.path(), "loose.db", 0o644);
    assert!(matches!(
        Engine::open(&path, OpenMode::ReadOnly, clock()),
        Err(Error::InsecurePermissions(_))
    ));
}

#[test]
fn read_only_open_of_a_wal_database_leaves_the_database_file_unchanged() {
    let f = fixture();
    f.engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    let Fixture {
        path,
        engine,
        scope,
        _dir,
        ..
    } = f;
    drop(engine); // closes the writer, which checkpoints the WAL into the database file
    let before = std::fs::read(&path).unwrap();
    {
        let ro = Engine::open(&path, OpenMode::ReadOnly, clock()).unwrap();
        assert_eq!(
            ro.search(scope, "Medellín", SearchOpts::default())
                .unwrap()
                .len(),
            1
        );
    }
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the database file must not change"
    );
}

#[test]
fn read_only_open_in_a_non_writable_directory_is_a_clear_error() {
    let f = fixture();
    f.engine
        .propose(prop(f.scope, f.person, "vive en Medellín"))
        .unwrap();
    let Fixture {
        path, engine, _dir, ..
    } = f;
    drop(engine);
    let dir = path.parent().unwrap().to_path_buf();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let restore =
        || std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    if std::fs::File::create(dir.join("probe")).is_ok() {
        restore(); // running as root: permissions are not enforced, nothing to test
        return;
    }
    let before: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    let result = Engine::open(&path, OpenMode::ReadOnly, clock());
    let after: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    restore();
    assert!(
        matches!(&result, Err(Error::UnwritableDirectory(d)) if *d == dir),
        "{:?}",
        result.err()
    );
    assert_eq!(before.len(), after.len(), "no files may appear");
}

#[test]
fn an_unreadable_database_file_is_not_blamed_on_the_directory() {
    let f = fixture();
    let Fixture {
        path, engine, _dir, ..
    } = f;
    drop(engine);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    let restore =
        || std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    if std::fs::File::open(&path).is_ok() {
        restore(); // running as root: permissions are not enforced, nothing to test
        return;
    }
    let result = Engine::open(&path, OpenMode::ReadOnly, clock());
    restore();
    assert!(
        !matches!(result, Err(Error::UnwritableDirectory(_))),
        "the directory is writable; the file is the problem: {:?}",
        result.err()
    );
    assert!(result.is_err());
}
