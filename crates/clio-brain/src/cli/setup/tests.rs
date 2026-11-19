//! The installer's screens (each fits 80x24, in both languages; secrets masked), its downloads (a
//! checksum mismatch or a cut download installs nothing) and its removals (Remove-keeping-data
//! leaves exactly the data). The whole command, with fakes, is in `tests/setup.rs`.
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use clio_vault::SecretValue;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::fetch::{self, Archive, Fail, MemoryPin, Release};
use super::screen::{self, Page, View};
use super::steps::{Action, Step};
use super::text::{Lang, M};
use super::{Answer, Ask, Opts, St, Sys, Ui};
use crate::cli::home::Dirs;
use crate::cli::tui::theme::Theme;

// ---- fixtures ----------------------------------------------------------------------------------

fn dirs(home: &Path) -> Dirs {
    let data = home.join(".local/share/clio");
    Dirs {
        config: home.join(".config/clio"),
        runs: data.join("runs"),
        data,
        runtime: Some(home.join("run/clio")),
        home: home.into(),
    }
}

fn write(p: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

fn script(p: &Path, body: &str) {
    write(p, format!("#!/bin/sh\n{body}\n").as_bytes());
    crate::cli::set_mode(p, 0o755).unwrap();
}

fn sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Every file under `root`, by path, with its bytes.
fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut todo = vec![root.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() && !p.is_symlink() {
                todo.push(p);
            } else {
                out.insert(
                    p.strip_prefix(root).unwrap().into(),
                    std::fs::read(&p).unwrap_or_default(),
                );
            }
        }
    }
    out
}

/// Answers questions from a list and keeps what was shown.
#[derive(Default)]
struct FakeUi {
    rows: BTreeMap<usize, St>,
    answers: VecDeque<Answer>,
    asked: Vec<String>,
}

impl Ui for FakeUi {
    fn set(&mut self, i: usize, st: St) {
        self.rows.insert(i, st);
    }
    fn ask(&mut self, q: Ask) -> Option<Answer> {
        let (Ask::Choice { title, .. } | Ask::Secret { title, .. } | Ask::Text { title, .. }) = q;
        self.asked.push(title);
        self.answers.pop_front()
    }
    fn outside(&mut self, f: &mut dyn FnMut()) {
        f();
    }
    fn busy(&mut self, f: &mut (dyn FnMut() + Send)) {
        f();
    }
}

fn fake_sys(dir: &Path) -> Sys {
    Sys {
        dirs: vec![dir.into()],
        linger: dir.join("linger"),
        root: false,
        user: Some("someone".into()),
        linux: true,
    }
}

// ---- the screens -------------------------------------------------------------------------------

const PASS: &str = "correct-horse-battery";

