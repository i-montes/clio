//! `clio memory ...`: look at what Clio remembers and decide what it keeps. Everything printed
//! comes from the memory file and is cleaned of escape sequences first. The commands that change
//! something (`remember`, `promote`, `archive`, `feedback`) act as the person; the agent has no
//! way to reach any of them (S-25, S-26). `purge` deletes for good, so it asks first and only the person can run it.

use clio_brain::memory_link::{
    BLOCK_NAMES, EmbedModel, MemoryLink, Res, kind_label, origin_label, state_label,
};
use clio_memory::{MemoryId, MemoryRow, Signal, State};
use serde_json::{Value, json};

use super::Args;
use super::apply::confirm;
use super::home::Dirs;
use super::tui::sanitize;
use super::ui::{self, Style, clip};

pub const USAGE: &str = "clio memory: what Clio remembers (all local, ~/.local/share/clio/memory.db)

  clio memory search <query> [--json]         search the active ones (quarantine does not show here)
  clio memory list [--pending] [--json]       recent ones; --pending: quarantine and procedures awaiting approval
  clio memory show <id> [--json]              one whole memory, with where it came from
  clio memory remember \"text\" [--block identidad|reglas|preferencias|vigilancia]
                                              a fact of yours (origin: persona), active at once
  clio memory identity [--json]               the primordial memories: who Clio is, how it works,
                                              who you are and what it watches (changed by talking to it)
  clio memory promote <id> [--yes]            approve a pending memory (only you; it asks first)
  clio memory archive <id> [--yes]            archive it: it is no longer remembered (it asks first)
  clio memory purge <id> [--reason \"reason\"] [--yes]
                                              deletes the content FOR GOOD (text, vector, history);
                                              leaves a tombstone with no content. It cannot be undone
  clio memory review [--json]                 quarantine, what was forgotten (same as list --pending)
  clio memory history <id> [--json]           every change that touched a memory (its versions)
  clio memory revert <change> [--yes]         undoes a change (its number comes from history)
  clio memory context [--json]                what a session that starts in this folder receives
  clio memory conflicts [--json]              memories Jev judged to contradict each other
  clio memory import <file>                   brings an export into an empty memory
  clio memory project [set <name>]            this folder's project and where it comes from; set fixes it (.clio-project)
  clio memory setup <agent>                   how to connect claude-code, cursor, codex, gemini or opencode over MCP
  clio memory setup sleep [--remove]          installs (or removes) the night sleep: a timer that runs
                                              `clio memory sleep` every half hour from 23:00 to 05:30
  clio memory update                          installs the latest clio-memory (the binary
                                              the other agents use) and updates its skill; Clio's own
                                              memory goes up when Clio is rebuilt
  clio memory sleep [--now] [--dry-run] [--report] [--json]
                                              one night of sleep: checks and copies, quarantines
                                              secrets and attacks, merges repeated and near-equal ones (with your
                                              model), purges what was discarded long ago. Only if nobody
                                              used the memory in 30 min and it did not sleep in 20 h (--now skips
                                              both waits; --dry-run only looks)
  clio memory projects [--json]               every place (personal, lessons, each project) and what it holds
  clio memory projects merge <from> <into>    moves a whole project to another (merge or rename; undoable)
  clio memory projects archive <project>      archives a whole project (does not delete it)
  clio memory review-due [--json]             what is past its review date (decisions, configuration...)
  clio memory reviewed <id>                   you looked at it and it is still true: the date starts again
  clio memory timeline <id> [--json]          what was saved before and after, in the same session
  clio memory pin|unpin <id>                  pins a memory (it comes first in the context) or lets it go
  clio memory feedback <id> useful|wrong      «wrong» only sends it to review; it never deletes
  clio memory export [file]                   JSON Lines (unencrypted); without a file, to standard output
  clio memory stats [--json]                  counts and size
  clio memory mcp                             MCP server for other agents (read and propose; nothing else)";

const SETUP_AGENTS: [&str; 5] = ["claude-code", "cursor", "codex", "gemini", "opencode"];

/// What to run or paste so `agent` reaches `clio memory mcp`. Clio does not edit another tool's
/// configuration: the person sees it and decides.
fn setup_text(agent: &str) -> Res<String> {
    let json =
        r#"{ "mcpServers": { "clio-memory": { "command": "clio", "args": ["memory", "mcp"] } } }"#;
    Ok(match agent {
        "claude-code" => "Run:\n  claude mcp add clio-memory -- clio memory mcp".into(),
        "cursor" => format!("Add to ~/.cursor/mcp.json (or the project's .cursor/mcp.json):\n  {json}"),
        "gemini" => format!("Add to ~/.gemini/settings.json:\n  {json}"),
        "codex" => "Add to ~/.codex/config.toml:\n  [mcp_servers.clio-memory]\n  command = \"clio\"\n  args = [\"memory\", \"mcp\"]".into(),
        "opencode" => "Add to opencode.json:\n  { \"mcp\": { \"clio-memory\": { \"type\": \"local\", \"command\": [\"clio\", \"memory\", \"mcp\"] } } }".into(),
        other => {
            return Err(format!(
                "I do not know {other:?}: {}. Any agent with MCP works with the command `clio memory mcp` over standard input",
                SETUP_AGENTS.join(", ")
            ));
        }
    } + "\nThe agent starts the server in the project folder: it reads and proposes there (nothing else).")
}

fn id_of(s: &str) -> Res<MemoryId> {
    s.trim_start_matches('m')
        .parse::<i64>()
        .map(MemoryId)
        .map_err(|_| format!("{s:?} is not a memory id (for example 12 or m12)"))
}

fn open_read(dirs: &Dirs, model: bool) -> Res<Option<MemoryLink>> {
    let m = if model {
        clio_brain::memory_daemon::shared(&dirs.data, EmbedModel::of_home(&dirs.home))
    } else {
        EmbedModel::none()
    };
    MemoryLink::open_read(&dirs.data, &m).map(|l| l.map(MemoryLink::for_person))
}

fn open_write(dirs: &Dirs) -> Res<MemoryLink> {
    MemoryLink::open_write(&dirs.data, &EmbedModel::none()).map(MemoryLink::for_person)
}

fn row_json(link: &MemoryLink, m: &MemoryRow) -> Value {
    json!({
        "id": m.id.0,
        "kind": kind_label(m.kind),
        "state": state_label(m.state),
        "origin": origin_label(m.origin),
        "actor": link.actor_name(m.actor),
        "content": m.content,
        "topic": m.topic_key,
        "source": m.source,
        "run": m.session,
        "title": m.title,
        "category": m.category.map(|c| c.label()),
        "place": link.place_label(m),
        "pinned": link.is_pinned(m),
        "relations": link.relation_notes(m),
        "valid_from": m.valid_from,
        "known_since": m.known_since,
    })
}

fn row_line(st: Style, m: &MemoryRow, width: usize) -> String {
    let head = format!(
        "m{:<4} {:<11} {:<13} {:<8} ",
        m.id.0,
        state_label(m.state),
        kind_label(m.kind),
        origin_label(m.origin)
    );
    let room = width.saturating_sub(head.chars().count()).max(20);
    format!("{head}{}", clip(st, &sanitize::line(&m.content), room))
}

fn print_json(v: &Value) -> Res<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(v).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn print_rows(st: Style, rows: &[MemoryRow], empty: &str) {
    if rows.is_empty() {
        println!("{empty}");
    }
    for m in rows {
        println!("{}", row_line(st, m, ui::width()));
    }
}

/// What each of the person's blocks is for, in a line.
fn block_about(name: &str) -> &'static str {
    match name {
        "identidad" => "who Clio is and how it speaks",
        "reglas" => "how it works and its limits",
        "preferencias" => "who you are",
        _ => "what it watches, one thing per entry",
    }
}

