//! MCP servers (stage 4, S-70): someone else's program from npm, at an exact version, that the
//! person approved. Downloaded, checked, probed, turned on and set only here, in the person's own
//! `clio` process: never from the jail, never by the tool host (which only starts one that is on,
//! through `Port`, and flags a tool that changed). `<data>/mcp` is laid out as the services'
//! folder (`skills.rs`, `Kind::Mcp`: the same lock, versions, records and `removed/`):
//!
//! - `versions/<name>/<n>/mcp.json`: what the person approved, byte for byte.
//! - `records/<name>.json`: on or off (and why), current version.
//! - `servers/<name>/<version>/`: the installed package (`npm install --ignore-scripts` of exactly
//!   `package@version`), its lockfile; read-only to the server, its digest checked at each start.
//! - `state/<name>.json`: the installed version, its entry point, its tree's digest, the
//!   integrity npm recorded, whether the person confirmed its tools, and the tools with modes.
//! - `data/<name>/` (0700): the only folder of Clio's a server may write, its `HOME`.
//! - `logs/<name>.json`: its last starts and calls (never arguments or results).
//! - `npm-cache/`: npm's cache. `notices.json`: what the person is told when a chat opens.
//!
//! npm runs in a service's jail too: the home hidden but its own folders, the network only to
//! the registry through the allow-list proxy, no install scripts. A server runs in the same jail
//! (`services::Runner::prepare`): its declared hosts, its own secrets from the vault bound to
//! `mcp:<host>`, on its standard input.

use std::fs;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use clio_node::HostJail;
use clio_toolhost::mcp::{self as m, Manifest, McpPort, Mode, Proc, State, Tool};
use clio_toolhost::pending::Pending;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::home::Dirs;
use super::services::{Prepared, Runner, Spec, quote};
use super::skills::{self, Kind};
use super::tui::sanitize;
use super::{Res, io_err, mkdir_private, write_private};

/// Why a server is off when its installed files changed.
pub const OFF_CHANGED: &str = "off: its installed files changed after it was approved";
/// npm's time for the facts, and for an install.
const VIEW_SECS: u64 = 60;
const INSTALL_SECS: u64 = 300;
/// What is read of npm's answer.
const NPM_OUTPUT: usize = 1 << 20;
/// A probe: its start and the tool list (no longer than a call may take).
const PROBE: Duration = Duration::from_secs(60);
/// An installed tree with more entries than this is refused.
const MAX_ENTRIES: usize = 50_000;
/// A long-lived server's processor-time budget, per process when no scope starts.
const RUN_BUDGET_SECS: u64 = 3600;

static NPM_RUNS: AtomicU32 = AtomicU32::new(0);

pub fn root(d: &Dirs) -> PathBuf {
    d.data.join("mcp")
}

fn tree_dir(root: &Path, name: &str, version: &str) -> PathBuf {
    root.join("servers").join(name).join(version)
}

fn data_dir(root: &Path, name: &str) -> PathBuf {
    root.join("data").join(name)
}

