//! The model's say over what it created (S-68, S-69, S-70): `service_manage`, `skill_manage` and
//! `mcp_manage {name, action, reason}`, host mode only. The tool host never writes a registry
//! (`<data>/skills`, `<data>/services`, `<data>/mcp`): it reads them, answers `status` and `logs`,
//! and records what is asked. The person's own `clio` carries it out, through the very functions
//! of the person's commands (`crates/clio-brain/src/cli/manage.rs`):
//!
//! - free, recorded in `<run>/manage.jsonl` and done when the turn ends: `off` (stopping is always
//!   safe), and `run` of a service that is on and reaches no host and holds no secret (the runner
//!   checks its files against what was approved and pauses it when they changed, as the timer
//!   does); a `run` that reaches hosts or holds secrets could send or spend: set aside as `send`;
//! - set aside for the person (hash-bound, the one approval box): `on`, `undo`, `remove`. A
//!   permission window answers them (`grant::covers`), except `on` of what the person turned off
//!   (`PERSON_OFF`), `remove` or `undo` of a skill Clio ships (`SHIPPED`) and `undo` of a service
//!   (`SERVICE_UNDO`: its older version's grants are shown in the box): those always ask;
//! - in a `suspect` session every action but `status` is set aside and always asks.
//!
//! At most `PER_HOUR` calls an hour per session. Registering something new or a new version, a
//! tool's mode, the services' timer and confirming a server's tools are never here: the person's.

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clio_vault::count_masks;
use serde_json::{Map, Value, json};

use crate::ask::Class;
use crate::host::{log_line, now_ms, private_dir};
use crate::judge::one_line;
use crate::pending::{Pending, State, set_aside};
use crate::{
    DecisionOut, Request, Response, SUSPECT_REASON, ToolHost, lock, mcp, services, skills,
};

/// The three tools.
pub const TOOLS: [&str; 3] = ["service_manage", "skill_manage", "mcp_manage"];
/// Calls of the three an hour, per session.
pub const PER_HOUR: usize = 20;
/// Why an `on` always asks: the person turned it off.
pub const PERSON_OFF: &str = "the person turned it off: only they turn it back on";
/// Why a `remove` always asks: a skill Clio ships.
pub const SHIPPED: &str = "a skill Clio ships: only the person removes it or takes it back";
/// Why a service's `undo` always asks: the older version may reach other hosts and secrets.
pub const SERVICE_UNDO: &str = "an older version of a service may reach other hosts, secrets and paths: only the person takes it back";
/// A record's `off` when the person turned it off (`/skills off`, `/services off`, `/mcp off`).
pub const OFF_BY_PERSON: &str = "turned off by the person";
/// A record's `off` when its skill is gone from the shared folder (S-74): the person's doing too.
pub const OFF_GONE: &str = "gone from ~/.agents/skills";

/// Only the person turns back on what is off for `off`: they turned it off, or took it out of
/// the shared folder (S-74).
pub fn off_by_person(off: &str) -> bool {
    off == OFF_BY_PERSON || off == OFF_GONE
}
/// A record's `off` when Clio turned it off (`*_manage off`).
pub const OFF_BY_CLIO: &str = "turned off by Clio";
/// The skills Clio ships (`crates/clio-brain/src/cli/skills.rs`).
pub const SHIPPED_SKILLS: [&str; 3] = ["skill-creator", "service-creator", "mcp-installer"];
const MAX_REASON: usize = 300;

/// What a manage tool acts on: its folder beside the skills folder, its word, its actions and the
/// class of what it sets aside.
pub struct Kind {
    pub folder: &'static str,
    pub word: &'static str,
    pub actions: &'static [&'static str],
    pub class: Class,
}

pub fn kind(tool: &str) -> Option<Kind> {
    let (folder, word, actions, class): (_, _, &'static [&'static str], _) = match tool {
        "service_manage" => (
            "services",
            "service",
            &["status", "logs", "run", "off", "on", "undo", "remove"],
            Class::Service,
        ),
        "skill_manage" => (
            "skills",
            "skill",
            &["status", "off", "on", "undo", "remove"],
            Class::Skill,
        ),
        "mcp_manage" => (
            "mcp",
            "MCP server",
            &["status", "logs", "off", "on", "remove"],
            Class::Mcp,
        ),
        _ => return None,
    };
    Some(Kind {
        folder,
        word,
        actions,
        class,
    })
}

/// An item one of the three tools set aside.
pub fn is_manage(p: &Pending) -> bool {
    TOOLS.contains(&p.tool.as_str())
}

