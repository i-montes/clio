//! Clio's catalog of MCP servers (`mcp/catalog.json`, shipped in the binary as the skills are):
//! servers that were installed and called for real in Clio's jail at the exact version listed.
//! An entry holds the manifest's own fields with the values that were verified, the tools that
//! only read (`modes`: they start `free`), and what it needs besides (`needs: browser`: Clio's own
//! Chromium, pinned here by revision and SHA-256). `clio mcp add <name>`, the installer and
//! `mcp_propose {catalog}` compose the same manifest from it, checked as any other.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::mcp::Manifest;
use crate::services::{Limits, SecretUse};

pub const CATALOG: &str = include_str!("../../../mcp/catalog.json");
/// Where an entry's `args` and `reads` name Clio's Chromium folder.
const BROWSER: &str = "{browser}";

/// One text in both of the installer's languages.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub en: String,
    pub es: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Secret {
    pub name: String,
    pub env: String,
    pub host: String,
    /// Where the person gets it.
    #[serde(rename = "where")]
    pub where_: Text,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub name: String,
    pub title: Text,
    pub description: Text,
    pub package: String,
    pub version: String,
    /// The tarball's integrity: the registry must give exactly this.
    pub integrity: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub network: Vec<String>,
    #[serde(default)]
    pub reads: Vec<String>,
    #[serde(default)]
    pub writes: Vec<String>,
    pub secrets: Vec<Secret>,
    pub limits: Limits,
    /// The tools that only read: they start `free`.
    #[serde(default)]
    pub modes: Vec<String>,
    /// Exact versions npm must install for packages deep in its tree (npm's `overrides`): what
    /// the package's own ranges now resolve to breaks it.
    #[serde(default)]
    pub overrides: BTreeMap<String, String>,
    /// `browser`: Clio's own Chromium.
    #[serde(default)]
    pub needs: Option<String>,
}

impl Entry {
    pub fn needs_browser(&self) -> bool {
        self.needs.as_deref() == Some("browser")
    }
}

/// One platform's Chromium archive.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub url: String,
    pub sha256: String,
}

/// The Chromium revision the catalog's `@playwright/mcp` asks for, per platform
/// (`linux-x64`, `linux-arm64`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Browser {
    pub revision: String,
    pub files: BTreeMap<String, File>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub browser: Browser,
    pub servers: Vec<Entry>,
}

/// The shipped catalog (its test holds that it reads).
pub fn catalog() -> Catalog {
    serde_json::from_str(CATALOG).unwrap_or(Catalog {
        browser: Browser {
            revision: String::new(),
            files: BTreeMap::new(),
        },
        servers: Vec::new(),
    })
}

pub fn entry(name: &str) -> Option<Entry> {
    catalog().servers.into_iter().find(|e| e.name == name)
}

