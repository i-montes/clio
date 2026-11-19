#![allow(clippy::unwrap_used)]
#![cfg(target_os = "linux")]
//! `clio setup --yes` and `clio update` end to end, with no network and nothing of the machine:
//! a temporary HOME, a local release (`CLIO_RELEASE_BASE`), a fake Node tarball, a fake
//! clio-memory release and model (`CLIO_TEST_PINS`), and fake `sudo`, `apt-get`, `loginctl` and
//! `systemctl` that log their arguments and environment (`CLIO_TEST_SYSTEM`). The binaries run
//! from the HOME's `.local/bin`, where the installer puts them. Linux only: the fake systemd and
//! `/proc`.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const CLIO: &str = env!("CARGO_BIN_EXE_clio");
const PASS: &str = "a-long-test-passphrase-7731";
const KEY: &str = "sk-ant-api03-test-key-0e6f1d9c2b";
const TEMIS_KEY: &str = "temis-test-key-55aa01";
const TEMIS_URL: &str = "https://api.imontes.dev/temis";

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn all(o: &Output) -> String {
    format!("{}{}", text(&o.stdout), text(&o.stderr))
}

fn sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn write(p: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

fn script(p: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    write(p, format!("#!/bin/sh\n{body}\n").as_bytes());
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn tar(dir: &Path, top: &str, file: &Path) {
    let ok = Command::new("tar")
        .arg("-czf")
        .arg(file)
        .arg("-C")
        .arg(dir)
        .arg(top)
        .status()
        .unwrap()
        .success();
    assert!(ok);
}

fn vault_bin() -> PathBuf {
    static BIN: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BIN.get_or_init(|| {
        let exe = std::env::current_exe().unwrap();
        let profile = exe.parent().unwrap().parent().unwrap().to_path_buf();
        let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        cmd.args(["build", "-q", "-p", "clio-vault", "--bin", "clio-vault"]);
        if profile.file_name().is_some_and(|n| n == "release") {
            cmd.arg("--release");
        }
        assert!(cmd.status().unwrap().success(), "cannot build clio-vault");
        profile.join("clio-vault")
    })
    .clone()
}

/// A hard link when it can be (the binaries are large), else a copy.
fn place(from: &Path, to: &Path) {
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    if std::fs::hard_link(from, to).is_err() {
        std::fs::copy(from, to).unwrap();
    }
}

/// The release's name for this system.
fn target() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64-unknown-linux-gnu",
        _ => "aarch64-unknown-linux-gnu",
    }
}

/// Every file under `root` with its bytes, but those `skip` names (relative prefixes).
fn tree(root: &Path, skip: &[&str]) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut todo = vec![root.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            let rel = p.strip_prefix(root).unwrap().display().to_string();
            if skip.iter().any(|s| rel.starts_with(s)) {
                continue;
            }
            let t = e.file_type().unwrap();
            if t.is_dir() {
                todo.push(p);
            } else if t.is_file() {
                out.insert(rel, std::fs::read(&p).unwrap_or_default());
            }
        }
    }
    out
}

struct Fx {
    /// Outside `/tmp` (beside the build, so the binaries can be hard-linked).
    root: tempfile::TempDir,
    /// Short, for the vault's sockets.
    run: tempfile::TempDir,
}

impl Fx {
    fn new() -> Self {
        let root = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let run = tempfile::tempdir().unwrap();
        let f = Self { root, run };
        std::fs::create_dir_all(f.home()).unwrap();
        place(Path::new(CLIO), &f.bin().join("clio"));
        place(&vault_bin(), &f.bin().join("clio-vault"));
        f.release("0.2.0", &[]);
        f.pins("22.23.5");
        f.system();
        write(&f.p("secrets/pass"), format!("{PASS}\n").as_bytes());
        write(&f.p("secrets/key"), format!("{KEY}\n").as_bytes());
        write(&f.p("secrets/temis"), TEMIS_KEY.as_bytes());
        for s in ["pass", "key", "temis"] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                f.p("secrets").join(s),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        script(&f.p("host-check.sh"), "exit 0");
        f
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.root.path().join(rel)
    }
    fn home(&self) -> PathBuf {
        self.p("home")
    }
    fn bin(&self) -> PathBuf {
        self.home().join(".local/bin")
    }
    fn data(&self) -> PathBuf {
        self.home().join(".local/share/clio")
    }

