//! Clio asks the person (`ask_person`, through Pi's own extension dialogs): a box titled
//! `Clio asks` with the question, its answers as a list (↑/↓ and Enter, or the digit), and
//! `Other…`, which opens a one-line field. Esc skips the question; it never aborts the turn.
//!
//! It is not the approval box (`Visto bueno`): another title and border, and nothing here can
//! approve an action set aside or take a secret. The question and the answers are the model's
//! text: cleaned, capped and drawn as plain text. Like an approval, it takes its first key only
//! after the quiet second (`App::quiet`), so what the person was typing never answers it.
//!
//! A secret the model asked for (`request_secret`) has its own box, `Clio asks for a secret`, at
//! the end of the turn: its name, where it may be sent, why, and a masked field. The value lives
//! only here, in a `SecretValue` (zeroed when dropped), and goes from here to the vault.

use std::cell::RefCell;
use std::hash::{DefaultHasher, Hash, Hasher};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};

use std::sync::Arc;

use clio_vault::SecretValue;

use super::app::{self, App, Cmd};
use super::{sanitize, text};

/// The last row of every question the guard asks: the person types their own answer.
pub const OTHER: &str = "Other…";
/// How the guard joins an answer and its description in one option string.
const SEP: &str = " — ";
pub const MAX_QUESTION: usize = 300;
const MAX_LABEL: usize = 60;
const MAX_DESC: usize = 120;
const MAX_OPTIONS: usize = 4;
const MAX_TYPED: usize = 2000;
/// A dialog with no time of its own closes after this long (the guard always gives one).
const DEFAULT_WAIT_MS: u64 = 10 * 60 * 1000;

/// One question of the guard's, on screen or waiting its turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// The Pi dialog it answers now (the `select`, then the `input` after `Other…`).
    pub id: String,
    /// Only the session that asked can answer.
    pub session: String,
    pub text: String,
    /// Label and description, cleaned and capped.
    pub options: Vec<(String, String)>,
    /// The option strings as Pi sent them: an answer is one of these, exactly.
    raw: Vec<String>,
    /// The guard offered `Other…` (it always does).
    pub other: bool,
    pub sel: usize,
    /// A key was taken: the quiet second is over for this question.
    pub ready: bool,
    /// `Other…`: what the person typed so far.
    pub typing: Option<String>,
    /// `Other…` went to Pi; its `input` dialog has not come yet. An answer decided meanwhile
    /// waits in `decided` (`Some(None)`: skipped).
    pub awaiting: bool,
    decided: Option<Option<String>>,
    pub shown_ms: u64,
    /// Pi gives up on the dialog then (it answers the model by itself): the box goes too.
    pub deadline_ms: u64,
    /// Where the last draw put the option rows, the option each row belongs to (a long one
    /// takes more than one), and what they said: a click picks a row only if the row drawn there
    /// is still that row.
    pub rows_at: RefCell<(Rect, Vec<usize>, u64)>,
}

/// Model text for the box: one line, cleaned, at most `max` characters.
fn clip(s: &str, max: usize) -> String {
    sanitize::line(s).trim().chars().take(max).collect()
}

impl Question {
    /// From the guard's `select`: `label — description` strings, `Other…` last.
    pub fn from_select(
        id: &str,
        session: &str,
        title: &str,
        options: &[String],
        timeout_ms: Option<u64>,
        now_ms: u64,
    ) -> Option<Self> {
        let raw: Vec<String> = options
            .iter()
            .filter(|o| *o != OTHER)
            .take(MAX_OPTIONS)
            .cloned()
            .collect();
        let shown: Vec<(String, String)> = raw
            .iter()
            .map(|o| {
                let (l, d) = o.split_once(SEP).unwrap_or((o, ""));
                (clip(l, MAX_LABEL), clip(d, MAX_DESC))
            })
            .collect();
        if shown.is_empty() || shown.iter().any(|(l, _)| l.is_empty()) {
            return None;
        }
        Some(Self {
            id: id.to_owned(),
            session: session.to_owned(),
            text: clip(title, MAX_QUESTION),
            options: shown,
            raw,
            other: options.iter().any(|o| o == OTHER),
            sel: 0,
            ready: false,
            typing: None,
            awaiting: false,
            decided: None,
            shown_ms: now_ms,
            deadline_ms: now_ms.saturating_add(timeout_ms.unwrap_or(DEFAULT_WAIT_MS)),
            rows_at: RefCell::new((Rect::default(), vec![], 0)),
        })
    }

    /// From an `input` that came on its own: the field only.
    pub fn from_input(
        id: &str,
        session: &str,
        title: &str,
        timeout_ms: Option<u64>,
        now_ms: u64,
    ) -> Self {
        Self {
            id: id.to_owned(),
            session: session.to_owned(),
            text: clip(title, MAX_QUESTION),
            options: vec![],
            raw: vec![],
            other: false,
            sel: 0,
            ready: false,
            typing: Some(String::new()),
            awaiting: false,
            decided: None,
            shown_ms: now_ms,
            deadline_ms: now_ms.saturating_add(timeout_ms.unwrap_or(DEFAULT_WAIT_MS)),
            rows_at: RefCell::new((Rect::default(), vec![], 0)),
        }
    }

