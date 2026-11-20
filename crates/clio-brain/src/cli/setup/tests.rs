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
        let (Ask::Choice { title, .. }
        | Ask::Secret { title, .. }
        | Ask::Text { title, .. }
        | Ask::Checks { title, .. }) = q;
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
        userns: dir.join("userns_restrict"),
        apparmor: dir.join("apparmor.d"),
        root: false,
        user: Some("someone".into()),
        linux: true,
        fake: true,
        os_release: dir.join("os-release"),
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
    write(&dirs(&installed).data.join("version"), b"0.2.1");
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
    let end = |blocked, left| screen::End {
        failed: false,
        blocked,
        left,
        new_path: true,
    };
    let mut done = view(screen::done_page(
        &dirs(&installed),
        lang,
        Action::Install,
        &end(None, Some(M::LeftModel)),
    ));
    // A warning is not repeated on the last screen: only the one thing left.
    done.rows = vec![(t(M::StSystem), St::Warn(t(M::NoteNoSudoLinger)))];
    let blocked = view(screen::done_page(
        &dirs(&installed),
        lang,
        Action::Install,
        &end(Some(M::LeftJailUserns), None),
    ));
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
                (Step::System, St::Warn(t(M::NoteNoSudo))),
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
            "sudo",
            view(Page::Choice {
                title: t(M::SudoTitle),
                lines: vec![
                    M::SudoBwrap.fill(lang, &["apt"]),
                    t(M::SudoLinger),
                    t(M::SudoRuns),
                    "  sudo /usr/bin/apt-get -o DPkg::Lock::Timeout=300 update -qq".into(),
                    "  sudo /usr/bin/apt-get -o DPkg::Lock::Timeout=300 install -y -qq bubblewrap"
                        .into(),
                    "  sudo /usr/bin/loginctl enable-linger someone".into(),
                    t(M::SudoProfile),
                    "  sudo /usr/bin/tee /etc/apparmor.d/clio-bwrap".into(),
                    "  sudo /usr/sbin/apparmor_parser -r /etc/apparmor.d/clio-bwrap".into(),
                    t(M::SudoHow),
                ],
                options: vec![t(M::SudoGo), t(M::SudoSkip)],
                sel: 0,
            }),
        ),
        (
            "mcp",
            view(Page::Checks {
                title: t(M::McpTitle),
                lines: vec![
                    t(M::McpHint),
                    String::new(),
                    "Playwright: Un Chromium sin ventana que abre páginas web públicas, las lee y actúa en ellas.".into(),
                    "Notion: El servidor de Notion: busca, lee y edita las páginas y bases de datos compartidas con su integración.".into(),
                ],
                options: vec!["Playwright (un navegador web)".into(), "Notion".into()],
                checked: vec![false, true],
                sel: 1,
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
        ("blocked", blocked),
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
    let mut m = memory_release(&t.path().join("rel"), "0.14.0");
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
    assert_eq!(fetch::memory_installed(&bin).as_deref(), Some("0.14.0"));
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

// The runtime comes first: when it cannot be downloaded, no model folder is left (a folder without
// its runtime would fail every time the memory opens it), and the model step ends as a warning.
#[test]
fn a_missing_runtime_leaves_no_model_folder_and_only_warns() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src/model.onnx");
    write(&src, b"weights");
    let m = fetch::ModelPin {
        dir: ".cache/clio-models/granite".into(),
        files: vec![fetch::ModelFile {
            url: src.display().to_string(),
            name: "model.onnx".into(),
            sha256: sha(b"weights"),
            size: 7,
        }],
        runtime_dir: ".cache/clio-models/onnxruntime".into(),
        runtime: vec![fetch::Runtime {
            url: t.path().join("src/gone.tgz").display().to_string(),
            sha256: sha(b"x"),
            into: "cpu".into(),
        }],
    };
    let home = t.path().join("home");
    assert!(fetch::install_model(&home, &m, &mut |_| {}).is_err());
    assert!(!home.join(".cache/clio-models/granite").exists());

    // The memory step: the binary installs, the model is left for later, nothing failed.
    let mem = t.path().join("mem");
    let pin = memory_release(&mem, "0.14.0");
    let mut ui = FakeUi::default();
    let mut r = run_on(&home, fake_sys(&t.path().join("sys")), &mut ui);
    r.pins.memory = Some(pin);
    r.pins.model = Some(m);
    r.all(&[Step::Memory]);
    assert!(!r.failed);
    drop(r);
    assert_eq!(
        ui.rows.get(&0),
        Some(&St::Warn(M::NoteModelLater.get(Lang::En).into()))
    );
    assert!(!home.join(".cache/clio-models/granite").exists());
}

// No systemd user session: the timer is a warning, and its unit files do not stay behind.
#[test]
fn a_timer_that_cannot_start_only_warns_and_leaves_no_units() {
    let t = tempfile::tempdir().unwrap();
    let sys_dir = t.path().join("sys");
    script(&sys_dir.join("systemctl"), "exit 1");
    let mut ui = FakeUi::default();
    let mut r = run_on(t.path(), fake_sys(&sys_dir), &mut ui);
    r.opts.exe = t.path().join(".local/bin/clio");
    r.all(&[Step::Timer]);
    assert!(!r.failed);
    drop(r);
    assert_eq!(
        ui.rows.get(&0),
        Some(&St::Warn(M::NoteTimerLater.get(Lang::En).into()))
    );
    assert!(
        !t.path()
            .join(".config/systemd/user/clio-services.timer")
            .exists()
    );
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
        // The model is kept with the data; the profile is the person's own file.
        (".cache/clio-models/fake/model.onnx", b"weights"),
        (".profile", b"export EDITOR=vi\n"),
    ] {
        write(&home.join(rel), bytes);
    }
    let kept = tree(home);
    // What install.sh added to the profile, as it adds it.
    let mut profile = std::fs::read(home.join(".profile")).unwrap();
    profile.extend_from_slice(
        b"\n# Clio: ~/.local/bin on the PATH\nexport PATH=\"$HOME/.local/bin:$PATH\"\n",
    );
    write(&home.join(".profile"), &profile);
    write(
        &home.join(".zshrc"),
        b"\n# Clio: ~/.local/bin on the PATH\nexport PATH=\"$HOME/.local/bin:$PATH\"\n",
    );
    for (rel, bytes) in [
        (".local/bin/clio", &b"clio"[..]),
        (".local/bin/clio-memory", b"memory binary"),
        (".local/bin/clio-vault", b"vault"),
        (".local/share/clio/node/bin/node", b"node"),
        (".local/share/clio/pi/0.70.0/package.json", b"{}"),
        (".local/share/clio/pi/.install-42/x", b"half"),
        (".local/share/clio/version", b"0.2.1"),
        (".local/share/clio/update.json", b"{}"),
    ] {
        write(&home.join(rel), bytes);
    }
    for u in crate::cli::services::UNITS {
        write(&home.join(".config/systemd/user").join(u), b"[Unit]");
    }
    kept
}

/// Runs `action`'s steps on an installed home with `systemctl` (a script's body); returns what
/// Remove-keeping-data must keep, and the rows.
fn remove(
    home: &Path,
    action: Action,
    systemctl: &str,
) -> (BTreeMap<PathBuf, Vec<u8>>, BTreeMap<usize, St>) {
    let kept = installed(home);
    let d = dirs(home);
    let bin = home.join(".local/bin");
    let sys_dir = home.join("sys");
    script(&sys_dir.join("systemctl"), systemctl);
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
            browser: None,
        },
        opts,
        &mut ui,
    );
    r.all(&action.steps());
    assert!(!r.failed, "{:?}", ui.rows);
    std::fs::remove_dir_all(&sys_dir).unwrap();
    (kept, ui.rows)
}