    /// The release `CLIO_RELEASE_BASE` points at; `wrong`: binaries whose sums do not match.
    fn release(&self, version: &str, wrong: &[&str]) {
        let dir = self.p("release");
        let _ = std::fs::remove_dir_all(&dir);
        let mut sums = String::new();
        for name in ["clio-vault", "clio"] {
            let body = format!("#!/bin/sh\necho \"{name} {version} $*\" >> \"$HOME/reexec.log\"\n");
            let asset = format!("{name}-{}", target());
            write(&dir.join(&asset), body.as_bytes());
            let listed = if wrong.contains(&name) {
                sha(b"x")
            } else {
                sha(body.as_bytes())
            };
            sums.push_str(&format!("{listed}  {asset}\n"));
        }
        write(&dir.join("SHA256SUMS"), sums.as_bytes());
        write(&dir.join("VERSION"), format!("{version}\n").as_bytes());
    }

    /// Pins of local fakes: Node `node`, clio-memory 0.14.0, a model of one file and a runtime.
    fn pins(&self, node: &str) {
        let src = self.p("src");
        let top = format!("node-v{node}-linux");
        script(&src.join(&top).join("bin/node"), &format!("echo v{node}"));
        let tgz = src.join(format!("{top}.tar.gz"));
        tar(&src, &top, &tgz);
        let mem = src.join("memory");
        let asset = format!("clio-memory-{}", target());
        let body = "#!/bin/sh\necho clio-memory 0.14.0\n";
        write(&mem.join(&asset), body.as_bytes());
        let sums = format!("{}  {asset}\n", sha(body.as_bytes()));
        write(&mem.join("SHA256SUMS"), sums.as_bytes());
        write(&src.join("model.onnx"), b"weights");
        write(&src.join("ort-1/lib/libonnxruntime.so"), b"runtime");
        let ort = src.join("ort.tgz");
        tar(&src, "ort-1", &ort);
        let pins = json!({
            "node": {
                "url": tgz, "sha256": sha(&std::fs::read(&tgz).unwrap()), "version": node,
            },
            "memory": {
                "version": "0.14.0", "base": mem, "sums_sha256": sha(sums.as_bytes()), "asset": asset,
            },
            "model": {
                "dir": ".cache/clio-models/fake",
                "files": [{
                    "url": src.join("model.onnx"), "name": "model.onnx",
                    "sha256": sha(b"weights"), "size": 7,
                }],
                "runtime_dir": ".cache/clio-models/onnxruntime",
                "runtime": [{
                    "url": ort, "sha256": sha(&std::fs::read(&ort).unwrap()), "into": "cpu",
                }],
            },
        });
        write(&self.p("pins.json"), pins.to_string().as_bytes());
    }

    /// The fake system: each program logs its arguments and environment and does what it says.
    fn system(&self) {
        let d = self.p("sys");
        let log = "D=$(dirname \"$0\"); echo \"$(basename \"$0\") $*\" >> \"$D/calls.log\"; env >> \"$D/env.log\"";
        script(&d.join("sudo"), &format!("{log}\nexec \"$@\""));
        script(
            &d.join("apt-get"),
            &format!(
                "{log}\ncase \"$*\" in *' install '*) printf '#!/bin/sh\\nexit 0\\n' > \"$D/bwrap\"; chmod +x \"$D/bwrap\";; esac"
            ),
        );
        script(
            &d.join("loginctl"),
            &format!("{log}\nmkdir -p \"$D/linger\"; touch \"$D/linger/$2\""),
        );
        script(
            &d.join("systemctl"),
            &format!(
                "{log}\ncase \"$2\" in is-active) [ -f \"$D/active\" ];; enable) touch \"$D/active\";; disable) rm -f \"$D/active\";; esac"
            ),
        );
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(self.bin().join("clio"));
        c.args(args)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", self.home())
            .env("USER", "someone")
            .env("LANG", "C.UTF-8")
            .env("XDG_RUNTIME_DIR", self.run.path())
            .env("CLIO_TEST_PINS", self.p("pins.json"))
            .env("CLIO_TEST_SYSTEM", self.p("sys"))
            .env("CLIO_RELEASE_BASE", self.p("release"))
            .env("CLIO_SKIP_PI_INSTALL", "1")
            .env("CLIO_HOST_CHECK", self.p("host-check.sh"))
            .current_dir(self.root.path())
            .stdin(Stdio::null());
        c
    }