/// The digest of everything under `dir`: each entry's path, kind, mode, and content (a link: its
/// target, never followed). Sorted, so the same tree gives the same digest.
pub fn tree_digest(dir: &Path) -> Res<String> {
    fn walk(base: &Path, rel: &Path, h: &mut Sha256, n: &mut usize) -> Res<()> {
        let at = base.join(rel);
        let mut entries: Vec<_> = fs::read_dir(&at)
            .map_err(|e| io_err(&at, &e))?
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect();
        entries.sort();
        for name in entries {
            *n += 1;
            if *n > MAX_ENTRIES {
                return Err(format!("more than {MAX_ENTRIES} files: not installed"));
            }
            let r = rel.join(&name);
            let p = base.join(&r);
            let md = fs::symlink_metadata(&p).map_err(|e| io_err(&p, &e))?;
            h.update(r.as_os_str().as_encoded_bytes());
            h.update([0]);
            h.update(format!("{:o}\0", md.mode() & 0o7777).as_bytes());
            if md.is_symlink() {
                let t = fs::read_link(&p).map_err(|e| io_err(&p, &e))?;
                h.update(b"l");
                h.update(t.as_os_str().as_encoded_bytes());
            } else if md.is_dir() {
                h.update(b"d");
                walk(base, &r, h, n)?;
            } else if md.is_file() {
                h.update(b"f");
                let mut f = fs::File::open(&p).map_err(|e| io_err(&p, &e))?;
                let mut buf = [0u8; 65536];
                loop {
                    let k = f.read(&mut buf).map_err(|e| io_err(&p, &e))?;
                    if k == 0 {
                        break;
                    }
                    h.update(&buf[..k]);
                }
            } else {
                return Err(format!("{} is not a file, a folder or a link", p.display()));
            }
            h.update([0]);
        }
        Ok(())
    }
    let mut h = Sha256::new();
    let mut n = 0;
    walk(dir, Path::new(""), &mut h, &mut n)?;
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// What a server needs from the person's side: the folders, the launcher (bwrap), Node and npm,
/// the `clio` binary (the forwarder inside the jail is `clio pi-net`), the vault's socket.
pub struct Port {
    pub dirs: Dirs,
    pub jail: HostJail,
    pub node: PathBuf,
    /// npm's own script (`npm-cli.js`), run by `node`.
    pub npm: PathBuf,
    pub exe: PathBuf,
    pub gateway: Option<PathBuf>,
}

impl Port {
    /// `node`: the Node Clio runs Pi with (found on `PATH` when not given). npm is the one next
    /// to it.
    pub fn new(dirs: Dirs, node: Option<PathBuf>) -> Res<Self> {
        let node = match node {
            Some(n) => n,
            None => {
                clio_brain::pi::install::find_node(&std::env::var("PATH").unwrap_or_default())
                    .map_err(|e| e.to_string())?
                    .path
            }
        };
        let node = node.canonicalize().map_err(|e| io_err(&node, &e))?;
        let npm = node.with_file_name("npm");
        let npm = npm
            .canonicalize()
            .map_err(|e| format!("npm next to Node ({}): {e}", npm.display()))?;
        Ok(Self {
            gateway: super::vault::gateway_socket(&dirs).ok(),
            jail: super::services::service_jail()?,
            exe: std::env::current_exe().map_err(|e| e.to_string())?,
            dirs,
            node,
            npm,
        })
    }

    fn runner(&self) -> Res<Runner<'_>> {
        let rt = self.dirs.need_runtime()?.to_path_buf();
        mkdir_private(&rt)?;
        Ok(Runner {
            dirs: &self.dirs,
            jail: &self.jail,
            exe: self.exe.clone(),
            gateway: self.gateway.clone(),
            proxy_parent: rt,
        })
    }

    /// Node's and npm's folders: shown read-only when the home hides them.
    fn tool_dirs(&self) -> Vec<PathBuf> {
        let up = |p: &Path, n: usize| {
            let mut p = p.to_path_buf();
            for _ in 0..n {
                p = p.parent().map(Path::to_path_buf).unwrap_or(p);
            }
            p
        };
        vec![up(&self.node, 2), up(&self.npm, 2)]
    }

    /// npm, in a service's jail: `args` in `cwd`, with a home and a config of its own, the
    /// cache in Clio's data, the network only to the registry, no scripts. Its standard output
    /// (cut at `NPM_OUTPUT`), or why it failed.
    fn npm(&self, args: &[&str], cwd: Option<&Path>, also: &[PathBuf], secs: u64) -> Res<String> {
        let root = root(&self.dirs);
        let home = root.join("npm").join(format!(
            "home-{}-{}",
            std::process::id(),
            NPM_RUNS.fetch_add(1, Ordering::SeqCst)
        ));
        let cache = root.join("npm-cache");
        let _ = fs::remove_dir_all(&home);
        mkdir_private(&home)?;
        mkdir_private(&cache)?;
        let rc = home.join(".npmrc");
        write_private(
            &rc,
            format!(
                "registry=https://{}/\nignore-scripts=true\naudit=false\nfund=false\nupdate-notifier=false\n",
                m::REGISTRY
            )
            .as_bytes(),
        )?;
        let mut command = format!(
            "exec {} {}",
            quote(&self.node.display().to_string()),
            quote(&self.npm.display().to_string())
        );
        for a in args {
            command.push(' ');
            command.push_str(&quote(a));
        }
        let mut own = vec![home.clone(), cache.clone()];
        own.extend(also.iter().cloned());
        let proxy = clio_brain::pi::net::PROXY_URL;
        let env: Vec<(String, String)> = [
            ("npm_config_userconfig", rc.display().to_string()),
            ("npm_config_globalconfig", rc.display().to_string()),
            ("npm_config_cache", cache.display().to_string()),
            ("npm_config_registry", format!("https://{}/", m::REGISTRY)),
            ("npm_config_ignore_scripts", "true".into()),
            ("npm_config_update_notifier", "false".into()),
            ("npm_config_https_proxy", proxy.into()),
            ("npm_config_proxy", proxy.into()),
        ]
        .map(|(k, v)| (k.to_owned(), v))
        .to_vec();
        let network = [m::REGISTRY.to_owned()];
        let spec = Spec {
            tool: "mcp",
            name: "npm",
            command: &command,
            stdin: false,
            cwd: cwd.unwrap_or(&home).to_path_buf(),
            home: home.clone(),
            network: &network,
            secrets: &[],
            reads: &[],
            writes: &[],
            own,
            shows: self.tool_dirs(),
            env,
            cpu_percent: 100,
            memory_mb: 1024,
            seconds: secs,
        };
        let out = (|| {
            let (mut job, input, _, proxy) = match self.runner()?.prepare(&spec)? {
                Prepared::Ready {
                    job,
                    input,
                    scrub,
                    proxy,
                } => (job, input, scrub, proxy),
                Prepared::Wait(why) => return Err(why),
            };
            job.job.max_output = NPM_OUTPUT;
            let out = self.jail.run_service(&job, input, &|| false);
            drop(proxy);
            let (out, _) = out?;
            if out.timed_out {
                return Err(format!("npm took more than {secs} s"));
            }
            if out.exit_code != Some(0) {
                let tail: String = out.stderr.chars().rev().take(400).collect();
                let tail: String = tail.chars().rev().collect();
                return Err(format!(
                    "npm failed ({}): {}",
                    out.exit_code.map_or("signal".into(), |c| c.to_string()),
                    sanitize::line(&tail)
                ));
            }
            Ok(out.stdout)
        })();
        let _ = fs::remove_dir_all(&home);
        out
    }

    /// The jail a server starts in: no secrets and no network for a `probe`.
    fn spawn(&self, mf: &Manifest, st: &State, probe: bool) -> Res<Proc> {
        let root = root(&self.dirs);
        let tree = tree_dir(&root, &mf.name, &st.version);
        let data = data_dir(&root, &mf.name);
        mkdir_private(&data)?;
        let entry = tree.join(&st.entry);
        // Without the trap handler V8 reserves no guard regions for Wasm: under a per-process
        // address-space limit (`prlimit --as`) `fetch` (undici's Wasm parser) works.
        let command = format!(
            "exec {} --disable-wasm-trap-handler {}",
            quote(&self.node.display().to_string()),
            quote(&entry.display().to_string())
        );
        let none: [String; 0] = [];
        let mut shows = vec![tree.clone()];
        shows.extend(self.tool_dirs());
        let spec = Spec {
            tool: "mcp",
            name: &mf.name,
            command: &command,
            stdin: true,
            cwd: tree,
            home: data.clone(),
            network: if probe { &none } else { &mf.network },
            secrets: if probe { &[] } else { &mf.secrets },
            reads: if probe { &none } else { &mf.reads },
            writes: if probe { &none } else { &mf.writes },
            own: vec![data],
            shows,
            env: vec![
                ("CLIO_MCP_SERVER".into(), mf.name.clone()),
                ("NODE_USE_ENV_PROXY".into(), "1".into()),
            ],
            cpu_percent: mf.limits.cpu_percent,
            memory_mb: mf.limits.memory_mb,
            seconds: RUN_BUDGET_SECS,
        };
        let (job, input, scrub, proxy) = match self.runner()?.prepare(&spec)? {
            Prepared::Ready {
                job,
                input,
                scrub,
                proxy,
            } => (job, input, scrub, proxy),
            Prepared::Wait(why) => return Err(why),
        };
        let (child, _) = self.jail.spawn_service(&job)?;
        Ok(Proc {
            child,
            keep: proxy.map(|p| Box::new(p) as Box<dyn Send>),
            scrub,
            input,
        })
    }

    /// Installs, checks and probes the server the person approved (`p`, set aside by
    /// `mcp_propose`), and registers it off: what the person is told.
    pub fn install(&self, p: &Pending) -> Res<String> {
        let (name, _, manifest, _) = clio_toolhost::pending::mcp_of(p)
            .ok_or_else(|| format!("{} is not an MCP server", p.id))?;
        if p.args["sha256"].as_str() != Some(&clio_toolhost::skills::digest(manifest, &[])) {
            return Err(format!(
                "{} is not what was shown: nothing was installed",
                p.id
            ));
        }
        let mf = m::check_stored(name, manifest, &[])?;
        let integrity = p.args["npm"]["integrity"].as_str().unwrap_or("");
        let root = root(&self.dirs);
        let control = self.dirs.control();
        for w in &mf.writes {
            if let Some(why) = clio_toolhost::services::write_refused(w, &self.dirs.home, &control)
            {
                return Err(format!("writes: {why}"));
            }
        }
        let parent = root.join("servers").join(&mf.name);
        mkdir_private(&parent)?;
        let stage = parent.join(format!(".stage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&stage);
        mkdir_private(&stage)?;
        let done = (|| -> Res<(String, String)> {
            write_private(&stage.join("package.json"), b"{\"private\": true}\n")?;
            let spec = format!("{}@{}", mf.package, mf.version);
            self.npm(
                &[
                    "install",
                    "--ignore-scripts",
                    "--no-audit",
                    "--no-fund",
                    "--save-exact",
                    "--loglevel=error",
                    &spec,
                ],
                Some(&stage),
                std::slice::from_ref(&stage),
                INSTALL_SECS,
            )?;
            check_lock(&stage, &mf.package, &mf.version, integrity)?;
            let entry = entry_of(&stage, &mf.package)?;
            let tree = tree_digest(&stage)?;
            let to = tree_dir(&root, &mf.name, &mf.version);
            let _ = fs::remove_dir_all(&to);
            fs::rename(&stage, &to).map_err(|e| io_err(&to, &e))?;
            Ok((entry, tree))
        })();
        let (entry, tree) = done.inspect_err(|_| {
            let _ = fs::remove_dir_all(&stage);
        })?;
        let mut st = State {
            name: mf.name.clone(),
            package: mf.package.clone(),
            version: mf.version.clone(),
            entry,
            tree,
            integrity: integrity.to_owned(),
            confirmed: false,
            tools: vec![],
        };
        // The probe: no secrets, no network; only `initialize` and `tools/list`.
        let probed = self
            .spawn(&mf, &st, true)
            .and_then(|p| m::Server::start(p, PROBE.min(Duration::from_secs(mf.limits.seconds))));
        let tools = match probed {
            Ok(s) => s.tools.clone(),
            Err(e) => {
                let _ = fs::remove_dir_all(tree_dir(&root, &mf.name, &mf.version));
                m::log_event(&root, &mf.name, "probe", "failed", &e);
                return Err(format!(
                    "{}@{} was downloaded, but it did not start and list its tools (without network or secrets): {e}. Nothing was registered",
                    mf.package, mf.version
                ));
            }
        };
        st.tools = m::first_modes(tools);
        {
            let _l = m::lock_root(&root)?;
            m::save_state(&root, &st)?;
        }
        skills::register_in(
            &root,
            Kind::Mcp,
            &skills::New {
                name: &mf.name,
                description: &mf.description,
                skill_md: manifest,
                files: &[],
                by: "person",
                verdict: "approved",
            },
        )?;
        m::log_event(&root, &mf.name, "install", "ok", "");
        Ok(format!(
            "the MCP server «{}» ({}@{}) is installed; it offers {} tool{}. It stays off until you confirm them: /mcp confirm {}",
            mf.name,
            mf.package,
            mf.version,
            st.tools.len(),
            if st.tools.len() == 1 { "" } else { "s" },
            mf.name
        ))
    }
}

/// The lockfile npm wrote: the package at exactly the approved version and integrity, and every
/// package a tarball from the registry with an integrity (no git, no link, no file, no URL).
fn check_lock(stage: &Path, package: &str, version: &str, integrity: &str) -> Res<()> {
    let p = stage.join("package-lock.json");
    let lock: Value = serde_json::from_str(&fs::read_to_string(&p).map_err(|e| io_err(&p, &e))?)
        .map_err(|e| format!("package-lock.json: {e}"))?;
    let pkgs = lock["packages"]
        .as_object()
        .ok_or("package-lock.json has no packages")?;
    let me = &pkgs
        .get(&format!("node_modules/{package}"))
        .ok_or_else(|| format!("npm did not install {package}"))?;
    if me["version"] != version || me["integrity"] != integrity {
        return Err(format!(
            "npm installed {package} {} with integrity {}, not {version} with {integrity}: not installed",
            sanitize::line(me["version"].as_str().unwrap_or("?")),
            sanitize::line(me["integrity"].as_str().unwrap_or("none"))
        ));
    }
    let registry = format!("https://{}/", m::REGISTRY);
    for (k, v) in pkgs {
        if k.is_empty() {
            continue;
        }
        let from_registry = v["resolved"]
            .as_str()
            .is_some_and(|r| r.starts_with(&registry));
        if v["link"] == true || !from_registry || !v["integrity"].is_string() {
            return Err(format!(
                "{} does not come from the registry with an integrity: not installed",
                sanitize::line(k)
            ));
        }
    }
    Ok(())
}

/// The file `node` starts: the package's only `bin` (or the one named after it), else its
/// `main`, else `index.js`; inside the package, a plain file. Relative to `stage`.
fn entry_of(stage: &Path, package: &str) -> Res<String> {
    let dir = stage.join("node_modules").join(package);
    let pj = dir.join("package.json");
    let v: Value = serde_json::from_str(&fs::read_to_string(&pj).map_err(|e| io_err(&pj, &e))?)
        .map_err(|e| format!("{package}'s package.json: {e}"))?;
    let short = package.rsplit('/').next().unwrap_or(package);
    let rel = match &v["bin"] {
        Value::String(s) => s.clone(),
        Value::Object(o) if o.len() == 1 => o
            .values()
            .next()
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        Value::Object(o) => o
            .get(short)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                format!("{package} has several programs and none named {short}: not installed")
            })?
            .to_owned(),
        _ => v["main"].as_str().unwrap_or("index.js").to_owned(),
    };
    let base = dir.canonicalize().map_err(|e| io_err(&dir, &e))?;
    let at = dir.join(&rel);
    let real = at.canonicalize().map_err(|e| io_err(&at, &e))?;
    if !real.starts_with(&base) || !real.is_file() {
        return Err(format!(
            "{package}'s entry point {} is not a file inside it: not installed",
            sanitize::line(&rel)
        ));
    }
    let stage = stage.canonicalize().map_err(|e| io_err(stage, &e))?;
    real.strip_prefix(&stage)
        .map(|p| p.display().to_string())
        .map_err(|_| "the entry point is outside the install".into())
}