    /// The rows of the list: the answers, and `Other…`.
    fn rows(&self) -> usize {
        self.options.len() + usize::from(self.other)
    }

    fn rows_id(&self) -> u64 {
        let mut h = DefaultHasher::new();
        (&self.id, &self.options, self.other, self.typing.is_some()).hash(&mut h);
        h.finish()
    }
}

/// Rows an answer takes at most: its label and description wrap onto dim rows under it.
const OPTION_ROWS: usize = 3;
/// Under the `Other…` field, always: a question is the model's, and could be after a secret.
pub const NO_SECRETS: &str = "Never type a secret here; Clio asks for secrets in their own box.";

/// The longest value the secret field takes, in bytes: its buffer never grows.
pub const MAX_SECRET: usize = 8192;

/// A secret the model asked for, waiting for the person's value.
pub struct SecretAsk {
    pub id: String,
    pub session: String,
    pub name: String,
    pub destination: String,
    pub why: String,
    /// The judge flagged the session: the box says so.
    pub suspect: bool,
    pub value: SecretValue,
    pub ready: bool,
    pub shown_ms: u64,
}

impl SecretAsk {
    pub fn new(
        id: &str,
        session: &str,
        (name, destination, why): (&str, &str, &str),
        suspect: bool,
    ) -> Self {
        Self {
            id: id.to_owned(),
            session: session.to_owned(),
            name: clip(name, 64),
            destination: clip(destination, 200),
            why: clip(why, 200),
            suspect,
            value: SecretValue::with_capacity(MAX_SECRET),
            ready: false,
            shown_ms: 0,
        }
    }
}

/// `BlockAsk::action` of a request about what Clio created (`*_manage`).
pub const MANAGE: &str = "manage";

/// The warning a box shows when the judge flagged the session (`suspect`).
pub const SUSPECT_LINE: &str =
    "this session read content that tried to give orders: be sure you want this";

/// What a row of a block change is, for its style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Head,
    Label,
    Text,
    Warn,
}

/// A change to one of the person's blocks set aside by a marked session, as the approval box and
/// `clio approve` show it: everything from the stored item, cleaned, nothing cut (S-36).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockAsk {
    pub name: String,
    pub action: String,
    pub id: Option<i64>,
    pub text: Option<String>,
    pub old: Option<String>,
    /// The judge flagged the session: the box says so.
    pub suspect: bool,
    /// The first row shown, when the change is longer than the box.
    pub scroll: usize,
    /// How far the last draw let it scroll, and whether it has shown the last row: only then
    /// can `y` arm it.
    pub max: std::cell::Cell<usize>,
    pub seen_end: std::cell::Cell<bool>,
    /// A skill to register (S-68) instead of a block change: drawn whole the same way.
    pub skill: Option<SkillAsk>,
    /// Rows already made, and what `y` does (an MCP call set aside, an MCP server's tools to
    /// confirm): drawn whole the same way.
    pub plain: Option<(Vec<(Part, String)>, String)>,
}

/// A skill set aside for the person: what it is for, why it waits, and every file of it whole
/// (SKILL.md and the rest: scripts too), all from the stored item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillAsk {
    pub description: String,
    pub reason: String,
    pub update: bool,
    pub skill_md: String,
    /// Each other file: its path and its content, whole.
    pub files: Vec<(String, String)>,
    /// A service (S-69): what the manifest grants, one row each (schedule, command, hosts,
    /// secrets and every host each can reach, reads, writes, limits), the review's verdict and
    /// its notes; `None` for a skill.
    pub service: Option<Vec<String>>,
    /// An MCP server (S-70): the package, npm's facts about it, hosts, secrets and every host
    /// each can reach, reads, writes, limits, and that it is someone else's code; `None` else.
    pub mcp: Option<Vec<String>>,
}

/// A service's rows for its box, from the stored item: `None` if its manifest does not read.
fn service_rows(p: &clio_toolhost::pending::Pending, manifest: &str) -> Option<Vec<String>> {
    let mut rows = grant_rows(manifest)?;
    rows.push(format!(
        "review: {}",
        match p.args["verdict"].as_str().unwrap_or("") {
            "could_not_check" => "could not check",
            v => v,
        }
    ));
    let notes = p.args["notes"].as_str().unwrap_or("").trim();
    if !notes.is_empty() {
        rows.push("review notes:".into());
        rows.extend(notes.lines().map(str::to_owned));
    }
    Some(rows)
}