    fn clio(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }

    fn setup(&self, extra: &[&str]) -> Output {
        let pass = self.p("secrets/pass").display().to_string();
        let mut args = vec!["setup", "--yes", "--lang", "en", "--passphrase-file", &pass];
        args.extend_from_slice(extra);
        self.clio(&args)
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.p("sys/calls.log")).unwrap_or_default()
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        // The vault server the setup started: stopped, whatever the test did.
        let _ = self.clio(&["vault", "stop"]);
    }
}

/// Files that change on every run by design.
const VOLATILE: &[&str] = &[".local/share/clio/update.json"];

#[test]
fn setup_installs_everything_and_a_second_run_changes_nothing() {
    let f = Fx::new();
    let key = f.p("secrets/key").display().to_string();
    let temis = f.p("secrets/temis").display().to_string();
    let extra = [
        "--connector",
        "anthropic-api",
        "--key-file",
        &key,
        "--temis-url",
        TEMIS_URL,
        "--temis-key-file",
        &temis,
    ];
    // A secret file other users can read is refused before anything is done.
    let open = f.p("open-pass");
    write(&open, PASS.as_bytes());
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let o = f.clio(&[
        "setup",
        "--yes",
        "--passphrase-file",
        &open.display().to_string(),
    ]);
    assert!(
        !o.status.success() && all(&o).contains("can be read by other users"),
        "{}",
        all(&o)
    );
    assert!(!f.data().exists());
    std::fs::remove_file(&open).unwrap();
    let o = f.setup(&extra);
    assert!(o.status.success(), "{}", all(&o));
    let out = all(&o);
    let home = f.home();
    // Node, clio-memory and its model, the vault, the model's key, the judge, the timer.
    assert!(f.data().join("node/bin/node").is_file(), "{out}");
    assert_eq!(
        text(
            &Command::new(f.bin().join("clio-memory"))
                .arg("--version")
                .output()
                .unwrap()
                .stdout
        )
        .trim(),
        "clio-memory 0.14.0"
    );
    assert_eq!(
        std::fs::read(home.join(".cache/clio-models/fake/model.onnx")).unwrap(),
        b"weights"
    );
    assert!(
        home.join(".cache/clio-models/onnxruntime/cpu/lib/libonnxruntime.so")
            .is_file()
    );
    let cfg: Value =
        serde_json::from_slice(&std::fs::read(home.join(".config/clio/config.json")).unwrap())
            .unwrap();
    assert_eq!(cfg["ui"]["language"], "en", "{cfg}");
    assert_eq!(cfg["jev"]["enabled"], true, "{cfg}");
    assert_eq!(cfg["model"]["provider"], "anthropic", "{cfg}");
    let list = text(&f.clio(&["vault", "list"]).stdout);
    assert!(
        list.contains("anthropic") && list.contains("temis"),
        "{list}"
    );
    assert!(
        home.join(".config/systemd/user/clio-services.timer")
            .is_file(),
        "{out}"
    );
    // One sudo moment: the package manager, then lingering.
    let calls = f.calls();
    let sudo: Vec<&str> = calls.lines().filter(|l| l.starts_with("sudo ")).collect();
    assert_eq!(sudo.len(), 3, "{calls}");
    assert!(
        sudo[0].ends_with("apt-get -o DPkg::Lock::Timeout=300 update -qq"),
        "{calls}"
    );
    assert!(
        sudo[1].ends_with("apt-get -o DPkg::Lock::Timeout=300 install -y -qq bubblewrap"),
        "{calls}"
    );
    // Each command is printed before it runs.
    assert!(
        out.contains("$ sudo ") && out.contains("enable-linger"),
        "{out}"
    );
    assert!(sudo[2].contains("/loginctl enable-linger "), "{calls}");
    assert!(
        calls.contains("systemctl --user enable --now clio-services.timer"),
        "{calls}"
    );
    assert_eq!(
        std::fs::read_to_string(f.data().join("version")).unwrap(),
        "0.2.0"
    );

    // No secret anywhere but the vault: not on the screen, in an argument or an environment
    // (the fake programs log theirs), in a log or in any other file.
    for s in [PASS, KEY, TEMIS_KEY] {
        assert!(!out.contains(s), "on the screen: {out}");
        let files = tree(
            f.root.path(),
            &["secrets/", "home/.local/share/clio/vault.bin"],
        );
        for (p, bytes) in files.iter().chain(&tree(f.run.path(), &[])) {
            assert!(
                !bytes.windows(s.len()).any(|w| w == s.as_bytes()),
                "a secret is in {p}"
            );
        }
    }
    let pid = std::fs::read_to_string(f.run.path().join("clio/vault.pid")).unwrap();
    {
        for what in ["cmdline", "environ"] {
            let b = std::fs::read(format!("/proc/{}/{what}", pid.trim())).unwrap_or_default();
            for s in [PASS, KEY, TEMIS_KEY] {
                assert!(
                    !b.windows(s.len()).any(|w| w == s.as_bytes()),
                    "the vault's {what}"
                );
            }
        }
    }

    // Again: every step says what is there, and nothing changes.
    let before = tree(&home, VOLATILE);
    let calls_before = f.calls().lines().count();
    let o = f.setup(&extra);
    assert!(o.status.success(), "{}", all(&o));
    let after = tree(&home, VOLATILE);
    let changed: Vec<&String> = after
        .iter()
        .filter(|(k, v)| before.get(*k) != Some(*v))
        .map(|(k, _)| k)
        .chain(before.keys().filter(|k| !after.contains_key(*k)))
        .collect();
    assert!(
        changed.is_empty(),
        "changed by a second run: {changed:?}\n{}",
        all(&o)
    );
    let again = f.calls();
    assert!(
        !again
            .lines()
            .skip(calls_before)
            .any(|l| l.starts_with("sudo ")),
        "sudo again: {again}"
    );
}