impl McpPort for Port {
    fn root(&self) -> PathBuf {
        root(&self.dirs)
    }

    fn npm_view(&self, package: &str, version: &str) -> Result<Value, String> {
        if !m::valid_package(package) || !m::exact_version(version) {
            return Err("not an npm package at an exact version".into());
        }
        let out = self.npm(
            &["view", &format!("{package}@{version}"), "--json"],
            None,
            &[],
            VIEW_SECS,
        )?;
        serde_json::from_str(&out).map_err(|_| "npm's answer is not JSON".into())
    }

    fn start(&self, name: &str) -> Result<Proc, String> {
        let root = root(&self.dirs);
        if !m::is_on(&root, name) {
            return Err(format!("«{name}» is not on"));
        }
        let mf = m::current_manifest(&root, name).ok_or("its approved manifest does not read")?;
        let st = m::load_state(&root, name).ok_or("its state does not read")?;
        if st.package != mf.package || st.version != mf.version {
            return Err("what is installed is not what was approved".into());
        }
        // Before each start: the files as they were installed, or it does not start, and it is
        // turned off.
        let now = tree_digest(&tree_dir(&root, name, &st.version));
        if now.as_deref() != Ok(st.tree.as_str()) {
            let _ = skills::set_enabled_in(&root, Kind::Mcp, name, false, OFF_CHANGED);
            super::services::notice(
                &root,
                name,
                "its installed files changed after it was approved: it was turned off and not started (remove it and propose it again)",
            );
            m::log_event(
                &root,
                name,
                "start",
                "refused",
                "its installed files changed",
            );
            return Err("its installed files changed after it was approved: it was turned off and not started".into());
        }
        self.spawn(&mf, &st, false)
    }
}

// ---- the person's commands ------------------------------------------------------------------

fn state_word(root: &Path, name: &str) -> String {
    let rec = skills::record(root, name);
    let st = m::load_state(root, name);
    match (rec, st) {
        (Some(r), Some(s)) if r.enabled && s.confirmed => "on".into(),
        (Some(r), Some(s)) if !s.confirmed => r.off.unwrap_or_else(|| skills::WAITING_TOOLS.into()),
        (Some(r), _) => r.off.unwrap_or_else(|| "off".into()),
        _ => "not installed".into(),
    }
}

fn tool_line(t: &Tool) -> String {
    let mode = match (t.mode, t.flagged) {
        (_, true) => "ask, changed: confirm it again",
        (Mode::Free, _) => "free",
        (Mode::Ask, _) => "ask",
    };
    format!(
        "  {} [{mode}]{}",
        sanitize::line(&t.name),
        if t.read_only {
            " (says it only reads)"
        } else {
            ""
        }
    )
}