/// What a service's manifest grants, one row each (schedule, command, hosts, secrets and every
/// host each can reach, reads, writes, limits): `None` if it does not read.
fn grant_rows(manifest: &str) -> Option<Vec<String>> {
    let m: clio_toolhost::services::Manifest = serde_json::from_str(manifest).ok()?;
    let none = |v: &[String]| {
        if v.is_empty() {
            "none".to_owned()
        } else {
            v.join(", ")
        }
    };
    Some(vec![
        format!(
            "schedule: every {}{}",
            m.schedule.every,
            m.schedule
                .at
                .as_ref()
                .map_or_else(String::new, |a| format!(" at {a} (local time)"))
        ),
        format!("command: {}", m.command),
        format!("hosts it may reach: {}", none(&m.network)),
        // A secret is in the run's variables: it can go to every host above, not only its own.
        format!(
            "secrets: {}",
            none(
                &m.secrets
                    .iter()
                    .map(|s| format!(
                        "{} (as ${}) can reach {}",
                        s.name,
                        s.env,
                        m.network.join(", ")
                    ))
                    .collect::<Vec<_>>()
            )
        ),
        format!(
            "it may read: {}",
            if m.reads.is_empty() {
                "nothing in your home".to_owned()
            } else {
                m.reads.join(", ")
            }
        ),
        format!(
            "it may write: its state folder{}",
            m.writes
                .iter()
                .map(|w| format!(", {w}"))
                .collect::<String>()
        ),
        format!(
            "limits: {}% CPU, {} MB of memory (its /tmp counts), {} processes, {} s; for the whole run where your user's systemd can hold them, else for each process",
            m.limits.cpu_percent,
            m.limits.memory_mb,
            clio_node::host::SERVICE_TASKS,
            m.limits.seconds
        ),
    ])
}

/// A size for a person: bytes, or KiB.
fn size(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else {
        format!("{:.1} KiB", n as f64 / 1024.0)
    }
}

/// A skill the person adds from outside (`skill_add`'s item, `a`), as its box and `clio skills
/// add` show it: what it is, where from (origin, ref, commit, folder), what the judge said in
/// plain words, every file with its size and whether it is a script, what was left out, and what
/// of a plugin is not installed. Every list whole.
pub fn added_rows(a: &serde_json::Value) -> Vec<(Part, String)> {
    let s = |k: &str| {
        sanitize::line(a[k].as_str().unwrap_or(""))
            .trim()
            .to_owned()
    };
    let mut rows = vec![(
        Part::Head,
        format!(
            "Add {} «{}» from {}?",
            if a["update"].as_bool() == Some(true) {
                "a new version of the skill"
            } else {
                "the skill"
            },
            s("name"),
            s("origin")
        ),
    )];
    rows.push((Part::Label, format!("what it is for: {}", s("description"))));
    let mut from = s("origin");
    if !s("ref").is_empty() && s("ref") != "HEAD" {
        from += &format!("@{}", s("ref"));
    }
    if !s("commit").is_empty() {
        from += &format!(" at commit {}", s("commit"));
    }
    if !s("from").is_empty() {
        from += &format!(", folder {}", s("from"));
    }
    rows.push((Part::Label, format!("from: {from}")));
    if !s("why").is_empty() {
        rows.push((Part::Label, format!("Clio's reason: {}", s("why"))));
    }
    rows.push(match a["judge"].as_str().unwrap_or("") {
        "clean" => (
            Part::Label,
            "the judge read every file: nothing in it tries to give Clio orders".into(),
        ),
        "flagged" => (
            Part::Warn,
            "the judge found text in it that tries to give Clio orders: Clio would read it as her own. No is the default; add it only if you know why that text is there.".into(),
        ),
        word => (
            Part::Warn,
            format!(
                "not judged ({}): nobody read this text for hidden orders. No is the default; add it only if you trust where it comes from.",
                if word == "off" {
                    "the judge is off"
                } else {
                    "the judge did not answer"
                }
            ),
        ),
    });
    let files: Vec<(String, String)> = std::iter::once((
        "SKILL.md".to_owned(),
        a["skill_md"].as_str().unwrap_or("").to_owned(),
    ))
    .chain(a["files"].as_array().into_iter().flatten().map(|f| {
        (
            f["path"].as_str().unwrap_or("").to_owned(),
            f["content"].as_str().unwrap_or("").to_owned(),
        )
    }))
    .collect();
    let total: usize = files.iter().map(|(_, c)| c.len()).sum();
    rows.push((
        Part::Label,
        format!("{} files, {}:", files.len(), size(total)),
    ));
    for (path, content) in &files {
        let script = content.starts_with("#!")
            || [
                ".sh", ".py", ".js", ".mjs", ".ts", ".rb", ".pl", ".ps1", ".bash",
            ]
            .iter()
            .any(|e| path.ends_with(e));
        rows.push((
            Part::Text,
            format!(
                "{}  {}{}",
                sanitize::line(path).trim(),
                size(content.len()),
                if script {
                    "  script: runs only if Clio runs it"
                } else {
                    ""
                }
            ),
        ));
    }
    let list = |k: &str| -> Vec<String> {
        a[k].as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .map(|v| sanitize::line(v).trim().to_owned())
            .collect()
    };
    let skipped = list("skipped");
    if !skipped.is_empty() {
        rows.push((Part::Label, "not taken:".into()));
        rows.extend(skipped.into_iter().map(|f| (Part::Text, f)));
    }
    let left = list("left");
    if !left.is_empty() {
        rows.push((
            Part::Label,
            format!(
                "not installed: the plugin's {} (Clio takes only its skills)",
                left.join(", ")
            ),
        ));
    }
    rows
}

