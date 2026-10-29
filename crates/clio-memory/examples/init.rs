//! One-time setup for a memory file: `cargo run -p clio-memory --example init -- DB SCOPE [LANG]`.
//! A library call on purpose: the command line cannot create scopes (S-25).

use std::sync::Arc;

use clio_memory::{Engine, OpenMode, SystemClock};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(db), Some(scope)) = (args.next(), args.next()) else {
        return Err("usage: init DB SCOPE [LANG]".into());
    };
    let lang = args.next().unwrap_or_else(|| "es".into());
    let engine = Engine::open(db.as_ref(), OpenMode::ReadWrite, Arc::new(SystemClock))?;
    if engine.scopes()?.iter().any(|s| s.name == scope) {
        println!("scope {scope} already exists");
    } else {
        engine.create_scope(&scope, &lang)?;
        println!("scope {scope} created in {db}");
    }
    Ok(())
}