fn last_error(root: &Path, name: &str) -> Option<String> {
    m::events(root, name)
        .into_iter()
        .rev()
        .find(|e| e.outcome == "failed" || e.outcome == "refused")
        .map(|e| {
            format!(
                "{} UTC {} {}: {}",
                super::stamp(e.at_ms),
                e.what,
                e.outcome,
                e.why
            )
        })
}

pub fn list_lines(d: &Dirs) -> Vec<String> {
    let root = root(d);
    let names = m::names(&root);
    if names.is_empty() {
        return vec![
            "no MCP server is installed (the model proposes one with mcp_propose; you approve it)"
                .into(),
        ];
    }
    let mut out = vec![];
    for n in names {
        let st = m::load_state(&root, &n);
        let mf = m::current_manifest(&root, &n);
        out.push(format!(
            "{n}  {}  {}  hosts: {}",
            state_word(&root, &n),
            st.as_ref()
                .map_or("?".into(), |s| format!("{}@{}", s.package, s.version)),
            mf.as_ref()
                .map(|m| if m.network.is_empty() {
                    "none".into()
                } else {
                    m.network.join(", ")
                })
                .unwrap_or_default()
        ));
        for t in st.iter().flat_map(|s| &s.tools) {
            out.push(tool_line(t));
        }
        if let Some(e) = last_error(&root, &n) {
            out.push(format!("  last error: {e}"));
        }
    }
    let notices = super::services::recent_notices(&root, 5);
    if !notices.is_empty() {
        out.push(String::new());
        out.push("recent notices:".into());
        out.extend(notices.iter().map(|n| {
            format!(
                "{} UTC MCP server «{}»: {}",
                super::stamp(n.at_ms),
                n.name,
                n.text
            )
        }));
    }
    out.into_iter().map(|l| sanitize::line(&l)).collect()
}

pub fn show_lines(d: &Dirs, name: &str) -> Res<Vec<String>> {
    let root = root(d);
    let mf = m::current_manifest(&root, name)
        .ok_or_else(|| format!("there is no MCP server «{}»", sanitize::line(name)))?;
    let st = m::load_state(&root, name);
    let mut out = vec![
        format!("«{name}»: {}", mf.description),
        format!("state: {}", state_word(&root, name)),
        format!("package: {}@{}", mf.package, mf.version),
        format!(
            "hosts: {}",
            if mf.network.is_empty() {
                "none".into()
            } else {
                mf.network.join(", ")
            }
        ),
    ];
    for s in &mf.secrets {
        out.push(format!(
            "secret {} as ${}, only to {}",
            s.name, s.env, s.host
        ));
    }
    if !mf.reads.is_empty() {
        out.push(format!("reads: {}", mf.reads.join(", ")));
    }
    if !mf.writes.is_empty() {
        out.push(format!("writes: {}", mf.writes.join(", ")));
    }
    out.push(format!(
        "limits: {}% of a processor, {} MB, {} s a call",
        mf.limits.cpu_percent, mf.limits.memory_mb, mf.limits.seconds
    ));
    if let Some(s) = &st {
        out.push(format!("integrity: {}", s.integrity));
        out.push(format!("entry: {}", s.entry));
        out.push(format!("tools ({}):", s.tools.len()));
        for t in &s.tools {
            out.push(tool_line(t));
            out.push(format!("    {}", t.description));
            out.push(format!("    arguments: {}", t.schema));
        }
    }
    Ok(out.into_iter().map(|l| sanitize::line(&l)).collect())
}

fn logs_lines(d: &Dirs, name: &str) -> Res<Vec<String>> {
    let root = root(d);
    skills::record(&root, name)
        .ok_or_else(|| format!("there is no MCP server «{}»", sanitize::line(name)))?;
    let ev = m::events(&root, name);
    if ev.is_empty() {
        return Ok(vec!["nothing recorded yet".into()]);
    }
    Ok(ev
        .iter()
        .map(|e| {
            sanitize::line(&format!(
                "{} UTC  {}  {}{}",
                super::stamp(e.at_ms),
                e.what,
                e.outcome,
                if e.why.is_empty() {
                    String::new()
                } else {
                    format!(": {}", e.why)
                }
            ))
        })
        .collect())
}

/// The person confirmed the tool list they were shown: changed tools are no longer flagged, and
/// the server is on.
pub fn confirm(d: &Dirs, name: &str) -> Res<String> {
    let root = root(d);
    let mut st = m::load_state(&root, name)
        .ok_or_else(|| format!("there is no MCP server «{}»", sanitize::line(name)))?;
    {
        let _l = m::lock_root(&root)?;
        st.confirmed = true;
        for t in &mut st.tools {
            t.flagged = false;
        }
        m::save_state(&root, &st)?;
    }
    skills::set_enabled_in(&root, Kind::Mcp, name, true, "")?;
    Ok(format!(
        "«{name}»'s {} tools are confirmed and it is on: the model sees it with mcp_tools",
        st.tools.len()
    ))
}

fn set_on(d: &Dirs, name: &str, on: bool) -> Res<String> {
    let root = root(d);
    if on && !m::load_state(&root, name).is_some_and(|s| s.confirmed) {
        return Err(format!(
            "«{}»'s tools are not confirmed: /mcp confirm {} (or clio mcp confirm) shows them and turns it on",
            sanitize::line(name),
            sanitize::line(name)
        ));
    }
    let why = if on { "" } else { skills::OFF_BY_PERSON };
    skills::set_enabled_in(&root, Kind::Mcp, name, on, why)
}

/// `allow` (free) or `ask` for one tool.
fn set_mode(d: &Dirs, name: &str, tool: &str, mode: Mode) -> Res<String> {
    let root = root(d);
    let _l = m::lock_root(&root)?;
    let mut st = m::load_state(&root, name)
        .ok_or_else(|| format!("there is no MCP server «{}»", sanitize::line(name)))?;
    let t = st
        .tools
        .iter_mut()
        .find(|t| t.name == tool)
        .ok_or_else(|| format!("«{name}» has no tool «{}»", sanitize::line(tool)))?;
    t.mode = mode;
    let flagged = t.flagged;
    m::save_state(&root, &st)?;
    Ok(format!(
        "«{name}» {tool}: {}{}",
        if mode == Mode::Free {
            "runs when the model calls it"
        } else {
            "asks you each time"
        },
        if flagged {
            " (it changed: it asks until you confirm the server's tools again)"
        } else {
            ""
        }
    ))
}

fn remove(d: &Dirs, name: &str) -> Res<String> {
    skills::remove_in(&root(d), Kind::Mcp, name)
}

/// The confirmation question for `confirm`, with the tools to be confirmed.
pub fn confirm_lines(d: &Dirs, name: &str) -> Res<Vec<String>> {
    let mut l = show_lines(d, name)?;
    l.push(String::new());
    l.push("These descriptions are the server's own text. Confirm them and turn it on?".into());
    Ok(l)
}

/// `/mcp confirm <name>` in the chat: the question, with the tools as the server described them.
pub fn confirm_ask(d: &Dirs, name: &str) -> Res<super::tui::app::Confirm> {
    let st = m::load_state(&root(d), name)
        .ok_or_else(|| format!("there is no MCP server «{}»", sanitize::line(name)))?;
    let mut c = super::tui::app::Confirm::new(
        super::tui::app::Action::ConfirmMcp,
        name.to_owned(),
        st.tools.len(),
        String::new(),
    );
    c.names = st
        .tools
        .iter()
        .map(|t| sanitize::line(&format!("{} [{}]: {}", t.name, mode_word(t), t.description)))
        .collect();
    Ok(c)
}