#[test]
fn remove_keeping_data_leaves_exactly_the_data() {
    let t = tempfile::tempdir().unwrap();
    let (kept, _) = remove(t.path(), Action::RemoveKeep, "exit 0");
    assert_eq!(tree(t.path()), kept);
}

#[test]
fn remove_everything_leaves_only_the_persons_own_files() {
    let t = tempfile::tempdir().unwrap();
    remove(t.path(), Action::RemoveAll, "exit 0");
    let after: Vec<(PathBuf, Vec<u8>)> = tree(t.path()).into_iter().collect();
    assert_eq!(
        after,
        [(PathBuf::from(".profile"), b"export EDITOR=vi\n".to_vec())]
    );
}

// No user bus: systemd is said in the row, not a failure, and the unit files go anyway.
#[test]
fn a_timer_systemd_cannot_reach_is_removed_with_a_warning() {
    let t = tempfile::tempdir().unwrap();
    let (_, rows) = remove(
        t.path(),
        Action::RemoveAll,
        "echo 'Failed to connect to bus: No such file or directory' >&2; exit 1",
    );
    let i = Action::RemoveAll
        .shown()
        .iter()
        .position(|s| *s == Step::RemoveTimer)
        .unwrap();
    assert_eq!(
        rows[&i],
        St::Warn("removed (systemd did not answer)".into())
    );
    assert!(
        !t.path()
            .join(".config/systemd/user")
            .join(crate::cli::services::UNITS[0])
            .exists()
    );
}

