//! `clio apply` and `clio undo`: the only way anything leaves the sandbox. A person confirms, the
//! paths are checked one by one, and whatever is overwritten is saved in the run first.
//!
//! Hard refusals (`--force` never overrides them): a path that is not plain and relative, a
//! protected path (`.git`, `.claude`, ...), a folder the copy leaves out (`node_modules`, ...), a
//! secret-looking name, a symbolic link anywhere on the way. Conflicts (`--force` overrides them):
//! the original changed since the copy, or a file the diff cannot show in full (binary or too big:
//! open it by hand first). What `plan` reads is what `commit` writes, byte for byte.
//!
//! The TUI calls the same functions (`plan`, `commit`, `undo_run`): the checks live only here.

use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use clio_brain::agent::PROTECTED_PATHS;
use clio_vault::has_mask;
use serde::{Deserialize, Serialize};

use super::diff::fully_shown;
use super::home::Dirs;
use super::runs::{self, Run, read_json, write_json};
use super::tui::sanitize;
use super::ui::Style;
use super::work::{self, Change, Kind, SKIP_DIRS, is_secret_name, plain_rel};
use super::{Res, io_err, now_ms, sha_hex};

#[derive(Serialize, Deserialize)]
struct Entry {
    path: String,
    /// "modified", "created" or "deleted".
    action: String,
    /// Hash of the original before (None if it did not exist) and of what was written.
    before: Option<String>,
    after: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct Applied {
    at_ms: i64,
    entries: Vec<Entry>,
    /// Folders `apply` created, outermost first.
    created_dirs: Vec<String>,
}

/// Why a path may never be written, or `None` if it is fine to consider.
fn hard_refusal(rel: &str) -> Option<String> {
    if !plain_rel(rel) {
        return Some("no es una ruta relativa simple".into());
    }
    let comps: Vec<&str> = rel.split('/').collect();
    // The names are lowercase: compared without case (a case-insensitive disk loads `.Git` too).
    // The root-level folders only at the root (`src/skills/` is a normal folder); the dot ones at
    // any depth.
    let lower: Vec<String> = comps.iter().map(|c| c.to_ascii_lowercase()).collect();
    if PROTECTED_PATHS.contains(&lower[0].as_str())
        || lower
            .iter()
            .any(|c| PROTECTED_PATHS.contains(&c.as_str()) && c.starts_with('.'))
    {
        return Some("ruta protegida".into());
    }
    if comps.iter().any(|c| SKIP_DIRS.contains(c)) {
        return Some("carpeta que la copia deja fuera (se genera sola)".into());
    }
    if comps.iter().any(|c| is_secret_name(c)) {
        return Some("parece un secreto".into());
    }
    None
}

/// Walks `rel` under `ws` and refuses if any existing step is a symbolic link or leaves `ws`.
pub(super) fn symlink_check(ws: &Path, rel: &str) -> Result<(), String> {
    let mut cur = ws.to_path_buf();
    for c in rel.split('/') {
        cur.push(c);
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err("hay un enlace simbólico en el camino".into());
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(e.to_string()),
        }
    }
    let mut existing = cur.as_path();
    while !existing.exists() {
        existing = existing.parent().unwrap_or(ws);
    }
    match existing.canonicalize() {
        Ok(c) if c.starts_with(ws) => Ok(()),
        _ => Err("sale de la carpeta del proyecto".into()),
    }
}

fn hash_of(p: &Path) -> Option<String> {
    std::fs::read(p).ok().map(|b| sha_hex(&b))
}

fn workspace(meta_ws: &str) -> Res<PathBuf> {
    Path::new(meta_ws)
        .canonicalize()
        .map_err(|e| format!("la carpeta original {meta_ws}: {e}"))
}

pub(super) fn confirm(q: &str) -> bool {
    // What was typed or pasted before the question (a pasted block ending in a newline, a key held
    // down) must not answer it: only what comes after the question counts.
    if std::io::stdin().is_terminal() {
        let _ = rustix::termios::tcflush(std::io::stdin(), rustix::termios::QueueSelector::IFlush);
    }
    print!("{q} [s/N] ");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    std::io::stdin().lock().read_line(&mut s).is_ok()
        && matches!(
            s.trim().to_lowercase().as_str(),
            "y" | "yes" | "s" | "si" | "sí"
        )
}

