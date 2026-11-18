//! Host mode: commands that run in the background (`exec` with `background: true`). They start
//! through the same path as a command in the foreground (list, launcher, masks, network rule,
//! action log, session marks) and answer at once with a job id; `job_output` reads what they
//! wrote since the last read (cleaned, and judged when they reached out), `job_stop` stops one.
//!
//! What a job writes goes to `<run>/jobs/<id>.log` (0600), capped: past `FILE_CAP` the oldest half
//! is dropped and the next read says so. At most `MAX_RUNNING` run per session. Every job stops
//! when the service stops (the session ends, the person quits, a signal): `ToolHost::stop`, and
//! the files go then.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clio_node::{HostJob, Limits, Sink};
use clio_vault::count_masks;
use serde_json::{Map, Value, json};

use crate::host::{Host, log_line, private_dir};
use crate::{DecisionOut, Request, Response, ToolHost, lock};

/// Background commands running at once in one session.
pub(crate) const MAX_RUNNING: usize = 4;
/// What a job's file keeps at most.
const FILE_CAP: u64 = 4 << 20;
/// What one `job_output` hands over at most.
const READ_CAP: u64 = 64 << 10;
/// The log's rule for a job stopped because its session became suspect.
const SUSPECT_RULE: &str = "judge/suspect";

/// What a job's file holds: bytes `dropped..written` of everything it wrote.
struct Out {
    file: PathBuf,
    written: u64,
    dropped: u64,
}

impl Out {
    fn append(&mut self, b: &[u8]) {
        let ok = fs::OpenOptions::new()
            .append(true)
            .open(&self.file)
            .and_then(|mut f| f.write_all(b));
        if ok.is_err() {
            return;
        }
        self.written += b.len() as u64;
        if self.written - self.dropped > FILE_CAP {
            self.drop_oldest();
        }
    }

    /// Keeps the newest half.
    fn drop_oldest(&mut self) {
        let keep = FILE_CAP / 2;
        let Ok(mut f) = fs::File::open(&self.file) else {
            return;
        };
        let len = self.written - self.dropped;
        let mut tail = Vec::new();
        if f.seek(SeekFrom::Start(len - keep)).is_err() || f.read_to_end(&mut tail).is_err() {
            return;
        }
        let tmp = self.file.with_extension("tmp");
        let written = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .and_then(|mut t| t.write_all(&tail))
            .and_then(|()| fs::rename(&tmp, &self.file));
        if written.is_ok() {
            self.dropped = self.written - tail.len() as u64;
        }
    }
}

#[derive(Clone, Copy)]
struct End {
    exit_code: Option<i32>,
    stopped: bool,
    timed_out: bool,
    /// Stopped because its session became suspect while it had the network.
    suspect: bool,
}

struct Job {
    session: String,
    what: String,
    /// It reached out or read a foreign folder: what it wrote is judged when it is read.
    judge: bool,
    /// It has the network (asked for, or a program that reaches out by itself).
    web: bool,
    stop: Arc<AtomicBool>,
    /// Set before `stop` when the session became suspect.
    suspect: Arc<AtomicBool>,
    out: Arc<Mutex<Out>>,
    end: Arc<Mutex<Option<End>>>,
    /// How far the model has read, in bytes of everything it wrote.
    read: u64,
}

impl Job {
    fn ended(&self) -> Option<End> {
        *lock(&self.end)
    }
}

#[derive(Default)]
pub(crate) struct Jobs {
    map: Mutex<HashMap<String, Job>>,
    next: AtomicU64,
}

impl ToolHost {
    fn job_answer(&self, req: &Request, content: String, data: Value, external: bool) -> Response {
        Response {
            id: req.id.clone(),
            ok: true,
            content,
            data,
            decision: DecisionOut {
                outcome: "allow".into(),
                rule: "host/job".into(),
                reason: "a background command of this session".into(),
            },
            tainted: self.is_tainted(&req.session),
            external,
        }
    }