impl BlockAsk {
    pub fn new(p: &clio_toolhost::pending::Pending, suspect: bool) -> Option<Self> {
        let clean = |s: &str| sanitize::line(s).trim().to_owned();
        if p.tool == "mcp_call" {
            return Some(Self::mcp_call(p, suspect));
        }
        // A skill from outside: what it is, where from and what the judge said, every list whole.
        if p.tool == clio_toolhost::skill_add::TOOL {
            let mut b = Self::rows(
                p.args["name"].as_str().unwrap_or(""),
                added_rows(&p.args),
                " yes, add it",
            );
            b.suspect = suspect;
            return Some(b);
        }
        // What the model asked of what it created: the act in plain words and Clio's reason.
        if let Some((does, why)) = clio_toolhost::manage::what(p) {
            let mut head = clean(&does);
            if let Some(c) = head.get(..1) {
                head = format!("{}{}?", c.to_uppercase(), &head[1..]);
            }
            // The box's label line already says why it always asks, when it does.
            let mut rows = vec![
                (Part::Head, head),
                (Part::Label, format!("Clio's reason: {}", clean(&why))),
            ];
            // A service's `undo`: what the version it goes back to was granted.
            if let Some(older) = p.args["older"].as_str() {
                rows.push((
                    Part::Label,
                    format!("version {} may:", p.args["to"].as_u64().unwrap_or(0)),
                ));
                match grant_rows(older) {
                    Some(g) => rows.extend(g.into_iter().map(|r| (Part::Text, clean(&r)))),
                    None => rows.push((Part::Warn, "its manifest does not read".into())),
                }
            }
            let yes = match p.args["action"].as_str().unwrap_or("") {
                "remove" => " yes, remove it",
                "undo" => " yes, go back",
                "on" => " yes, turn it on",
                "off" => " yes, turn it off",
                "run" => " yes, run it",
                _ => " yes, show it",
            };
            let mut b = Self::rows(p.args["name"].as_str().unwrap_or(""), rows, yes);
            b.action = MANAGE.into();
            b.suspect = suspect;
            return Some(b);
        }
        let service = clio_toolhost::pending::service_of(p);
        let mcp = clio_toolhost::pending::mcp_of(p);
        if let Some((name, description, md, files)) = clio_toolhost::pending::skill_of(p)
            .or(service.clone())
            .or(mcp.clone())
        {
            return Some(Self {
                name: clean(name),
                action: "register".into(),
                id: None,
                text: None,
                old: None,
                suspect,
                scroll: 0,
                max: std::cell::Cell::new(0),
                seen_end: std::cell::Cell::new(false),
                plain: None,
                skill: Some(SkillAsk {
                    description: clean(description),
                    reason: clean(p.reason.as_deref().unwrap_or("")),
                    update: p.args["update"].as_bool() == Some(true),
                    skill_md: sanitize::multiline(md),
                    files: files
                        .iter()
                        .map(|(path, content)| (clean(path), sanitize::multiline(content)))
                        .collect(),
                    service: match service {
                        Some(_) => Some(
                            service_rows(p, md)?
                                .iter()
                                .map(|r| sanitize::line(r).trim_end().to_owned())
                                .collect(),
                        ),
                        None => None,
                    },
                    mcp: match mcp {
                        // The first row repeats the name and the description.
                        Some(_) => Some(
                            crate::cli::mcp_servers::proposal_rows(p)?
                                .into_iter()
                                .skip(1)
                                .collect(),
                        ),
                        None => None,
                    },
                }),
            });
        }
        let b = clio_toolhost::pending::block_of(p)?;
        Some(Self {
            name: clean(b.name),
            action: clean(b.action),
            id: b.id,
            text: b.text.map(clean),
            old: b.old.map(clean),
            suspect,
            scroll: 0,
            max: std::cell::Cell::new(0),
            seen_end: std::cell::Cell::new(false),
            skill: None,
            plain: None,
        })
    }

    /// A box of `rows` made elsewhere, `yes` under it.
    pub fn rows(name: &str, rows: Vec<(Part, String)>, yes: &str) -> Self {
        Self {
            name: sanitize::line(name),
            action: String::new(),
            id: None,
            text: None,
            old: None,
            suspect: false,
            scroll: 0,
            max: std::cell::Cell::new(0),
            seen_end: std::cell::Cell::new(false),
            skill: None,
            plain: Some((rows, yes.to_owned())),
        }
    }

    /// An MCP tool's call set aside (S-70): the server, the tool, why it waits, and the whole
    /// arguments, pretty-printed and cleaned (up to 32 KiB, keys sorted: nothing may fall off).
    fn mcp_call(p: &clio_toolhost::pending::Pending, suspect: bool) -> Self {
        let a = &p.args;
        let s = |k: &str| {
            sanitize::line(a[k].as_str().unwrap_or(""))
                .trim()
                .to_owned()
        };
        let mut rows = vec![
            (
                Part::Head,
                format!(
                    "Clio wants to call a tool of the MCP server «{}».",
                    s("server")
                ),
            ),
            (
                Part::Label,
                format!("server: {} (version {})", s("server"), s("version")),
            ),
            (Part::Label, format!("tool: {}", s("tool"))),
            (
                Part::Warn,
                format!(
                    "why it waits for you: {}",
                    sanitize::line(p.reason.as_deref().unwrap_or("")).trim()
                ),
            ),
            (Part::Label, "arguments:".into()),
        ];
        let pretty = serde_json::to_string_pretty(&a["arguments"]).unwrap_or_default();
        rows.extend(sanitize::multiline(&pretty).lines().map(|l| {
            (
                Part::Text,
                if l.is_empty() {
                    " ".into()
                } else {
                    l.to_owned()
                },
            )
        }));
        let mut b = Self::rows(&s("server"), rows, " yes, run it");
        b.suspect = suspect;
        b
    }