/// The catalog's names, for a message.
pub fn names() -> String {
    catalog()
        .servers
        .iter()
        .map(|e| e.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Clio's Chromium of the catalog's revision: `<data>/browsers/chromium-<revision>`, beside
/// `<data>/mcp` (`root`). Only Clio's `clio` writes it.
pub fn browser_dir(root: &Path) -> PathBuf {
    root.parent()
        .unwrap_or(root)
        .join("browsers")
        .join(format!("chromium-{}", catalog().browser.revision))
}

/// The manifest `e` stands for, with Clio's browser folder (beside `root`, `<data>/mcp`) in place.
pub fn manifest(e: &Entry, root: &Path) -> Manifest {
    let b = browser_dir(root).display().to_string();
    let fill = |v: &[String]| -> Vec<String> { v.iter().map(|s| s.replace(BROWSER, &b)).collect() };
    Manifest {
        name: e.name.clone(),
        description: e.description.en.clone(),
        package: e.package.clone(),
        version: e.version.clone(),
        args: fill(&e.args),
        env: e.env.clone(),
        network: e.network.clone(),
        secrets: e
            .secrets
            .iter()
            .map(|s| SecretUse {
                name: s.name.clone(),
                env: s.env.clone(),
                host: s.host.clone(),
            })
            .collect(),
        reads: fill(&e.reads),
        writes: fill(&e.writes),
        limits: e.limits,
    }
}

/// The entry whose manifest is exactly `m`: only then do its integrity and modes apply.
pub fn of(m: &Manifest, root: &Path) -> Option<Entry> {
    entry(&m.name).filter(|e| manifest(e, root) == *m)
}

/// `of`, where `<data>/mcp` is not at hand (the approval box): a manifest that reads Clio's
/// browser says where it is; one that does not needs no folder.
pub fn of_shown(m: &Manifest) -> Option<Entry> {
    let root = m
        .reads
        .iter()
        .map(Path::new)
        .find(|r| {
            r.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("chromium-"))
        })
        .and_then(|r| Some(r.parent()?.parent()?.join("mcp")))
        .unwrap_or_default();
    of(m, &root)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // S-70: every entry of the shipped catalog is a manifest the tool host accepts (names, exact
    // versions, hosts, secrets, limits, the any-host rule), with a sha512 integrity, modes that
    // name tools, and a Chromium pinned by SHA-256 for each Linux processor. The network is not
    // reached: what an entry says was verified by installing and calling it in the jail.
    #[test]
    fn the_shipped_catalog_holds_only_manifests_clio_accepts() {
        let c: Catalog = serde_json::from_str(CATALOG).unwrap();
        assert!(!c.servers.is_empty());
        let root = Path::new("/home/someone/.local/share/clio/mcp");
        let mut seen = std::collections::HashSet::new();
        for e in &c.servers {
            assert!(seen.insert(e.name.clone()), "{} twice", e.name);
            let m = manifest(e, root);
            crate::mcp::check(&m).unwrap_or_else(|why| panic!("{}: {why}", e.name));
            assert!(crate::mcp::exact_version(&e.version), "{}", e.name);
            assert!(e.integrity.starts_with("sha512-") && e.integrity.len() > 80);
            assert!(e.modes.iter().all(|t| !t.is_empty()), "{}", e.name);
            for (p, v) in &e.overrides {
                assert!(crate::mcp::exact_version(v), "{}: {p}@{v}", e.name);
            }
            for t in [&e.title, &e.description] {
                assert!(!t.en.is_empty() && !t.es.is_empty(), "{}", e.name);
            }
            for s in &e.secrets {
                assert!(!s.where_.en.is_empty() && !s.where_.es.is_empty());
            }
            // A placeholder left over would reach the server as text.
            assert!(!crate::mcp::compose(&m).contains(BROWSER), "{}", e.name);
            assert_eq!(of(&m, root).map(|x| x.name), Some(e.name.clone()));
            assert_eq!(of_shown(&m).map(|x| x.name), Some(e.name.clone()));
            if e.needs_browser() {
                assert!(m.reads.contains(&browser_dir(root).display().to_string()));
            }
        }
        assert!(c.browser.revision.bytes().all(|b| b.is_ascii_digit()));
        for p in ["linux-x64", "linux-arm64"] {
            let f = &c.browser.files[p];
            assert!(f.url.starts_with("https://") && f.url.contains(&c.browser.revision));
            assert!(f.sha256.len() == 64 && f.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
        }
        // A catalog's modes free the tools it names; every other tool asks unless the server
        // itself says it only reads.
        let tool = |name: &str, read_only: bool| crate::mcp::Tool {
            name: name.into(),
            description: String::new(),
            schema: serde_json::json!({"type": "object"}),
            read_only,
            mode: crate::mcp::Mode::Ask,
            flagged: true,
        };
        let modes: Vec<_> = crate::mcp::first_modes(
            vec![
                tool("snap", false),
                tool("click", false),
                tool("list", true),
            ],
            &["snap".to_owned()],
        )
        .into_iter()
        .map(|t| (t.mode, t.flagged))
        .collect();
        use crate::mcp::Mode::{Ask, Free};
        assert_eq!(modes, [(Free, false), (Ask, false), (Free, false)]);
        assert_eq!(
            browser_dir(root),
            Path::new("/home/someone/.local/share/clio/browsers")
                .join(format!("chromium-{}", c.browser.revision))
        );
    }
}