    /// Starts `req` (an allowed `exec`, already past the list) in the background.
    pub(crate) fn job_start(&self, h: &Host, req: &Request, decision: DecisionOut) -> Response {
        let refuse =
            |why: String| self.refuse(&req.id, "host/job", why, self.is_tainted(&req.session));
        if self.stopping.load(Ordering::SeqCst) {
            return refuse("the session is ending".into());
        }
        let Some(jail) = h.jail.clone() else {
            return refuse("there is no host launcher".into());
        };
        let running = lock(&self.jobs.map)
            .values()
            .filter(|j| j.session == req.session && j.ended().is_none())
            .count();
        if running >= MAX_RUNNING {
            return refuse(format!(
                "{MAX_RUNNING} commands already run in the background in this session: stop one with job_stop or wait for one to end"
            ));
        }
        let args = req.args.as_object().cloned().unwrap_or_default();
        let command = args.get("command").and_then(Value::as_str).unwrap_or("");
        let network = args.get("network").and_then(Value::as_bool) == Some(true);
        let what = self.clean(command);
        let id = format!("j{}", self.jobs.next.fetch_add(1, Ordering::SeqCst) + 1);
        let file = self.run_dir.join("jobs").join(format!("{id}.log"));
        let made = private_dir(&self.run_dir.join("jobs")).and_then(|()| {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&file)
                .map(drop)
        });
        if let Err(e) = made {
            return refuse(format!("its output file cannot be made ({e})"));
        }
        let (state, run) = (h.cfg.state.clone(), h.cfg.run.clone());
        let line = |outcome: &str, code: Option<i64>| {
            log_line(
                &state,
                &run,
                &req.session,
                "exec",
                &what,
                network,
                outcome,
                &decision.rule,
                code,
            )
        };
        if let Err(e) = line("start", None) {
            return refuse(format!(
                "it was not started: the action log cannot be written ({e})"
            ));
        }
        let web = network || h.cfg.rules.reaches_out(command);
        let foreign = h.cfg.rules.reads_foreign(command);
        if web {
            self.mark(&req.session, "web");
        }
        if foreign {
            self.mark(&req.session, "foreign");
        }
        let job = HostJob {
            command: command.into(),
            cwd: h.cfg.home.clone(),
            home: h.cfg.home.clone(),
            network,
            timeout: args
                .get("timeout")
                .and_then(Value::as_u64)
                .map(|s| Duration::from_secs(s.clamp(1, crate::host::MAX_TIMEOUT_SECS))),
            max_output: Limits::default().max_output,
            masks: h.cfg.control.clone(),
        };
        let out = Arc::new(Mutex::new(Out {
            file,
            written: 0,
            dropped: 0,
        }));
        let (stop, end) = (Arc::new(AtomicBool::new(false)), Arc::new(Mutex::new(None)));
        let suspect = Arc::new(AtomicBool::new(false));
        let sink: Sink = {
            let out = out.clone();
            Arc::new(move |b: &[u8]| lock(&out).append(b))
        };
        {
            let (stop, end, out, suspect) =
                (stop.clone(), end.clone(), out.clone(), suspect.clone());
            let (session, what, rule) = (req.session.clone(), what.clone(), decision.rule.clone());
            std::thread::spawn(move || {
                let r = jail.run_streaming(&job, &|| stop.load(Ordering::SeqCst), sink);
                let e = match r {
                    Ok(o) => End {
                        exit_code: o.exit_code,
                        stopped: o.stopped,
                        timed_out: o.timed_out,
                        suspect: o.stopped && suspect.load(Ordering::SeqCst),
                    },
                    Err(e) => {
                        lock(&out).append(format!("\n[it could not start: {e}]\n").as_bytes());
                        End {
                            exit_code: None,
                            stopped: false,
                            timed_out: false,
                            suspect: false,
                        }
                    }
                };
                let outcome = if e.stopped { "stopped" } else { "done" };
                let code = e.exit_code.map(i64::from);
                let rule = if e.suspect { SUSPECT_RULE } else { &rule };
                if let Err(x) = log_line(
                    &state, &run, &session, "exec", &what, network, outcome, rule, code,
                ) {
                    eprintln!("clio-toolhost: the action log lost a line: {x}");
                }
                *lock(&end) = Some(e);
            });
        }
        lock(&self.jobs.map).insert(
            id.clone(),
            Job {
                session: req.session.clone(),
                what,
                judge: web || foreign,
                web,
                stop,
                suspect,
                out,
                end,
                read: 0,
            },
        );
        self.job_answer(
            req,
            format!(
                "started {id} in the background. Check it with job_output (id {id}) now and then, not in a tight loop; stop it with job_stop. It stops when the session ends."
            ),
            json!({ "job": id }),
            false,
        )
    }

    /// `job_output`, `job_stop` and `jobs` (how many run, for the chat).
    pub(crate) fn job_op(&self, req: &Request, args: &Map<String, Value>) -> Response {
        let tainted = self.is_tainted(&req.session);
        if self.host.is_none() {
            return self.refuse(
                &req.id,
                "host/job",
                "background commands exist only on the person's machine (host mode)".into(),
                tainted,
            );
        }
        if req.tool == "jobs" {
            let n = lock(&self.jobs.map)
                .values()
                .filter(|j| j.session == req.session && j.ended().is_none())
                .count();
            return self.job_answer(req, format!("{n} running"), json!({ "running": n }), false);
        }
        let id = args.get("id").and_then(Value::as_str).unwrap_or("");
        let found = lock(&self.jobs.map)
            .get(id)
            .filter(|j| j.session == req.session)
            .map(|j| (j.stop.clone(), j.end.clone()));
        let Some((stop, end)) = found else {
            return self.refuse(
                &req.id,
                "host/job",
                format!("there is no background command {id:?} in this session"),
                tainted,
            );
        };
        if req.tool == "job_stop" {
            let was = *lock(&end);
            if let Some(e) = was {
                return self.job_answer(
                    req,
                    format!("{id} had already ended{}", ending(&e)),
                    Value::Null,
                    false,
                );
            }
            stop.store(true, Ordering::SeqCst);
            let until = Instant::now() + Duration::from_secs(10);
            while lock(&end).is_none() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
            }
            return self.job_answer(req, format!("stopped {id}"), Value::Null, false);
        }
        self.job_read(req, id)
    }

    /// What `id` wrote since the last read, and whether it still runs.
    fn job_read(&self, req: &Request, id: &str) -> Response {
        let mut map = lock(&self.jobs.map);
        let Some(job) = map.get_mut(id) else {
            return self.refuse(&req.id, "host/job", format!("no {id}"), false);
        };
        // The end first: what it wrote before it ended is all in the file by then.
        let ended = job.ended();
        let (raw, dropped_note, more) = {
            let out = lock(&job.out);
            let mut note = String::new();
            if job.read < out.dropped {
                note = format!(
                    "[{} bytes of older output were dropped]\n",
                    out.dropped - job.read
                );
                job.read = out.dropped;
            }
            let from = job.read - out.dropped;
            let mut buf = Vec::new();
            if let Ok(mut f) = fs::File::open(&out.file)
                && f.seek(SeekFrom::Start(from)).is_ok()
            {
                let _ = f.take(READ_CAP).read_to_end(&mut buf);
            }
            // Whole lines only while more may come: a value cut in two would get past the
            // scrubber, which knows only whole values.
            let last = ended.is_some() && job.read + (buf.len() as u64) >= out.written;
            if !last && let Some(nl) = buf.iter().rposition(|b| *b == b'\n') {
                buf.truncate(nl + 1);
            } // shortcut: a line longer than READ_CAP is still handed over in parts.
            job.read += buf.len() as u64;
            (buf, note, job.read < out.written)
        };
        let judge = job.judge;
        let what = job.what.clone();
        drop(map);
        let raw = String::from_utf8_lossy(&raw).into_owned();
        let text = self.clean(&raw);
        let masked = count_masks(&text).saturating_sub(count_masks(&raw));
        let status = match ended {
            None => format!("[{id} still runs]"),
            Some(e) => format!("[{id} ended{}]", ending(&e)),
        };
        let more = if more {
            "\n[more output: call job_output again]"
        } else {
            ""
        };
        if judge {
            self.judge_later(req, &what);
        }
        self.job_answer(
            req,
            format!(
                "{dropped_note}{text}{}{status}{more}",
                if text.is_empty() || text.ends_with('\n') {
                    ""
                } else {
                    "\n"
                }
            ),
            json!({
                "running": ended.is_none(),
                "exit_code": ended.and_then(|e| e.exit_code),
                "masked": masked,
            }),
            true,
        )
    }

    /// The session just became suspect: its jobs with the network stop now (no wait), each with
    /// its line in the log under `judge/suspect`; what stays on the machine goes on.
    pub(crate) fn stop_web_jobs(&self, session: &str) {
        for j in lock(&self.jobs.map).values() {
            if j.session == session && j.web && j.ended().is_none() {
                j.suspect.store(true, Ordering::SeqCst);
                j.stop.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Every background command is stopped (the service stops); waits up to `grace` for them.
    pub(crate) fn stop_jobs(&self, grace: Duration) {
        let ends: Vec<_> = lock(&self.jobs.map)
            .values()
            .map(|j| {
                j.stop.store(true, Ordering::SeqCst);
                j.end.clone()
            })
            .collect();
        let until = Instant::now() + grace;
        while ends.iter().any(|e| lock(e).is_none()) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        // What they wrote is raw (cleaned only when read): it does not outlive the session. A job
        // still ending cannot make its file again (it only appends to one that exists).
        if let Err(e) = fs::remove_dir_all(self.run_dir.join("jobs"))
            && e.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("clio-toolhost: the background commands' output could not be removed: {e}");
        }
    }
}

fn ending(e: &End) -> String {
    if e.suspect {
        ": it was stopped because the session became suspect (it read text Clio's judge flagged) while this command had the network".into()
    } else if e.stopped {
        ": it was stopped".into()
    } else if e.timed_out {
        ": its time limit ran out".into()
    } else {
        e.exit_code.map_or_else(
            || ", killed by a signal".into(),
            |c| format!(", exit code {c}"),
        )
    }
}
