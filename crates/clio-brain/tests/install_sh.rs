#![allow(clippy::unwrap_used)]
#![cfg(target_os = "linux")]
//! The repository's `install.sh` with a local release (`CLIO_RELEASE_BASE`): it installs both
//! binaries or neither, and a script cut at any point runs nothing. It runs in a new session (no
//! terminal), so it ends by printing the command to run. Linux only: `setsid`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn script_text() -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../install.sh")).unwrap()
}

fn sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn target() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64-unknown-linux-gnu",
        _ => "aarch64-unknown-linux-gnu",
    }
}

struct Fx {
    t: tempfile::TempDir,
}

impl Fx {
    /// A release whose `clio` writes what it was run with to `$HOME/ran.log`.
    fn new() -> Self {
        let t = tempfile::tempdir().unwrap();
        let f = Self { t };
        std::fs::create_dir_all(f.home()).unwrap();
        f.release(&[]);
        f
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.t.path().join(rel)
    }
    fn home(&self) -> PathBuf {
        self.p("home")
    }
    fn body(name: &str) -> String {
        format!("#!/bin/sh\necho \"{name} $*\" >> \"$HOME/ran.log\"\n")
    }
    fn release(&self, wrong: &[&str]) {
        let r = self.p("release");
        std::fs::create_dir_all(&r).unwrap();
        let mut sums = String::new();
        for name in ["clio-vault", "clio"] {
            let asset = format!("{name}-{}", target());
            let body = Self::body(name);
            std::fs::write(r.join(&asset), &body).unwrap();
            let s = if wrong.contains(&name) {
                sha(b"x")
            } else {
                sha(body.as_bytes())
            };
            sums.push_str(&format!("{s}  {asset}\n"));
        }
        std::fs::write(r.join("SHA256SUMS"), sums).unwrap();
        std::fs::write(r.join("VERSION"), "0.2.0\n").unwrap();
    }
    /// `curl … | sh`: the script on standard input, in a session with no terminal.
    fn run(&self, script: &str, path_first: Option<&Path>) -> Output {
        self.run_with(script, path_first, &[])
    }
    fn run_with(&self, script: &str, path_first: Option<&Path>, env: &[(&str, &str)]) -> Output {
        use std::io::Write;
        let path = std::env::var("PATH").unwrap_or_default();
        let path = match path_first {
            Some(p) => format!("{}:{path}", p.display()),
            None => path,
        };
        let mut c = Command::new("setsid")
            .args(["-w", "sh"])
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.home())
            .env("CLIO_RELEASE_BASE", self.p("release"))
            .envs(env.iter().copied())
            .current_dir(self.t.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _ = c.stdin.take().unwrap().write_all(script.as_bytes());
        c.wait_with_output().unwrap()
    }
    /// Every file and folder under the HOME.
    fn files(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut todo = vec![self.home()];
        while let Some(d) = todo.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                out.push(p.strip_prefix(self.home()).unwrap().display().to_string());
                if p.is_dir() {
                    todo.push(p);
                }
            }
        }
        out.sort();
        out
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn it_installs_both_binaries_and_says_what_to_run_without_a_terminal() {
    let f = Fx::new();
    let o = f.run(&script_text(), None);
    let (out, err) = (text(&o.stdout), text(&o.stderr));
    assert!(o.status.success(), "{out}{err}");
    assert_eq!(err, "");
    let bin = f.home().join(".local/bin");
    // Almost nothing: where it installs from (set here), then the one command.
    let from = format!("file://{}", f.p("release").display());
    assert_eq!(
        out,
        format!(
            "clio: instalando desde {from} · installing from {from}\n{}/clio setup\n",
            bin.display()
        )
    );
    assert_eq!(
        f.files(),
        [
            ".local",
            ".local/bin",
            ".local/bin/clio",
            ".local/bin/clio-vault",
            ".profile"
        ]
    );
    // ~/.local/bin was not on the PATH: the login profile puts it there from now on.
    let profile = std::fs::read_to_string(f.home().join(".profile")).unwrap();
    assert!(
        profile.ends_with("\nexport PATH=\"$HOME/.local/bin:$PATH\"\n"),
        "{profile}"
    );
    use std::os::unix::fs::PermissionsExt;
    for name in ["clio", "clio-vault"] {
        assert_eq!(
            std::fs::read_to_string(bin.join(name)).unwrap(),
            Fx::body(name)
        );
        let mode = std::fs::metadata(bin.join(name))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }
    // Again, over the installed ones: the same, and the profile is not added to twice.
    assert!(f.run(&script_text(), None).status.success());
    assert_eq!(f.files().len(), 5);
    assert_eq!(
        std::fs::read_to_string(f.home().join(".profile")).unwrap(),
        profile
    );
}

