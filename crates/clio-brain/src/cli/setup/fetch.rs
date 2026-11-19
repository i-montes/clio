//! What `clio setup` and `clio update` download, and how each is verified (`docs/security.md`,
//! "What the installer downloads"):
//!
//! - Clio's `clio` and `clio-vault`: from the release (or `CLIO_RELEASE_BASE`), checked against
//!   that release's `SHA256SUMS`; both are verified before either is replaced.
//! - The private Node: the official tarball of [`NODE_VERSION`], its SHA-256 pinned here.
//! - clio-memory: its release [`MEMORY_VERSION`], whose `SHA256SUMS` is pinned here by its own
//!   SHA-256; the binary is checked against it.
//! - The memory's model: the files and the ONNX runtime clio-memory pins (`clio_memory::setup`).
//!
//! A file that does not match replaces nothing. Downloads use `curl` (or `wget`); a local path in
//! place of a URL is copied (`CLIO_RELEASE_BASE` as a folder, and the tests).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Where Clio's releases are.
pub const RELEASES: &str = "https://github.com/i-montes/clio/releases";
/// The private Node: Pi needs 22.19 or newer.
pub const NODE_VERSION: &str = "22.23.5";
/// From <https://nodejs.org/dist/v22.23.5/SHASUMS256.txt>.
const NODE_SUMS: [(&str, &str); 4] = [
    (
        "linux-x64",
        "c54118104acc524633b5abe106bbf44a53126ec8b4e5fa105cb92b5d7de8a1a1",
    ),
    (
        "linux-arm64",
        "8bd3dc69eee8f3f226315a80d7d1d4faccf217a4c8fae500168ac2e38f3bdba3",
    ),
    (
        "darwin-arm64",
        "e7131995759b61a0170b646010d2ad35a5d37b4cb0d471f3e8317eac6f742f3d",
    ),
    (
        "darwin-x64",
        "c53383238206b9ae4db23c5d8a63645e2e753bec8ab51a49cc81939aa64e9422",
    ),
];
/// The clio-memory release Clio installs: the one its library is built with (Cargo.toml).
pub const MEMORY_VERSION: &str = "0.13.0";
/// The SHA-256 of that release's `SHA256SUMS`, which lists each binary's.
const MEMORY_SUMS_SHA256: &str = "6a3611dd665c19339fffa04c08840880ce24a78615d0c096f356549a61be6d12";

/// What went wrong, in a form the installer says in the person's language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fail {
    /// `what` could not be downloaded.
    Net(String),
    /// `what` did not match its checksum: nothing was installed.
    Sum(String),
    /// A failure the installer words itself: the sentence and what to do.
    Said(super::text::M, super::text::M),
    /// Anything else, as the code said it.
    Other(String),
}

impl From<String> for Fail {
    fn from(s: String) -> Self {
        Self::Other(s)
    }
}

pub type FRes<T> = Result<T, Fail>;

/// The release's name for this system, `None` where Clio has no build.
pub fn target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        _ => None,
    }
}

/// An archive to unpack: where from, its SHA-256 and the version it carries.
#[derive(Debug, Clone, Deserialize)]
pub struct Archive {
    pub url: String,
    pub sha256: String,
    pub version: String,
}

/// clio-memory's release: its folder of files and the SHA-256 of its `SHA256SUMS`.
#[derive(Debug, Clone, Deserialize)]
pub struct MemoryPin {
    pub version: String,
    pub base: String,
    pub sums_sha256: String,
    pub asset: String,
}

/// One file of the model.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelFile {
    pub url: String,
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

/// An archive of the model's runtime, unpacked into `<runtime_dir>/<into>`.
#[derive(Debug, Clone, Deserialize)]
pub struct Runtime {
    pub url: String,
    pub sha256: String,
    pub into: String,
}