    /// What `y` does, for the keys under the box.
    pub fn yes(&self) -> &str {
        if let Some((_, yes)) = &self.plain {
            yes
        } else if self.skill.as_ref().is_some_and(|s| s.mcp.is_some()) {
            " yes, install it and turn it on"
        } else if self.skill.is_some() {
            " yes, register it"
        } else {
            " yes, write it"
        }
    }

    /// A skill's rows: what it is, why it waits, every file line by line under `--- <path> ---`,
    /// the warning.
    fn skill_parts(&self, s: &SkillAsk) -> Vec<(Part, String)> {
        let kind = if s.service.is_some() {
            "service"
        } else if s.mcp.is_some() {
            "MCP server"
        } else {
            "skill"
        };
        let mut out = vec![
            (
                Part::Head,
                format!(
                    "Clio wants to {} {} «{}».",
                    if s.mcp.is_some() {
                        "install"
                    } else {
                        "register"
                    },
                    if s.update {
                        format!("a new version of the {kind}")
                    } else {
                        format!("the {kind}")
                    },
                    self.name
                ),
            ),
            (Part::Label, format!("what it is for: {}", s.description)),
            (Part::Warn, format!("why it waits for you: {}", s.reason)),
        ];
        // A service: what it is granted, then the review, before its files.
        for row in s.service.iter().flatten() {
            out.push((Part::Label, row.clone()));
        }
        // An MCP server: the package and npm's facts, what it is granted, and whose code it is.
        if let Some(rows) = &s.mcp {
            let n = rows.len();
            for (i, row) in rows.iter().enumerate() {
                // The last two say whose code it is and what approving does, and a deprecation is
                // the registry's warning: in the warning color.
                let part = if i + 2 >= n || row.starts_with("deprecated") {
                    Part::Warn
                } else {
                    Part::Label
                };
                out.push((part, row.clone()));
            }
        }
        let main = if s.service.is_some() {
            clio_toolhost::services::MANIFEST
        } else if s.mcp.is_some() {
            clio_toolhost::mcp::MANIFEST
        } else {
            "SKILL.md"
        };
        // Every file whole, scripts too; an empty line still takes a row: shown as it is.
        for (path, text) in std::iter::once((main, s.skill_md.as_str()))
            .chain(s.files.iter().map(|(p, t)| (p.as_str(), t.as_str())))
        {
            out.push((Part::Label, format!("--- {path} ---")));
            out.extend(text.lines().map(|l| {
                (
                    Part::Text,
                    if l.is_empty() {
                        " ".into()
                    } else {
                        l.to_owned()
                    },
                )
            }));
        }
        if self.suspect {
            out.push((Part::Warn, SUSPECT_LINE.into()));
        }
        out
    }

    /// Its rows, in order: that it is written as said by the person, the block, the action, the
    /// text whole, the entry it replaces or removes, and the warning of a suspect session.
    pub fn parts(&self) -> Vec<(Part, String)> {
        if let Some(s) = &self.skill {
            return self.skill_parts(s);
        }
        if let Some((rows, _)) = &self.plain {
            let mut out = rows.clone();
            if self.suspect {
                out.push((Part::Warn, SUSPECT_LINE.into()));
            }
            return out;
        }
        let id = self.id.map_or_else(String::new, |n| format!(" m{n}"));
        let mut out = vec![
            (
                Part::Head,
                "This is written to your own block, as said by you.".to_owned(),
            ),
            (Part::Label, format!("block: {}", self.name)),
            (
                Part::Label,
                match self.action.as_str() {
                    "add" => "action: add an entry".to_owned(),
                    a => format!("action: {a} entry{id}"),
                },
            ),
        ];
        if let Some(t) = &self.text {
            out.push((Part::Label, "text:".into()));
            out.push((Part::Text, t.clone()));
        }
        if let Some(o) = &self.old {
            out.push((
                Part::Label,
                format!(
                    "the entry it {}, as it reads now:",
                    if self.action == "remove" {
                        "removes"
                    } else {
                        "replaces"
                    }
                ),
            ));
            out.push((Part::Text, o.clone()));
        }
        if self.suspect {
            out.push((Part::Warn, SUSPECT_LINE.into()));
        }
        out
    }
}

/// The value on its way from the box to the vault: shared, never copied, printed as `[redacted]`.
#[derive(Clone)]
pub struct SecretBox(pub Arc<SecretValue>);

impl std::fmt::Debug for SecretBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretBox([redacted])")
    }
}

impl PartialEq for SecretBox {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for SecretBox {}

/// What waits for the person besides an approval: a question of the guard's, or a secret.
pub enum Prompt {
    Question(Question),
    Secret(SecretAsk),
}

impl Prompt {
    pub fn session(&self) -> &str {
        match self {
            Self::Question(q) => &q.session,
            Self::Secret(s) => &s.session,
        }
    }

    pub fn shown_ms(&self) -> u64 {
        match self {
            Self::Question(q) => q.shown_ms,
            Self::Secret(s) => s.shown_ms,
        }
    }