#[test]
fn the_steps_of_each_choice() {
    assert_eq!(Action::Install.steps().first(), Some(&Step::Folders));
    // The MCP servers are a row of the install and of an update (which says what is newer).
    for a in [Action::Install, Action::Repair, Action::Update] {
        assert!(a.shown().contains(&Step::Mcp), "{a:?}");
    }
    for a in [Action::Update, Action::RemoveKeep] {
        assert!(!a.steps().contains(&Step::RemoveData), "{a:?}");
    }
    // An update replaces Clio first; it finishes a half-done install (the vault, the model) but
    // never asks for the judge or the system.
    let u = Action::Update.steps();
    assert_eq!(u[0], Step::Binaries);
    for s in [Step::Vault, Step::Model] {
        assert!(
            u.contains(&s) && Action::Repair.steps().contains(&s),
            "{s:?}"
        );
    }
    for s in [Step::Judge, Step::Folders, Step::System] {
        assert!(!u.contains(&s), "{s:?}");
    }
    assert_eq!(Action::RemoveAll.steps().last(), Some(&Step::RemoveModel));
    assert!(!Action::RemoveKeep.steps().contains(&Step::RemoveModel));
}

// No version noted (a run that did not finish): Repair is the choice offered first.
#[test]
fn an_install_without_its_version_offers_repair_first() {
    let t = tempfile::tempdir().unwrap();
    let d = dirs(t.path());
    write(&d.config_file(), b"{}");
    let sys = fake_sys(&t.path().join("sys"));
    let sel = |d: &Dirs| match screen::found_page(d, &sys, Lang::En) {
        Page::Choice { sel, .. } => sel,
        _ => panic!("not the existing-install page"),
    };
    assert_eq!(sel(&d), 1);
    write(&d.data.join("version"), b"0.2.1");
    assert_eq!(sel(&d), 0);
}

// ---- asking first, and the sudo moment ---------------------------------------------------------

fn run_on<'a>(home: &Path, sys: Sys, ui: &'a mut FakeUi) -> super::steps::Run<'a> {
    let opts = Opts {
        bin: home.join(".local/bin"),
        ..Opts::default()
    };
    let pins = super::fetch::Pins {
        node: None,
        memory: None,
        model: None,
        browser: None,
    };
    super::run_with(&dirs(home), Lang::En, sys, pins, opts, ui)
}

/// A fake system whose `bwrap` starts only once `ok` (a path) exists, and whose apt-get puts it in
/// place; `tee` and `apparmor_parser` log, `tee` writes what it reads.
fn jail_sys(dir: &Path, ok: &Path, restricted: bool) -> Sys {
    let log = "D=$(dirname \"$0\"); echo \"$(basename \"$0\") $*\" >> \"$D/calls.log\"";
    script(&dir.join("sudo"), &format!("{log}\nexec \"$@\""));
    script(
        &dir.join("apt-get"),
        &format!(
            "{log}\ncase \"$*\" in *install*) printf '#!/bin/sh\\n[ -e {} ]\\n' > \"$D/bwrap\"; chmod +x \"$D/bwrap\";; esac",
            ok.display()
        ),
    );
    script(&dir.join("tee"), &format!("{log}\ncat > \"$1\""));
    script(
        &dir.join("apparmor_parser"),
        &format!("{log}\ntouch {}", ok.display()),
    );
    std::fs::create_dir_all(dir.join("apparmor.d")).unwrap();
    if restricted {
        write(&dir.join("userns_restrict"), b"1\n");
    }
    fake_sys(dir)
}