const NONE_YET: &str = "there is no memory yet: it is created by `clio init` or by the first run";

pub fn run(dirs: &Dirs, raw: Vec<String>) -> Res<i32> {
    let st = Style::stdout();
    let mut it = raw.into_iter();
    let Some(sub) = it.next() else {
        println!("{USAGE}");
        return Ok(0);
    };
    let mut args = Args::new(it.collect());
    match sub.as_str() {
        "-h" | "--help" | "help" => println!("{USAGE}"),
        // Other agents, over MCP: read and propose only.
        "mcp" => {
            args.rest()?;
            return super::mcp::serve(dirs);
        }
        // The standalone clio-memory and its skill: the same installer as `clio-memory update`.
        "update" => {
            args.rest()?;
            clio_memory::setup::update(clio_memory::setup::INSTALLER_URL)?;
        }
        "sleep" => {
            let (now, dry_run) = (args.flag("--now"), args.flag("--dry-run"));
            let (report, json) = (args.flag("--report"), args.flag("--json"));
            args.rest()?;
            return super::sleep::run(dirs, now, dry_run, report, json);
        }
        "search" => {
            let json = args.flag("--json");
            let limit = args
                .value("--limit")?
                .map(|n| n.parse::<usize>().map_err(|e| format!("--limit: {e}")))
                .transpose()?
                .unwrap_or(10);
            let query = args.rest()?.join(" ");
            if query.trim().is_empty() {
                return Err("missing the query: clio memory search \"what to look for\"".into());
            }
            let Some(link) = open_read(dirs, true)? else {
                println!("{NONE_YET}");
                return Ok(0);
            };
            let (rows, abstained) = link.search(&query, limit)?;
            if json {
                let items: Vec<Value> = rows.iter().map(|m| row_json(&link, m)).collect();
                print_json(&json!({"abstained": abstained, "results": items}))?;
            } else {
                print_rows(
                    st,
                    &rows,
                    "nothing relevant (Clio prefers not to invent a memory)",
                );
                if let Some(n) = link.note() {
                    println!("{}", st.dim(&n));
                }
            }
        }
        "list" => {
            let (json, pending) = (args.flag("--json"), args.flag("--pending"));
            args.rest()?;
            let Some(link) = open_read(dirs, false)? else {
                println!("{NONE_YET}");
                return Ok(0);
            };
            let mut rows = if pending {
                link.pending()?
            } else {
                link.list(State::Active)?
            };
            rows.truncate(if pending { 200 } else { 30 });
            if json {
                let items: Vec<Value> = rows.iter().map(|m| row_json(&link, m)).collect();
                print_json(&Value::Array(items))?;
            } else {
                print_rows(
                    st,
                    &rows,
                    if pending {
                        "nothing pending review"
                    } else {
                        "there are no active memories yet"
                    },
                );
            }
        }
        "identity" => {
            let json = args.flag("--json");
            args.rest()?;
            let Some(link) = open_read(dirs, false)? else {
                println!("{NONE_YET}");
                return Ok(0);
            };
            let mut blocks = Vec::new();
            for name in BLOCK_NAMES {
                blocks.push((name, link.block_rows(name)?));
            }
            if json {
                let items: Vec<Value> = blocks
                    .iter()
                    .map(|(name, rows)| {
                        let entries: Vec<Value> = rows
                            .iter()
                            .map(|r| json!({"id": r.id.0, "text": r.content, "origin": origin_label(r.origin)}))
                            .collect();
                        json!({"name": name, "about": block_about(name), "entries": entries})
                    })
                    .collect();
                print_json(&Value::Array(items))?;
            } else {
                for (name, rows) in &blocks {
                    println!("{} {}", st.bold(name), st.dim(block_about(name)));
                    if rows.is_empty() {
                        println!("  {}", st.dim("(empty)"));
                    }
                    for (n, r) in rows.iter().enumerate() {
                        println!(
                            "  {}. [m{}] {} {}",
                            n + 1,
                            r.id.0,
                            sanitize::line(&r.content),
                            st.dim(&format!("({})", origin_label(r.origin)))
                        );
                    }
                }
            }
        }
        "show" => {
            let json = args.flag("--json");
            let rest = args.rest()?;
            let [id] = rest.as_slice() else {
                return Err("missing the id: clio memory show <id>".into());
            };
            let Some(link) = open_read(dirs, false)? else {
                println!("{NONE_YET}");
                return Ok(0);
            };
            let mid = id_of(id)?;
            if let Ok(Some(t)) = link.tombstone(mid) {
                if json {
                    print_json(
                        &json!({"id": format!("m{}", mid.0), "state": "purged", "purged_at": t.at, "reason": sanitize::line(&t.reason)}),
                    )?;
                } else {
                    println!("m{} was purged: the content no longer exists", mid.0);
                    println!(
                        "{}",
                        st.dim(&format!("reason: {}", sanitize::line(&t.reason)))
                    );
                }
                return Ok(0);
            }
            let m = link.get(mid)?;
            if json {
                print_json(&row_json(&link, &m))?;
            } else {
                let label = |l: &str| st.bold(&ui::pad(l, 10));
                println!("{}m{}", label("id:"), m.id.0);
                println!("{}{}", label("state:"), state_label(m.state));
                println!("{}{}", label("type:"), kind_label(m.kind));
                println!(
                    "{}{} (written by: {})",
                    label("origin:"),
                    origin_label(m.origin),
                    sanitize::line(&link.actor_name(m.actor))
                );
                if let Some(s) = &m.source {
                    println!("{}{}", label("source:"), sanitize::line(s));
                }
                println!(
                    "{}{}",
                    label("place:"),
                    sanitize::line(&link.place_label(&m))
                );
                if let Some(t) = &m.title {
                    println!("{}{}", label("title:"), sanitize::line(t));
                }
                if let Some(c) = m.category {
                    println!("{}{}", label("category:"), c.label());
                }
                if link.is_pinned(&m) {
                    println!("{}yes (it comes first in the context)", label("pinned:"));
                }
                if link.is_due(&m) {
                    println!(
                        "{}{}",
                        label("review:"),
                        st.yellow("past its review date: is it still true? (clio memory reviewed)")
                    );
                }
                for n in link.relation_notes(&m) {
                    println!("{}{}", label("relation:"), sanitize::line(&n));
                }
                if m.state == State::Quarantine {
                    println!(
                        "{}{}",
                        label("warning:"),
                        st.yellow(
                            "in quarantine (forgotten): it is not remembered; it comes back with promote or revert"
                        )
                    );
                }
                for l in ui::wrap(
                    &sanitize::multiline(&m.content),
                    ui::width().saturating_sub(10),
                ) {
                    println!("  {l}");
                }
            }
        }
        "remember" => {
            let block = args.value("--block")?;
            let text = args.rest()?.join(" ");
            if text.trim().is_empty() {
                return Err(
                    "missing the text: clio memory remember \"what you want remembered\"".into(),
                );
            }
            let link = open_write(dirs)?;
            let id = link.remember(&text, block.as_deref())?;
            println!("remembered: m{} (origin: persona, active)", id.0);
        }
        "promote" | "archive" => {
            let yes = args.flag("--yes");
            let rest = args.rest()?;
            let [id] = rest.as_slice() else {
                return Err(format!("missing the id: clio memory {sub} <id>"));
            };
            let id = id_of(id)?;
            let link = open_write(dirs)?;
            let m = link.get(id)?;
            let (verb, q) = if sub == "promote" {
                ("approved", "Approve")
            } else {
                ("archived", "Archive")
            };
            println!("{}", row_line(st, &m, ui::width()));
            if !yes && !confirm(&format!("{q} m{}?", id.0)) {
                println!("nothing was changed");
                return Ok(0);
            }
            if sub == "promote" {
                link.promote(id)?;
            } else {
                link.archive(id)?;
            }
            println!("{verb}: m{}", id.0);
        }
        // The project of this folder: which one and from where, or name it by hand.
        "project" => {
            let rest = args.rest()?;
            let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            match rest.as_slice() {
                [] => match clio_brain::project::settle(&dirs.data, &cwd) {
                    Some((name, from, note)) => {
                        println!("{name} ({})", from.label());
                        if let Some(n) = note {
                            println!("{}", st.yellow(&sanitize::line(&n)));
                        }
                    }
                    None => println!("none: what is saved here goes to the personal place"),
                },
                [set, name] if set == "set" => {
                    let n = clio_brain::project::normal(name);
                    if n.is_empty() {
                        return Err(format!("{name:?} is no good as a project name"));
                    }
                    let file = cwd.join(clio_brain::project::PROJECT_FILE);
                    std::fs::write(&file, format!("{n}\n"))
                        .map_err(|e| format!("{}: {e}", file.display()))?;
                    println!("this folder's project: {n} (in {})", file.display());
                }
                _ => return Err("usage: clio memory project [set <name>]".into()),
            }
        }
        // How to plug another agent into the memory (MCP): printed, never written for it.
        "setup" => {
            let remove = args.flag("--remove");
            let rest = args.rest()?;
            if rest.as_slice() == ["sleep"] {
                return super::sleep::setup(dirs, remove);
            }
            if remove {
                return Err("--remove goes with: clio memory setup sleep".into());
            }
            let [agent] = rest.as_slice() else {
                return Err(format!(
                    "usage: clio memory setup <agent>  ({})",
                    SETUP_AGENTS.join(", ")
                ));
            };
            println!("{}", setup_text(agent)?);
        }
        // Waiting for the person: the same as `list --pending`, by the name the docs use.
        "review" => {
            let json = args.flag("--json");
            args.rest()?;
            let Some(link) = open_read(dirs, false)? else {
                println!("{NONE_YET}");
                return Ok(0);
            };
            let rows = link.pending()?;
            if json {
                let v: Vec<Value> = rows.iter().map(|m| row_json(&link, m)).collect();
                print_json(&Value::Array(v))?;
            } else {
                print_rows(st, &rows, "nothing pending review");
            }
        }
        // The versions of a memory: every change that touched it, with who and when.
        "history" => {
            let json = args.flag("--json");
            let rest = args.rest()?;
            let [id] = rest.as_slice() else {
                return Err("missing the id: clio memory history <id>".into());
            };
            let Some(link) = open_read(dirs, false)? else {
                println!("{NONE_YET}");
                return Ok(0);
            };
            let ops = link.history(id_of(id)?)?;
            if json {
                let v: Vec<Value> = ops
                    .iter()
                    .map(|o| json!({"change": o.id.0, "kind": format!("{:?}", o.kind), "actor": link.actor_name(o.actor), "at": o.at, "reverts": o.reverts.map(|r| r.0)}))
                    .collect();
                print_json(&Value::Array(v))?;
            } else if ops.is_empty() {
                println!("no changes recorded");
            } else {
                for o in ops {
                    println!(
                        "change {:<6} {:<10} {}{}",
                        o.id.0,
                        format!("{:?}", o.kind).to_lowercase(),
                        sanitize::line(&link.actor_name(o.actor)),
                        o.reverts
                            .map_or(String::new(), |r| format!(" (undoes {})", r.0))
                    );
                }
            }
        }
        // Undo one change (its number from `history`), as the person.
        "revert" => {
            let yes = args.flag("--yes");
            let rest = args.rest()?;
            let [op] = rest.as_slice() else {
                return Err("missing the change: clio memory revert <change> (see it with clio memory history ID)".into());
            };
            let op: i64 = op
                .parse()
                .map_err(|_| format!("{op:?} is not a change number"))?;
            if !yes && !confirm(&format!("Undo change {op}?")) {
                println!("nothing was changed");
                return Ok(0);
            }
            let done = open_write(dirs)?.revert(clio_memory::OpId(op))?;
            println!("undone: change {op} (recorded as change {})", done.0);
        }
        // What the memory gives a session started in this folder (the summary, pinned, recent).
        "context" => {
            let json = args.flag("--json");
            args.rest()?;
            let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            let project = clio_brain::project::settle(&dirs.data, &cwd).map(|(n, _, _)| n);
            let port = clio_brain::memory_port::LinkPort::new(&dirs.data, &[]).in_project(project);
            let items = clio_toolhost::MemoryPort::context(&port)?;
            if json {
                let v: Vec<Value> = items
                    .iter()
                    .map(|i| json!({"id": i.id, "title": i.title, "text": i.text, "place": i.place, "notes": i.notes}))
                    .collect();
                print_json(&Value::Array(v))?;
            } else if items.is_empty() {
                println!("nothing yet");
            } else {
                for i in items {
                    let head = match &i.title {
                        Some(t) => format!("{t} — "),
                        None => String::new(),
                    };
                    println!(
                        "m{:<5} {} {}{}",
                        i.id,
                        st.dim(&sanitize::line(&i.place)),
                        sanitize::line(&head),
                        sanitize::line(&i.text)
                    );
                }
            }
        }
        // Pairs of memories Jev judged to contradict each other: to decide which one stays.
        "conflicts" => {
            let json = args.flag("--json");
            args.rest()?;
            let Some(link) = open_read(dirs, false)? else {
                println!("{NONE_YET}");
                return Ok(0);
            };
            let pairs = link.conflicts()?;
            if json {
                let v: Vec<Value> = pairs
                    .iter()
                    .map(|(a, b)| json!([row_json(&link, a), row_json(&link, b)]))
                    .collect();
                print_json(&Value::Array(v))?;
            } else if pairs.is_empty() {
                println!("no contradiction noted");
            } else {
                for (a, b) in pairs {
                    println!("{}", row_line(st, &a, ui::width()));
                    println!("  contradicts");
                    println!("{}\n", row_line(st, &b, ui::width()));
                }
            }
        }
        // An export back into an empty memory (another machine, a restore).
        "import" => {
            let rest = args.rest()?;
            let [file] = rest.as_slice() else {
                return Err("usage: clio memory import <file> (only into an empty memory)".into());
            };
            let db = MemoryLink::db_path(&dirs.data);
            if db.exists() {
                return Err(format!(
                    "there is already a memory at {}: an import only goes into an empty one (move it first)",
                    db.display()
                ));
            }
            let f = std::fs::File::open(file).map_err(|e| format!("{file}: {e}"))?;
            let engine = clio_memory::Engine::open(
                &db,
                clio_memory::OpenMode::ReadWrite,
                std::sync::Arc::new(clio_memory::SystemClock),
            )
            .map_err(|e| e.to_string())?;
            let s = engine
                .import(&mut std::io::BufReader::new(f))
                .map_err(|e| format!("{file}: {e}"));
            drop(engine);
            match s {
                Ok(s) => println!("imported: {} operations", s.operations),
                Err(why) => {
                    // Nothing came in (it is one transaction): no half memory is left behind.
                    let _ = std::fs::remove_file(&db);
                    return Err(why);
                }
            }
        }
        "projects" => {
            let json = args.flag("--json");
            let yes = args.flag("--yes");
            let rest = args.rest()?;
            match rest.as_slice() {
                [] => {}
                [verb, from, into] if verb == "merge" => {
                    if !yes && !confirm(&format!("Move everything of project {from} to {into}?")) {
                        println!("nothing was changed");
                        return Ok(0);
                    }
                    let n = open_write(dirs)?.merge_projects(from, into)?;
                    println!(
                        "moved {n} memories from {} to {} (each one is undone with clio memory revert; see it in history)",
                        sanitize::line(from),
                        sanitize::line(into)
                    );
                    return Ok(0);
                }
                [verb, name] if verb == "archive" => {
                    if !yes && !confirm(&format!("Archive everything of project {name}?")) {
                        println!("nothing was changed");
                        return Ok(0);
                    }
                    let n = open_write(dirs)?.archive_project(name)?;
                    println!(
                        "archived {n} memories of {} (they are not deleted)",
                        sanitize::line(name)
                    );
                    return Ok(0);
                }
                _ => {
                    return Err("usage: clio memory projects [--json] | merge <from> <into> [--yes] | archive <project> [--yes]".into());
                }
            }
            let Some(link) = open_read(dirs, false)? else {
                println!("{NONE_YET}");
                return Ok(0);
            };
            let places = link.places()?;
            if json {
                let v: Vec<Value> = places
                    .iter()
                    .map(|(l, a, q, r)| json!({"place": l, "active": a, "quarantine": q, "archived": r}))
                    .collect();
                print_json(&Value::Array(v))?;
            } else {
                for (l, a, q, r) in places {
                    println!(
                        "{} {a} active · {q} in quarantine · {r} archived",
                        st.bold(&ui::pad(&sanitize::line(&l), 28))
                    );
                }
            }
        }
        "review-due" => {
            let json = args.flag("--json");
            args.rest()?;
            let Some(link) = open_read(dirs, false)? else {
                println!("{NONE_YET}");
                return Ok(0);
            };
            let due = link.review_due()?;
            if json {
                let v: Vec<Value> = due.iter().map(|m| row_json(&link, m)).collect();
                print_json(&Value::Array(v))?;
            } else {
                print_rows(st, &due, "nothing is past its review date");
            }
        }
        "reviewed" => {
            let rest = args.rest()?;
            let [id] = rest.as_slice() else {
                return Err("missing the id: clio memory reviewed <id>".into());
            };
            let id = id_of(id)?;
            open_write(dirs)?.reviewed(id)?;
            println!("reviewed: m{} (its date starts again)", id.0);
        }
        "timeline" => {
            let json = args.flag("--json");
            let rest = args.rest()?;
            let [id] = rest.as_slice() else {
                return Err("missing the id: clio memory timeline <id>".into());
            };
            let Some(link) = open_read(dirs, false)? else {
                println!("{NONE_YET}");
                return Ok(0);
            };
            let rows = link.timeline(id_of(id)?, 5, 5)?;
            if json {
                let v: Vec<Value> = rows.iter().map(|m| row_json(&link, m)).collect();
                print_json(&Value::Array(v))?;
            } else {
                print_rows(st, &rows, "nothing around it");
            }
        }
        // Pinned: first in the context of its place. Undoes what the model pinned when asked.
        "pin" | "unpin" => {
            let rest = args.rest()?;
            let [id] = rest.as_slice() else {
                return Err(format!("missing the id: clio memory {sub} <id>"));
            };
            let id = id_of(id)?;
            let link = open_write(dirs)?;
            link.get(id)?;
            if sub == "pin" {
                link.pin(id)?;
                println!("pinned: m{}", id.0);
            } else {
                link.unpin(id)?;
                println!("no longer pinned: m{}", id.0);
            }
        }
        "feedback" => {
            let rest = args.rest()?;
            let [id, word] = rest.as_slice() else {
                return Err("usage: clio memory feedback <id> useful|wrong".into());
            };
            let signal = match word.as_str() {
                "useful" | "util" | "útil" => Signal::Useful,
                "wrong" | "mal" | "equivocado" => Signal::Wrong,
                _ => return Err("the signal is `useful` or `wrong`".into()),
            };
            let id = id_of(id)?;
            let link = open_write(dirs)?;
            link.feedback(id, signal)?;
            println!(
                "noted {} for m{} (it never deletes or changes the memory; three «wrong» send it to review)",
                word, id.0
            );
        }
        "export" => {
            let rest = args.rest()?;
            let Some(link) = open_read(dirs, false)? else {
                println!("{NONE_YET}");
                return Ok(0);
            };
            match rest.as_slice() {
                [] => {
                    let mut out = std::io::stdout().lock();
                    link.engine().export(&mut out).map_err(|e| e.to_string())?;
                }
                [file] => {
                    let s = link
                        .engine()
                        .export_to_file(std::path::Path::new(file))
                        .map_err(|e| format!("{file}: {e}"))?;
                    println!(
                        "exported to {} ({} operations; unencrypted, 0600)",
                        sanitize::line(file),
                        s.operations
                    );
                }
                _ => return Err("usage: clio memory export [file]".into()),
            }
        }
        "stats" => {
            let json = args.flag("--json");
            args.rest()?;
            let Some(link) = open_read(dirs, false)? else {
                println!("{NONE_YET}");
                return Ok(0);
            };
            let (active, quarantine, archived) = link.counts()?;
            let s = link.engine().stats().map_err(|e| e.to_string())?;
            let model = !MemoryLink::model_dirs(&dirs.home).is_empty();
            if json {
                print_json(&json!({
                    "active": active, "quarantine": quarantine, "archived": archived,
                    "operations": s.operations, "vectors": s.vectors,
                    "file_bytes": s.file_bytes, "embedding_model_on_disk": model,
                }))?;
            } else {
                println!("active {active} · in quarantine {quarantine} · archived {archived}");
                println!(
                    "operations {} · vectors {} · file {} bytes",
                    s.operations, s.vectors, s.file_bytes
                );
                println!(
                    "{}",
                    st.dim(if model {
                        "embedding model: present (search by text and by meaning)"
                    } else {
                        "no embedding model: the memory searches by text only"
                    })
                );
            }
        }
        "purge" => {
            let yes = args.flag("--yes");
            let reason = args
                .value("--reason")?
                .unwrap_or_else(|| "asked by the person".into());
            let rest = args.rest()?;
            let [id] = rest.as_slice() else {
                return Err("missing the id: clio memory purge <id> [--reason \"reason\"]".into());
            };
            let id = id_of(id)?;
            let link = open_write(dirs)?;
            let m = link.get(id)?;
            println!("{}", row_line(st, &m, ui::width()));
            println!(
                "{}",
                st.dim("It deletes the content everywhere (text, index, history and vector) and cannot be undone.\nBackups and exports you already made keep the text. The reason stays in the clear: do not put what was deleted in it.")
            );
            if !yes && !confirm(&format!("Purge m{} for good?", id.0)) {
                println!("nothing was changed");
                return Ok(0);
            }
            let out = link.purge(id, &reason)?;
            println!("purged: m{} (operation {})", id.0, out.op.0);
            if !out.wal_truncated {
                println!(
                    "{}",
                    st.dim("warning: a reader kept the write-ahead log from being emptied; old copies may stay in the -wal file until the next checkpoint")
                );
            }
        }
        other => {
            eprintln!("clio memory: unknown subcommand {other:?}\n\n{USAGE}");
            return Ok(2);
        }
    }
    Ok(0)
}