/// A reason that makes an item ask the person whatever window is open.
pub fn always_asks(reason: &str) -> bool {
    [PERSON_OFF, SHIPPED, SERVICE_UNDO, SUSPECT_REASON].contains(&reason)
}

/// Where the free requests wait for the person's `clio`.
pub fn requests_file(run_dir: &Path) -> PathBuf {
    run_dir.join("manage.jsonl")
}

/// The version `undo` goes back to: the newest before the current one whose folder is there, as
/// the person's `undo` chooses it.
pub fn prev_version(root: &Path, name: &str, rec: &Value) -> Option<u64> {
    let cur = rec["current"].as_u64()?;
    rec["versions"]
        .as_array()?
        .iter()
        .filter_map(|v| v["n"].as_u64())
        .filter(|n| {
            *n < cur
                && root
                    .join(skills::VERSIONS)
                    .join(name)
                    .join(n.to_string())
                    .is_dir()
        })
        .max()
}

/// Service `name`'s manifest at `version`, as approved (`None`: it does not read).
pub fn manifest_at(root: &Path, name: &str, version: u64) -> Option<(String, services::Manifest)> {
    let text = fs::read_to_string(
        root.join(skills::VERSIONS)
            .join(name)
            .join(version.to_string())
            .join(services::MANIFEST),
    )
    .ok()?;
    let m = serde_json::from_str(&text).ok()?;
    Some((text, m))
}

/// What a set-aside item does, in plain words, from its arguments only (never the stored `what`):
/// the act and Clio's reason.
pub fn what(p: &Pending) -> Option<(String, String)> {
    if !is_manage(p) {
        return None;
    }
    let k = kind(&p.tool)?;
    let s = |key: &str| p.args[key].as_str().unwrap_or("").to_owned();
    let n = |key: &str| p.args[key].as_u64().unwrap_or(0);
    let (name, word) = (s("name"), k.word);
    let goes = match k.class {
        Class::Skill => "its versions and record",
        Class::Service => "its versions, record, state and runs",
        _ => "its versions, record, installed files, state, data and log",
    };
    let does = match s("action").as_str() {
        "on" => format!(
            "turn the {word} «{name}» back on (version {}; now it is {})",
            n("version"),
            s("off")
        ),
        "undo" => format!(
            "take the {word} «{name}» back to version {} (it is at version {} now)",
            n("to"),
            n("version")
        ),
        "remove" => format!(
            "remove the {word} «{name}» (version {}): {goes} go to removed/, where they are kept",
            n("version")
        ),
        "off" => format!("turn the {word} «{name}» off"),
        "run" if p.args["reaches"].is_string() => format!(
            "run the service «{name}» now (it reaches {}{})",
            s("reaches"),
            match s("with").as_str() {
                "" => String::new(),
                w => format!(" with {w}"),
            }
        ),
        "run" => format!("run the service «{name}» now"),
        "logs" => format!("show Clio the log of the {word} «{name}»"),
        other => format!("{other} the {word} «{name}»"),
    };
    Some((does, s("why")))
}

impl ToolHost {
    fn may_manage(&self, session: &str) -> bool {
        let mut all = lock(&self.manages);
        let times = all.entry(session.to_owned()).or_default();
        times.retain(|t| t.elapsed() < Duration::from_secs(3600));
        if times.len() >= PER_HOUR {
            return false;
        }
        times.push(Instant::now());
        true
    }

    fn manage_answer(&self, req: &Request, ok: bool, rule: &str, content: String) -> Response {
        Response {
            id: req.id.clone(),
            ok,
            content: content.clone(),
            data: Value::Null,
            decision: DecisionOut {
                outcome: if ok { "allow" } else { "deny" }.into(),
                rule: rule.into(),
                reason: content,
            },
            tainted: self.is_tainted(&req.session),
            external: false,
        }
    }

