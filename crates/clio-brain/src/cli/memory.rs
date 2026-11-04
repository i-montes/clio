//! `clio memory ...`: look at what Clio remembers and decide what it keeps. Everything printed
//! comes from the memory file and is cleaned of escape sequences first. The commands that change
//! something (`remember`, `promote`, `archive`, `feedback`) act as the person; the agent has no
//! way to reach any of them (S-25, S-26). `purge` deletes for good, so it asks first and only the person can run it.

use clio_brain::memory_link::{EmbedModel, MemoryLink, Res, kind_label, origin_label, state_label};
use clio_memory::{MemoryId, MemoryRow, Signal, State};
use serde_json::{Value, json};

use super::Args;
use super::apply::confirm;
use super::home::Dirs;
use super::tui::sanitize;
use super::ui::{self, Style, clip};

pub const USAGE: &str = "clio memory: lo que Clio recuerda (todo local, ~/.local/share/clio/memory.db)

  clio memory search <consulta> [--json]      busca entre lo activo (la cuarentena no sale aquí)
  clio memory list [--pending] [--json]       recientes; --pending: cuarentena y procedimientos por aprobar
  clio memory show <id> [--json]              un recuerdo completo, con de dónde salió
  clio memory remember \"texto\" [--block identidad|reglas|preferencias]
                                              un hecho tuyo (origen: persona), activo al instante
  clio memory promote <id> [--yes]            aprueba un recuerdo pendiente (solo tú; pregunta antes)
  clio memory archive <id> [--yes]            lo archiva: ya no se recuerda (pregunta antes)
  clio memory purge <id> [--reason \"motivo\"] [--yes]
                                              borra el contenido PARA SIEMPRE (texto, vector, historial);
                                              deja una lápida sin contenido. No se puede deshacer
  clio memory review [--json]                 lo que espera tu revisión (igual que list --pending)
  clio memory history <id> [--json]           cada cambio que tocó un recuerdo (sus versiones)
  clio memory revert <cambio> [--yes]         deshace un cambio (su número sale en history)
  clio memory context [--json]                lo que recibe una sesión que empieza en esta carpeta
  clio memory conflicts [--json]              recuerdos que Jev juzgó que se contradicen
  clio memory import <archivo>                trae una exportación a una memoria vacía
  clio memory project [set <nombre>]          el proyecto de esta carpeta y de dónde sale; set lo fija (.clio-project)
  clio memory setup <agente>                  cómo conectar claude-code, cursor, codex, gemini u opencode por MCP
  clio memory projects [--json]               cada sitio (personal, lecciones, cada proyecto) y lo que tiene
  clio memory review-due [--json]             lo que pasó su plazo de revisión (decisiones, configuración…)
  clio memory reviewed <id>                   lo miraste y sigue siendo cierto: el plazo vuelve a empezar
  clio memory timeline <id> [--json]          lo que se guardó antes y después, en la misma sesión
  clio memory pin|unpin <id>                  fija un recuerdo (sale primero en el contexto) o lo suelta
  clio memory feedback <id> useful|wrong      «wrong» solo lo manda a revisión; nunca borra
  clio memory export [archivo]                JSON Lines (sin cifrar); sin archivo, a la salida estándar
  clio memory stats [--json]                  conteos y tamaño
  clio memory mcp                             servidor MCP para otros agentes (leer y proponer; nada más)";

const SETUP_AGENTS: [&str; 5] = ["claude-code", "cursor", "codex", "gemini", "opencode"];

/// What to run or paste so `agent` reaches `clio memory mcp`. Clio does not edit another tool's
/// configuration: the person sees it and decides.
fn setup_text(agent: &str) -> Res<String> {
    let json =
        r#"{ "mcpServers": { "clio-memory": { "command": "clio", "args": ["memory", "mcp"] } } }"#;
    Ok(match agent {
        "claude-code" => "Ejecuta:\n  claude mcp add clio-memory -- clio memory mcp".into(),
        "cursor" => format!("Añade a ~/.cursor/mcp.json (o .cursor/mcp.json del proyecto):\n  {json}"),
        "gemini" => format!("Añade a ~/.gemini/settings.json:\n  {json}"),
        "codex" => "Añade a ~/.codex/config.toml:\n  [mcp_servers.clio-memory]\n  command = \"clio\"\n  args = [\"memory\", \"mcp\"]".into(),
        "opencode" => "Añade a opencode.json:\n  { \"mcp\": { \"clio-memory\": { \"type\": \"local\", \"command\": [\"clio\", \"memory\", \"mcp\"] } } }".into(),
        other => {
            return Err(format!(
                "no conozco {other:?}: {}. Cualquier agente con MCP sirve con el comando `clio memory mcp` por la entrada estándar",
                SETUP_AGENTS.join(", ")
            ));
        }
    } + "\nEl agente arranca el servidor en la carpeta del proyecto: lee y propone ahí (nada más).")
}

fn id_of(s: &str) -> Res<MemoryId> {
    s.trim_start_matches('m')
        .parse::<i64>()
        .map(MemoryId)
        .map_err(|_| format!("{s:?} no es un id de recuerdo (por ejemplo 12 o m12)"))
}

fn open_read(dirs: &Dirs, model: bool) -> Res<Option<MemoryLink>> {
    let m = if model {
        EmbedModel::of_home(&dirs.home)
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

const NONE_YET: &str = "todavía no hay memoria: se crea con `clio init` o con el primer run";

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
        "search" => {
            let json = args.flag("--json");
            let limit = args
                .value("--limit")?
                .map(|n| n.parse::<usize>().map_err(|e| format!("--limit: {e}")))
                .transpose()?
                .unwrap_or(10);
            let query = args.rest()?.join(" ");
            if query.trim().is_empty() {
                return Err("falta la consulta: clio memory search \"qué buscar\"".into());
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
                    "nada relevante (Clio prefiere no inventar un recuerdo)",
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
                        "nada pendiente de revisar"
                    } else {
                        "todavía no hay recuerdos activos"
                    },
                );
            }
        }
        "show" => {
            let json = args.flag("--json");
            let rest = args.rest()?;
            let [id] = rest.as_slice() else {
                return Err("falta el id: clio memory show <id>".into());
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
                    println!("m{} fue purgado: el contenido ya no existe", mid.0);
                    println!(
                        "{}",
                        st.dim(&format!("motivo: {}", sanitize::line(&t.reason)))
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
                println!("{}{}", label("estado:"), state_label(m.state));
                println!("{}{}", label("tipo:"), kind_label(m.kind));
                println!(
                    "{}{} (la escribió: {})",
                    label("origen:"),
                    origin_label(m.origin),
                    sanitize::line(&link.actor_name(m.actor))
                );
                if let Some(s) = &m.source {
                    println!("{}{}", label("fuente:"), sanitize::line(s));
                }
                println!(
                    "{}{}",
                    label("sitio:"),
                    sanitize::line(&link.place_label(&m))
                );
                if let Some(t) = &m.title {
                    println!("{}{}", label("título:"), sanitize::line(t));
                }
                if let Some(c) = m.category {
                    println!("{}{}", label("categoría:"), c.label());
                }
                if link.is_pinned(&m) {
                    println!("{}sí (sale primero en el contexto)", label("fijado:"));
                }
                if link.is_due(&m) {
                    println!(
                        "{}{}",
                        label("revisar:"),
                        st.yellow("pasó su plazo de revisión: ¿sigue siendo cierto? (clio memory reviewed)")
                    );
                }
                for n in link.relation_notes(&m) {
                    println!("{}{}", label("relación:"), sanitize::line(&n));
                }
                if m.state == State::Quarantine {
                    println!(
                        "{}{}",
                        label("aviso:"),
                        st.yellow("en cuarentena: no se recuerda hasta que lo apruebes (promote)")
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
                    "falta el texto: clio memory remember \"lo que quieres que recuerde\"".into(),
                );
            }
            let link = open_write(dirs)?;
            let id = link.remember(&text, block.as_deref())?;
            println!("recordado: m{} (origen: persona, activo)", id.0);
        }
        "promote" | "archive" => {
            let yes = args.flag("--yes");
            let rest = args.rest()?;
            let [id] = rest.as_slice() else {
                return Err(format!("falta el id: clio memory {sub} <id>"));
            };
            let id = id_of(id)?;
            let link = open_write(dirs)?;
            let m = link.get(id)?;
            let (verb, q) = if sub == "promote" {
                ("aprobado", "¿Aprobar")
            } else {
                ("archivado", "¿Archivar")
            };
            println!("{}", row_line(st, &m, ui::width()));
            if !yes && !confirm(&format!("{q} m{}?", id.0)) {
                println!("no se cambió nada");
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
                    None => println!("ninguno: lo que se guarde aquí va a lo personal"),
                },
                [set, name] if set == "set" => {
                    let n = clio_brain::project::normal(name);
                    if n.is_empty() {
                        return Err(format!("{name:?} no sirve como nombre de proyecto"));
                    }
                    let file = cwd.join(clio_brain::project::PROJECT_FILE);
                    std::fs::write(&file, format!("{n}\n"))
                        .map_err(|e| format!("{}: {e}", file.display()))?;
                    println!("proyecto de esta carpeta: {n} (en {})", file.display());
                }
                _ => return Err("uso: clio memory project [set <nombre>]".into()),
            }
        }
        // How to plug another agent into the memory (MCP): printed, never written for it.
        "setup" => {
            let rest = args.rest()?;
            let [agent] = rest.as_slice() else {
                return Err(format!(
                    "uso: clio memory setup <agente>  ({})",
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
                print_rows(st, &rows, "nada pendiente de revisar");
            }
        }
        // The versions of a memory: every change that touched it, with who and when.
        "history" => {
            let json = args.flag("--json");
            let rest = args.rest()?;
            let [id] = rest.as_slice() else {
                return Err("falta el id: clio memory history <id>".into());
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
                println!("sin cambios registrados");
            } else {
                for o in ops {
                    println!(
                        "cambio {:<6} {:<10} {}{}",
                        o.id.0,
                        format!("{:?}", o.kind).to_lowercase(),
                        sanitize::line(&link.actor_name(o.actor)),
                        o.reverts
                            .map_or(String::new(), |r| format!(" (deshace {})", r.0))
                    );
                }
            }
        }
        // Undo one change (its number from `history`), as the person.
        "revert" => {
            let yes = args.flag("--yes");
            let rest = args.rest()?;
            let [op] = rest.as_slice() else {
                return Err("falta el cambio: clio memory revert <cambio> (míralo con clio memory history ID)".into());
            };
            let op: i64 = op
                .parse()
                .map_err(|_| format!("{op:?} no es un número de cambio"))?;
            if !yes && !confirm(&format!("¿Deshacer el cambio {op}?")) {
                println!("no se cambió nada");
                return Ok(0);
            }
            let done = open_write(dirs)?.revert(clio_memory::OpId(op))?;
            println!(
                "deshecho: el cambio {op} (lo registra el cambio {})",
                done.0
            );
        }
        // What the memory gives a session started in this folder (the summary, pinned, recent).
        "context" => {
            let json = args.flag("--json");
            args.rest()?;
            let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            let project = clio_brain::project::settle(&dirs.data, &cwd).map(|(n, _, _)| n);
            let port = clio_brain::memory_port::LinkPort::new(&dirs.data, None).in_project(project);
            let items = clio_toolhost::MemoryPort::context(&port)?;
            if json {
                let v: Vec<Value> = items
                    .iter()
                    .map(|i| json!({"id": i.id, "title": i.title, "text": i.text, "place": i.place, "notes": i.notes}))
                    .collect();
                print_json(&Value::Array(v))?;
            } else if items.is_empty() {
                println!("nada todavía");
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
                println!("ninguna contradicción anotada");
            } else {
                for (a, b) in pairs {
                    println!("{}", row_line(st, &a, ui::width()));
                    println!("  contradice a");
                    println!("{}\n", row_line(st, &b, ui::width()));
                }
            }
        }
        // An export back into an empty memory (another machine, a restore).
        "import" => {
            let rest = args.rest()?;
            let [file] = rest.as_slice() else {
                return Err("uso: clio memory import <archivo> (solo en una memoria vacía)".into());
            };
            let db = MemoryLink::db_path(&dirs.data);
            if db.exists() {
                return Err(format!(
                    "ya hay una memoria en {}: la importación solo va a una vacía (muévela antes)",
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
                Ok(s) => println!("importado: {} operaciones", s.operations),
                Err(why) => {
                    // Nothing came in (it is one transaction): no half memory is left behind.
                    let _ = std::fs::remove_file(&db);
                    return Err(why);
                }
            }
        }
        "projects" => {
            let json = args.flag("--json");
            args.rest()?;
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
                        "{} {a} activos · {q} en cuarentena · {r} archivados",
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
                print_rows(st, &due, "nada pasó su plazo de revisión");
            }
        }
        "reviewed" => {
            let rest = args.rest()?;
            let [id] = rest.as_slice() else {
                return Err("falta el id: clio memory reviewed <id>".into());
            };
            let id = id_of(id)?;
            open_write(dirs)?.reviewed(id)?;
            println!("revisado: m{} (su plazo vuelve a empezar)", id.0);
        }
        "timeline" => {
            let json = args.flag("--json");
            let rest = args.rest()?;
            let [id] = rest.as_slice() else {
                return Err("falta el id: clio memory timeline <id>".into());
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
                print_rows(st, &rows, "nada alrededor");
            }
        }
        // Pinned: first in the context of its place. Undoes what the model pinned when asked.
        "pin" | "unpin" => {
            let rest = args.rest()?;
            let [id] = rest.as_slice() else {
                return Err(format!("falta el id: clio memory {sub} <id>"));
            };
            let id = id_of(id)?;
            let link = open_write(dirs)?;
            link.get(id)?;
            if sub == "pin" {
                link.pin(id)?;
                println!("fijado: m{}", id.0);
            } else {
                link.unpin(id)?;
                println!("ya no está fijado: m{}", id.0);
            }
        }
        "feedback" => {
            let rest = args.rest()?;
            let [id, word] = rest.as_slice() else {
                return Err("uso: clio memory feedback <id> useful|wrong".into());
            };
            let signal = match word.as_str() {
                "useful" | "util" | "útil" => Signal::Useful,
                "wrong" | "mal" | "equivocado" => Signal::Wrong,
                _ => return Err("la señal es `useful` o `wrong`".into()),
            };
            let id = id_of(id)?;
            let link = open_write(dirs)?;
            link.feedback(id, signal)?;
            println!(
                "anotado {} para m{} (nunca borra ni cambia el recuerdo; tres «wrong» lo mandan a revisión)",
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
                        "exportado a {} ({} operaciones; sin cifrar, 0600)",
                        sanitize::line(file),
                        s.operations
                    );
                }
                _ => return Err("uso: clio memory export [archivo]".into()),
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
            let model = MemoryLink::model_dir(&dirs.home).is_dir();
            if json {
                print_json(&json!({
                    "active": active, "quarantine": quarantine, "archived": archived,
                    "operations": s.operations, "vectors": s.vectors,
                    "file_bytes": s.file_bytes, "embedding_model_on_disk": model,
                }))?;
            } else {
                println!("activos {active} · en cuarentena {quarantine} · archivados {archived}");
                println!(
                    "operaciones {} · vectores {} · archivo {} bytes",
                    s.operations, s.vectors, s.file_bytes
                );
                println!(
                    "{}",
                    st.dim(if model {
                        "modelo de embeddings: presente (búsqueda por texto y por significado)"
                    } else {
                        "sin modelo de embeddings: la memoria busca solo por texto"
                    })
                );
            }
        }
        "purge" => {
            let yes = args.flag("--yes");
            let reason = args
                .value("--reason")?
                .unwrap_or_else(|| "pedido por la persona".into());
            let rest = args.rest()?;
            let [id] = rest.as_slice() else {
                return Err("falta el id: clio memory purge <id> [--reason \"motivo\"]".into());
            };
            let id = id_of(id)?;
            let link = open_write(dirs)?;
            let m = link.get(id)?;
            println!("{}", row_line(st, &m, ui::width()));
            println!(
                "{}",
                st.dim("Borra el contenido en todas partes (texto, índice, historial y vector) y no se puede deshacer.\nLos respaldos y exportaciones que ya hiciste conservan el texto. El motivo queda en claro: no pongas ahí lo borrado.")
            );
            if !yes && !confirm(&format!("¿Purgar m{} para siempre?", id.0)) {
                println!("no se cambió nada");
                return Ok(0);
            }
            let out = link.purge(id, &reason)?;
            println!("purgado: m{} (operación {})", id.0, out.op.0);
            if !out.wal_truncated {
                println!(
                    "{}",
                    st.dim("aviso: un lector impidió vaciar el registro de escritura; copias viejas pueden quedar en el archivo -wal hasta el próximo checkpoint")
                );
            }
        }
        other => {
            eprintln!("clio memory: subcomando desconocido {other:?}\n\n{USAGE}");
            return Ok(2);
        }
    }
    Ok(0)
}