    /// The quiet second is over for it.
    pub fn ready(&self) -> bool {
        match self {
            Self::Question(q) => q.ready,
            Self::Secret(s) => s.ready,
        }
    }

    pub fn show(&mut self, now_ms: u64) {
        match self {
            Self::Question(q) => q.shown_ms = now_ms,
            Self::Secret(s) => s.shown_ms = now_ms,
        }
    }

    /// Pi has already given up on it (a secret request does not end with the turn).
    pub fn expired(&self, now_ms: u64) -> bool {
        match self {
            Self::Question(q) => now_ms >= q.deadline_ms,
            // It waits a day, in `clio pending` too: the toolhost says when it is gone.
            Self::Secret(_) => false,
        }
    }
}

/// An extension dialog came from Pi: a question for the person, or the field that follows
/// `Other…`. Anything else is not drawn here.
pub fn on_request(
    app: &mut App,
    id: &str,
    method: &str,
    title: &str,
    options: &[String],
    timeout_ms: Option<u64>,
) -> Vec<Cmd> {
    let (now, session) = (app.now_ms, app.chat.session_id.clone());
    match method {
        "select" => {
            if let Some(q) = Question::from_select(id, &session, title, options, timeout_ms, now) {
                app.offer(Prompt::Question(q));
            }
            vec![]
        }
        "input" => {
            if let Some(Prompt::Question(q)) = &mut app.prompt
                && q.awaiting
            {
                q.id = id.to_owned();
                q.awaiting = false;
                q.deadline_ms = now.saturating_add(timeout_ms.unwrap_or(DEFAULT_WAIT_MS));
                if let Some(answer) = q.decided.take() {
                    return close(app, answer);
                }
                return vec![];
            }
            app.offer(Prompt::Question(Question::from_input(
                id, &session, title, timeout_ms, now,
            )));
            vec![]
        }
        _ => vec![],
    }
}

/// The box goes; Pi gets the answer (`None`: skipped).
fn close(app: &mut App, value: Option<String>) -> Vec<Cmd> {
    match app.prompt.take() {
        Some(Prompt::Question(q)) => vec![Cmd::ChatUi { id: q.id, value }],
        other => {
            app.prompt = other;
            vec![]
        }
    }
}

/// A terminal that sends Shift apart sends Shift+1 as `1` with SHIFT: what was meant depends on
/// the layout, so a non-letter key with SHIFT is not taken (Shift+Space is a space anywhere).
fn shift_apart(k: &KeyEvent) -> bool {
    matches!(k.code, KeyCode::Char(c) if !c.is_alphabetic() && c != ' ')
        && k.modifiers.contains(KeyModifiers::SHIFT)
}

/// What the flash says when `shift_apart` refused a key.
const SHIFT_APART: &str = "this terminal sends Shift apart";

/// The person picked row `i`: an answer goes to Pi; `Other…` opens the field.
fn choose(app: &mut App, i: usize) -> Vec<Cmd> {
    let Some(Prompt::Question(q)) = &mut app.prompt else {
        return vec![];
    };
    if i < q.raw.len() {
        let value = q.raw[i].clone();
        return close(app, Some(value));
    }
    if q.other && i == q.raw.len() {
        q.typing = Some(String::new());
        q.awaiting = true;
        return vec![Cmd::ChatUi {
            id: q.id.clone(),
            value: Some(OTHER.into()),
        }];
    }
    vec![]
}

/// A key while the question is on screen.
pub fn key(app: &mut App, k: &KeyEvent, quiet: bool) -> Vec<Cmd> {
    if matches!(app.prompt, Some(Prompt::Secret(_))) {
        return secret_key(app, k, quiet);
    }
    let Some(Prompt::Question(q)) = &mut app.prompt else {
        return vec![];
    };
    // Before the quiet second nothing counts: what the person was typing never answers it.
    if !q.ready && !quiet {
        return vec![];
    }
    q.ready = true;
    let plain = (k.modifiers - KeyModifiers::SHIFT).is_empty();
    if shift_apart(k) {
        app.flash = Some(if q.typing.is_some() {
            format!("{SHIFT_APART}: type it without that character")
        } else {
            format!("{SHIFT_APART}: use the arrows and Enter")
        });
        return vec![];
    }
    if let Some(typed) = &mut q.typing {
        let answer = match k.code {
            KeyCode::Esc => Some(None),
            KeyCode::Enter if plain && !typed.trim().is_empty() => {
                Some(Some(typed.trim().to_owned()))
            }
            KeyCode::Backspace => {
                typed.pop();
                None
            }
            KeyCode::Char(c)
                if !k
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && !c.is_control()
                    && !clio_brain::pi::text::is_deceptive(c)
                    && typed.chars().count() < MAX_TYPED =>
            {
                typed.push(c);
                None
            }
            _ => None,
        };
        return match answer {
            Some(a) if q.awaiting => {
                q.decided = Some(a);
                vec![]
            }
            Some(a) => close(app, a),
            None => vec![],
        };
    }
    let n = q.rows();
    match k.code {
        KeyCode::Esc => close(app, None),
        KeyCode::Up => {
            q.sel = q.sel.saturating_sub(1);
            vec![]
        }
        KeyCode::Down => {
            q.sel = (q.sel + 1).min(n.saturating_sub(1));
            vec![]
        }
        KeyCode::Enter if plain => {
            let i = q.sel;
            choose(app, i)
        }
        KeyCode::Char(d @ '1'..='9') if plain => {
            let i = usize::from(d as u8 - b'1');
            if i < n { choose(app, i) } else { vec![] }
        }
        _ => vec![],
    }
}

/// A click on an answer selects it (it never answers): only if the row drawn there is still
/// that row.
pub fn click(app: &mut App, m: &MouseEvent) {
    let Some(Prompt::Question(q)) = &mut app.prompt else {
        return;
    };
    if m.kind != MouseEventKind::Down(MouseButton::Left) || q.typing.is_some() {
        return;
    }
    let picked = {
        let (area, owner, drawn) = &*q.rows_at.borrow();
        if !area.contains(Position::new(m.column, m.row)) || *drawn != q.rows_id() {
            return;
        }
        owner.get(usize::from(m.row - area.y)).copied()
    };
    if let Some(i) = picked {
        q.sel = i;
    }
}

/// The box: the question, the answers (the first is the recommended one), `Other…`, the field
/// when it is open, and the keys.
pub fn draw(f: &mut Frame, area: Rect, app: &App, q: &Question) {
    let t = app.theme;
    let w = app::dialog_inner(area);
    let mut rows: Vec<Line<'static>> = Vec::new();
    rows.extend(app::wrap_line(
        Line::from(Span::styled(q.text.clone(), t.bold())),
        w,
    ));
    rows.push(Line::from(""));
    let first = rows.len();
    // The option of each row, for a click.
    let mut owner = Vec::new();
    for (i, (label, desc)) in q.options.iter().enumerate() {
        let mut head = format!("{}  {label}", i + 1);
        if i == 0 {
            head.push_str(" (recommended)");
        }
        let line = if desc.is_empty() {
            head
        } else {
            format!("{head}  {} {desc}", t.dot())
        };
        let selected = i == q.sel && q.typing.is_none();
        let option = option_rows(t, &line, selected, w);
        owner.extend(std::iter::repeat_n(i, option.len()));
        rows.extend(option);
    }
    if q.other {
        let i = q.options.len();
        rows.push(row(
            t,
            &format!("{}  {OTHER}", i + 1),
            i == q.sel && q.typing.is_none(),
            w,
        ));
        owner.push(i);
    }
    if let Some(typed) = &q.typing {
        if !q.options.is_empty() {
            rows.push(Line::from(""));
        }
        let shown = text::fit(
            &format!("{typed}{}", t.cursor()),
            w.saturating_sub(4),
            t.ellipsis(),
        );
        rows.push(Line::from(vec![
            Span::styled(format!("  {} ", t.prompt()), t.accent_line()),
            Span::raw(shown),
        ]));
        rows.extend(app::wrap_line(
            Line::from(Span::styled(NO_SECRETS, t.dim())),
            w,
        ));
    }
    let waiting = !q.ready && !app.quiet();
    if waiting {
        rows.push(Line::from(""));
        rows.push(Line::from(Span::styled(
            "wait a second without typing to answer…",
            t.dim(),
        )));
    }
    rows.push(Line::from(""));
    let d = format!("   {}   ", t.dot());
    let hint = if q.typing.is_some() {
        Line::from(vec![
            Span::styled("Enter", t.key()),
            Span::styled(" send", t.dim()),
            Span::styled(d, t.dim()),
            Span::styled("Esc", t.key()),
            Span::styled(" skip the question", t.dim()),
        ])
    } else {
        Line::from(vec![
            Span::styled(format!("{}{} Enter", t.up(), t.down()), t.key()),
            Span::styled(format!(" or 1-{}", q.rows()), t.key()),
            Span::styled(" choose", t.dim()),
            Span::styled(d, t.dim()),
            Span::styled("Esc", t.key()),
            Span::styled(" skip the question", t.dim()),
        ])
    };
    rows.extend(app::wrap_line(hint, w));
    let at = app::dialog_box(f, area, t, " Clio asks ", rows, t.accent_line());
    if !owner.is_empty() {
        let y =
            at.y.saturating_add(u16::try_from(first).unwrap_or(u16::MAX));
        let h = u16::try_from(owner.len()).unwrap_or(0);
        // Only what the box really shows can be clicked.
        let area = Rect::new(at.x, y, at.width, h).intersection(at);
        owner.truncate(usize::from(area.height));
        q.rows_at.replace((area, owner, q.rows_id()));
    }
}

/// One answer: on one row when it fits; else wrapped, the rest on dim rows under its label, at
/// most `OPTION_ROWS` (the last cut with an ellipsis).
fn option_rows(t: super::theme::Theme, s: &str, selected: bool, w: usize) -> Vec<Line<'static>> {
    // The mark and its space, and the digit and its two spaces: where the label starts.
    let indent = 5;
    if text::width(s) + 2 <= w {
        return vec![row(t, s, selected, w)];
    }
    let room = w.saturating_sub(indent).max(1);
    let parts = text::wrap_ranges(s, room);
    let mut out = vec![row(t, &s[parts[0].0..parts[0].1], selected, w)];
    for (k, &(a, b)) in parts.iter().enumerate().skip(1).take(OPTION_ROWS - 1) {
        let last = k == OPTION_ROWS - 1 && parts.len() > OPTION_ROWS;
        // The last row takes the rest, cut: it ends in the ellipsis.
        let part = text::fit(if last { &s[a..] } else { &s[a..b] }, room, t.ellipsis());
        out.push(Line::from(Span::styled(
            format!("{}{part}", " ".repeat(indent)),
            t.dim(),
        )));
    }
    out
}