#[test]
fn update_keeps_the_persons_data_byte_for_byte_and_says_what_changed() {
    let f = Fx::new();
    let o = f.setup(&["--no-model"]);
    assert!(o.status.success(), "{}", all(&o));
    // The person's: conversations, skills, services, servers, commands.
    for (rel, b) in [
        (".local/share/clio/runs/r1/session.jsonl", "a conversation"),
        (".local/share/clio/pi/sessions/s.jsonl", "pi conversation"),
        (".local/share/clio/services/watch/service.json", "{}"),
        (".config/clio/commands/hola.md", "hola"),
    ] {
        write(&f.home().join(rel), b.as_bytes());
    }
    let skip: Vec<&str> = VOLATILE
        .iter()
        .copied()
        .chain([
            ".local/share/clio/node/",
            ".local/share/clio/version",
            ".local/bin/",
            ".cache/clio-models/",
        ])
        .collect();
    let before = tree(&f.home(), &skip);
    assert!(
        before.keys().any(|k| k.ends_with("vault.bin")),
        "{:?}",
        before.keys()
    );
    f.pins("22.24.0");
    let o = f.clio(&["update"]);
    let out = all(&o);
    assert!(o.status.success(), "{out}");
    assert!(out.contains("Node 22.23.5 → 22.24.0"), "{out}");
    assert!(
        f.calls()
            .contains("systemctl --user restart clio-services.timer"),
        "{out}"
    );
    assert_eq!(tree(&f.home(), &skip), before, "{out}");
    // Nothing newer: said in one line, and still nothing of the person's changed.
    let o = f.clio(&["update"]);
    let out = all(&o);
    assert!(
        o.status.success() && out.contains("Clio is already up to date (0.2.0)."),
        "{out}"
    );
    assert_eq!(tree(&f.home(), &skip), before);
    let o = f.clio(&["update", "--check"]);
    assert!(all(&o).contains("already up to date"), "{}", all(&o));
    f.release("9.9.9", &[]);
    let o = f.clio(&["update", "--check"]);
    assert!(all(&o).contains("9.9.9"), "{}", all(&o));
    assert!(
        !f.home().join("reexec.log").exists(),
        "--check replaced the binaries"
    );

    // While a chat is open (its run folder's lock is held) the update waits.
    let lock = std::fs::File::create(f.data().join("runs/r1/session.lock")).unwrap();
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive).unwrap();
    let o = f.clio(&["update"]);
    assert_eq!(o.status.code(), Some(1), "{}", all(&o));
    assert!(all(&o).contains("A Clio chat is open"), "{}", all(&o));
    assert!(!f.home().join("reexec.log").exists());
}