fn action(k: Kind) -> &'static str {
    match k {
        Kind::Modified => "modified",
        Kind::Created => "created",
        Kind::Deleted => "deleted",
    }
}

/// A run whose every path passed the checks: ready to be written.
pub struct Plan {
    pub run: Run,
    pub ws: PathBuf,
    pub manifest: BTreeMap<String, String>,
    pub changes: Vec<Change>,
    /// Hash of each file to write, as `plan` read it: `commit` writes only if it still matches.
    reviewed: BTreeMap<String, String>,
}

/// The message `plan` gives when there is nothing to apply.
pub const NO_CHANGES: &str = "no tiene cambios; no hay nada que aplicar";

/// All the checks of `apply`, before anything is written. An error means nothing was touched.
pub fn plan(dirs: &Dirs, id: &str, force: bool) -> Res<Plan> {
    let run = runs::resolve(dirs, id)?;
    if run.path.join("applied.json").exists() {
        return Err(format!(
            "el run {} ya está aplicado; primero `clio undo {}`",
            run.id, run.id
        ));
    }
    let (meta, manifest) = (run.meta()?, run.manifest()?);
    let ws = workspace(&meta.workspace)?;
    let ch = work::changes(&run.sandbox(), &manifest)?;
    if ch.is_empty() {
        return Err(format!("el run {} {NO_CHANGES}", run.id));
    }

    // Preflight everything before touching anything.
    let (mut refused, mut conflicts, mut unshown) = (Vec::new(), Vec::new(), Vec::new());
    let mut reviewed = BTreeMap::new();
    for c in &ch {
        let rel = sanitize::line(&c.rel);
        if let Some(why) = hard_refusal(&c.rel) {
            refused.push(format!("{rel}: {why}"));
            continue;
        }
        if let Err(why) = symlink_check(&ws, &c.rel) {
            refused.push(format!("{rel}: {why}"));
            continue;
        }
        let orig = std::fs::read(ws.join(&c.rel)).ok();
        let now = orig.as_deref().map(sha_hex);
        let new = if c.kind == Kind::Deleted {
            None
        } else {
            match work::read_regular(&run.sandbox().join(&c.rel)) {
                Ok(b) => Some(b),
                Err(e) => {
                    refused.push(format!("{rel}: no se pudo leer la copia: {e}"));
                    continue;
                }
            }
        };
        // The agent was shown marks where values were hidden: a file that carries one would put
        // the mark over the real value in the original (S-18).
        if new
            .as_deref()
            .is_some_and(|b| has_mask(&String::from_utf8_lossy(b)))
        {
            refused.push(format!(
                "{rel}: trae un valor tachado (el agente no vio el valor real); no se aplica"
            ));
            continue;
        }
        if let Some(b) = &new {
            reviewed.insert(c.rel.clone(), sha_hex(b));
        }
        if !fully_shown(orig.as_deref(), new.as_deref()) {
            unshown.push(format!(
                "{rel}: binario o demasiado grande para mostrarlo línea a línea"
            ));
        }
        match (c.kind, &now) {
            (Kind::Created, Some(_)) => conflicts.push(format!("{rel}: ya existe en el original")),
            (Kind::Modified | Kind::Deleted, None) => {
                conflicts.push(format!("{rel}: ya no existe en el original"));
            }
            (Kind::Modified | Kind::Deleted, Some(h)) if manifest.get(&c.rel) != Some(h) => {
                conflicts.push(format!("{rel}: cambió en el original desde la copia"));
            }
            _ => {}
        }
    }
    if !refused.is_empty() {
        return Err(format!(
            "rechazado, no se aplicó nada:\n  {}",
            refused.join("\n  ")
        ));
    }
    if !conflicts.is_empty() && !force {
        return Err(format!(
            "el original cambió, no se aplicó nada (--force lo pisa, con respaldo):\n  {}",
            conflicts.join("\n  ")
        ));
    }
    if !unshown.is_empty() && !force {
        return Err(format!(
            "no se pueden mostrar completos, no se aplicó nada. Ábrelos a mano en {} y, si están bien, repite con --force:\n  {}",
            sanitize::line(&run.sandbox().display().to_string()),
            unshown.join("\n  ")
        ));
    }
    Ok(Plan {
        run,
        ws,
        manifest,
        changes: ch,
        reviewed,
    })
}