/// One answer, on one row: the selected one across the width.
fn row(t: super::theme::Theme, s: &str, selected: bool, w: usize) -> Line<'static> {
    let mark = if selected { t.sel() } else { " " };
    let s = text::fit(&format!("{mark} {s}"), w, t.ellipsis());
    if selected {
        let fill = " ".repeat(w.saturating_sub(text::width(&s)));
        Line::from(Span::styled(format!("{s}{fill}"), t.selection()))
    } else {
        Line::from(Span::raw(s))
    }
}

/// A key in the secret box: typed into the masked field; Enter sends it to the vault, Esc declines.
fn secret_key(app: &mut App, k: &KeyEvent, quiet: bool) -> Vec<Cmd> {
    let Some(Prompt::Secret(s)) = &mut app.prompt else {
        return vec![];
    };
    if !s.ready && !quiet {
        return vec![];
    }
    s.ready = true;
    let plain = (k.modifiers - KeyModifiers::SHIFT).is_empty();
    if shift_apart(k) {
        app.flash = Some(format!("{SHIFT_APART}: paste the value"));
        return vec![];
    }
    let value = match k.code {
        KeyCode::Esc => None,
        KeyCode::Enter if plain && !s.value.is_empty() => Some(std::mem::replace(
            &mut s.value,
            SecretValue::with_capacity(0),
        )),
        KeyCode::Backspace => {
            s.value.pop();
            return vec![];
        }
        KeyCode::Char(c)
            if !k
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                && !c.is_control() =>
        {
            s.value.push(c);
            return vec![];
        }
        _ => return vec![],
    };
    let Some(Prompt::Secret(s)) = app.prompt.take() else {
        return vec![];
    };
    app.answering += 1;
    vec![Cmd::ChatSecret {
        id: s.id,
        name: s.name,
        value: value.map(|v| SecretBox(Arc::new(v))),
    }]
}

