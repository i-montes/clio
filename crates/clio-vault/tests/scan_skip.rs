#![allow(clippy::unwrap_used)]
//! `scan_dir` over a folder that holds the person's own copies and installs next to Clio's output:
//! it can skip folders by name (`clio doctor` skips the copies of projects, the originals `apply`
//! saved and what npm installed for Pi, which are not what Clio wrote).

use std::sync::Arc;

use clio_core::SystemClock;
use clio_vault::proto::{Request, Side};
use clio_vault::server::handle;
use clio_vault::{Dest, Kind, Scope, SecretSpec, Vault};

// A value no key pattern would recognise: only the vault knows it.
const VALUE: &str = "violeta-azul-7731-clio";

fn vault() -> Vault {
    let (v, cap) =
        Vault::init(None, "correct horse battery staple", Arc::new(SystemClock)).unwrap();
    v.put(
        &cap,
        SecretSpec {
            name: "notion".into(),
            value: VALUE.into(),
            kind: Kind::ApiKey,
            destinations: vec![Dest::new("notion-api", "api.notion.com")],
            scope: Scope::Personal,
            rotate_every_ms: None,
            expires_at_ms: None,
        },
    )
    .unwrap();
    v
}

fn files(v: &Vault, dir: &std::path::Path, skip: &[&str]) -> Vec<String> {
    let r = handle(
        v,
        Side::Person,
        Request::ScanDir {
            path: dir.display().to_string(),
            skip: skip.iter().map(|s| (*s).to_owned()).collect(),
        },
    );
    assert!(r.ok, "{:?}", r.error);
    let mut out: Vec<String> = r
        .data
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            h["file"]
                .as_str()
                .unwrap()
                .strip_prefix(dir.to_str().unwrap())
                .unwrap()
                .trim_start_matches('/')
                .to_owned()
        })
        .collect();
    out.sort();
    out
}

// S-18: finding the value in a folder, never printing it, and not walking where it was told not to.
#[test]
fn scan_dir_skips_the_folders_it_is_told_to_at_any_depth() {
    let d = tempfile::tempdir().unwrap();
    for rel in [
        "a.txt",
        "sandbox/b.txt",
        "deep/sandbox/c.txt",
        "deep/d.txt",
        "node_modules/x/e.js",
    ] {
        let p = d.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("key={VALUE}\n")).unwrap();
    }
    let v = vault();
    assert_eq!(
        files(&v, d.path(), &[]).len(),
        5,
        "without a list it walks everything, as `clio-vault scan` does"
    );
    assert_eq!(
        files(&v, d.path(), &["sandbox", "node_modules"]),
        ["a.txt", "deep/d.txt"]
    );
}