/// The model clio-memory and Clio load: its folder (relative to the home), files and runtime.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelPin {
    pub dir: String,
    pub files: Vec<ModelFile>,
    pub runtime_dir: String,
    pub runtime: Vec<Runtime>,
}

/// Everything the installer downloads besides Clio itself, pinned in this binary.
#[derive(Debug, Clone, Deserialize)]
pub struct Pins {
    pub node: Option<Archive>,
    pub memory: Option<MemoryPin>,
    pub model: Option<ModelPin>,
}

impl Pins {
    /// What this binary pins for this system.
    pub fn real() -> Self {
        let node_os = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => Some("linux-x64"),
            ("linux", "aarch64") => Some("linux-arm64"),
            ("macos", "aarch64") => Some("darwin-arm64"),
            ("macos", "x86_64") => Some("darwin-x64"),
            _ => None,
        };
        let node = node_os.and_then(|os| {
            let (_, sha) = NODE_SUMS.iter().find(|(p, _)| *p == os)?;
            Some(Archive {
                url: format!(
                    "https://nodejs.org/dist/v{NODE_VERSION}/node-v{NODE_VERSION}-{os}.tar.gz"
                ),
                sha256: (*sha).into(),
                version: NODE_VERSION.into(),
            })
        });
        let memory = target().map(|t| MemoryPin {
            version: MEMORY_VERSION.into(),
            base: format!(
                "https://github.com/i-montes/clio-memory/releases/download/v{MEMORY_VERSION}"
            ),
            sums_sha256: MEMORY_SUMS_SHA256.into(),
            asset: format!("clio-memory-{t}"),
        });
        Self {
            node,
            memory,
            model: Some(model_pin()),
        }
    }

    /// The real pins; in a debug build, `CLIO_TEST_PINS` (a JSON file) replaces them, so the
    /// tests of the whole binary use local fakes. Release builds never read it.
    pub fn from_env() -> Self {
        if cfg!(debug_assertions)
            && let Some(p) = std::env::var_os("CLIO_TEST_PINS")
            && let Ok(text) = std::fs::read_to_string(p)
            && let Ok(pins) = serde_json::from_str(&text)
        {
            return pins;
        }
        Self::real()
    }
}

/// The model clio-memory's own installer puts on this system: Granite and its CPU runtime where
/// one is published, the static model elsewhere (`clio_memory::setup::model_choice`).
fn model_pin() -> ModelPin {
    use clio_memory::setup::model_choice::{ModelChoice, default_choice};
    use clio_memory::setup::{GRANITE, POTION, RUNTIME_DIR, runtime_here};
    let granite =
        default_choice(std::env::consts::OS, std::env::consts::ARCH) == ModelChoice::Granite;
    let spec = if granite { &GRANITE } else { &POTION };
    ModelPin {
        dir: spec.dir.into(),
        files: spec
            .files
            .iter()
            .map(|f| ModelFile {
                url: spec.url(f.local).unwrap_or_default(),
                name: f.local.into(),
                sha256: f.sha256.into(),
                size: f.size,
            })
            .collect(),
        runtime_dir: RUNTIME_DIR.into(),
        runtime: if granite {
            runtime_here()
                .into_iter()
                .map(|a| Runtime {
                    url: a.url.into(),
                    sha256: a.sha256.into(),
                    into: a.into.into(),
                })
                .collect()
        } else {
            Vec::new()
        },
    }
}

// ---- downloading -------------------------------------------------------------------------------

/// The last part of a URL or path: what a message names.
pub fn name_of(src: &str) -> String {
    src.rsplit('/').next().unwrap_or(src).to_owned()
}

/// Kills and reaps the download when dropped: an early return never leaves one running.
struct Reap(std::process::Child);