#[test]
fn update_replaces_both_binaries_or_neither_and_goes_on_in_the_new_one() {
    let f = Fx::new();
    let ino = |n: &str| {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(f.bin().join(n)).unwrap().ino()
    };
    let (clio, vault) = (ino("clio"), ino("clio-vault"));
    // `clio` does not match its sum: neither is replaced, and the old one still works.
    f.release("9.9.9", &["clio"]);
    let o = f.clio(&["update"]);
    assert_eq!(o.status.code(), Some(1), "{}", all(&o));
    assert!(
        all(&o).contains("does not match its checksum"),
        "{}",
        all(&o)
    );
    assert_eq!((ino("clio"), ino("clio-vault")), (clio, vault));
    assert!(text(&f.clio(&["--version"]).stdout).contains("0.2.0"));
    let left: Vec<String> = std::fs::read_dir(f.bin())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.'))
        .collect();
    assert!(left.is_empty(), "{left:?}");
    // Both right: both replaced, and the new `clio` finishes the update.
    f.release("9.9.9", &[]);
    let o = f.clio(&["update"]);
    assert!(o.status.success(), "{}", all(&o));
    let log = std::fs::read_to_string(f.home().join("reexec.log")).unwrap();
    assert_eq!(log.trim(), "clio 9.9.9 update --reexec");
    assert!(
        std::fs::read_to_string(f.bin().join("clio-vault"))
            .unwrap()
            .contains("clio-vault 9.9.9")
    );
}