fn mode_word(t: &Tool) -> &'static str {
    match (t.mode, t.flagged) {
        (_, true) => "ask, changed",
        (Mode::Free, _) => "free",
        (Mode::Ask, _) => "ask",
    }
}

/// `clio mcp [list|show|on|off|confirm|allow|ask|remove|logs]`. The person's: never the model's.
pub fn cmd(d: &Dirs, args: &[String]) -> Res<()> {
    let w: Vec<&str> = args.iter().map(String::as_str).collect();
    let yes = w.contains(&"--yes");
    let w: Vec<&str> = w.into_iter().filter(|a| *a != "--yes").collect();
    let print = |v: Vec<String>| {
        for l in v {
            println!("{l}");
        }
    };
    let done = match w.as_slice() {
        [] | ["list"] => {
            print(list_lines(d));
            println!("\n{FOOT}");
            return Ok(());
        }
        ["show", n] => {
            print(show_lines(d, n)?);
            return Ok(());
        }
        ["logs", n] => {
            print(logs_lines(d, n)?);
            return Ok(());
        }
        ["on", n] => set_on(d, n, true)?,
        ["off", n] => set_on(d, n, false)?,
        ["allow", n, t] => set_mode(d, n, t, Mode::Free)?,
        ["ask", n, t] => set_mode(d, n, t, Mode::Ask)?,
        ["confirm", n] => {
            print(confirm_lines(d, n)?);
            if !yes && !super::apply::confirm("confirm these tools and turn it on?") {
                println!("nothing changed");
                return Ok(());
            }
            confirm(d, n)?
        }
        ["remove", n] => {
            skills::record(&root(d), n)
                .ok_or_else(|| format!("there is no MCP server «{}»", sanitize::line(n)))?;
            if !yes
                && !super::apply::confirm(&format!(
                    "remove the MCP server «{n}»? its files, data and record go to removed/"
                ))
            {
                println!("nothing was removed");
                return Ok(());
            }
            remove(d, n)?
        }
        _ => return Err(format!("clio mcp: {FOOT}")),
    };
    println!("{}", sanitize::multiline(&done));
    Ok(())
}

pub const FOOT: &str = "/mcp show|on|off|confirm|remove|logs <name> · allow|ask <name> <tool>";

/// `/mcp [...]` in the chat: the same as `clio mcp`, in a box. A removal and a confirmation reach
/// here only after the person said yes to them.
pub fn chat_command(d: &Dirs, args: &str) -> Res<super::tui::commands::Modal> {
    let w: Vec<&str> = args.split_whitespace().collect();
    let (title, lines) = match w.as_slice() {
        [] | ["list"] => ("MCP servers", list_lines(d)),
        ["show", n] => ("MCP server", show_lines(d, n)?),
        ["logs", n] => ("MCP server log", logs_lines(d, n)?),
        [verb, ..] => {
            let done = match w.as_slice() {
                ["on", n] => set_on(d, n, true)?,
                ["off", n] => set_on(d, n, false)?,
                ["allow", n, t] => set_mode(d, n, t, Mode::Free)?,
                ["ask", n, t] => set_mode(d, n, t, Mode::Ask)?,
                ["confirm", n, "--yes"] => confirm(d, n)?,
                ["remove", n, "--yes"] => remove(d, n)?,
                _ => return Err(format!("/mcp: unknown {verb:?}; {FOOT}")),
            };
            let mut l: Vec<String> = sanitize::multiline(&done)
                .lines()
                .map(str::to_owned)
                .collect();
            l.push(String::new());
            l.extend(list_lines(d));
            ("MCP servers", l)
        }
    };
    let mut md = super::tui::commands::Modal::text(title, lines);
    md.foot = Some(FOOT.into());
    Ok(md)
}

/// The notices no chat has shown yet (a server turned off because its files changed).
pub fn take_notices(d: &Dirs) -> Vec<String> {
    super::services::take_notices_in(&root(d), "MCP server")
}