/// Writes a checked plan into the original, saving what it overwrites. Returns how many files.
pub fn commit(p: &Plan) -> Res<usize> {
    // Read every file once and check it is what `plan` saw, before writing anything: a copy that
    // changed (or became a link) in between is refused, and what is written is what was checked.
    let mut contents = BTreeMap::new();
    for c in p.changes.iter().filter(|c| c.kind != Kind::Deleted) {
        let rel = sanitize::line(&c.rel);
        let bytes = work::read_regular(&p.run.sandbox().join(&c.rel))
            .map_err(|e| format!("{rel}: no se pudo leer la copia: {e}"))?;
        if p.reviewed.get(&c.rel) != Some(&sha_hex(&bytes)) {
            return Err(format!(
                "{rel}: la copia cambió después de revisarla; no se aplicó nada, revisa de nuevo"
            ));
        }
        contents.insert(c.rel.clone(), bytes);
    }
    let mut done = Applied {
        at_ms: now_ms(),
        ..Applied::default()
    };
    let result = write_all(&p.ws, &p.run, &p.changes, &contents, &mut done);
    // Record what was done even if it stopped halfway, so `undo` can put it back.
    write_json(&p.run.path.join("applied.json"), &done)?;
    result?;
    Ok(done.entries.len())
}

pub fn apply(dirs: &Dirs, id: &str, force: bool, yes: bool) -> Res<()> {
    let st = Style::stdout();
    let p = match plan(dirs, id, force) {
        Err(e) if e.ends_with(NO_CHANGES) => {
            println!("{e}");
            return Ok(());
        }
        r => r?,
    };
    runs::print_diff(st, &p.ws, &p.run, &p.manifest, &p.changes);
    if !yes
        && !confirm(&format!(
            "¿Aplicar {} archivo(s) en {}?",
            p.changes.len(),
            sanitize::line(&p.ws.display().to_string())
        ))
    {
        println!("no se aplicó");
        return Ok(());
    }
    let n = commit(&p)?;
    println!(
        "{} {n} archivo(s) aplicados; para deshacer: clio undo {}",
        st.green(if st.unicode { "✓" } else { "ok:" }),
        p.run.id
    );
    Ok(())
}

fn write_all(
    ws: &Path,
    run: &Run,
    ch: &[Change],
    contents: &BTreeMap<String, Vec<u8>>,
    done: &mut Applied,
) -> Res<()> {
    for c in ch {
        let target = ws.join(&c.rel);
        let before = hash_of(&target);
        if before.is_some() {
            let b = run.path.join("backup").join(&c.rel);
            if let Some(p) = b.parent() {
                super::mkdir_private(p)?;
            }
            let bytes = std::fs::read(&target).map_err(|e| io_err(&target, &e))?;
            super::write_private(&b, &bytes)?;
        }
        let after = match c.kind {
            Kind::Deleted => {
                std::fs::remove_file(&target).map_err(|e| io_err(&target, &e))?;
                None
            }
            Kind::Modified | Kind::Created => {
                let bytes = contents
                    .get(&c.rel)
                    .ok_or_else(|| format!("{}: falta lo revisado", sanitize::line(&c.rel)))?;
                if let Some(parent) = target.parent() {
                    make_dirs(ws, parent, done)?;
                }
                std::fs::write(&target, bytes).map_err(|e| io_err(&target, &e))?;
                Some(sha_hex(bytes))
            }
        };
        done.entries.push(Entry {
            path: c.rel.clone(),
            action: action(c.kind).into(),
            before,
            after,
        });
    }
    Ok(())
}

/// Creates missing folders under `ws`, noting each one so `undo` can remove it.
fn make_dirs(ws: &Path, dir: &Path, done: &mut Applied) -> Res<()> {
    let mut missing = Vec::new();
    let mut cur = dir;
    while !cur.exists() {
        missing.push(cur.to_path_buf());
        cur = cur.parent().ok_or("no parent folder")?;
    }
    for d in missing.iter().rev() {
        std::fs::create_dir(d).map_err(|e| io_err(d, &e))?;
        done.created_dirs.push(
            d.strip_prefix(ws)
                .map_err(|e| e.to_string())?
                .display()
                .to_string(),
        );
    }
    Ok(())
}

