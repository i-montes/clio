//! Subagents (S-73): the chat's model hands tasks to child agents with `subagent`, and waits.
//!
//! - A chat's tool host (`--subagents`) holds the call: the guard's `subagent` request waits here
//!   (before the one-call-at-a-time lock, so the brain's own requests are answered meanwhile)
//!   until the brain, which runs the children, takes it (`subagent_take`) and hands back their
//!   reports (`subagent_done`), both with the brain's token. The caller leaving ends the wait.
//!   The reports are outside text: the session is marked first, each report is judged (one the
//!   judge flags is withheld, the session becomes `suspect` and the window and bypass close) and
//!   wrapped as a subagent summary, all before the answer goes back, in the same step.
//! - A child's own tool host (`--child read|write`) serves less than its parent's, never more:
//!   a reader is a review's (`Config::review`); a writer reads, writes, edits and runs commands
//!   under the session's rules, and what would wait for the person is refused, never set aside.

use std::collections::HashMap;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use clio_gateway::{Source, wrap_untrusted};
use serde_json::{Map, Value, json};

use crate::judge::{ToJudge, one_line};
use crate::{DecisionOut, MAX_RAW_BYTES, Request, Response, ToolHost, lock};

/// What kind of child a tool host serves (`clio toolhost --child read|write`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Child {
    /// Reads and runs commands that change nothing: a review's tool host.
    Read,
    /// Reads, writes, edits and runs commands, under the session's own rules.
    Write,
}

impl Child {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            _ => None,
        }
    }
}

/// What a child is told instead of an action being set aside for the person.
pub const CHILD_CANNOT_ASK: &str = "a subagent cannot ask the person: this was not run and nothing waits for them; say in your report what is left for your parent to do";
/// What a reading child is told when a command asks for the network or `sudo`.
pub const CHILD_NO_NETWORK: &str =
    "a reading subagent has no network and no sudo: say in your report what needs them";
/// The only requests a writing child's tool host serves: the file tools, `exec`, and the
/// internal operations of the guard and of Clio. Everything else is refused, whoever sent it.
pub(crate) const WRITER_OPS: [&str; 12] = [
    "fs_read", "fs_stat", "fs_list", "fs_glob", "fs_write", "fs_mkdir", "exec", "filter", "judge",
    "jobs", "taint", "closing",
];
/// Calls of one tool host held at once (a call is alone in its step: one is the usual).
const MAX_CALLS: usize = 16;
/// How often a waiting call looks whether its caller left.
const POLL: Duration = Duration::from_millis(100);

enum Call {
    /// The guard's arguments, until the brain takes them.
    Waiting(Value),
    Taken,
    /// The brain's answer.
    Done(Value),
}

/// The `subagent` calls a chat's tool host holds.
#[derive(Default)]
pub(crate) struct Calls {
    calls: Mutex<HashMap<String, Call>>,
    changed: Condvar,
}

fn key(session: &str, call: &str) -> String {
    format!("{session}\n{call}")
}

impl ToolHost {
    fn ok_answer(req: &Request, content: String, data: Value, tainted: bool) -> Response {
        Response {
            id: req.id.clone(),
            ok: true,
            content,
            data,
            decision: DecisionOut {
                outcome: "allow".into(),
                rule: "subagent".into(),
                reason: "the children ran in tool hosts of their own".into(),
            },
            tainted,
            external: false,
        }
    }