/// What the approval box and `clio approve` show of a proposed server: rows, outside text
/// cleaned (npm's facts are someone else's words).
pub fn proposal_rows(p: &Pending) -> Option<Vec<String>> {
    let (name, description, manifest, _) = clio_toolhost::pending::mcp_of(p)?;
    let mf: Manifest = serde_json::from_str(manifest).ok()?;
    let npm = &p.args["npm"];
    let s = |v: &Value| sanitize::line(v.as_str().unwrap_or(""));
    let mut rows = vec![
        format!(
            "{} the MCP server «{}»: {}",
            if p.args["update"] == true {
                "update"
            } else {
                "install"
            },
            sanitize::line(name),
            sanitize::line(description)
        ),
        format!("package: {}@{} (npm)", mf.package, mf.version),
        format!(
            "publisher: {} · maintainers: {}{}",
            s(&npm["publisher"]),
            npm["maintainers"]
                .as_array()
                .map(|a| a.iter().map(s).collect::<Vec<_>>().join(", "))
                .unwrap_or_default(),
            match npm["more_maintainers"].as_u64() {
                Some(n) if n > 0 => format!(" and {n} more"),
                _ => String::new(),
            }
        ),
        format!(
            "integrity: {} · {} dependencies · license {}",
            s(&npm["integrity"]),
            npm["dependencies"].as_u64().unwrap_or(0),
            s(&npm["license"])
        ),
    ];
    if !s(&npm["repository"]).is_empty() {
        rows.push(format!("repository: {}", s(&npm["repository"])));
    }
    rows.push(format!(
        "hosts: {}",
        if mf.network.is_empty() {
            "none (no network)".into()
        } else {
            mf.network.join(", ")
        }
    ));
    for sec in &mf.secrets {
        let reach: Vec<&str> = mf
            .network
            .iter()
            .map(|h| clio_toolhost::services::host_of(h))
            .collect();
        rows.push(format!(
            "secret {} as ${}, bound to mcp:{}; it can reach {}",
            sec.name,
            sec.env,
            sec.host,
            reach.join(", ")
        ));
    }
    if !mf.reads.is_empty() {
        rows.push(format!("reads: {}", mf.reads.join(", ")));
    }
    if !mf.writes.is_empty() {
        rows.push(format!("writes: {}", mf.writes.join(", ")));
    }
    rows.push(format!(
        "limits: {}% of a processor, {} MB, {} s a call",
        mf.limits.cpu_percent, mf.limits.memory_mb, mf.limits.seconds
    ));
    rows.push(if mf.secrets.is_empty() {
        "This is someone else's code: it runs on this machine, in a jail, with what is listed above.".into()
    } else {
        "This is someone else's code: it will hold these credentials and can send them to the hosts above.".into()
    });
    rows.push("Its tools are not known yet: approving downloads it, starts it once without network or secrets to list them, and it stays off until you confirm them.".into());
    Some(rows.into_iter().map(|r| sanitize::line(&r)).collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::time::Instant;

    use clio_core::SystemClock;
    use clio_node::{HostJob, Launcher};
    use clio_toolhost::ask::HostRules;
    use clio_toolhost::{Config, HostCfg, JailMode, Request, ToolHost};
    use clio_vault::{Dest, Scope, SecretSpec, Vault};
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    const INTEGRITY: &str = "sha512-QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo=";
    const KEY: &str = "s3cr3t-value-0123456789";
    const SERVER: &str = include_str!("../../../clio-toolhost/tests/fixtures/fake-mcp.js");

    /// A stand-in for npm (`node npm-cli.js ...`): `view` prints canned facts, `install` writes a
    /// package (the fake MCP server, in the mode `config.json` says), its lockfile, and what it
    /// could see from inside its jail; every call's arguments go to `argv.log` in npm's cache.
    const FAKE_NPM: &str = r#""use strict";
const fs = require("node:fs");
const path = require("node:path");
const conf = JSON.parse(fs.readFileSync(path.join(__dirname, "config.json"), "utf8"));
const args = process.argv.slice(2);
fs.appendFileSync(path.join(process.env.npm_config_cache, "argv.log"), `${JSON.stringify(args)}\n`);
const spec = args[0] === "view" ? args[1] : args[args.length - 1];
const at = spec.lastIndexOf("@");
const [name, version] = [spec.slice(0, at), spec.slice(at + 1)];
if (args[0] === "view") {
  console.log(JSON.stringify({ name, version, dist: { integrity: conf.integrity }, _npmUser: { name: "example-publisher" },
    maintainers: [{ name: "one" }], dependencies: {}, license: "MIT" }));
  process.exit(0);
}
const dir = path.join("node_modules", name);
fs.mkdirSync(dir, { recursive: true });
fs.writeFileSync(path.join(dir, "package.json"), JSON.stringify({ name, version, bin: { [name.split("/").pop()]: "server.js" } }));
fs.writeFileSync(path.join(dir, "server.js"), `process.env.FAKE_MCP = ${JSON.stringify(conf.mode)};\n${fs.readFileSync(path.join(__dirname, "server.js"), "utf8")}`);
const seen = {};
for (const p of conf.probe) {
  try { seen[p] = fs.statSync(p).isDirectory() ? `dir:${fs.readdirSync(p).length}` : `file:${fs.readFileSync(p).length}`; } catch (e) { seen[p] = `no:${e.code}`; }
}
for (const k of ["HOME", "npm_config_https_proxy", "npm_config_registry", "npm_config_ignore_scripts", "HTTPS_PROXY"]) seen[k] = process.env[k] || "";
fs.writeFileSync(path.join(dir, "seen.json"), JSON.stringify(seen));
const pkgs = { "": { dependencies: { [name]: version } }, [`node_modules/${name}`]: { version: conf.version || version, resolved: `https://registry.npmjs.org/${name}/-/x-${version}.tgz`, integrity: conf.integrity } };
if (conf.git) pkgs["node_modules/dep"] = { version: "1.0.0", resolved: "git+ssh://git@github.com/evil/dep.git#abc" };
fs.writeFileSync("package-lock.json", JSON.stringify({ lockfileVersion: 3, packages: pkgs }));
"#;

    fn tmp() -> TempDir {
        let exe = std::env::current_exe().unwrap();
        TempDir::new_in(exe.parent().unwrap().parent().unwrap().parent().unwrap()).unwrap()
    }

    fn node() -> Option<PathBuf> {
        let home = std::env::var("HOME").unwrap_or_default();
        std::env::var("PATH")
            .unwrap_or_default()
            .split(':')
            .map(|d| Path::new(d).join("node"))
            .chain([Path::new(&home).join(".local/share/node22/bin/node")])
            .find(|p| p.is_file())
            .and_then(|p| p.canonicalize().ok())
    }

    /// bwrap and node, if both run here (else the test says it was skipped).
    fn jail() -> Option<(HostJail, PathBuf)> {
        let Some(node) = node() else {
            eprintln!("SKIPPED (no node here)");
            return None;
        };
        let h = HostJail::with(Launcher::Bwrap).ok()?;
        let t = tmp();
        if t.path().starts_with("/tmp") {
            eprintln!("SKIPPED (the build folder is under /tmp, which a run does not see)");
            return None;
        }
        let j = HostJob {
            command: "true".into(),
            cwd: t.path().into(),
            home: t.path().into(),
            network: false,
            timeout: Some(Duration::from_secs(10)),
            max_output: 1024,
            masks: vec![],
            shows: vec![],
            read_only: false,
        };
        match h.run(&j) {
            Ok(o) if o.success() => Some((h, node)),
            r => {
                eprintln!("SKIPPED (bwrap cannot start here, nothing was tested): {r:?}");
                None
            }
        }
    }

    struct Env {
        t: TempDir,
        port: Arc<Port>,
    }

    impl Env {
        fn d(&self) -> &Dirs {
            &self.port.dirs
        }
        fn root(&self) -> PathBuf {
            root(self.d())
        }
        /// What the fake npm does next: the server's mode, and more.
        fn npm_conf(&self, v: Value) {
            let mut c = json!({"integrity": INTEGRITY, "mode": "normal",
                "probe": [self.t.path().join("home/secret.txt"), "/run", self.d().data.join("vault.bin"), self.root().join("state")]});
            for (k, x) in v.as_object().unwrap() {
                c[k] = x.clone();
            }
            fs::write(self.t.path().join("npm/bin/config.json"), c.to_string()).unwrap();
        }
        fn argv(&self) -> Vec<Vec<String>> {
            fs::read_to_string(self.root().join("npm-cache/argv.log"))
                .unwrap_or_default()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect()
        }
        /// A host-mode tool host with this port, as a chat's.
        fn host(&self) -> ToolHost {
            let d = self.d();
            let mut c = Config::new(
                &d.home,
                &self.t.path().join("scratch"),
                &d.runs.join("r1/toolhost"),
            );
            c.jail = JailMode::Off;
            c.mcp = Some(self.port.clone() as Arc<dyn McpPort>);
            c.host = Some(HostCfg {
                home: d.home.clone(),
                state: super::super::pending::state(d),
                control: d.control(),
                rules: HostRules::factory(),
                run: "r1".into(),
                launcher: Some(Launcher::Bwrap),
            });
            ToolHost::new(c).unwrap()
        }
    }

    fn env() -> Option<Env> {
        let (jail, node) = jail()?;
        let t = tmp();
        let r = t.path();
        let data = r.join("data");
        for p in [
            &data,
            &r.join("home"),
            &r.join("config"),
            &r.join("run/clio"),
            &r.join("npm/bin"),
        ] {
            mkdir_private(p).unwrap();
        }
        fs::write(r.join("home/secret.txt"), "the person's own file").unwrap();
        fs::write(data.join("vault.bin"), "vault").unwrap();
        fs::write(r.join("npm/bin/npm-cli.js"), FAKE_NPM).unwrap();
        fs::write(r.join("npm/bin/server.js"), SERVER).unwrap();
        let fake = r.join("fake-clio");
        fs::write(&fake, "#!/bin/sh\nwhile [ \"$#\" -gt 0 ] && [ \"$1\" != \"--\" ]; do shift; done\nshift\nexec \"$@\"\n").unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        let dirs = Dirs {
            config: r.join("config"),
            runs: data.join("runs"),
            data,
            runtime: Some(r.join("run/clio")),
            home: r.join("home"),
        };
        let gateway = Some(r.join("run/clio/vault-gateway.sock"));
        let port = Arc::new(Port {
            dirs,
            jail,
            node,
            npm: r.join("npm/bin/npm-cli.js"),
            exe: fake,
            gateway,
        });
        let e = Env { t, port };
        e.npm_conf(json!({}));
        Some(e)
    }

    fn propose(e: &Env, h: &ToolHost, name: &str, secrets: bool) -> Response {
        let mut a = json!({
            "name": name,
            "description": format!("The test server {name}, for the tests."),
            "package": format!("@example/{name}"),
            "version": "1.0.0",
            "limits": {"seconds": 10},
            "reason": "the tests ask for it",
        });
        if secrets {
            a["network"] = json!(["api.example.com"]);
            a["secrets"] =
                json!([{"name": "status-key", "env": "STATUS_KEY", "host": "api.example.com"}]);
        }
        let _ = e;
        h.handle(Request {
            id: "i".into(),
            tool: "mcp_propose".into(),
            args: a,
            session: "s".into(),
            call_id: "c".into(),
            tainted: false,
        })
    }

    use clio_toolhost::Response;

    fn approve(e: &Env, id: &str) -> Res<Pending> {
        let st = super::super::pending::state(e.d());
        clio_toolhost::pending::approve_mcp(&st, id, super::super::now_ms(), |p| e.port.install(p))
    }

    fn pending_id(r: &Response) -> String {
        r.content
            .split_whitespace()
            .nth(2)
            .unwrap()
            .trim_end_matches(':')
            .to_owned()
    }

    // S-70, with bwrap: a proposal asks npm (in its jail) for the facts and nothing else: nothing
    // is installed before the person approves. Approved, exactly `package@version` is installed
    // with no scripts, checked against the approved integrity, probed WITHOUT secrets or network
    // (only `initialize` and `tools/list`), and registered OFF until its tools are confirmed.
    #[test]
    fn nothing_is_installed_before_approval_and_a_probed_server_waits_for_its_tools() {
        let Some(e) = env() else { return };
        e.npm_conf(json!({"mode": "report"}));
        let h = e.host();
        let r = propose(&e, &h, "srv", true);
        assert!(r.ok, "{r:?}");
        assert_eq!(e.argv(), [vec!["view", "@example/srv@1.0.0", "--json"]]);
        assert!(!e.root().join("servers").exists(), "nothing was downloaded");
        assert!(skills::record(&e.root(), "srv").is_none());
        let id = pending_id(&r);
        let rows = proposal_rows(
            &clio_toolhost::pending::check(
                &super::super::pending::state(e.d()),
                &id,
                super::super::now_ms(),
            )
            .unwrap(),
        )
        .unwrap();
        let all = rows.join("\n");
        assert!(
            all.contains("@example/srv@1.0.0")
                && all.contains("example-publisher")
                && all.contains(INTEGRITY),
            "{all}"
        );
        assert!(
            all.contains("secret status-key")
                && all.contains("api.example.com")
                && all.contains("someone else's code"),
            "{all}"
        );
        let said = approve(&e, &id).unwrap();
        assert!(
            matches!(&said.state, clio_toolhost::pending::State::Done { output_tail, .. } if output_tail.contains("stays off")),
            "{said:?}"
        );
        let argv = e.argv();
        assert_eq!(
            argv[1],
            [
                "install",
                "--ignore-scripts",
                "--no-audit",
                "--no-fund",
                "--save-exact",
                "--loglevel=error",
                "@example/srv@1.0.0"
            ]
        );
        let tree = e.root().join("servers/srv/1.0.0");
        assert!(tree.join("package-lock.json").is_file());
        let st = m::load_state(&e.root(), "srv").unwrap();
        assert!(!st.confirmed && st.integrity == INTEGRITY, "{st:?}");
        assert_eq!(st.tree, tree_digest(&tree).unwrap());
        assert_eq!(st.entry, "node_modules/@example/srv/server.js");
        // The probe had no secret and no network.
        let inspect = st.tools.iter().find(|t| t.name == "inspect").unwrap();
        assert_eq!(inspect.description, "key 0 proxy none");
        let echo = st.tools.iter().find(|t| t.name == "echo").unwrap();
        let write = st.tools.iter().find(|t| t.name == "write_note").unwrap();
        assert!(echo.mode == Mode::Free && write.mode == Mode::Ask);
        let rec = skills::record(&e.root(), "srv").unwrap();
        assert!(
            !rec.enabled && rec.off.as_deref() == Some(skills::WAITING_TOOLS),
            "{rec:?}"
        );
        assert!(!m::is_on(&e.root(), "srv"));
        assert!(list_lines(e.d()).join("\n").contains(skills::WAITING_TOOLS));
        // Not on before its tools are confirmed; confirmed, it is on.
        assert!(set_on(e.d(), "srv", true).is_err());
        assert!(confirm(e.d(), "srv").unwrap().contains("on"));
        assert!(m::is_on(&e.root(), "srv"));
        // `ask` and `allow` set one tool; `off` and `remove` (to removed/) are the person's too.
        set_mode(e.d(), "srv", "write_note", Mode::Free).unwrap();
        assert_eq!(
            m::load_state(&e.root(), "srv").unwrap().tools[1].mode,
            Mode::Free
        );
        set_on(e.d(), "srv", false).unwrap();
        assert!(!m::is_on(&e.root(), "srv"));
        remove(e.d(), "srv").unwrap();
        assert!(
            !e.root().join("servers/srv").exists() && m::load_state(&e.root(), "srv").is_none()
        );
        let removed: Vec<_> = fs::read_dir(e.root().join("removed"))
            .unwrap()
            .flatten()
            .collect();
        assert!(removed[0].path().join("servers/1.0.0").is_dir());
    }

    // S-70, with bwrap: npm is confined: it sees nothing of the home, not `/run` or Clio's data
    // (but its own folders), and its only way out is the proxy to the registry.
    #[test]
    fn npm_runs_confined_to_its_own_folders_and_the_registry() {
        let Some(e) = env() else { return };
        let h = e.host();
        let id = pending_id(&propose(&e, &h, "srv", false));
        approve(&e, &id).unwrap();
        let seen: Value = serde_json::from_str(
            &fs::read_to_string(
                e.root()
                    .join("servers/srv/1.0.0/node_modules/@example/srv/seen.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let s = |k: &Path| seen[k.display().to_string()].as_str().unwrap().to_owned();
        assert!(
            s(&e.t.path().join("home/secret.txt")).starts_with("no:"),
            "{seen}"
        );
        assert!(
            s(&e.d().data.join("vault.bin")).starts_with("no:"),
            "{seen}"
        );
        assert!(s(&e.root().join("state")).starts_with("no:"), "{seen}");
        assert_eq!(seen["/run"], "dir:0", "{seen}");
        assert!(
            seen["HOME"].as_str().unwrap().contains("/mcp/npm/home-"),
            "{seen}"
        );
        assert_eq!(seen["npm_config_https_proxy"], "http://127.0.0.1:18080");
        assert_eq!(seen["npm_config_registry"], "https://registry.npmjs.org/");
        assert_eq!(seen["npm_config_ignore_scripts"], "true");
        // npm's own home is gone afterwards.
        assert_eq!(fs::read_dir(e.root().join("npm")).unwrap().count(), 0);
    }

    // S-70, with bwrap: what npm installed must be the approved version and integrity, every
    // package from the registry with an integrity; and a server that does not answer its probe
    // is not registered. Either way the item still waits and nothing is left installed.
    #[test]
    fn a_wrong_lockfile_or_a_failed_probe_installs_nothing() {
        let Some(e) = env() else { return };
        let h = e.host();
        for (conf, why) in [
            (json!({"integrity": "sha512-T1RIRVI="}), "integrity"),
            (json!({"version": "1.0.1"}), "1.0.1"),
            (json!({"git": true}), "registry"),
            (json!({"mode": "mute"}), "did not start"),
        ] {
            e.npm_conf(json!({}));
            let id = pending_id(&propose(&e, &h, "srv", false));
            e.npm_conf(conf.clone());
            let err = approve(&e, &id).unwrap_err();
            assert!(err.contains(why), "{conf}: {err}");
            assert!(skills::record(&e.root(), "srv").is_none(), "{conf}");
            assert!(!e.root().join("servers/srv/1.0.0").exists(), "{conf}");
            let st = super::super::pending::state(e.d());
            assert!(
                clio_toolhost::pending::check(&st, &id, super::super::now_ms()).is_ok(),
                "it still waits"
            );
            clio_toolhost::pending::reject(&st, &id).unwrap();
        }
    }

    fn installed(e: &Env, name: &str, secrets: bool) {
        let h = e.host();
        let id = pending_id(&propose(e, &h, name, secrets));
        approve(e, &id).unwrap();
        confirm(e.d(), name).unwrap();
    }

    // S-70, with bwrap: a server sees nothing of the home, not `/run`, Clio's data, the vault or
    // other servers' folders, and no other process; it gets only its own secret, bound to
    // `mcp:<host>`, as its variable, never in an argument, and its hosts through the proxy.
    #[test]
    fn a_server_runs_in_the_jail_with_only_its_own_secret() {
        let Some(e) = env() else { return };
        let rt = e.d().runtime.clone().unwrap();
        let (v, cap) =
            Vault::init(None, "correct horse battery staple", Arc::new(SystemClock)).unwrap();
        for (name, dest) in [
            ("status-key", "mcp:api.example.com"),
            ("other-key", "service:api.example.com"),
        ] {
            v.put(
                &cap,
                SecretSpec {
                    name: name.into(),
                    value: KEY.into(),
                    kind: clio_vault::Kind::ApiKey,
                    destinations: vec![Dest::parse(dest).unwrap()],
                    scope: Scope::Personal,
                    rotate_every_ms: None,
                    expires_at_ms: None,
                },
            )
            .unwrap();
        }
        let v = Arc::new(v);
        let (g, p) = (rt.join("vault-gateway.sock"), rt.join("vault-person.sock"));
        let person = p.clone();
        std::thread::spawn(move || clio_vault::server::serve(v, &g, &p));
        let end = Instant::now() + Duration::from_secs(5);
        while !person.exists() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        installed(&e, "srv", true);
        installed(&e, "other", false);
        let proc = e.port.start("srv").unwrap();
        let mut s = m::Server::start(proc, Duration::from_secs(30)).unwrap();
        let paths = [
            e.t.path().join("home/secret.txt"),
            e.d().data.join("vault.bin"),
            e.root().join("servers/other"),
            e.root().join("state"),
            rt.join("vault-gateway.sock"),
            PathBuf::from("/run"),
        ];
        let (text, _) = s
            .call(
                "inspect",
                &json!({"paths": paths, "env": ["STATUS_KEY", "HTTPS_PROXY"]}),
                Duration::from_secs(20),
            )
            .unwrap();
        let seen: Value = serde_json::from_str(&text).unwrap();
        for p in &paths[..5] {
            assert!(
                seen[p.display().to_string()]
                    .as_str()
                    .unwrap()
                    .starts_with("no:"),
                "{p:?}: {seen}"
            );
        }
        assert_eq!(seen["/run"], "dir:0", "{seen}");
        assert_eq!(seen["$STATUS_KEY"], KEY.len(), "{seen}");
        assert_eq!(
            seen["$HTTPS_PROXY"],
            "http://127.0.0.1:18080".len(),
            "{seen}"
        );
        assert_eq!(seen["argv_with_key"], 0, "{seen}");
        assert!(
            seen["pids"].as_u64().unwrap() < 10,
            "its own pid namespace: {seen}"
        );
        // What it says of its secret is struck out.
        let (echo, _) = s
            .call("echo", &json!({"text": "x"}), Duration::from_secs(20))
            .unwrap();
        assert!(!echo.contains(KEY), "{echo}");
        drop(s);
        // A server whose secret is bound to another tool does not start.
        let mut bad = m::current_manifest(&e.root(), "srv").unwrap();
        bad.secrets[0].name = "other-key".into();
        let st = m::load_state(&e.root(), "srv").unwrap();
        let err = e.port.spawn(&bad, &st, false).err().unwrap();
        assert!(err.contains("other-key") && err.contains("mcp:"), "{err}");
        // Nowhere it is kept.
        let mut all = String::new();
        for d in [e.root(), super::super::pending::state(e.d())] {
            for f in walk(&d) {
                all.push_str(&String::from_utf8_lossy(&fs::read(f).unwrap_or_default()));
            }
        }
        assert!(!all.contains(KEY));
    }

    fn walk(d: &Path) -> Vec<PathBuf> {
        let mut out = vec![];
        for e in fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() && !p.is_symlink() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    // S-70, with bwrap: the installed tree's digest is checked before each start: a changed file
    // means it does not start, it is turned off, and the person is told.
    #[test]
    fn a_changed_tree_is_not_started_and_turns_the_server_off() {
        let Some(e) = env() else { return };
        installed(&e, "srv", false);
        let proc = e.port.start("srv").unwrap();
        drop(m::Server::start(proc, Duration::from_secs(30)).unwrap());
        let f = e
            .root()
            .join("servers/srv/1.0.0/node_modules/@example/srv/server.js");
        let t = fs::read_to_string(&f).unwrap();
        fs::write(&f, format!("{t}\n// changed\n")).unwrap();
        let err = e.port.start("srv").err().unwrap();
        assert!(err.contains("changed"), "{err}");
        let rec = skills::record(&e.root(), "srv").unwrap();
        assert!(
            !rec.enabled && rec.off.as_deref() == Some(OFF_CHANGED),
            "{rec:?}"
        );
        let told = take_notices(e.d());
        assert!(
            told.len() == 1 && told[0].contains("MCP server «srv»"),
            "{told:?}"
        );
        assert!(
            list_lines(e.d())
                .join("\n")
                .contains("installed files changed")
        );
    }

    // S-70: installing or removing a server changes nothing in the prompt's prefix: the prompts
    // are fixed, the model's tools are the three fixed ones, and the skills Pi lists are untouched.
    #[test]
    fn installing_or_removing_a_server_leaves_the_prompt_prefix_as_it_was() {
        let Some(e) = env() else { return };
        skills::ensure(e.d()).unwrap();
        let live = skills::live(e.d());
        let before = walk(&live);
        let prompt = clio_brain::pi::session::HOST_PROMPT;
        let tools = clio_brain::pi::session::HOST_GUARD_TOOLS;
        installed(&e, "srv", false);
        assert_eq!(walk(&live), before);
        remove(e.d(), "srv").unwrap();
        assert_eq!(walk(&live), before);
        assert_eq!(prompt, clio_brain::pi::session::HOST_PROMPT);
        assert_eq!(tools, clio_brain::pi::session::HOST_GUARD_TOOLS);
        assert!(tools.ends_with(&["mcp_propose", "mcp_tools", "mcp_call"]));
    }
}
