//! Creates a small memory file to try the `clio-memory` command line on:
//! `cargo run -p clio-memory --example demo -- demo.db`
//! The CLI cannot create scopes or actors (only code that embeds the crate can), so this does.

use std::sync::Arc;

use clio_memory::*;

fn main() -> Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "demo.db".into());
    let engine = Engine::open(path.as_ref(), OpenMode::ReadWrite, Arc::new(SystemClock))?;
    let scope = engine.create_scope("main", "es")?;
    let ana = engine.register_actor("ana", ActorKind::Person, Quota::default())?;
    let bot = engine.register_actor("bot", ActorKind::Agent, Quota::default())?;
    let propose = |actor, origin, content: &str, topic: Option<&str>| {
        engine.propose(Proposal {
            scope,
            actor,
            kind: MemoryType::Semantic,
            content: content.into(),
            topic_key: topic.map(String::from),
            origin,
            session: None,
            episode: None,
            source: None,
            valid_from: None,
            valid_to: None,
        })
    };
    propose(ana, Origin::User, "Vive en Medellín", Some("residencia"))?;
    propose(ana, Origin::User, "Trabaja en un periódico digital", None)?;
    // something that came from a web page waits in quarantine until a person promotes it
    propose(bot, Origin::External, "El periódico se fundó en 2010", None)?;
    println!("created {path}");
    Ok(())
}