/// Serves `files` over HTTP; `stall`: the file sent only in part, and then held until the
/// returned sender is dropped.
fn serve(
    files: BTreeMap<String, Vec<u8>>,
    stall: &str,
) -> (
    String,
    std::sync::mpsc::Sender<()>,
    std::thread::JoinHandle<()>,
) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let stall = stall.to_owned();
    let h = std::thread::spawn(move || {
        l.set_nonblocking(true).unwrap();
        let mut held = Vec::new();
        loop {
            match l.accept() {
                Ok((mut s, _)) => {
                    s.set_nonblocking(false).unwrap();
                    let mut buf = [0u8; 4096];
                    let n = s.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let path = req
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("/")
                        .trim_start_matches('/')
                        .to_owned();
                    match files.get(&path) {
                        Some(body) => {
                            let _ = write!(
                                s,
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            if path == stall {
                                let _ = s.write_all(&body[..body.len() / 2]);
                                held.push(s);
                            } else {
                                let _ = s.write_all(body);
                            }
                        }
                        None => {
                            let _ = write!(
                                s,
                                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            );
                        }
                    }
                }
                Err(_) => {
                    if !matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    });
    (format!("http://127.0.0.1:{port}"), tx, h)
}

#[test]
fn an_update_killed_mid_download_leaves_a_working_install() {
    let f = Fx::new();
    f.release("9.9.9", &[]);
    let mut files = BTreeMap::new();
    for n in [
        "VERSION",
        "SHA256SUMS",
        &format!("clio-vault-{}", target()),
        &format!("clio-{}", target()),
    ] {
        let mut body = std::fs::read(f.p("release").join(n)).unwrap();
        if n.starts_with("clio-vault") {
            // Big enough that half of it is on disk while the rest waits.
            body.extend(std::iter::repeat_n(b'#', 1 << 20));
        }
        files.insert(n.to_owned(), body);
    }
    // The sums must name the padded file.
    let vault_asset = format!("clio-vault-{}", target());
    let sums = std::fs::read_to_string(f.p("release/SHA256SUMS")).unwrap();
    let sums: String = sums
        .lines()
        .map(|l| {
            if l.ends_with(&vault_asset) {
                format!("{}  {vault_asset}\n", sha(&files[&vault_asset]))
            } else {
                format!("{l}\n")
            }
        })
        .collect();
    files.insert("SHA256SUMS".into(), sums.into_bytes());
    let (base, stop, server) = serve(files, &vault_asset);
    let mut child = f
        .cmd(&["update"])
        .env("CLIO_RELEASE_BASE", &base)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stage = f.bin().join(".clio-vault.new");
    let t = Instant::now();
    while std::fs::metadata(&stage).map_or(0, |m| m.len()) == 0 {
        assert!(
            t.elapsed() < Duration::from_secs(60),
            "the download never started"
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "the update ended before the download"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    drop(stop);
    server.join().unwrap();
    // The install works as it was.
    let o = f.clio(&["--version"]);
    assert!(
        o.status.success() && text(&o.stdout).contains("0.2.0"),
        "{}",
        all(&o)
    );
    let o = f.clio(&["vault", "status"]);
    assert!(!all(&o).contains("cannot find `clio-vault`"), "{}", all(&o));
    // And the next update finishes, over what the killed one left.
    let o = f.clio(&["update"]);
    assert!(o.status.success(), "{}", all(&o));
    assert_eq!(
        std::fs::read_to_string(f.home().join("reexec.log"))
            .unwrap()
            .trim(),
        "clio 9.9.9 update --reexec"
    );
    assert!(!stage.exists());
}

#[test]
fn without_a_terminal_setup_says_what_to_run_and_exits_0() {
    let f = Fx::new();
    let c = f.cmd(&["setup"]);
    let prog = c.get_program().to_owned();
    let args: Vec<_> = c.get_args().map(ToOwned::to_owned).collect();
    let envs: Vec<_> = c
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_owned(), v?.to_owned())))
        .collect();
    // A new session has no controlling terminal: `/dev/tty` cannot be opened.
    let o = Command::new("setsid")
        .arg("-w")
        .arg(prog)
        .args(args)
        .env_clear()
        .envs(envs)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let out = all(&o);
    assert_eq!(o.status.code(), Some(0), "{out}");
    assert!(
        out.contains("ejecuta «clio setup» en una") && out.contains("run `clio setup` in one"),
        "{out}"
    );
    assert!(!f.home().join(".config/clio").exists() && !f.data().exists());
}

const PTY: &str = r#"
import json, os, pty, select, sys, time
cfg = json.loads(sys.argv[1])
pid, fd = pty.fork()
if pid == 0:
    import fcntl, struct, termios
    fcntl.ioctl(1, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
    null = os.open("/dev/null", os.O_RDONLY)
    os.dup2(null, 0)
    os.execve(cfg["clio"], [cfg["clio"], "setup"], cfg["env"])
seen, saw, eof, t = b"", False, False, time.time()
while time.time() - t < 30:
    r, _, _ = ([], 0, 0) if eof else select.select([fd], [], [], 0.2)
    if r:
        try:
            seen += os.read(fd, 65536)
        except OSError:
            eof = True
    if eof:
        time.sleep(0.05)
    if not saw and "Idioma · Language".encode() in seen:
        saw = True
        os.write(fd, b"\x1b")
    if saw:
        p, st = os.waitpid(pid, os.WNOHANG)
        if p:
            print(json.dumps({"saw": True, "code": os.waitstatus_to_exitcode(st), "restored": b"\x1b[?1049l" in seen}))
            sys.exit(0)
os.kill(pid, 9)
print(json.dumps({"saw": saw, "code": None, "tail": seen[-400:].decode("utf-8", "replace")}))
"#;

#[test]
fn setup_with_stdin_not_a_terminal_uses_dev_tty() {
    let f = Fx::new();
    let c = f.cmd(&[]);
    let env: serde_json::Map<String, Value> = c
        .get_envs()
        .filter_map(|(k, v)| {
            Some((
                k.to_string_lossy().into_owned(),
                json!(v?.to_string_lossy()),
            ))
        })
        .chain([("TERM".into(), json!("xterm-256color"))])
        .collect();
    let cfg = json!({"clio": f.bin().join("clio"), "env": env});
    let o = Command::new("python3")
        .args(["-I", "-c", PTY])
        .arg(cfg.to_string())
        .output()
        .unwrap();
    let v: Value = serde_json::from_str(text(&o.stdout).trim()).unwrap_or(json!({"raw": all(&o)}));
    assert_eq!(v["saw"], true, "{v}");
    assert_eq!(v["code"], 0, "{v}");
    assert_eq!(v["restored"], true, "{v}");
}

/// The real thing, run by hand once: real Node, clio-memory with its model, and Pi, downloaded
/// into a temporary HOME outside `/tmp`; no sudo and no system packages (the fake system), then
/// `clio doctor`.
///
/// `cargo test -p clio-brain --test setup real_flow -- --ignored --nocapture`
#[test]
#[ignore = "downloads Node, clio-memory, its model and Pi"]
fn real_flow() {
    let f = Fx::new();
    let o = f
        .cmd(&[
            "setup",
            "--yes",
            "--lang",
            "en",
            "--no-model",
            "--passphrase-file",
        ])
        .arg(f.p("secrets/pass"))
        .env_remove("CLIO_TEST_PINS")
        .env_remove("CLIO_SKIP_PI_INSTALL")
        .env_remove("CLIO_RELEASE_BASE")
        .output()
        .unwrap();
    println!("{}", all(&o));
    let setup_ok = o.status.success();
    // The model and its runtime came (a failure there is only a warning, so it is checked here).
    let model_ok = !all(&o).contains("search by meaning: finishes later");
    // The doctor of a copy-mode Clio (host mode would try the console with the real sudo).
    let cfg = f.home().join(".config/clio/config.json");
    let mut c: Value = serde_json::from_slice(&std::fs::read(&cfg).unwrap()).unwrap();
    c["mode"] = json!("copy");
    write(&cfg, serde_json::to_string_pretty(&c).unwrap().as_bytes());
    let o = f
        .cmd(&["doctor"])
        .env_remove("CLIO_SKIP_PI_INSTALL")
        .output()
        .unwrap();
    println!("{}", all(&o));
    let node = f.data().join("node/bin/node");
    let v = Command::new(&node).arg("--version").output().unwrap();
    println!("private node: {}", text(&v.stdout).trim());
    assert!(text(&v.stdout).starts_with("v22."));
    let m = Command::new(f.bin().join("clio-memory"))
        .arg("--version")
        .output()
        .unwrap();
    println!("{}", text(&m.stdout).trim());
    assert!(text(&m.stdout).contains("0.14.0"));
    assert!(setup_ok, "a step failed: see above");
    assert!(model_ok, "the memory's model did not install: see above");
}