/// Every state of the gallery, in `lang`: (name, view).
fn states(lang: Lang, home: &Path) -> Vec<(&'static str, View)> {
    let t = |m: M| m.get(lang).to_owned();
    let empty = home.join("empty");
    let installed = home.join("installed");
    write(&dirs(&installed).config_file(), b"{}");
    write(&dirs(&installed).data.join("version"), b"0.2.0");
    let sys = fake_sys(&home.join("sys"));
    let view = |page: Page| {
        let mut v = View::new(Some(lang));
        v.page = page;
        v
    };
    let doing = |rows: Vec<(Step, St)>| {
        let mut v = view(Page::Doing);
        v.title = t(M::DoingTitle);
        v.rows = Action::Install
            .shown()
            .into_iter()
            .map(|s| {
                let st = rows
                    .iter()
                    .find(|(r, _)| *r == s)
                    .map_or(St::Waiting, |(_, st)| st.clone());
                (t(s.label()), st)
            })
            .collect();
        v
    };
    let mut done = view(screen::done_page(
        &dirs(&installed),
        lang,
        Action::Install,
        false,
        Some(M::LeftModel),
    ));
    done.rows = vec![(
        t(M::StSystem),
        St::Warn(M::NoteNoSudo.fill(lang, &["bubblewrap, loginctl enable-linger"])),
    )];
    let mut language = View::new(None);
    language.page = Page::Language { sel: 0 };
    vec![
        ("language", language),
        (
            "found-nothing",
            view(screen::found_page(&dirs(&empty), &sys, lang)),
        ),
        (
            "found-existing",
            view(screen::found_page(&dirs(&installed), &sys, lang)),
        ),
        (
            "plan",
            view(Page::Plan {
                title: t(M::PlanTitle),
                items: Action::Install
                    .shown()
                    .iter()
                    .map(|s| t(s.label()))
                    .collect(),
            }),
        ),
        (
            "progress",
            doing(vec![
                (Step::Folders, St::Done(t(M::NoteCreated))),
                (
                    Step::System,
                    St::Warn(M::NoteNoSudo.fill(lang, &["bubblewrap, loginctl enable-linger"])),
                ),
                (Step::Node, St::Working(Some((12_400_000, 30_000_000)))),
            ]),
        ),
        (
            "passphrase",
            view(Page::Secret {
                title: t(M::VaultNewTitle),
                lines: vec![t(M::VaultNewHint)],
                labels: vec![t(M::FieldPass), t(M::FieldAgain)],
                values: vec![
                    SecretValue::new(PASS.into()),
                    SecretValue::new("corr".into()),
                ],
                focus: 1,
                error: Some(t(M::ErrMismatch)),
            }),
        ),
        (
            "model",
            view(Page::Choice {
                title: t(M::ModelTitle),
                lines: vec![t(M::ModelHint)],
                options: vec![
                    t(M::ModelClaude),
                    t(M::ModelApi),
                    t(M::ModelOpencode),
                    t(M::ModelLater),
                ],
                sel: 0,
            }),
        ),
        (
            "failed-step",
            doing(vec![
                (Step::Folders, St::Done(t(M::NoteCreated))),
                (Step::System, St::Done(t(M::NoteDone))),
                (
                    Step::Node,
                    St::Failed(
                        M::FailSum.fill(lang, &["node-v22.23.5-linux-arm64.tar.gz"]),
                        t(M::FixSum),
                    ),
                ),
                (Step::Pi, St::Working(None)),
            ]),
        ),
        ("done", done),
    ]
}

fn render(v: &View, t: Theme) -> String {
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    term.draw(|f| screen::draw(f, v, t)).unwrap();
    let buf = term.backend().buffer();
    let mut out = String::new();
    for y in 0..24 {
        let mut skip = 0;
        for x in 0..80 {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let s = buf[(x, y)].symbol();
            skip = crate::cli::tui::text::width(s).saturating_sub(1);
            out.push_str(s);
        }
        out.push('\n');
    }
    out
}

#[test]
fn every_screen_fits_80x24_in_both_languages_and_secrets_are_masked() {
    let home = tempfile::tempdir().unwrap();
    for lang in [Lang::Es, Lang::En] {
        for unicode in [true, false] {
            let t = Theme::decide(true, unicode, "truecolor");
            for (name, v) in states(lang, home.path()) {
                let body = screen::lines(&v, t, 80);
                assert!(body.len() <= 23, "{name} {lang:?}: {} lines", body.len());
                for l in &body {
                    assert!(
                        l.width() <= 80,
                        "{name} {lang:?}: a line is {} wide",
                        l.width()
                    );
                }
                let s = render(&v, t);
                assert!(
                    !s.contains(PASS) && !s.contains("corr "),
                    "{name}: a secret shows\n{s}"
                );
                // The keys are the last line, always in view.
                assert!(
                    s.lines().nth(23).is_some_and(|k| !k.trim().is_empty()),
                    "{name}\n{s}"
                );
                if name == "passphrase" {
                    let dots = if unicode { "•" } else { "*" };
                    assert!(s.contains(&dots.repeat(PASS.chars().count())), "{s}");
                }
                if name == "progress" {
                    assert!(s.contains("12/30 MB"), "the download bar: {s}");
                }
            }
        }
    }
}