impl Drop for Reap {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Downloads `src` (a URL; a path is copied) to `to`; `on` sees the bytes so far, every 100 ms.
/// Over https a redirect may not leave https. Nothing secret is ever part of a URL.
pub fn get(src: &str, to: &Path, on: &mut dyn FnMut(u64)) -> FRes<()> {
    let net = || Fail::Net(name_of(src));
    if !(src.starts_with("https://") || src.starts_with("http://")) {
        std::fs::copy(src, to).map_err(|_| net())?;
        on(std::fs::metadata(to).map_or(0, |m| m.len()));
        return Ok(());
    }
    let path = std::env::var("PATH").unwrap_or_default();
    let https = src.starts_with("https://");
    let find = |n| clio_brain::pi::install::find_in_path(n, &path);
    let mut cmd = if let Some(curl) = find("curl") {
        let mut c = Command::new(curl);
        // Stalled for a minute is a failure, not a wait forever.
        c.args([
            "-fsSL",
            "--retry",
            "3",
            "--connect-timeout",
            "20",
            "-y",
            "60",
            "-Y",
            "1",
        ]);
        if https {
            c.args(["--proto", "=https", "--proto-redir", "=https"]);
        }
        c.arg("-o").arg(to).arg(src);
        c
    } else if let Some(wget) = find("wget") {
        let mut c = Command::new(wget);
        c.args(["-q", "--tries=3", "--timeout=60"]);
        if https {
            c.arg("--https-only");
        }
        c.arg("-O").arg(to).arg(src);
        c
    } else {
        return Err(Fail::Other("curl or wget is needed to download".into()));
    };
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| net())?;
    let mut child = Reap(child);
    let size = || std::fs::metadata(to).map_or(0, |m| m.len());
    loop {
        match child.0.try_wait() {
            Ok(Some(st)) if st.success() => {
                on(size());
                return Ok(());
            }
            Ok(Some(_)) | Err(_) => return Err(net()),
            Ok(None) => {
                on(size());
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// The SHA-256 of a file, read in pieces; `None` when it cannot be read.
pub fn sha256_file(p: &Path) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let (mut h, mut buf) = (Sha256::new(), vec![0u8; 1 << 16]);
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The sum `SHA256SUMS` lists for `name` (`<sum>  <name>` or `<sum> *<name>`).
pub fn sum_in(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (sum, file) = l.split_once(char::is_whitespace)?;
        (file.trim_start().trim_start_matches('*') == name
            && sum.len() == 64
            && sum.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| sum.to_ascii_lowercase())
    })
}

/// Downloads `src` to `to` and keeps it only when its SHA-256 is `want`; otherwise `to` is gone.
pub fn get_verified(src: &str, to: &Path, want: &str, on: &mut dyn FnMut(u64)) -> FRes<()> {
    let r = get(src, to, on).and_then(|()| {
        if sha256_file(to).as_deref() == Some(want) {
            Ok(())
        } else {
            Err(Fail::Sum(name_of(src)))
        }
    });
    if r.is_err() {
        let _ = std::fs::remove_file(to);
    }
    r
}

/// A small text file (`VERSION`, `SHA256SUMS`), downloaded through `tmp`.
fn get_text(src: &str, tmp: &Path) -> FRes<String> {
    let to = tmp.join(format!(".{}.{}", name_of(src), std::process::id()));
    let r = get(src, &to, &mut |_| {})
        .and_then(|()| std::fs::read_to_string(&to).map_err(|_| Fail::Net(name_of(src))));
    let _ = std::fs::remove_file(&to);
    r
}

/// `x.y.z` to numbers, for comparing versions.
pub fn version(v: &str) -> Option<(u32, u32, u32)> {
    clio_brain::pi::install::parse_node_version(v)
}

// ---- Clio's own release ------------------------------------------------------------------------

/// A release of Clio: where its files are and its version.
#[derive(Debug, Clone)]
pub struct Release {
    pub base: String,
    pub version: String,
}

impl Release {
    /// The latest release (its `VERSION` names it), or the one `CLIO_RELEASE_BASE` points at (a
    /// URL or a folder with the same file names). `tmp`: a folder to download into.
    pub fn find(tmp: &Path) -> FRes<Self> {
        let over = std::env::var("CLIO_RELEASE_BASE")
            .ok()
            .filter(|b| !b.is_empty());
        let (base, version_at) = match over {
            Some(b) => {
                let b = b.trim_end_matches('/').to_owned();
                (Some(b.clone()), format!("{b}/VERSION"))
            }
            None => (None, format!("{RELEASES}/latest/download/VERSION")),
        };
        let v = get_text(&version_at, tmp)?.trim().to_owned();
        if version(&v).is_none() {
            return Err(Fail::Other(format!(
                "the release's VERSION is not x.y.z: {}",
                crate::cli::tui::sanitize::line(&v)
            )));
        }
        Ok(Self {
            // Each file of the same release, never `latest` again: a release published while
            // this one downloads cannot mix two.
            base: base.unwrap_or_else(|| format!("{RELEASES}/download/v{v}")),
            version: v,
        })
    }

    pub fn file(&self, name: &str) -> String {
        format!("{}/{name}", self.base)
    }

    /// Whether this release is newer than the running Clio.
    pub fn is_newer(&self) -> bool {
        version(&self.version) > version(env!("CARGO_PKG_VERSION"))
    }
}

/// Clio's two binaries, in the order they are put in place.
pub const BINARIES: [&str; 2] = ["clio-vault", "clio"];

/// Puts `rel`'s `clio-vault` and `clio` in `bin`. Both are downloaded beside their destination and
/// checked against the release's `SHA256SUMS` before either is replaced; then each is renamed
/// over the old one (an atomic swap of one file), with the terminal's signals held off between
/// the two. Returns whether anything changed. `on`: bytes so far and the total, per file.
pub fn install_binaries(rel: &Release, bin: &Path, on: &mut dyn FnMut(&str, u64)) -> FRes<bool> {
    let t = target().ok_or_else(|| Fail::Other("Clio has no build for this system".into()))?;
    std::fs::create_dir_all(bin).map_err(|e| crate::cli::io_err(bin, &e))?;
    let sums = get_text(&rel.file("SHA256SUMS"), bin)?;
    let mut staged: Vec<(PathBuf, PathBuf)> = Vec::new();
    let clean = |staged: &[(PathBuf, PathBuf)]| {
        for (s, _) in staged {
            let _ = std::fs::remove_file(s);
        }
    };
    for name in BINARIES {
        let asset = format!("{name}-{t}");
        let Some(want) = sum_in(&sums, &asset) else {
            clean(&staged);
            return Err(Fail::Sum(asset));
        };
        let dest = bin.join(name);
        if sha256_file(&dest).as_deref() == Some(want.as_str()) {
            continue;
        }
        let stage = bin.join(format!(".{name}.new"));
        if let Err(e) = get_verified(&rel.file(&asset), &stage, &want, &mut |n| on(name, n)) {
            clean(&staged);
            return Err(e);
        }
        if let Err(e) = crate::cli::set_mode(&stage, 0o755) {
            staged.push((stage, dest));
            clean(&staged);
            return Err(e.into());
        }
        staged.push((stage, dest));
    }
    if staged.is_empty() {
        return Ok(false);
    }
    // Between the two renames a Ctrl-C, a hangup or a TERM waits: only a kill could split them.
    let held = Hold::new();
    for (stage, dest) in &staged {
        std::fs::rename(stage, dest).map_err(|e| Fail::Other(crate::cli::io_err(dest, &e)))?;
    }
    drop(held);
    Ok(true)
}

/// While alive, SIGINT, SIGTERM and SIGHUP are caught and dropped.
struct Hold(Vec<signal_hook::SigId>);

impl Hold {
    fn new() -> Self {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        Self(
            [
                signal_hook::consts::SIGINT,
                signal_hook::consts::SIGTERM,
                signal_hook::consts::SIGHUP,
            ]
            .into_iter()
            .filter_map(|s| signal_hook::flag::register(s, flag.clone()).ok())
            .collect(),
        )
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        for id in self.0.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

// ---- Node, clio-memory and the model -----------------------------------------------------------

/// The file inside the private Node that says which archive it came from.
const NODE_MARK: &str = ".clio-node";

/// The folder of the private Node: `<data>/node` (`bin/node` inside).
pub fn node_dir(data: &Path) -> PathBuf {
    data.join("node")
}

/// The version of the private Node installed, if any.
pub fn node_installed(data: &Path) -> Option<String> {
    let mark = std::fs::read_to_string(node_dir(data).join(NODE_MARK)).ok()?;
    mark.split_whitespace().next().map(str::to_owned)
}

/// Puts the archive `a` in `<data>/node`, unless that is already it: downloaded, checked,
/// unpacked beside it with the system's `tar`, made private, and only then swapped in. Returns
/// the version it replaced (`Some("")` when there was none), `None` when nothing changed.
pub fn install_node(data: &Path, a: &Archive, on: &mut dyn FnMut(u64)) -> FRes<Option<String>> {
    let dir = node_dir(data);
    let mark = format!("{} {}", a.version, a.sha256);
    if std::fs::read_to_string(dir.join(NODE_MARK)).ok().as_deref() == Some(mark.as_str())
        && dir.join("bin/node").is_file()
    {
        return Ok(None);
    }
    let old = node_installed(data).unwrap_or_default();
    crate::cli::mkdir_private(data)?;
    let file = data.join(".node.download");
    let staging = data.join(".node.new");
    let _ = std::fs::remove_dir_all(&staging);
    let r = get_verified(&a.url, &file, &a.sha256, on).and_then(|()| {
        crate::cli::mkdir_private(&staging)?;
        unpack(&file, &staging)?;
        if !staging.join("bin/node").is_file() {
            return Err(Fail::Other(format!("{} has no bin/node", name_of(&a.url))));
        }
        std::fs::write(staging.join(NODE_MARK), &mark)
            .map_err(|e| crate::cli::io_err(&staging, &e))?;
        clio_brain::pi::install::lock_down(&staging).map_err(|e| Fail::Other(e.to_string()))
    });
    let _ = std::fs::remove_file(&file);
    if let Err(e) = r {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    swap_dir(&staging, &dir)?;
    Ok(Some(old))
}

/// Puts the folder `new` where `dir` is: the old one is moved aside first and removed after.
fn swap_dir(new: &Path, dir: &Path) -> FRes<()> {
    let aside = dir.with_extension("old");
    let _ = std::fs::remove_dir_all(&aside);
    if dir.exists() {
        std::fs::rename(dir, &aside).map_err(|e| crate::cli::io_err(dir, &e))?;
    }
    std::fs::rename(new, dir).map_err(|e| crate::cli::io_err(dir, &e))?;
    let _ = std::fs::remove_dir_all(&aside);
    Ok(())
}

/// Unpacks a `.tar.gz` (or `.tgz`) into `into`, without its top folder.
fn unpack(file: &Path, into: &Path) -> FRes<()> {
    let ok = Command::new("tar")
        .arg("-xzf")
        .arg(file)
        .args(["--strip-components=1", "-C"])
        .arg(into)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        Ok(())
    } else {
        Err(Fail::Other(format!(
            "could not unpack {}",
            name_of(&file.display().to_string())
        )))
    }
}

/// The clio-memory installed in `bin`, by what `--version` says.
pub fn memory_installed(bin: &Path) -> Option<String> {
    let out = Command::new(bin.join("clio-memory"))
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let v = text.split_whitespace().nth(1)?;
    version(v).map(|_| v.to_owned())
}

/// Puts clio-memory `m` in `bin`, unless the one there is it or newer (the person may have
/// updated it on their own): its `SHA256SUMS` must have the pinned SHA-256, the binary must be
/// what it lists, and it is renamed into place. Returns the version replaced, as `install_node`.
pub fn install_memory(bin: &Path, m: &MemoryPin, on: &mut dyn FnMut(u64)) -> FRes<Option<String>> {
    let have = memory_installed(bin);
    if have.as_deref().and_then(version) >= version(&m.version) {
        return Ok(None);
    }
    std::fs::create_dir_all(bin).map_err(|e| crate::cli::io_err(bin, &e))?;
    let sums_src = format!("{}/SHA256SUMS", m.base.trim_end_matches('/'));
    let sums_file = bin.join(".clio-memory.SHA256SUMS");
    get_verified(&sums_src, &sums_file, &m.sums_sha256, &mut |_| {})?;
    let sums = std::fs::read_to_string(&sums_file).unwrap_or_default();
    let _ = std::fs::remove_file(&sums_file);
    let want = sum_in(&sums, &m.asset).ok_or_else(|| Fail::Sum(m.asset.clone()))?;
    let stage = bin.join(".clio-memory.new");
    let src = format!("{}/{}", m.base.trim_end_matches('/'), m.asset);
    get_verified(&src, &stage, &want, on)?;
    crate::cli::set_mode(&stage, 0o755)?;
    std::fs::rename(&stage, bin.join("clio-memory"))
        .map_err(|e| crate::cli::io_err(&bin.join("clio-memory"), &e))?;
    Ok(Some(have.unwrap_or_default()))
}

/// The model's files and runtime under `home`, each kept when its SHA-256 is already right (the
/// runtime by the mark clio-memory's installer leaves, `.sha256`). `on`: bytes of this call so
/// far, out of [`model_size`]. Returns whether anything was downloaded.
pub fn install_model(home: &Path, m: &ModelPin, on: &mut dyn FnMut(u64)) -> FRes<bool> {
    let dir = home.join(&m.dir);
    std::fs::create_dir_all(&dir).map_err(|e| crate::cli::io_err(&dir, &e))?;
    let mut done = 0u64;
    let mut changed = false;
    for f in &m.files {
        let target = dir.join(&f.name);
        if sha256_file(&target).as_deref() != Some(f.sha256.as_str()) {
            let tmp = dir.join(format!(".{}.download", f.name));
            get_verified(&f.url, &tmp, &f.sha256, &mut |n| on(done + n))?;
            std::fs::rename(&tmp, &target).map_err(|e| crate::cli::io_err(&target, &e))?;
            changed = true;
        }
        done += f.size;
        on(done);
    }
    let root = home.join(&m.runtime_dir);
    for r in &m.runtime {
        let into = root.join(&r.into);
        if std::fs::read_to_string(into.join(".sha256"))
            .ok()
            .as_deref()
            == Some(r.sha256.as_str())
        {
            continue;
        }
        std::fs::create_dir_all(&root).map_err(|e| crate::cli::io_err(&root, &e))?;
        let file = root.join(format!(".{}.download", r.into));
        let staging = root.join(format!(".{}.new", r.into));
        let _ = std::fs::remove_dir_all(&staging);
        let res = get_verified(&r.url, &file, &r.sha256, &mut |n| on(done + n)).and_then(|()| {
            std::fs::create_dir_all(&staging).map_err(|e| crate::cli::io_err(&staging, &e))?;
            unpack(&file, &staging)?;
            std::fs::write(staging.join(".sha256"), &r.sha256)
                .map_err(|e| Fail::Other(crate::cli::io_err(&staging, &e)))
        });
        let _ = std::fs::remove_file(&file);
        if let Err(e) = res {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
        swap_dir(&staging, &into)?;
        changed = true;
    }
    Ok(changed)
}

/// What the model's files weigh (the runtime is not counted: its size is not pinned).
pub fn model_size(m: &ModelPin) -> u64 {
    m.files.iter().map(|f| f.size).sum()
}