#[test]
fn the_path_goes_in_bash_profile_when_there_is_one_and_in_zshrc_for_zsh() {
    let f = Fx::new();
    std::fs::write(f.home().join(".bash_profile"), "# mine\n").unwrap();
    let zsh = [("SHELL", "/usr/bin/zsh")];
    for _ in 0..2 {
        assert!(f.run_with(&script_text(), None, &zsh).status.success());
    }
    let line = "export PATH=\"$HOME/.local/bin:$PATH\"";
    for rc in [".bash_profile", ".zshrc"] {
        let text = std::fs::read_to_string(f.home().join(rc)).unwrap();
        assert_eq!(text.matches(line).count(), 1, "{rc}: {text}");
    }
    assert!(!f.home().join(".profile").exists());
    // Already on the PATH: nothing is written.
    let g = Fx::new();
    let bin = g.home().join(".local/bin");
    assert!(g.run(&script_text(), Some(&bin)).status.success());
    assert!(!g.home().join(".profile").exists());
}

#[test]
fn a_folder_in_the_way_root_and_plain_http_are_refused_before_anything_is_installed() {
    let f = Fx::new();
    std::fs::create_dir_all(f.home().join(".local/bin/clio")).unwrap();
    let o = f.run(&script_text(), None);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        text(&o.stderr).contains("is a folder"),
        "{}",
        text(&o.stderr)
    );
    assert_eq!(f.files(), [".local", ".local/bin", ".local/bin/clio"]);

    let f = Fx::new();
    let o = f.run_with(
        &script_text(),
        None,
        &[("CLIO_RELEASE_BASE", "http://example.com/r")],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("must be https://"));
    assert_eq!(f.files(), [".local", ".local/bin"]);

    // Root, through a fake `id`: refused unless it is asked for.
    let f = Fx::new();
    let fake = f.p("fake");
    std::fs::create_dir_all(&fake).unwrap();
    std::fs::write(fake.join("id"), "#!/bin/sh\necho 0\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(fake.join("id"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let o = f.run(&script_text(), Some(&fake));
    assert_eq!(
        text(&o.stderr),
        "clio: ejecútalo como tu usuario, sin sudo · run it as your own user, without sudo\n"
    );
    assert!(f.files().is_empty());
    let o = f.run_with(&script_text(), Some(&fake), &[("CLIO_ALLOW_ROOT", "1")]);
    assert!(o.status.success(), "{}", text(&o.stderr));
}

#[test]
fn a_checksum_mismatch_or_a_missing_file_installs_nothing() {
    for case in ["sum", "missing", "short"] {
        let f = Fx::new();
        let r = f.p("release");
        match case {
            "sum" => f.release(&["clio"]),
            "missing" => std::fs::remove_file(r.join(format!("clio-{}", target()))).unwrap(),
            _ => {
                // A download cut short: fewer bytes than were published.
                let a = r.join(format!("clio-{}", target()));
                let b = std::fs::read(&a).unwrap();
                std::fs::write(&a, &b[..b.len() / 2]).unwrap();
            }
        }
        let o = f.run(&script_text(), None);
        assert_eq!(o.status.code(), Some(1), "{case}");
        let err = text(&o.stderr);
        assert_eq!(err.lines().count(), 1, "{case}: {err}");
        assert!(
            err.contains("no se instaló nada") && err.contains("nothing was installed"),
            "{case}: {err}"
        );
        assert_eq!(f.files(), [".local", ".local/bin"], "{case}");
    }
}

#[test]
fn another_system_and_macos_are_refused_in_one_line_in_both_languages() {
    for (os, said) in [
        (
            "FreeBSD",
            "clio: Clio no funciona en FreeBSD · Clio does not run on FreeBSD\n",
        ),
        (
            "Darwin",
            "clio: macOS todavía no es compatible · macOS is not supported yet\n",
        ),
    ] {
        let f = Fx::new();
        let fake = f.p("fake");
        std::fs::create_dir_all(&fake).unwrap();
        let uname = fake.join("uname");
        std::fs::write(
            &uname,
            format!("#!/bin/sh\n[ \"$1\" = -s ] && echo {os} || echo x86_64\n"),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&uname, std::fs::Permissions::from_mode(0o755)).unwrap();
        let o = f.run(&script_text(), Some(&fake));
        assert_eq!(o.status.code(), Some(1));
        assert_eq!(text(&o.stderr), said);
        assert!(f.files().is_empty());
    }
}

#[test]
fn the_script_cut_at_any_point_runs_nothing() {
    let full = script_text();
    assert!(
        full.ends_with("\nclio_install \"$@\"\n"),
        "the last line calls it"
    );
    let mut cuts: Vec<usize> = full.match_indices('\n').map(|(i, _)| i + 1).collect();
    cuts.extend((0..full.len()).step_by(37));
    cuts.retain(|&n| n < full.len() && full.is_char_boundary(n));
    cuts.sort_unstable();
    cuts.dedup();
    let f = Fx::new();
    for n in cuts {
        let part = &full[..n];
        // The whole function and a call to it is the whole script.
        if part.trim_end().ends_with("\nclio_install") {
            continue;
        }
        let o = f.run(part, None);
        assert!(
            f.files().is_empty(),
            "cut at byte {n} ({:?}) left {:?}: {}",
            &part[part.len().saturating_sub(30)..],
            f.files(),
            text(&o.stderr)
        );
        assert_eq!(text(&o.stdout), "", "cut at byte {n}");
    }
}
