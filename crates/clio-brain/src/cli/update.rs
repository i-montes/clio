//! `clio update [--check]`: everything that is Clio's brought up to date in one step, with no
//! questions: the same steps as the installer's Update (`setup::steps`), without the screens. It
//! never touches config, vault, memory, conversations, skills, services or servers.

use super::home::{Config, Dirs};
use super::setup::fetch::{Pins, Release};
use super::setup::steps::Action;
use super::setup::text::{Lang, M};
use super::setup::{Opts, Plain, Sys};
use super::{Args, Res};

/// The language the installer was run in, else English.
fn lang(dirs: &Dirs) -> Lang {
    Config::load(dirs)
        .ok()
        .and_then(|c| c.ui.language)
        .and_then(|l| Lang::from_code(&l))
        .unwrap_or(Lang::En)
}

pub fn run(dirs: &Dirs, args: Vec<String>) -> Res<i32> {
    let mut a = Args::new(args);
    let check = a.flag("--check");
    if !a.rest()?.is_empty() {
        return Err("usage: clio update [--check]".into());
    }
    let lang = lang(dirs);
    let here = env!("CARGO_PKG_VERSION");
    if check {
        let rel = match Release::find(&std::env::temp_dir()) {
            Ok(r) => r,
            Err(e) => {
                let (why, fix) = super::setup::steps::said(lang, &e);
                println!("{why}\n  {fix}");
                return Ok(1);
            }
        };
        save(dirs, &rel.version);
        if rel.is_newer() {
            println!("{}", M::UpdateAvailable.fill(lang, &[here, &rel.version]));
        } else {
            println!("{}", M::UpdateNone.fill(lang, &[here]));
        }
        return Ok(0);
    }
    if super::pending::any_open(dirs) {
        println!("{}", M::UpdateChatOpen.get(lang));
        return Ok(1);
    }
    let mut opts = Opts::here()?;
    opts.update = true;
    let steps = Action::Update.steps();
    let mut ui = Plain {
        lang,
        rows: Action::Update
            .shown()
            .iter()
            .map(|s| s.label().get(lang).to_owned())
            .collect(),
        quiet: true,
    };
    let mut r =
        super::setup::run_with(dirs, lang, Sys::from_env(), Pins::from_env(), opts, &mut ui);
    r.all(&steps);
    for c in &r.changes {
        println!("{c}");
    }
    if r.replaced {
        // The rest (Node, Pi, clio-memory, skills) is what the new binary pins: it goes on.
        return super::setup::reexec(&r.opts.exe, &["update"]);
    }
    if r.changes.is_empty() && !r.failed {
        println!("{}", M::UpdateNone.fill(lang, &[here]));
    }
    if let Some(d) = &r.doctor {
        println!("doctor: {d}");
    }
    let failed = r.failed;
    if !failed {
        super::setup::mark_version(dirs);
        save(dirs, here);
    }
    Ok(i32::from(failed))
}

// ---- what the round tells -----------------------------------------------------------------------

/// The last release seen, `<data>/update.json`.
pub(crate) fn cache(d: &Dirs) -> std::path::PathBuf {
    d.data.join("update.json")
}

fn save(d: &Dirs, latest: &str) {
    if d.data.is_dir() {
        let v = serde_json::json!({"checked_ms": super::now_ms(), "latest": latest});
        let _ = super::write_private(&cache(d), v.to_string().as_bytes());
    }
}

/// Asks for the latest release at most once a day (the round, before it starts).
pub fn refresh(d: &Dirs, now: i64) {
    let checked = std::fs::read_to_string(cache(d))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["checked_ms"].as_i64());
    if checked.is_some_and(|c| now - c < 24 * 3_600_000) {
        return;
    }
    if let Ok(rel) = Release::find(&std::env::temp_dir()) {
        save(d, &rel.version);
    }
}

/// One line for the round's data when the last release seen is newer than this Clio.
pub fn notice(d: &Dirs) -> Option<String> {
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(cache(d)).ok()?).ok()?;
    let latest = v["latest"].as_str()?;
    let here = env!("CARGO_PKG_VERSION");
    use super::setup::fetch::version;
    (version(latest)? > version(here)?).then(|| {
        format!(
            "a newer Clio exists: {here} → {}; the person updates with `clio update` (Clio never updates by herself)",
            super::tui::sanitize::line(latest)
        )
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_round_hears_of_a_newer_release_only_when_there_is_one() {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let d = Dirs {
            config: t.path().join("config"),
            runs: data.join("runs"),
            data,
            runtime: None,
            home: t.path().into(),
        };
        assert_eq!(notice(&d), None);
        save(&d, env!("CARGO_PKG_VERSION"));
        assert_eq!(notice(&d), None);
        save(&d, "99.0.0");
        let n = notice(&d).unwrap();
        assert!(n.contains("99.0.0") && n.contains("clio update"), "{n}");
        // Checked a moment ago: no new question within the day.
        refresh(&d, super::super::now_ms());
        assert!(notice(&d).unwrap().contains("99.0.0"));
    }
}