// Every question is asked before any step runs, the sudo moment last; then the steps ask nothing.
#[test]
fn every_question_comes_before_the_work() {
    let t = tempfile::tempdir().unwrap();
    let sys_dir = t.path().join("sys");
    let ok = t.path().join("ok");
    std::fs::write(&ok, "").unwrap();
    script(&sys_dir.join("systemctl"), "exit 0");
    script(
        &sys_dir.join("loginctl"),
        "D=$(dirname \"$0\"); echo \"loginctl $*\" >> \"$D/calls.log\"",
    );
    let sys = jail_sys(&sys_dir, &ok, false);
    let mut ui = FakeUi {
        answers: [
            Answer::Secret(SecretValue::new(PASS.into())),
            Answer::Choice(3),
            Answer::Choice(0),
            // Nothing ticked: Enter installs no server.
            Answer::Checks(vec![false, false]),
            Answer::Choice(0),
        ]
        .into(),
        ..FakeUi::default()
    };
    {
        let mut r = run_on(t.path(), sys, &mut ui);
        r.ask_all(&Action::Install.steps());
        assert!(r.blocked.is_none());
        assert!(r.answers.mcp.is_empty() && r.answers.packages.is_empty());
    }
    assert!(ui.rows.is_empty(), "a step ran while asking: {:?}", ui.rows);
    let en = |m: M| m.get(Lang::En).to_owned();
    assert_eq!(
        ui.asked,
        [
            en(M::VaultNewTitle),
            en(M::ModelTitle),
            en(M::JudgeTitle),
            en(M::McpTitle),
            en(M::SudoTitle)
        ]
    );
    // The moment ran right then: bubblewrap installed, lingering on, no profile (not restricted).
    let calls = std::fs::read_to_string(sys_dir.join("calls.log")).unwrap();
    assert!(calls.contains("install -y -qq bubblewrap"), "{calls}");
    assert!(calls.contains("enable-linger someone"), "{calls}");
    assert!(!calls.contains("tee"), "{calls}");
}

// A Claude subscription asks how, and "sign in when the install finishes" asks for no key; when the
// sign-in cannot happen, the one thing left is choosing the model again, not a command to type.
#[test]
fn claude_sign_in_comes_at_the_end_and_its_failure_points_back_to_the_installer() {
    let t = tempfile::tempdir().unwrap();
    let mut ui = FakeUi {
        answers: [
            Answer::Secret(SecretValue::new(PASS.into())),
            Answer::Choice(0),
            Answer::Choice(0),
        ]
        .into(),
        ..FakeUi::default()
    };
    {
        let mut r = run_on(t.path(), fake_sys(&t.path().join("sys")), &mut ui);
        r.ask_all(&[Step::Vault, Step::Model]);
        assert!(r.answers.sign_in);
        // No Claude Code in this home: the sign-in does not run, the row says later.
        r.answers.sign_in_ready = clio_brain::connector::by_name("claude");
        r.sign_in(&[Step::Model]);
        assert_eq!(r.left, Some(M::LeftModelAgain));
    }
    let en = |m: M| m.get(Lang::En).to_owned();
    assert_eq!(
        ui.asked,
        [
            en(M::VaultNewTitle),
            en(M::ModelTitle),
            en(M::ClaudeHowTitle)
        ]
    );
    assert_eq!(ui.rows[&0], St::Warn(en(M::NoteLater)));
}

