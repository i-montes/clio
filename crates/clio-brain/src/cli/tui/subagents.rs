//! Subagents in the chat (S-73): the children of the call running under its row, `N subagents
//! running` in the status line, one note when a call ends, their line in `/usage`, and
//! `/subagents`. Everything comes from the call's `ToolUpdate`s (the brain's, `chat::subagent`).

use clio_brain::pi::text;
use serde_json::Value;

use super::app::App;
use super::chat::Item;
use super::commands::Modal;
use crate::cli::usage;

/// The chat's subagents.
#[derive(Debug, Default)]
pub struct Subagents {
    /// The call running, and its children as its last update said.
    pub live: Option<(String, Vec<Value>)>,
    /// Every child of this chat whose call ended, oldest first.
    pub past: Vec<Value>,
}

impl Subagents {
    /// A `ToolUpdate` of the call `id`: its children now; with `summary`, the call ended, and
    /// that is the note to show.
    pub fn update(&mut self, id: &str, partial: &Value) -> Option<String> {
        let rows = partial["subagents"].as_array().cloned().unwrap_or_default();
        match partial["summary"].as_str() {
            Some(s) => {
                self.live = None;
                self.past.extend(rows);
                Some(text::line(s, 200))
            }
            None => {
                self.live = Some((id.to_owned(), rows));
                None
            }
        }
    }

    /// The turn is over: a call still drawn as running did not end on its own.
    pub fn stop(&mut self) {
        if let Some((_, rows)) = self.live.take() {
            self.past.extend(rows.into_iter().map(|mut r| {
                if r["state"] == "running" || r["state"] == "waiting" {
                    r["state"] = "stopped".into();
                }
                r
            }));
        }
    }

    pub fn running(&self) -> usize {
        self.live.as_ref().map_or(0, |(_, rows)| {
            rows.iter().filter(|r| r["state"] == "running").count()
        })
    }

    /// What the ended children spent: (cost in micro-dollars, tokens in, tokens out).
    pub fn spent(&self) -> (u64, u64, u64) {
        self.past.iter().fold((0, 0, 0), |(c, i, o), r| {
            (
                c + (r["cost"].as_f64().unwrap_or(0.0).max(0.0) * 1e6).round() as u64,
                i + r["input"].as_u64().unwrap_or(0),
                o + r["output"].as_u64().unwrap_or(0),
            )
        })
    }
}

fn secs(r: &Value, now_ms: u64) -> Option<u64> {
    let start = r["started_ms"].as_u64()?;
    let end = r["ended_ms"].as_u64().unwrap_or(now_ms);
    Some(end.saturating_sub(start) / 1000)
}

/// One child: `name · read · model · running · 1 min 4 s`.
fn row(r: &Value, now_ms: u64) -> String {
    let mut s = format!(
        "{} · {} · {} · {}",
        text::line(r["name"].as_str().unwrap_or("?"), 40),
        text::line(r["mode"].as_str().unwrap_or("?"), 8),
        text::line(r["model"].as_str().unwrap_or("?"), 60),
        text::line(r["state"].as_str().unwrap_or("?"), 12),
    );
    if let Some(t) = secs(r, now_ms) {
        s.push_str(&format!(" · {}", usage::duration(t)));
    }
    s
}

/// The children of the call running, one line each, under its row.
pub fn lines(app: &App) -> Vec<String> {
    app.chat
        .subagents
        .live
        .as_ref()
        .map_or_else(Vec::new, |(_, rows)| {
            rows.iter()
                .map(|r| format!("    {}", row(r, app.now_ms)))
                .collect()
        })
}

/// `2 subagents running` for the status line.
pub fn status(app: &App) -> Option<String> {
    match app.chat.subagents.running() {
        0 => None,
        1 => Some("1 subagent running".into()),
        n => Some(format!("{n} subagents running")),
    }
}

/// The `/usage` line: `subagents     7 launched (5 read, 2 write) · 210 steps · $0.84`.
pub fn usage_line(app: &App) -> Option<String> {
    let past = &app.chat.subagents.past;
    if past.is_empty() {
        return None;
    }
    let read = past.iter().filter(|r| r["mode"] == "read").count();
    let steps: u64 = past.iter().filter_map(|r| r["steps"].as_u64()).sum();
    Some(format!(
        "subagents     {} launched ({read} read, {} write) · {steps} steps · {}",
        past.len(),
        past.len() - read,
        usage::usd(app.chat.subagents.spent().0)
    ))
}