/// A paste into the secret field, the only field of these boxes that takes one (a token is
/// pasted more often than typed). The line ends are not part of it.
pub fn paste(app: &mut App, text: &str, quiet: bool) {
    let Some(Prompt::Secret(s)) = &mut app.prompt else {
        return;
    };
    if !s.ready && !quiet {
        return;
    }
    s.ready = true;
    for c in text.chars().filter(|c| !c.is_control()) {
        if !s.value.push(c) {
            break;
        }
    }
}

/// The secret box: whose secret, where it may go (in the warning colour), why, and the field,
/// one dot per character.
pub fn draw_secret(f: &mut Frame, area: Rect, app: &App, s: &SecretAsk) {
    let t = app.theme;
    let w = app::dialog_inner(area);
    let mut rows: Vec<Line<'static>> = Vec::new();
    rows.extend(app::wrap_line(
        Line::from(vec![
            Span::styled("Clio asks you to store the secret ", t.plain()),
            Span::styled(s.name.clone(), t.bold()),
        ]),
        w,
    ));
    rows.push(Line::from(""));
    rows.extend(app::wrap_line(
        Line::from(Span::styled(
            format!("it can only be sent to {}", s.destination),
            t.warn(),
        )),
        w,
    ));
    rows.extend(app::wrap_line(
        Line::from(vec![
            Span::styled("why: ", t.dim()),
            Span::raw(s.why.clone()),
        ]),
        w,
    ));
    if s.suspect {
        rows.extend(app::wrap_line(
            Line::from(Span::styled(SUSPECT_LINE, t.warn())),
            w,
        ));
    }
    rows.push(Line::from(""));
    let dots = if t.unicode { "•" } else { "*" };
    let shown = text::fit(
        &format!("{}{}", dots.repeat(s.value.chars()), t.cursor()),
        w.saturating_sub(4),
        t.ellipsis(),
    );
    rows.push(Line::from(vec![
        Span::styled(format!("  {} ", t.prompt()), t.accent_line()),
        Span::raw(shown),
    ]));
    if !s.ready && !app.quiet() {
        rows.push(Line::from(""));
        rows.push(Line::from(Span::styled(
            "wait a second without typing to answer…",
            t.dim(),
        )));
    }
    rows.push(Line::from(""));
    let d = format!("   {}   ", t.dot());
    rows.extend(app::wrap_line(
        Line::from(vec![
            Span::styled("Enter", t.key()),
            Span::styled(" store it in the vault", t.dim()),
            Span::styled(d, t.dim()),
            Span::styled("Esc", t.key()),
            Span::styled(" decline", t.dim()),
        ]),
        w,
    ));
    app::dialog_box(
        f,
        area,
        t,
        " Clio asks for a secret ",
        rows,
        t.accent_line(),
    );
}
