#![allow(clippy::unwrap_used)] // tests: panicking is the failure mode
//! Schema 2 (spec `2026-11-04-memory-engram-design.md`): a memory has a title and a category; the
//! person pins and reviews; judged relations; a timeline around a memory; recall over several
//! scopes; and a file of schema 1 is upgraded by its first writer, never by a reader.

mod common;

use clio_memory::*;
use common::*;

fn titled(f: &Fixture, title: &str, content: &str, category: Category) -> Proposal {
    let mut p = prop(f.scope, f.person, content);
    p.title = Some(title.into());
    p.category = Some(category);
    p
}

const DAY: i64 = 86_400;

#[test]
fn a_title_and_a_category_are_kept_searched_and_survive_a_rebuild_and_an_export() {
    let f = fixture();
    let o = f
        .engine
        .propose(titled(
            &f,
            "  Middleware   de JWT ",
            "Qué: se valida el token en cada petición · Por qué: el cliente lo pedía",
            Category::Decision,
        ))
        .unwrap();
    let row = f.engine.get(o.memory).unwrap();
    assert_eq!(
        row.title.as_deref(),
        Some("Middleware de JWT"),
        "one line, trimmed"
    );
    assert_eq!(row.category, Some(Category::Decision));
    // The title is searched with the content: «middleware» is only in the title.
    let hits = f
        .engine
        .search(f.scope, "middleware", SearchOpts::default())
        .unwrap();
    assert_eq!(hits.len(), 1);
    f.engine.rebuild_projection().unwrap();
    assert_eq!(f.engine.get(o.memory).unwrap(), row);
    assert_eq!(
        f.engine
            .search(f.scope, "middleware", SearchOpts::default())
            .unwrap()
            .len(),
        1
    );
    let mut out = Vec::new();
    f.engine.export(&mut out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let other = Engine::open(
        &dir.path().join("copy.db"),
        OpenMode::ReadWrite,
        std::sync::Arc::new(FixedClock::new(1000)),
    )
    .unwrap();
    other.import(&mut out.as_slice()).unwrap();
    assert_eq!(other.get(o.memory).unwrap(), row);
}

#[test]
fn a_title_is_checked_like_the_content() {
    let f = fixture();
    let key = format!("{}{}", "sk-", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6");
    let e = f
        .engine
        .propose(titled(
            &f,
            &format!("clave {key}"),
            "algo",
            Category::Config,
        ))
        .unwrap_err();
    assert!(matches!(e, Error::SecretDetected), "{e:?}");
    let e = f
        .engine
        .propose(titled(&f, &"x".repeat(300), "algo", Category::Config))
        .unwrap_err();
    assert!(matches!(e, Error::TooLong), "{e:?}");
    // A blank title is no title.
    let o = f
        .engine
        .propose(titled(&f, "   ", "algo más", Category::Config))
        .unwrap();
    assert_eq!(f.engine.get(o.memory).unwrap().title, None);
}

#[test]
fn a_category_has_a_name_and_a_review_cycle() {
    for c in Category::ALL {
        assert_eq!(Category::parse(c.label()), Some(c));
        assert_eq!(Category::parse(&c.label().to_uppercase()), Some(c));
    }
    assert_eq!(Category::parse("otra"), None);
    assert_eq!(Category::Config.review_days(), Some(90));
    assert_eq!(Category::Preference.review_days(), None);
    assert_eq!(
        Category::Profile.review_days(),
        None,
        "who the person is keeps"
    );
    assert_eq!(Category::parse(" Profile "), Some(Category::Profile));
    assert_eq!(Category::Profile.label(), "profile");
    for r in Relation::ALL {
        assert_eq!(Relation::parse(r.label()), Some(r));
    }
}

#[test]
fn the_person_pins_and_a_pin_goes_with_a_purge() {
    let f = fixture();
    let a = f
        .engine
        .propose(prop(f.scope, f.person, "uno"))
        .unwrap()
        .memory;
    let b = f
        .engine
        .propose(prop(f.scope, f.person, "dos"))
        .unwrap()
        .memory;
    f.clock.set(2000);
    f.engine.pin(b, f.person).unwrap();
    f.clock.set(3000);
    f.engine.pin(a, f.person).unwrap();
    f.engine.pin(a, f.person).unwrap(); // again: still once, still second
    let ids: Vec<_> = f
        .engine
        .pinned(f.scope)
        .unwrap()
        .iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, [b, a], "in the order they were pinned");
    assert!(matches!(
        f.engine.pin(a, f.agent).unwrap_err(),
        Error::NotAllowed
    ));
    let mut ext = prop(f.scope, f.agent, "de fuera");
    ext.origin = Origin::External;
    let waiting = f.engine.propose(ext).unwrap().memory;
    assert!(matches!(
        f.engine.pin(waiting, f.person).unwrap_err(),
        Error::InvalidState
    ));
    f.engine.unpin(b, f.person).unwrap();
    f.engine.purge(a, f.person, "prueba").unwrap();
    assert!(f.engine.pinned(f.scope).unwrap().is_empty());
}

#[test]
fn a_memory_comes_due_for_review_by_its_category_and_a_review_starts_it_over() {
    let f = fixture();
    let config = f
        .engine
        .propose(titled(
            &f,
            "Puerto",
            "el servidor escucha en 8080",
            Category::Config,
        ))
        .unwrap()
        .memory;
    f.engine
        .propose(titled(
            &f,
            "Café",
            "prefiere café sin azúcar",
            Category::Preference,
        ))
        .unwrap();
    f.engine
        .propose(prop(f.scope, f.person, "sin categoría"))
        .unwrap();
    assert!(f.engine.review_due(f.scope).unwrap().is_empty());
    f.clock.set(1000 + 91 * DAY);
    let due: Vec<_> = f
        .engine
        .review_due(f.scope)
        .unwrap()
        .iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(due, [config], "only the setting; a preference keeps");
    assert!(matches!(
        f.engine.mark_reviewed(config, f.agent).unwrap_err(),
        Error::NotAllowed
    ));
    f.engine.mark_reviewed(config, f.person).unwrap();
    assert!(f.engine.review_due(f.scope).unwrap().is_empty());
    f.clock.set(1000 + 182 * DAY);
    assert_eq!(f.engine.review_due(f.scope).unwrap().len(), 1, "due again");
}

#[test]
fn relations_are_judged_revised_listed_and_go_with_a_purge() {
    let f = fixture();
    let a = f
        .engine
        .propose(prop(f.scope, f.person, "usa Postgres"))
        .unwrap()
        .memory;
    let b = f
        .engine
        .propose(prop(f.scope, f.person, "usa MySQL"))
        .unwrap()
        .memory;
    f.engine
        .relate(a, b, Relation::ConflictsWith, "jev", 0.9)
        .unwrap();
    let r = f.engine.relations(b).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(
        (r[0].source, r[0].target, r[0].relation),
        (a, b, Relation::ConflictsWith)
    );
    assert_eq!(r[0].by, "jev");
    // A second verdict on the same pair revises the first.
    f.engine.relate(a, b, Relation::Scoped, "ana", 1.0).unwrap();
    let r = f.engine.relations(a).unwrap();
    assert_eq!((r.len(), r[0].relation), (1, Relation::Scoped));
    assert!(
        f.engine
            .relate(a, a, Relation::Related, "jev", 0.5)
            .is_err()
    );
    assert!(
        f.engine
            .relate(a, b, Relation::Related, "jev", 1.5)
            .is_err()
    );
    assert!(
        f.engine
            .relate(a, MemoryId(999), Relation::Related, "jev", 0.5)
            .is_err()
    );
    f.engine.purge(b, f.person, "prueba").unwrap();
    assert!(f.engine.relations(a).unwrap().is_empty());
}

#[test]
fn the_timeline_is_the_session_around_a_memory_and_else_the_scope() {
    let f = fixture();
    let in_session = |t: i64, text: &str, ep: &str| {
        f.clock.set(t);
        let mut p = prop(f.scope, f.person, text);
        p.episode = Some(ep.into());
        f.engine.propose(p).unwrap().memory
    };
    let a = in_session(100, "primero", "s1");
    let other = in_session(150, "de otra sesión", "s2");
    let b = in_session(200, "segundo", "s1");
    let c = in_session(300, "tercero", "s1");
    let d = in_session(400, "cuarto", "s1");
    let ids = |v: Vec<MemoryRow>| v.into_iter().map(|r| r.id).collect::<Vec<_>>();
    assert_eq!(ids(f.engine.timeline(c, 1, 1).unwrap()), [b, c, d]);
    assert_eq!(ids(f.engine.timeline(a, 5, 5).unwrap()), [a, b, c, d]);
    assert_eq!(
        ids(f.engine.timeline(other, 2, 2).unwrap()),
        [other],
        "alone in its session"
    );
    // Without a session: the scope, in time order.
    f.clock.set(500);
    let loose = f
        .engine
        .propose(prop(f.scope, f.person, "suelto"))
        .unwrap()
        .memory;
    assert_eq!(
        ids(f.engine.timeline(loose, 2, 0).unwrap()).last(),
        Some(&loose)
    );
    assert_eq!(f.engine.timeline(loose, 2, 0).unwrap().len(), 3);
    // Only around an active memory.
    f.engine.archive(a, f.person).unwrap();
    assert!(matches!(
        f.engine.timeline(a, 1, 1).unwrap_err(),
        Error::NotFound
    ));
    let r = f.engine.recent(f.scope, 2).unwrap();
    assert_eq!(ids(r), [loose, d]);
}

#[test]
fn recall_over_several_scopes_merges_and_abstains_only_if_all_do() {
    let f = fixture();
    let lessons = f.engine.create_scope("lecciones", "es").unwrap();
    f.engine
        .propose(prop(f.scope, f.person, "el despliegue usa docker compose"))
        .unwrap();
    f.engine
        .propose(prop(
            lessons,
            f.person,
            "docker compose sin healthcheck rompe el despliegue",
        ))
        .unwrap();
    let opts = RecallOpts::default();
    let r = f
        .engine
        .recall_many(&[f.scope, lessons], "docker compose", &opts)
        .unwrap();
    assert!(!r.abstained);
    let scopes: Vec<_> = r.hits.iter().map(|h| h.memory.scope).collect();
    assert!(
        scopes.contains(&f.scope) && scopes.contains(&lessons),
        "{scopes:?}"
    );
    assert!(
        r.hits.windows(2).all(|w| w[0].score >= w[1].score),
        "by score"
    );
    let none = f
        .engine
        .recall_many(&[f.scope, lessons], "zzz qqq", &opts)
        .unwrap();
    assert!(none.abstained && none.hits.is_empty());
}

// The first writer of a file of schema 1 upgrades it in one transaction; a reader never does.
#[test]
fn a_file_of_schema_1_is_upgraded_by_its_first_writer_and_never_by_a_reader() {
    let f = fixture();
    let kept = f
        .engine
        .propose(prop(f.scope, f.person, "un dato de antes"))
        .unwrap()
        .memory;
    let path = f.path.clone();
    drop(f.engine);
    // Back to schema 1: what the file looked like before.
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(
            "DROP INDEX memories_episode; DROP TABLE pins; DROP TABLE reviews; DROP TABLE relations;
             ALTER TABLE memories DROP COLUMN title; ALTER TABLE memories DROP COLUMN category;
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }
    let clock = std::sync::Arc::new(FixedClock::new(5000));
    let e = Engine::open(&path, OpenMode::ReadOnly, clock.clone())
        .err()
        .expect("a reader refuses an older file");
    assert!(matches!(e, Error::SchemaTooOld(1)), "{e:?}");
    let user_version = |p: &std::path::Path| -> i64 {
        rusqlite::Connection::open(p)
            .unwrap()
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap()
    };
    assert_eq!(user_version(&path), 1, "the reader changed nothing");
    let e = Engine::open(&path, OpenMode::ReadWrite, clock.clone()).unwrap();
    assert_eq!(user_version(&path), 2);
    let row = e.get(kept).unwrap();
    assert_eq!(
        (row.content.as_str(), row.title.clone()),
        ("un dato de antes", None)
    );
    e.pin(kept, ActorId(f.person.0)).unwrap();
    drop(e);
    let r = Engine::open(&path, OpenMode::ReadOnly, clock).unwrap();
    assert_eq!(r.pinned(row.scope).unwrap().len(), 1);
    assert!(
        r.doctor()
            .unwrap()
            .checks
            .iter()
            .all(|c| c.name != "schema_version" || c.ok)
    );
}

// A person moves a memory to another scope (projects merged or renamed): it is found there and no
// longer in the old one, with its history; revert brings it back; nobody else may move.
#[test]
fn a_person_moves_a_memory_and_revert_brings_it_back() {
    let f = fixture();
    let other = f.engine.create_scope("proyecto:nuevo", "es").unwrap();
    let id = f
        .engine
        .propose(prop(f.scope, f.person, "el despliegue usa compose"))
        .unwrap()
        .memory;
    let opts = RecallOpts::default();
    let found = |s| {
        f.engine
            .recall(s, "despliegue compose", &opts)
            .unwrap()
            .hits
            .iter()
            .any(|h| h.memory.id == id)
    };
    assert!(
        f.engine.move_to(id, other, f.agent).is_err(),
        "only a person"
    );
    assert!(
        f.engine.move_to(id, f.scope, f.person).is_err(),
        "same scope"
    );
    let op = f.engine.move_to(id, other, f.person).unwrap();
    assert_eq!(f.engine.get(id).unwrap().scope, other);
    assert!(found(other) && !found(f.scope));
    assert_eq!(f.engine.history(id).unwrap().len(), 2);
    f.engine.revert(op, f.person).unwrap();
    assert_eq!(f.engine.get(id).unwrap().scope, f.scope);
    assert!(found(f.scope) && !found(other));
}