    /// `subagent`, `subagent_take` and `subagent_done` of a chat's tool host.
    pub(crate) fn subagent_op(
        &self,
        calls: &Calls,
        req: &Request,
        args: &Map<String, Value>,
        cancel: &dyn Fn() -> bool,
    ) -> Response {
        let tainted = self.is_tainted(&req.session);
        let refuse = |why: &str| self.refuse(&req.id, "subagent", why.into(), tainted);
        if req.tool != "subagent" {
            // Only the brain takes a call and answers it.
            if self.said_token.is_none()
                || self.said_token.as_deref() != args.get("token").and_then(Value::as_str)
            {
                return refuse("only Clio runs subagents");
            }
            let call = args.get("call").and_then(Value::as_str).unwrap_or("");
            let k = key(&req.session, call);
            let mut m = lock(&calls.calls);
            return match (req.tool.as_str(), m.get(&k)) {
                ("subagent_take", Some(Call::Waiting(a))) => {
                    let a = a.clone();
                    m.insert(k, Call::Taken);
                    Self::ok_answer(req, String::new(), a, tainted)
                }
                ("subagent_done", Some(Call::Taken)) => {
                    m.insert(k, Call::Done(Value::Object(args.clone())));
                    calls.changed.notify_all();
                    Self::ok_answer(req, String::new(), Value::Null, tainted)
                }
                _ => refuse("no such call waits"),
            };
        }
        let k = key(&req.session, &req.call_id);
        {
            let mut m = lock(&calls.calls);
            if m.contains_key(&k) || m.len() >= MAX_CALLS {
                return refuse("this call already runs");
            }
            m.insert(k.clone(), Call::Waiting(Value::Object(args.clone())));
        }
        let mut m = lock(&calls.calls);
        let done = loop {
            if matches!(m.get(&k), Some(Call::Done(_)))
                && let Some(Call::Done(v)) = m.remove(&k)
            {
                break v;
            }
            if cancel() || self.stopping.load(std::sync::atomic::Ordering::SeqCst) {
                m.remove(&k);
                return refuse("the call was stopped: its subagents are stopped too");
            }
            m = calls
                .changed
                .wait_timeout(m, POLL)
                .map_or_else(|e| e.into_inner().0, |(g, _)| g);
        };
        drop(m);
        self.subagent_result(req, &done)
    }

    /// The brain's answer, as the model gets it: Clio's note, then each report judged and
    /// wrapped, the session marked before any of it is handed over.
    fn subagent_result(&self, req: &Request, done: &Value) -> Response {
        if let Some(why) = done["refused"].as_str() {
            return self.refuse(
                &req.id,
                "subagent",
                why.into(),
                self.is_tainted(&req.session),
            );
        }
        let reports = done["reports"].as_array().cloned().unwrap_or_default();
        let total: usize = reports
            .iter()
            .map(|r| r["text"].as_str().map_or(0, str::len))
            .sum();
        if total > MAX_RAW_BYTES {
            return self.refuse(
                &req.id,
                "subagent",
                "the reports are too large".into(),
                self.is_tainted(&req.session),
            );
        }
        // A child's report is outside text; a child the judge flagged flags this conversation.
        if !reports.is_empty() {
            self.mark(&req.session, "subagent");
        }
        if done["suspect"].as_bool() == Some(true) {
            self.mark(&req.session, "suspect");
            self.close_window_flagged();
        }
        let mut blocks = vec![self.clean(done["note"].as_str().unwrap_or(""))];
        let mut withheld = Vec::new();
        for r in &reports {
            let name = one_line(&self.clean(r["name"].as_str().unwrap_or("")));
            let w = ToJudge {
                tool: "subagent",
                source: name.clone(),
            };
            let mut out = [Value::String(self.clean(r["text"].as_str().unwrap_or("")))];
            let judged = self.judge_texts(req, &mut out, &[0], &w);
            let text = out[0].as_str().unwrap_or_default().to_owned();
            if let Some(h) = judged.get("withheld") {
                withheld.push(h.clone());
                blocks.push(text);
            } else {
                blocks.push(wrap_untrusted(
                    Source::SubagentSummary,
                    &format!("The report of subagent «{name}»:\n{text}"),
                ));
            }
        }
        let content = blocks.join("\n\n");
        Self::ok_answer(
            req,
            content,
            json!({ "blocks": blocks, "withheld": withheld }),
            self.is_tainted(&req.session),
        )
    }
}
