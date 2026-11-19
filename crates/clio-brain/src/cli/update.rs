//! `clio update [--check]`: everything that is Clio's brought up to date in one step, with no
//! questions: the same steps as the installer's Update (`setup::steps`), without the screens. It
//! never touches config, vault, memory, conversations, skills, services or servers.

use super::home::{Config, Dirs};
use super::setup::fetch::{Pins, Release};
use super::setup::steps::{Action, Step};
use super::setup::text::{Lang, M, say};
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
    // Hidden: the new binary an update started (it replaces nothing again).
    let reexec = a.flag("--reexec");
    if !a.rest()?.is_empty() {
        return Err("usage: clio update [--check]".into());
    }
    let lang = lang(dirs);
    let here = env!("CARGO_PKG_VERSION");
    if check {
        let found = super::mkdir_private(&dirs.data)
            .map_err(super::setup::fetch::Fail::Other)
            .and_then(|()| Release::find(&dirs.data));
        let rel = match found {
            Ok(r) => r,
            Err(e) => {
                // Only why: running the setup again would not help an offline check.
                println!("{}", say(&super::setup::steps::said(lang, &e).0));
                return Ok(1);
            }
        };
        save(dirs, &rel.version);
        if rel.is_newer() {
            println!(
                "{}",
                say(&M::UpdateAvailable.fill(lang, &[here, &rel.version]))
            );
        } else {
            println!("{}", say(&M::UpdateNone.fill(lang, &[here])));
        }
        return Ok(0);
    }
    if super::pending::any_open(dirs) {
        println!("{}", say(M::UpdateChatOpen.get(lang)));
        return Ok(1);
    }
    let mut opts = Opts::here()?;
    opts.update = true;
    opts.reexec = reexec;
    // No questions here: the vault and the model are the wizard's (`clio setup`).
    let steps: Vec<_> = Action::Update
        .steps()
        .into_iter()
        .filter(|s| !matches!(s, Step::Vault | Step::Model))
        .collect();
    let mut ui = Plain {
        lang,
        rows: Action::Update
            .shown()
            .iter()
            .filter(|s| steps.contains(s))
            .map(|s| s.label().get(lang).to_owned())
            .collect(),
        quiet: true,
    };
    let mut r =
        super::setup::run_with(dirs, lang, Sys::from_env(), Pins::from_env(), opts, &mut ui);
    r.all(&steps);
    for c in &r.changes {
        println!("{}", say(c));
    }
    if r.replaced {
        // The rest (Node, Pi, clio-memory, skills) is what the new binary pins: it goes on.
        return super::setup::reexec(&r.opts.exe, &["update", "--reexec"]);
    }
    if r.changes.is_empty() && !r.failed {
        println!("{}", say(&M::UpdateNone.fill(lang, &[here])));
    }
    if let Some(d) = &r.doctor {
        println!(
            "{}",
            say(&format!("{} · {d}", Step::Doctor.label().get(lang)))
        );
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

fn read_cache(d: &Dirs) -> serde_json::Value {
    std::fs::read_to_string(cache(d))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Asks for the latest release at most once a day (the round, before it starts). A failed ask
/// counts too (offline, the round does not wait on it every time); the last release seen stays.
pub fn refresh(d: &Dirs, now: i64) {
    refresh_with(d, now, Release::find);
}

fn refresh_with(
    d: &Dirs,
    now: i64,
    find: impl FnOnce(&std::path::Path) -> super::setup::fetch::FRes<Release>,
) {
    let v = read_cache(d);
    if v["checked_ms"]
        .as_i64()
        .is_some_and(|c| now - c < 24 * 3_600_000)
    {
        return;
    }
    match find(&d.data) {
        Ok(rel) => save(d, &rel.version),
        Err(_) => save(d, v["latest"].as_str().unwrap_or(env!("CARGO_PKG_VERSION"))),
    }
}

/// One line for the round's data when the last release seen is newer than this Clio.
pub fn notice(d: &Dirs) -> Option<String> {
    let v = read_cache(d);
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

    // Offline: the failed ask is recorded, the last release seen stays, and the next round does
    // not ask again within the day.
    #[test]
    fn a_failed_check_is_recorded_and_keeps_the_last_release_seen() {
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
        std::fs::write(cache(&d), r#"{"checked_ms": 0, "latest": "99.0.0"}"#).unwrap();
        let offline =
            |_: &std::path::Path| Err(super::super::setup::fetch::Fail::Net("VERSION".into()));
        refresh_with(&d, super::super::now_ms(), offline);
        let v = read_cache(&d);
        assert_eq!(v["latest"], "99.0.0");
        assert!(v["checked_ms"].as_i64().unwrap() > 0);
    }
}