/// `/subagents`: this chat's children, newest first, with their state and where each report is.
pub fn modal(app: &App) -> Modal {
    let s = &app.chat.subagents;
    let all: Vec<&Value> = s
        .live
        .iter()
        .flat_map(|(_, rows)| rows.iter().rev())
        .chain(s.past.iter().rev())
        .collect();
    let mut lines = Vec::new();
    for r in &all {
        lines.push(row(r, app.now_ms));
        if let Some(w) = r["why"].as_str().filter(|w| !w.is_empty()) {
            lines.push(format!("  {}", text::line(w, 300)));
        }
        if let Some(p) = r["report"].as_str() {
            lines.push(format!("  report: {}", text::line(p, 300)));
        }
    }
    if lines.is_empty() {
        lines.push("no subagent in this chat yet".into());
    }
    Modal::text("Subagents", lines)
}

/// What the chat does with a `subagent` call's update.
pub fn on_update(app_chat: &mut super::chat::Chat, id: &str, partial: &Value) {
    if let Some(note) = app_chat.subagents.update(id, partial) {
        app_chat.items.push(Item::Note(note));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use clio_brain::pi::rpc::Event;
    use serde_json::json;

    use super::super::tests_gallery::screen_text;
    use super::super::tests_review::idle;
    use super::*;

    // A call's children under its row and in the status line while it runs; one note when it
    // ends; then their line in `/usage` and `/subagents`, newest first, with where each report is.
    #[test]
    fn a_calls_children_show_while_it_runs_then_one_note_usage_and_the_list() {
        let mut a = idle();
        let now = a.now_ms;
        a.chat.start_turn(now);
        a.chat.on_event(&Event::AgentStart);
        a.chat.on_event(&Event::ToolStart {
            id: "c1".into(),
            name: "subagent".into(),
            args: json!({"reason": "repartir", "tasks": []}),
        });
        let row = |name: &str, mode: &str, state: &str, ended: Option<u64>| {
            json!({"name": name, "mode": mode, "model": "m1", "state": state,
                "started_ms": now - 64_000, "ended_ms": ended, "steps": 3, "cost": 0.21,
                "input": 1000, "output": 100, "report": format!("/runs/r/subagents/{name}/report.txt")})
        };
        let update = |rows: Vec<Value>, summary: Option<&str>| Event::ToolUpdate {
            id: "c1".into(),
            name: "subagent".into(),
            partial: json!({"subagents": rows, "summary": summary}),
        };
        a.chat.on_event(&update(
            vec![
                row("look", "read", "running", None),
                row("check", "read", "done", Some(now - 4_000)),
            ],
            None,
        ));
        assert_eq!(status(&a).as_deref(), Some("1 subagent running"));
        let s = screen_text(&a, 100, 30);
        assert!(s.contains("1 subagent running"), "{s}");
        assert!(s.contains("look · read · m1 · running · 1 min 4 s"), "{s}");
        assert!(s.contains("check · read · m1 · done · 1 min"), "{s}");
        a.chat.on_event(&update(
            vec![
                row("look", "read", "failed", Some(now)),
                row("check", "read", "done", Some(now - 4_000)),
            ],
            Some("2 subagents: 1 failed, 1 done · 1 min 4 s · $0.42"),
        ));
        assert_eq!(status(&a), None);
        assert!(lines(&a).is_empty());
        assert!(a.chat.items.iter().any(
            |i| matches!(i, Item::Note(n) if n == "2 subagents: 1 failed, 1 done · 1 min 4 s · $0.42")
        ));
        // Their spending is the session's.
        assert_eq!(a.chat.subagents.spent(), (420_000, 2000, 200));
        assert_eq!(
            usage_line(&a).unwrap(),
            "subagents     2 launched (2 read, 0 write) · 6 steps · $0.42"
        );
        let m = modal(&a);
        assert_eq!(m.lines[0], "check · read · m1 · done · 1 min");
        assert!(
            m.lines
                .contains(&"  report: /runs/r/subagents/look/report.txt".to_owned())
        );
        // A call the turn's end cut is listed as stopped.
        a.chat
            .on_event(&update(vec![row("slow", "write", "running", None)], None));
        a.chat.on_event(&Event::AgentSettled { aborted: true });
        assert_eq!(status(&a), None);
        assert!(modal(&a).lines[0].starts_with("slow · write · m1 · stopped"));
    }
}