#[test]
fn the_screens_speak_the_chosen_language() {
    let home = tempfile::tempdir().unwrap();
    let t = Theme::decide(false, true, "");
    let es: Vec<String> = states(Lang::Es, home.path())
        .iter()
        .map(|(_, v)| render(v, t))
        .collect();
    let en: Vec<String> = states(Lang::En, home.path())
        .iter()
        .map(|(_, v)| render(v, t))
        .collect();
    // The language screen is the same in both: it asks in both.
    assert!(
        es[0].contains("Idioma · Language")
            && es[0].contains("Español")
            && es[0].contains("English")
    );
    for (e, n) in es.iter().zip(&en).skip(1) {
        assert_ne!(e, n);
    }
    assert!(
        es[3].contains(M::PlanTitle.get(Lang::Es)) && en[3].contains(M::PlanTitle.get(Lang::En))
    );
}

/// `cargo test -p clio-brain --bin clio setup_gallery -- --ignored --nocapture`: every state of the
/// wizard, in both languages, as an 80x24 terminal shows it.
#[test]
#[ignore = "prints the gallery"]
fn setup_gallery() {
    let home = tempfile::tempdir().unwrap();
    let t = Theme::decide(false, true, "");
    for lang in [Lang::Es, Lang::En] {
        for (name, v) in states(lang, home.path()) {
            println!("── {name} · {} {}", lang.code(), "─".repeat(60));
            print!("{}", render(&v, t));
        }
    }
}

// ---- the downloads -----------------------------------------------------------------------------

/// Serves one request over HTTP: the headers say `full` bytes, only `sent` of them come, and the
/// connection closes (a cut download).
fn cut_server(full: usize, sent: usize) -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = l.accept() {
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Length: {full}\r\nConnection: close\r\n\r\n"
            );
            let _ = s.write_all(&vec![b'x'; sent]);
        }
    });
    format!("http://127.0.0.1:{port}/file.bin")
}