// The language screen starts on the saved language, else the locale's.
#[test]
fn the_language_preselected_is_the_saved_one_else_the_locales() {
    let var = |pairs: &'static [(&str, &str)]| {
        move |k: &str| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_owned())
        }
    };
    assert_eq!(Lang::from_locale(var(&[("LANG", "es_CO.UTF-8")])), Lang::Es);
    assert_eq!(
        Lang::from_locale(var(&[("LC_MESSAGES", "es_ES")])),
        Lang::Es
    );
    assert_eq!(
        Lang::from_locale(var(&[("LC_ALL", "en_US.UTF-8"), ("LANG", "es_CO.UTF-8")])),
        Lang::En
    );
    assert_eq!(Lang::from_locale(var(&[])), Lang::En);
    let t = tempfile::tempdir().unwrap();
    let d = dirs(t.path());
    assert_eq!(Lang::saved(&d), None);
    std::fs::create_dir_all(&d.config).unwrap();
    crate::cli::home::write_default_config(&d).unwrap();
    let mut cfg: serde_json::Value =
        serde_json::from_slice(&std::fs::read(d.config_file()).unwrap()).unwrap();
    cfg["ui"]["language"] = "es".into();
    write(&d.config_file(), cfg.to_string().as_bytes());
    assert_eq!(Lang::saved(&d), Some(Lang::Es));
    assert_eq!(Lang::saved_or_env(&d), Lang::Es);
}

// Where the kernel restricts user namespaces and the jail does not start, the AppArmor profile is
// part of the same moment, as fixed text; where the jail already starts, it is never written.
#[test]
fn the_apparmor_profile_is_written_only_when_the_jail_needs_it() {
    let t = tempfile::tempdir().unwrap();
    // Restricted, bwrap missing: installed, still not starting, so the profile goes in.
    let sys_dir = t.path().join("sys");
    let ok = t.path().join("ok");
    let mut ui = FakeUi::default();
    let mut r = run_on(t.path(), jail_sys(&sys_dir, &ok, true), &mut ui);
    r.opts.yes = true;
    r.ask_all(&[Step::System]);
    assert!(r.blocked.is_none());
    let profile = sys_dir.join("apparmor.d/clio-bwrap");
    assert_eq!(
        std::fs::read_to_string(&profile).unwrap(),
        super::BWRAP_PROFILE
    );
    let calls = std::fs::read_to_string(sys_dir.join("calls.log")).unwrap();
    assert!(
        calls.contains("sudo ") && calls.contains("apparmor_parser -r "),
        "{calls}"
    );
    drop(r);

    // Again: the jail starts, so nothing is written or run.
    std::fs::remove_file(sys_dir.join("calls.log")).unwrap();
    let mut r = run_on(t.path(), jail_sys(&sys_dir, &ok, true), &mut ui);
    r.opts.yes = true;
    r.ask_all(&[Step::System]);
    assert!(r.blocked.is_none());
    assert!(!sys_dir.join("calls.log").exists());
    drop(r);

    // The jail still does not start and a profile already names bwrap: none is added, and the
    // last screen says what to ask an administrator for.
    std::fs::remove_file(&ok).unwrap();
    let mut r = run_on(t.path(), jail_sys(&sys_dir, &ok, true), &mut ui);
    r.opts.yes = true;
    r.ask_all(&[Step::System]);
    assert_eq!(r.blocked, Some(M::LeftJailUserns));
    assert!(!sys_dir.join("calls.log").exists());
}

// ---- the MCP catalog ---------------------------------------------------------------------------

/// A Chromium archive as Playwright publishes it (`chrome-linux/chrome` inside a zip), made with
/// Python's `zipfile`; `None` (the test says it was skipped) without `python3` or `unzip`.
fn chromium_zip(dir: &Path, revision: &str) -> Option<Archive> {
    let path = std::env::var("PATH").unwrap_or_default();
    let have = |p: &str| clio_brain::pi::install::find_in_path(p, &path).is_some();
    if !have("python3") || !have("unzip") {
        eprintln!("SKIPPED (no python3 or unzip here)");
        return None;
    }
    let src = dir.join("src");
    script(
        &src.join("chrome-linux64").join(fetch::CHROME),
        "echo chromium",
    );
    let file = dir.join("chromium-linux.zip");
    let ok = std::process::Command::new("python3")
        .args(["-I", "-m", "zipfile", "-c"])
        .arg(&file)
        .arg(src.join("chrome-linux64"))
        .status()
        .unwrap()
        .success();
    assert!(ok);
    Some(Archive {
        url: file.display().to_string(),
        sha256: sha(&std::fs::read(&file).unwrap()),
        version: revision.into(),
    })
}

