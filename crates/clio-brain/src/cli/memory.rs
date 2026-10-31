//! `clio memory ...`: look at what Clio remembers and decide what it keeps. Everything printed
//! comes from the memory file and is cleaned of escape sequences first. The commands that change
//! something (`remember`, `promote`, `archive`, `feedback`) act as the person; the agent has no
//! way to reach any of them (S-25, S-26). There is no `purge` here: it comes with PURGE.

use clio_brain::memory_link::{
    MemoryLink, Res, kind_label, origin_label, state_label,
};
use clio_memory::{MemoryId, MemoryRow, Signal, State};
use serde_json::{Value, json};

use super::apply::confirm;
use super::home::Dirs;
use super::tui::sanitize;
use super::ui::{self, Style, clip};
use super::Args;

pub const USAGE: &str = "clio memory: lo que Clio recuerda (todo local, ~/.local/share/clio/memory.db)

  clio memory search <consulta> [--json]      busca entre lo activo (la cuarentena no sale aquí)
  clio memory list [--pending] [--json]       recientes; --pending: cuarentena y procedimientos por aprobar
  clio memory show <id> [--json]              un recuerdo completo, con de dónde salió
  clio memory remember \"texto\" [--block identidad|reglas|preferencias]
                                              un hecho tuyo (origen: persona), activo al instante
  clio memory promote <id> [--yes]            aprueba un recuerdo pendiente (solo tú; pregunta antes)
  clio memory archive <id> [--yes]            lo archiva: ya no se recuerda (pregunta antes)
  clio memory feedback <id> useful|wrong      «wrong» solo lo manda a revisión; nunca borra
  clio memory export [archivo]                JSON Lines (sin cifrar); sin archivo, a la salida estándar
  clio memory stats [--json]                  conteos y tamaño";

fn id_of(s: &str) -> Res<MemoryId> {
    s.trim_start_matches('m')
        .parse::<i64>()
        .map(MemoryId)
        .map_err(|_| format!("{s:?} no es un id de recuerdo (por ejemplo 12 o m12)"))
}

fn open_read(dirs: &Dirs, model: bool) -> Res<Option<MemoryLink>> {
    let dir = model.then(|| MemoryLink::model_dir(&dirs.home)).filter(|m| m.is_dir());
    MemoryLink::open_read(&dirs.data, dir.as_deref())
}

fn open_write(dirs: &Dirs) -> Res<MemoryLink> {
    MemoryLink::open_write(&dirs.data, None)
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
                print_rows(st, &rows, "nada relevante (Clio prefiere no inventar un recuerdo)");
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
            let m = link.get(id_of(id)?)?;
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
                if m.state == State::Quarantine {
                    println!(
                        "{}{}",
                        label("aviso:"),
                        st.yellow("en cuarentena: no se recuerda hasta que lo apruebes (promote)")
                    );
                }
                for l in ui::wrap(&sanitize::multiline(&m.content), ui::width().saturating_sub(10)) {
                    println!("  {l}");
                }
            }
        }
        "remember" => {
            let block = args.value("--block")?;
            let text = args.rest()?.join(" ");
            if text.trim().is_empty() {
                return Err("falta el texto: clio memory remember \"lo que quieres que recuerde\"".into());
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
                    link.engine()
                        .export(&mut out)
                        .map_err(|e| e.to_string())?;
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
                println!(
                    "activos {active} · en cuarentena {quarantine} · archivados {archived}"
                );
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
            return Err(
                "`clio memory purge` todavía no existe: llegará aparte, con PURGE. Mientras tanto `archive` oculta un recuerdo sin borrarlo."
                    .into(),
            );
        }
        other => {
            eprintln!("clio memory: subcomando desconocido {other:?}\n\n{USAGE}");
            return Ok(2);
        }
    }
    Ok(0)
}