#[test]
fn a_cut_download_or_a_wrong_checksum_keeps_nothing() {
    let t = tempfile::tempdir().unwrap();
    let to = t.path().join("got");
    let whole = sha(&vec![b'x'; 1000]);
    let e = fetch::get_verified(&cut_server(1000, 300), &to, &whole, &mut |_| {}).unwrap_err();
    // Without curl or wget the download cannot start: nothing either.
    assert!(matches!(e, Fail::Net(_) | Fail::Other(_)), "{e:?}");
    assert!(!to.exists());
    let src = t.path().join("src");
    write(&src, b"what was published");
    let e = fetch::get_verified(
        &src.display().to_string(),
        &to,
        &sha(b"something else"),
        &mut |_| {},
    )
    .unwrap_err();
    assert_eq!(e, Fail::Sum("src".into()));
    assert!(!to.exists());
    fetch::get_verified(
        &src.display().to_string(),
        &to,
        &sha(b"what was published"),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(std::fs::read(&to).unwrap(), b"what was published");
}

#[test]
fn sums_are_read_strictly() {
    let a = "a".repeat(64);
    let sums = format!("{a}  clio-x\n{a} *clio-y\nnot-a-sum  clio-z\n");
    assert_eq!(fetch::sum_in(&sums, "clio-x"), Some(a.clone()));
    assert_eq!(fetch::sum_in(&sums, "clio-y"), Some(a));
    assert_eq!(fetch::sum_in(&sums, "clio-z"), None);
    assert_eq!(fetch::sum_in(&sums, "clio"), None);
}

/// A release folder with `clio-vault` and `clio` for this system; `wrong`: the file whose bytes do
/// not match `SHA256SUMS`.
fn release(dir: &Path, version: &str, wrong: Option<&str>) -> Release {
    let target = fetch::target().unwrap();
    let mut sums = String::new();
    for name in fetch::BINARIES {
        let body = format!("#!/bin/sh\necho {name} {version}\n");
        write(&dir.join(format!("{name}-{target}")), body.as_bytes());
        let listed = if wrong == Some(name) {
            sha(b"other")
        } else {
            sha(body.as_bytes())
        };
        sums.push_str(&format!("{listed}  {name}-{target}\n"));
    }
    write(&dir.join("SHA256SUMS"), sums.as_bytes());
    write(&dir.join("VERSION"), format!("{version}\n").as_bytes());
    Release {
        base: dir.display().to_string(),
        version: version.into(),
    }
}

#[test]
fn both_binaries_are_verified_before_either_is_replaced() {
    if fetch::target().is_none() {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let bin = t.path().join("bin");
    write(&bin.join("clio"), b"old clio");
    write(&bin.join("clio-vault"), b"old vault");
    let before = tree(&bin);
    // `clio-vault` is right, `clio` is not: neither is replaced, and nothing is left beside them.
    let rel = release(&t.path().join("r1"), "9.0.0", Some("clio"));
    let e = fetch::install_binaries(&rel, &bin, &mut |_, _| {}).unwrap_err();
    assert!(
        matches!(e, Fail::Sum(ref s) if s.starts_with("clio-")),
        "{e:?}"
    );
    assert_eq!(tree(&bin), before);
    // A release with a file missing: the same.
    let rel = release(&t.path().join("r2"), "9.0.0", None);
    std::fs::remove_file(
        t.path()
            .join("r2")
            .join(format!("clio-{}", fetch::target().unwrap())),
    )
    .unwrap();
    assert!(fetch::install_binaries(&rel, &bin, &mut |_, _| {}).is_err());
    assert_eq!(tree(&bin), before);
    // Both right: both replaced, executable; again, nothing changes.
    let rel = release(&t.path().join("r3"), "9.0.0", None);
    assert!(fetch::install_binaries(&rel, &bin, &mut |_, _| {}).unwrap());
    let after = tree(&bin);
    assert_eq!(after.len(), 2, "{:?}", after.keys());
    assert!(String::from_utf8_lossy(&after[Path::new("clio")]).contains("echo clio 9.0.0"));
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(bin.join("clio"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
    assert!(!fetch::install_binaries(&rel, &bin, &mut |_, _| {}).unwrap());
}

/// A Node archive as nodejs.org publishes it: one top folder with `bin/node`.
fn node_tarball(dir: &Path, version: &str) -> Archive {
    let top = dir.join(format!("node-v{version}"));
    script(&top.join("bin/node"), &format!("echo v{version}"));
    let file = dir.join(format!("node-v{version}.tar.gz"));
    let ok = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&file)
        .arg("-C")
        .arg(dir)
        .arg(format!("node-v{version}"))
        .status()
        .unwrap()
        .success();
    assert!(ok);
    Archive {
        url: file.display().to_string(),
        sha256: sha(&std::fs::read(&file).unwrap()),
        version: version.into(),
    }
}

#[test]
fn the_private_node_is_verified_unpacked_and_swapped_in_once() {
    let t = tempfile::tempdir().unwrap();
    let data = t.path().join("data");
    let mut a = node_tarball(t.path(), "22.23.5");
    let good = a.sha256.clone();
    a.sha256 = sha(b"not it");
    assert!(matches!(
        fetch::install_node(&data, &a, &mut |_| {}),
        Err(Fail::Sum(_))
    ));
    assert!(!fetch::node_dir(&data).exists());
    assert_eq!(
        tree(&data).len(),
        0,
        "nothing left: {:?}",
        tree(&data).keys()
    );
    a.sha256 = good;
    assert_eq!(
        fetch::install_node(&data, &a, &mut |_| {}).unwrap(),
        Some(String::new())
    );
    assert!(fetch::node_dir(&data).join("bin/node").is_file());
    assert_eq!(fetch::node_installed(&data).as_deref(), Some("22.23.5"));
    assert_eq!(fetch::install_node(&data, &a, &mut |_| {}).unwrap(), None);
    // A newer pin replaces it; a broken one leaves the working one.
    let b = node_tarball(t.path(), "22.30.0");
    assert_eq!(
        fetch::install_node(&data, &b, &mut |_| {})
            .unwrap()
            .as_deref(),
        Some("22.23.5")
    );
    let mut c = node_tarball(t.path(), "22.31.0");
    c.url = cut_server(10_000, 10);
    assert!(fetch::install_node(&data, &c, &mut |_| {}).is_err());
    assert_eq!(fetch::node_installed(&data).as_deref(), Some("22.30.0"));
    assert!(!data.join(".node.new").exists() && !data.join(".node.download").exists());
}

/// A clio-memory release folder; returns its pin.
fn memory_release(dir: &Path, version: &str) -> MemoryPin {
    let asset = "clio-memory-test".to_owned();
    let body = format!("#!/bin/sh\necho clio-memory {version}\n");
    write(&dir.join(&asset), body.as_bytes());
    let sums = format!("{}  {asset}\n", sha(body.as_bytes()));
    write(&dir.join("SHA256SUMS"), sums.as_bytes());
    MemoryPin {
        version: version.into(),
        base: dir.display().to_string(),
        sums_sha256: sha(sums.as_bytes()),
        asset,
    }
}

#[test]
fn clio_memory_is_checked_against_its_pinned_sums() {
    let t = tempfile::tempdir().unwrap();
    let bin = t.path().join("bin");
    let mut m = memory_release(&t.path().join("rel"), "0.13.0");
    let good = m.sums_sha256.clone();
    // SHA256SUMS is not the one pinned: nothing.
    m.sums_sha256 = sha(b"other");
    assert!(matches!(
        fetch::install_memory(&bin, &m, &mut |_| {}),
        Err(Fail::Sum(_))
    ));
    assert_eq!(tree(&bin).len(), 0);
    // The binary is not what SHA256SUMS lists: nothing.
    m.sums_sha256 = good;
    let real = std::fs::read(t.path().join("rel/clio-memory-test")).unwrap();
    write(
        &t.path().join("rel/clio-memory-test"),
        b"#!/bin/sh\necho evil\n",
    );
    assert!(matches!(
        fetch::install_memory(&bin, &m, &mut |_| {}),
        Err(Fail::Sum(_))
    ));
    assert_eq!(tree(&bin).len(), 0);
    write(&t.path().join("rel/clio-memory-test"), &real);
    assert_eq!(
        fetch::install_memory(&bin, &m, &mut |_| {}).unwrap(),
        Some(String::new())
    );
    assert_eq!(fetch::memory_installed(&bin).as_deref(), Some("0.13.0"));
    assert_eq!(fetch::install_memory(&bin, &m, &mut |_| {}).unwrap(), None);
    // One the person updated on their own is newer: it stays.
    script(&bin.join("clio-memory"), "echo clio-memory 0.14.0");
    assert_eq!(fetch::install_memory(&bin, &m, &mut |_| {}).unwrap(), None);
    assert_eq!(fetch::memory_installed(&bin).as_deref(), Some("0.14.0"));
}

#[test]
fn the_model_files_are_kept_when_right_and_replaced_only_when_verified() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src/model.onnx");
    write(&src, b"weights");
    let m = fetch::ModelPin {
        dir: ".cache/clio-models/fake".into(),
        files: vec![fetch::ModelFile {
            url: src.display().to_string(),
            name: "model.onnx".into(),
            sha256: sha(b"weights"),
            size: 7,
        }],
        runtime_dir: ".cache/clio-models/onnxruntime".into(),
        runtime: vec![],
    };
    let home = t.path().join("home");
    assert!(fetch::install_model(&home, &m, &mut |_| {}).unwrap());
    assert!(!fetch::install_model(&home, &m, &mut |_| {}).unwrap());
    // A file changed on disk is fetched again; a source that changed does not replace it.
    let file = home.join(".cache/clio-models/fake/model.onnx");
    write(&file, b"tampered");
    write(&src, b"tampered upstream");
    assert!(matches!(
        fetch::install_model(&home, &m, &mut |_| {}),
        Err(Fail::Sum(_))
    ));
    assert_eq!(std::fs::read(&file).unwrap(), b"tampered");
    write(&src, b"weights");
    assert!(fetch::install_model(&home, &m, &mut |_| {}).unwrap());
    assert_eq!(std::fs::read(&file).unwrap(), b"weights");
}

// ---- removing ----------------------------------------------------------------------------------

/// An installed home: the person's data (kept by Remove-keeping-data, returned) and Clio's own
/// files.
fn installed(home: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    for (rel, bytes) in [
        // The person's: config, vault, memory, conversations, skills, services, servers.
        (
            ".config/clio/config.json",
            &b"{\"ui\":{\"language\":\"es\"}}"[..],
        ),
        (".config/clio/commands/hola.md", b"hola"),
        (".local/share/clio/vault.bin", b"sealed"),
        (".local/share/clio/memory/clio.db", b"memories"),
        (".local/share/clio/runs/r1/session.jsonl", b"a conversation"),
        (".local/share/clio/pi/sessions/s.jsonl", b"pi conversation"),
        (".local/share/clio/pi/agent/settings.json", b"{}"),
        (".local/share/clio/pi/home/.claude/x", b"login"),
        (".local/share/clio/skills/mine/SKILL.md", b"a skill"),
        (".local/share/clio/services/watch/service.json", b"{}"),
        (".config/clio/mcp.json", b"{}"),
        // Clio-memory and its model stay in both removals.
        (".local/bin/clio-memory", b"memory binary"),
        (".cache/clio-models/fake/model.onnx", b"weights"),
    ] {
        write(&home.join(rel), bytes);
    }
    let kept = tree(home);
    for (rel, bytes) in [
        (".local/bin/clio", &b"clio"[..]),
        (".local/bin/clio-vault", b"vault"),
        (".local/share/clio/node/bin/node", b"node"),
        (".local/share/clio/pi/0.70.0/package.json", b"{}"),
        (".local/share/clio/pi/.install-42/x", b"half"),
        (".local/share/clio/version", b"0.2.0"),
        (".local/share/clio/update.json", b"{}"),
    ] {
        write(&home.join(rel), bytes);
    }
    for u in crate::cli::services::UNITS {
        write(&home.join(".config/systemd/user").join(u), b"[Unit]");
    }
    kept
}

/// Runs `action`'s steps on an installed home; returns what Remove-keeping-data must keep.
fn remove(home: &Path, action: Action) -> BTreeMap<PathBuf, Vec<u8>> {
    let kept = installed(home);
    let d = dirs(home);
    let bin = home.join(".local/bin");
    let sys_dir = home.join("sys");
    script(&sys_dir.join("systemctl"), "exit 0");
    let mut ui = FakeUi::default();
    let opts = Opts {
        yes: true,
        bin,
        ..Opts::default()
    };
    let mut r = super::run_with(
        &d,
        Lang::En,
        fake_sys(&sys_dir),
        super::fetch::Pins {
            node: None,
            memory: None,
            model: None,
        },
        opts,
        &mut ui,
    );
    r.all(&action.steps());
    assert!(!r.failed, "{:?}", ui.rows);
    std::fs::remove_dir_all(&sys_dir).unwrap();
    kept
}

#[test]
fn remove_keeping_data_leaves_exactly_the_data() {
    let t = tempfile::tempdir().unwrap();
    let kept = remove(t.path(), Action::RemoveKeep);
    assert_eq!(tree(t.path()), kept);
}

#[test]
fn remove_everything_keeps_only_clio_memory_and_its_model() {
    let t = tempfile::tempdir().unwrap();
    remove(t.path(), Action::RemoveAll);
    let after: Vec<PathBuf> = tree(t.path()).into_keys().collect();
    assert_eq!(
        after,
        [
            PathBuf::from(".cache/clio-models/fake/model.onnx"),
            PathBuf::from(".local/bin/clio-memory"),
        ]
    );
}

#[test]
fn the_steps_of_each_choice() {
    assert_eq!(Action::Install.steps().first(), Some(&Step::Folders));
    assert!(Action::Install.steps().contains(&Step::Mcp));
    assert!(!Action::Install.shown().contains(&Step::Mcp));
    for a in [Action::Update, Action::RemoveKeep] {
        assert!(!a.steps().contains(&Step::RemoveData), "{a:?}");
    }
    // An update never asks: no vault, model or judge step; it replaces Clio first.
    let u = Action::Update.steps();
    assert_eq!(u[0], Step::Binaries);
    for s in [
        Step::Vault,
        Step::Model,
        Step::Judge,
        Step::Folders,
        Step::System,
    ] {
        assert!(!u.contains(&s), "{s:?}");
    }
    assert!(!Action::Repair.steps().contains(&Step::Model));
    assert_eq!(Action::RemoveAll.steps().last(), Some(&Step::RemoveData));
}