// Clio's Chromium: its archive is verified before anything is unpacked, unpacked beside its place
// with `unzip`, made private and moved in once; a wrong one leaves nothing.
#[test]
fn chromium_is_verified_unpacked_and_moved_in_once() {
    let t = tempfile::tempdir().unwrap();
    let Some(mut a) = chromium_zip(t.path(), "1255") else {
        return;
    };
    let data = t.path().join("data");
    let good = a.sha256.clone();
    a.sha256 = sha(b"not it");
    assert!(matches!(
        fetch::install_browser(&data, &a, &mut |_| {}),
        Err(Fail::Sum(_))
    ));
    assert_eq!(tree(&data).len(), 0, "{:?}", tree(&data).keys());
    a.sha256 = good;
    assert!(fetch::install_browser(&data, &a, &mut |_| {}).unwrap());
    let chrome = data.join("browsers/chromium-1255").join(fetch::CHROME);
    let out = std::process::Command::new(&chrome).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "chromium");
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(data.join("browsers/chromium-1255"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0, "private");
    assert!(!fetch::install_browser(&data, &a, &mut |_| {}).unwrap());
    assert!(fetch::missing_libs(&chrome).is_empty());
}

// What Chromium lacks becomes the packages of this system's names (Ubuntu 24.04 and Debian 13 took
// the `t64` names); where they are not known, nothing can be installed (`None`), unless nothing
// is missing.
#[test]
fn chromiums_missing_libraries_become_this_systems_packages() {
    // `readelf -d chrome` of the pinned build (the same on both processors), but the C library's
    // own and `libgcc_s`: every one must be in the table, or a missing one is never installed.
    const NEEDED: [&str; 26] = [
        "libX11.so.6",
        "libXcomposite.so.1",
        "libXdamage.so.1",
        "libXext.so.6",
        "libXfixes.so.3",
        "libXrandr.so.2",
        "libasound.so.2",
        "libatk-1.0.so.0",
        "libatk-bridge-2.0.so.0",
        "libatspi.so.0",
        "libcairo.so.2",
        "libcups.so.2",
        "libdbus-1.so.3",
        "libexpat.so.1",
        "libgbm.so.1",
        "libgio-2.0.so.0",
        "libglib-2.0.so.0",
        "libgobject-2.0.so.0",
        "libnspr4.so",
        "libnss3.so",
        "libnssutil3.so",
        "libpango-1.0.so.0",
        "libsmime3.so",
        "libudev.so.1",
        "libxcb.so.1",
        "libxkbcommon.so.0",
    ];
    let mut table: Vec<&str> = fetch::CHROME_LIBS.iter().map(|(so, ..)| *so).collect();
    table.sort_unstable();
    assert_eq!(table, NEEDED);
    let all: String = fetch::CHROME_LIBS
        .iter()
        .map(|(so, ..)| format!("\t{so} (libc6,AArch64) => /lib/{so}\n"))
        .collect();
    let some: String = all
        .lines()
        .filter(|l| !l.contains("libnss3.so ") && !l.contains("libasound.so.2 "))
        .map(|l| format!("{l}\n"))
        .collect();
    let os = |id: &str, v: &str| format!("NAME=x\nID={id}\nVERSION_ID=\"{v}\"\n");
    assert_eq!(fetch::chrome_packages(&all, ""), Some(vec![]));
    for (id, v, sound) in [
        ("ubuntu", "24.04", "libasound2t64"),
        ("ubuntu", "22.04", "libasound2"),
        ("debian", "12", "libasound2"),
        ("debian", "13", "libasound2t64"),
    ] {
        assert_eq!(
            fetch::chrome_packages(&some, &os(id, v)),
            Some(vec!["libnss3".to_owned(), sound.to_owned()]),
            "{id} {v}"
        );
    }
    assert_eq!(fetch::chrome_packages(&some, &os("fedora", "41")), None);
    // Nothing known: every package, once each.
    let every = fetch::chrome_packages("", &os("ubuntu", "24.04")).unwrap();
    assert!(every.contains(&"libglib2.0-0t64".to_owned()));
    assert_eq!(every.iter().filter(|p| *p == "libnss3").count(), 1);
}