/// Puts the originals back; returns how many files per action (`modified`, `created`, `deleted`).
pub fn undo_run(dirs: &Dirs, id: &str, force: bool) -> Res<BTreeMap<String, usize>> {
    let run = runs::resolve(dirs, id)?;
    let ap = run.path.join("applied.json");
    if !ap.exists() {
        return Err(format!(
            "el run {} no tiene nada aplicado (o ya se deshizo)",
            run.id
        ));
    }
    let done: Applied = read_json(&ap)?;
    let ws = workspace(&run.meta()?.workspace)?;
    let mut problems = Vec::new();
    for e in &done.entries {
        let rel = sanitize::line(&e.path);
        if let Some(why) = hard_refusal(&e.path).or_else(|| symlink_check(&ws, &e.path).err()) {
            problems.push(format!("{rel}: {why}"));
        } else if hash_of(&ws.join(&e.path)) != e.after && !force {
            problems.push(format!("{rel}: cambió después de `apply`"));
        }
    }
    if !problems.is_empty() {
        return Err(format!(
            "no se deshizo (--force pisa lo que cambió después de apply):\n  {}",
            problems.join("\n  ")
        ));
    }
    for e in done.entries.iter().rev() {
        let target = ws.join(&e.path);
        match &e.before {
            Some(_) => {
                let bytes = std::fs::read(run.path.join("backup").join(&e.path))
                    .map_err(|x| format!("respaldo de {}: {x}", sanitize::line(&e.path)))?;
                std::fs::write(&target, bytes).map_err(|x| io_err(&target, &x))?;
            }
            None => {
                if target.exists() {
                    std::fs::remove_file(&target).map_err(|x| io_err(&target, &x))?;
                }
            }
        }
    }
    for d in done.created_dirs.iter().rev() {
        let _ = std::fs::remove_dir(ws.join(d)); // only succeeds when empty
    }
    std::fs::rename(&ap, run.path.join("undone.json")).map_err(|e| io_err(&ap, &e))?;
    Ok(done.entries.iter().fold(BTreeMap::new(), |mut m, e| {
        *m.entry(e.action.clone()).or_default() += 1;
        m
    }))
}

pub fn undo(dirs: &Dirs, id: &str, force: bool) -> Res<()> {
    let st = Style::stdout();
    let n = undo_run(dirs, id, force)?;
    let total: usize = n.values().sum();
    let word = |k: &str, w: &str| n.get(k).map(|c| format!("{c} {w}"));
    let parts: Vec<String> = [
        word("modified", "restaurados"),
        word("created", "eliminados"),
        word("deleted", "recuperados"),
    ]
    .into_iter()
    .flatten()
    .collect();
    println!(
        "{} deshecho: {total} archivo(s) ({})",
        st.green(if st.unicode { "✓" } else { "ok:" }),
        parts.join(", ")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_paths_are_refused_by_any_case_and_depth() {
        for r in [
            ".git/hooks/pre-commit",
            ".Git/config",
            "sub/.git/config",
            ".agents/skills/x/SKILL.md",
            ".AGENTS/x",
            "pkg/.claude/settings.json",
            "a/.pi/x",
            "skills/x.md",
            "Extensions/x.js",
            "node_modules/x/index.js",
            "a/__pycache__/m.pyc",
            ".venv/lib/sitecustomize.py",
            "target/debug/build.rs",
            // Folders a tool loads and RUNS when the project is opened or built (S-16).
            ".vscode/tasks.json",
            ".VSCode/settings.json",
            "pkg/.vscode/tasks.json",
            ".husky/pre-commit",
            ".githooks/pre-push",
            ".cargo/config.toml",
            "crates/x/.cargo/config.toml",
            ".devcontainer/devcontainer.json",
        ] {
            assert!(hard_refusal(r).is_some(), "{r}");
        }
    }

    #[test]
    fn normal_paths_that_share_a_word_pass() {
        for r in [
            "src/skills/mod.rs",
            "docs/extensions/guide.md",
            "docs/vscode/guide.md",
            "src/cargo/mod.rs",
            "src/husky.rs",
            "src/main.rs",
            "README.md",
            ".github/workflows/ci.yml",
        ] {
            assert!(hard_refusal(r).is_none(), "{r}");
        }
    }
}