    /// `service_manage`, `skill_manage`, `mcp_manage`: see the module's comment.
    pub(crate) fn manage(&self, req: &Request, args: &Map<String, Value>) -> Response {
        let fail = |why: &str| {
            self.manage_answer(
                req,
                false,
                "manage/refused",
                format!(
                    "{} failed: {}. Nothing was done.",
                    req.tool,
                    one_line(&self.clean(why))
                ),
            )
        };
        let Some(k) = kind(&req.tool) else {
            return fail("not a manage tool");
        };
        let (Some(h), Some(skills_root)) = (self.host.as_ref(), self.skills.as_deref()) else {
            return fail(&format!(
                "{}s exist only in a chat on the person's machine (host mode)",
                k.word
            ));
        };
        let root = match k.class {
            Class::Skill => skills_root.to_path_buf(),
            _ => match skills_root.parent() {
                Some(p) => p.join(k.folder),
                None => return fail("Clio's folders are not set up"),
            },
        };
        let s = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or("").trim();
        let (name, action, why) = (s("name"), s("action"), s("reason"));
        if !k.actions.contains(&action) {
            return fail(&format!("action: one of {}", k.actions.join(", ")));
        }
        let list = name.is_empty() && action == "status";
        if !list && !skills::valid_name(name) {
            return fail("name: lowercase letters, digits and single hyphens, at most 64");
        }
        if why.is_empty()
            || why.chars().count() > MAX_REASON
            || why.chars().any(|c| c.is_control() || skills::hidden(c))
        {
            return fail(&format!(
                "reason: why, in one line of at most {MAX_REASON} characters"
            ));
        }
        if !self.may_manage(&req.session) {
            return fail(&format!(
                "at most {PER_HOUR} calls of service_manage, skill_manage and mcp_manage an hour in a session: wait, or ask the person"
            ));
        }
        if list {
            return self.manage_read(req, &k, &root, None);
        }
        let Some(rec) = skills::record(&root, name) else {
            return self.manage_answer(
                req,
                false,
                "manage/missing",
                format!("there is no {} «{name}»", k.word),
            );
        };
        let suspect = self.is_suspect(&req.session);
        if action == "status" || (action == "logs" && !suspect) {
            return self.manage_read(req, &k, &root, Some((&rec, action)));
        }
        let on = rec["enabled"] == Value::Bool(true);
        let off_why = rec["off"].as_str().unwrap_or("off");
        let prev = prev_version(&root, name, &rec);
        // What does not apply is said plainly, and nothing is recorded.
        let plain = match action {
            "off" if !on => Some(format!("«{name}» is already off ({off_why})")),
            "on" if on => Some(format!("«{name}» is already on")),
            "run" if !on => Some(format!(
                "«{name}» is off ({off_why}): it does not run; `on` turns it back on first"
            )),
            "undo" if prev.is_none() => Some(format!(
                "«{name}» has no earlier version: `remove` takes it away"
            )),
            "on" if k.class == Class::Mcp
                && !mcp::load_state(&root, name).is_some_and(|s| s.confirmed) =>
            {
                Some(format!(
                    "the person has not seen «{name}»'s tools yet: only they turn it on (/mcp confirm)"
                ))
            }
            _ => None,
        };
        if let Some(t) = plain {
            return self.manage_answer(req, false, "manage/plain", t);
        }
        let current = rec["current"].as_u64().unwrap_or(0);
        // A run that reaches hosts or holds secrets could send or spend: it asks, as `send`.
        let reach = (action == "run")
            .then(|| match manifest_at(&root, name, current) {
                Some((_, m)) if m.network.is_empty() && m.secrets.is_empty() => None,
                Some((_, m)) => Some((
                    m.network.join(", "),
                    m.secrets
                        .iter()
                        .map(|s| s.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                )),
                None => Some((
                    "hosts it declared (its manifest does not read)".into(),
                    String::new(),
                )),
            })
            .flatten();
        if !suspect && (action == "off" || (action == "run" && reach.is_none())) {
            return self.manage_request(req, h, &k, name, action, why);
        }
        let shipped = k.class == Class::Skill && SHIPPED_SKILLS.contains(&name);
        let reason = if suspect {
            Some(SUSPECT_REASON)
        } else if action == "on" && off_by_person(off_why) {
            Some(PERSON_OFF)
        } else if matches!(action, "remove" | "undo") && shipped {
            Some(SHIPPED)
        } else if action == "undo" && k.class == Class::Service {
            Some(SERVICE_UNDO)
        } else {
            None
        };
        let mut a = json!({
            "name": name,
            "action": action,
            "why": why,
            "version": current,
        });
        if let Some((hosts, secrets)) = &reach {
            a["reaches"] = Value::from(hosts.as_str());
            a["with"] = Value::from(secrets.as_str());
        }
        // The version a service goes back to, as approved: its grants are in the box.
        if let (Some(to), "undo", Class::Service) = (prev, action, k.class)
            && let Some((text, _)) = manifest_at(&root, name, to)
        {
            a["older"] = Value::from(text);
        }
        if let (Some(to), "undo") = (prev, action) {
            a["to"] = Value::from(to);
        }
        // Who turned it off is part of what is asked (and of its hash): an `on` asked while Clio
        // had turned it off is not the one asked after the person did.
        if action == "on" {
            a["off"] = Value::from(off_why);
        }
        let class = if reach.is_some() {
            Class::Send
        } else {
            k.class
        };
        self.manage_aside(req, h, class, a, reason)
    }

    /// `status` and `logs`, read from the registry. What a service or a server printed is outside
    /// text: judged when the guard filters it, and marking the session. A skill's record is not.
    fn manage_read(
        &self,
        req: &Request,
        k: &Kind,
        root: &Path,
        one: Option<(&Value, &str)>,
    ) -> Response {
        let now = now_ms();
        let offset = services::local_offset(now);
        let name = one.map_or("", |(r, _)| r["name"].as_str().unwrap_or(""));
        let text = match (k.class, one) {
            (Class::Service, None) => services::status_list(root, now, offset),
            (Class::Service, Some((r, "status"))) => services::status_one(root, r, now, offset),
            (Class::Service, Some(_)) => {
                let runs = services::runs(root, name);
                let mut out = vec![format!(
                    "the last {} runs of «{name}», newest first:",
                    runs.len()
                )];
                for r in runs.iter().rev() {
                    out.push(format!(
                        "- {} · by {}",
                        services::run_line(r, now, offset),
                        r.by
                    ));
                    out.extend(r.tail.lines().map(|l| format!("  {l}")));
                }
                out.join("\n")
            }
            (Class::Mcp, Some((_, "logs"))) => {
                let ev = mcp::events(root, name);
                let mut out = vec![format!(
                    "the log of «{name}» (starts and calls), oldest first:"
                )];
                out.extend(ev.iter().map(|e| {
                    format!(
                        "- {} {} {}{}",
                        services::when(e.at_ms, now, offset),
                        e.what,
                        e.outcome,
                        if e.why.is_empty() {
                            String::new()
                        } else {
                            format!(": {}", e.why)
                        }
                    )
                }));
                out.join("\n")
            }
            (Class::Mcp, _) => {
                let names = one.map_or_else(|| mcp::names(root), |_| vec![name.to_owned()]);
                let mut out = vec![];
                for n in names {
                    let Some(r) = skills::record(root, &n) else {
                        continue;
                    };
                    let st = mcp::load_state(root, &n);
                    out.push(format!(
                        "{n} [{}] version {}{}",
                        state_word(&r),
                        r["current"],
                        st.as_ref().map_or_else(String::new, |s| format!(
                            " · {}@{}",
                            s.package, s.version
                        ))
                    ));
                    for t in st.iter().flat_map(|s| &s.tools) {
                        out.push(format!(
                            "  {} [{}]",
                            t.name,
                            if t.flagged || t.mode == mcp::Mode::Ask {
                                "ask"
                            } else {
                                "free"
                            }
                        ));
                    }
                }
                if out.is_empty() {
                    "no MCP server is installed".into()
                } else {
                    out.join("\n")
                }
            }
            (_, Some((r, _))) => skill_line(r),
            (_, None) => {
                let mut v: Vec<String> = fs::read_dir(root.join(skills::RECORDS))
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter_map(|e| {
                        let n = e.file_name().to_str()?.strip_suffix(".json")?.to_owned();
                        skills::record(root, &n).map(|r| skill_line(&r))
                    })
                    .collect();
                v.sort();
                if v.is_empty() {
                    "no skills yet".into()
                } else {
                    v.join("\n")
                }
            }
        };
        let raw = text.clone();
        let text = self.clean(&text);
        let outside = k.class != Class::Skill;
        if outside {
            self.mark(
                &req.session,
                if k.class == Class::Mcp {
                    "mcp"
                } else {
                    "service"
                },
            );
            self.judge_later(req, &req.tool);
        }
        Response {
            id: req.id.clone(),
            ok: true,
            content: text.clone(),
            data: json!({ "masked": count_masks(&text).saturating_sub(count_masks(&raw)) }),
            decision: DecisionOut {
                outcome: "allow".into(),
                rule: "manage/read".into(),
                reason: format!("reading the {}s' records", k.word),
            },
            tainted: self.is_tainted(&req.session),
            external: outside,
        }
    }

    /// `off`, or `run` of a service that is on: free. Recorded for the person's `clio`, which does
    /// it when the turn ends; nothing here touches the registry.
    fn manage_request(
        &self,
        req: &Request,
        h: &crate::host::Host,
        k: &Kind,
        name: &str,
        action: &str,
        why: &str,
    ) -> Response {
        let line = json!({
            "at_ms": now_ms(),
            "session": req.session,
            "tool": req.tool,
            "name": name,
            "action": action,
            "why": why,
        });
        let file = requests_file(&self.run_dir);
        let stored = private_dir(&self.run_dir).and_then(|()| {
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(&file)
                .and_then(|mut f| f.write_all(format!("{line}\n").as_bytes()))
        });
        if let Err(e) = stored {
            return self.manage_answer(
                req,
                false,
                "manage/refused",
                format!(
                    "{} failed: the request could not be kept ({e}). Nothing was done.",
                    req.tool
                ),
            );
        }
        if let Err(e) = h.log(
            &req.session,
            &req.tool,
            &format!("{action} {name}"),
            false,
            "requested",
            "manage/free",
            None,
        ) {
            eprintln!("clio-toolhost: the action log lost a line: {e}");
        }
        let does = if action == "off" {
            format!("turns the {} «{name}» off", k.word)
        } else {
            format!("runs the service «{name}» (in its jail, checked against what was approved)")
        };
        self.manage_answer(
            req,
            true,
            "manage/free",
            format!(
                "Clio {does} when this turn ends and says so in the chat; you hear what came of it right after, in a turn Clio starts by herself (or with the person's next message). It needs no approval: do not ask the person for one."
            ),
        )
    }

    /// `on`, `undo`, `remove` (and, in a `suspect` session, all but `status`): set aside for the
    /// person, hash-bound like every pending item, with `reason` when no window may answer it.
    fn manage_aside(
        &self,
        req: &Request,
        h: &crate::host::Host,
        class: Class,
        args: Value,
        reason: Option<&str>,
    ) -> Response {
        let now = now_ms();
        let name = args["name"].as_str().unwrap_or("").to_owned();
        let p = Pending {
            id: String::new(),
            class,
            tool: req.tool.clone(),
            args,
            hash: String::new(),
            what: name,
            run: h.cfg.run.clone(),
            session: req.session.clone(),
            created_ms: now,
            seq: 0,
            expires_ms: now + 24 * 3600 * 1000,
            state: State::Waiting,
            reason: reason.map(str::to_owned),
            granted: None,
        };
        let content = match set_aside(&h.cfg.state, p) {
            Ok(p) => {
                let action = p.args["action"].as_str().unwrap_or("");
                if let Err(e) = log_line(
                    &h.cfg.state,
                    &p.run,
                    &p.session,
                    &p.tool,
                    &format!("{action} {}", p.what),
                    false,
                    "aside",
                    "manage/aside",
                    None,
                ) {
                    eprintln!("clio-toolhost: the action log lost a line: {e}");
                }
                let st = &h.cfg.state;
                let bypass = crate::grant::Kind::Bypass;
                match self.answered_later(st, &p) {
                    // Bypass does it when this turn ends: said as it will happen.
                    Some(crate::grant::Kind::Bypass) => format!(
                        "set aside {}: «{}» is done by Clio by herself when this turn ends, under the person's bypass; you hear what came of it right after, in a turn she starts by herself. Do not ask the person for it.",
                        p.id, p.class
                    ),
                    // A window may end before the turn does: said as a condition.
                    _ if reason.is_none()
                        && bypass.current(st).is_none()
                        && self.acts_unasked(&p.session) =>
                    {
                        format!(
                            "{} If the person's permission window is open, Clio does it when this turn ends, and you hear what came of it right after.",
                            self.kept_text(st, &p)
                        )
                    }
                    _ => self.kept_text(st, &p),
                }
            }
            Err(e) => format!(
                "[toolhost] {} was not done: it needs the person's approval and could not be set aside ({e})",
                req.tool
            ),
        };
        Response {
            id: req.id.clone(),
            ok: false,
            content: content.clone(),
            data: Value::Null,
            decision: DecisionOut {
                outcome: "ask".into(),
                rule: "manage/aside".into(),
                reason: content,
            },
            tainted: self.is_tainted(&req.session),
            external: false,
        }
    }
}

fn state_word(r: &Value) -> String {
    if r["enabled"] == Value::Bool(true) {
        "on".into()
    } else {
        format!("off ({})", r["off"].as_str().unwrap_or("off"))
    }
}

fn skill_line(r: &Value) -> String {
    let fails = r["failures"].as_u64().unwrap_or(0);
    format!(
        "{} [{}] version {}{}: {}",
        r["name"].as_str().unwrap_or(""),
        state_word(r),
        r["current"],
        if fails > 0 {
            format!(" · {fails} failed in a row")
        } else {
            String::new()
        },
        r["description"].as_str().unwrap_or("")
    )
}