// The MCP checklist comes with the other questions, before the sudo moment, nothing ticked; a
// ticked Playwright puts what Chromium lacks (and `unzip`) into that one sudo moment; where those
// packages are not known it is left out, and its row says why, without failing the install.
#[test]
fn the_mcp_checklist_comes_first_and_chromiums_needs_go_into_the_sudo_moment() {
    let t = tempfile::tempdir().unwrap();
    let sys_dir = t.path().join("sys");
    let ok = t.path().join("ok");
    std::fs::write(&ok, "").unwrap();
    let sys = jail_sys(&sys_dir, &ok, false);
    let known: String = fetch::CHROME_LIBS
        .iter()
        .filter(|(so, ..)| *so != "libnss3.so")
        .map(|(so, ..)| format!("\t{so} (libc6) => /lib/{so}\n"))
        .collect();
    write(&sys_dir.join("ldconfig.out"), known.as_bytes());
    script(
        &sys_dir.join("ldconfig"),
        "cat \"$(dirname \"$0\")/ldconfig.out\"",
    );
    write(
        &sys_dir.join("os-release"),
        b"ID=ubuntu\nVERSION_ID=\"24.04\"\n",
    );
    let mut ui = FakeUi {
        answers: [Answer::Checks(vec![true, false]), Answer::Choice(0)].into(),
        ..FakeUi::default()
    };
    {
        let mut r = run_on(t.path(), sys.clone(), &mut ui);
        r.ask_all(&[Step::Mcp, Step::System]);
        assert_eq!(r.answers.mcp.len(), 1);
        assert_eq!(r.answers.mcp[0].0, "playwright");
    }
    let en = |m: M| m.get(Lang::En).to_owned();
    assert_eq!(ui.asked, [en(M::McpTitle), en(M::SudoTitle)]);
    let calls = std::fs::read_to_string(sys_dir.join("calls.log")).unwrap();
    // The browser's packages are their own install, after the jail's.
    let jail = calls.find("install -y -qq bubblewrap\n");
    let browser = calls.find("install -y -qq libnss3 unzip\n");
    assert!(jail.is_some() && browser > jail, "{calls}");
    // Their install fails: Playwright is left out, said on its row, and nothing else stops.
    let apt = std::fs::read_to_string(sys_dir.join("apt-get")).unwrap();
    script(
        &sys_dir.join("apt-get"),
        &apt.replacen('\n', "\ncase \"$*\" in *libnss3*) exit 1;; esac\n", 1)
            .replacen("#!/bin/sh\n", "", 1),
    );
    let mut ui = FakeUi {
        answers: [Answer::Checks(vec![true, false]), Answer::Choice(0)].into(),
        ..FakeUi::default()
    };
    {
        let mut r = run_on(t.path(), sys.clone(), &mut ui);
        r.ask_all(&[Step::Mcp, Step::System]);
        assert!(r.answers.mcp.is_empty(), "{:?}", r.answers.mcp);
        assert!(r.blocked.is_none());
        r.all(&[Step::Mcp]);
        assert!(!r.failed);
    }
    assert_eq!(ui.rows[&0], St::Warn(en(M::NoteMcpLibsFailed)));
    // A system whose package names are not known: Playwright is left out, said on its row.
    write(&sys_dir.join("os-release"), b"ID=fedora\nVERSION_ID=41\n");
    let mut ui = FakeUi {
        answers: [Answer::Checks(vec![true, false])].into(),
        ..FakeUi::default()
    };
    {
        let mut r = run_on(t.path(), sys, &mut ui);
        r.ask_all(&[Step::Mcp]);
        assert!(r.answers.mcp.is_empty() && r.answers.packages.is_empty());
        r.all(&[Step::Mcp]);
        assert!(!r.failed);
    }
    assert_eq!(ui.rows[&0], St::Warn(en(M::NoteMcpNoLibs)));
}
