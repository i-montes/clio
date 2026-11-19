//! The reviewer in the chat: `/review`, the automatic review the judge decides after a turn that
//! changed something, what the person sees when one ends, and the turn that gives its findings to
//! the model. The review itself runs in the background (`Session::review`).

use clio_brain::pi::preprompt::{REVIEW_ASK, SKILL_NOTES_ASK};
use clio_brain::pi::review::Verdict;
use clio_brain::pi::rpc::{Cancel, Usage};

use super::app::{App, Cmd};
use super::chat::Item;
use crate::cli::chat::{ReviewRequest, Reviewed, Session, SkillOutcome, TurnWork};
use crate::cli::usage;

/// Findings shown in the chat (the rest are in the full text); what each shows at most.
const SHOWN: usize = 8;
const SHOWN_WHAT: usize = 300;
const SHOWN_EVIDENCE: usize = 600;
const SHOWN_RAW: usize = 1200;
const HOUR_MS: u64 = 3_600_000;
/// The chat's line for a turn the review started (it has no words of the person).
pub const REVIEW_TURN: &str = "turn started by the review: Clio answers its findings";
/// How such a turn's message starts in the session file, after Clio's envelope.
pub const FINDINGS_MARK: &str = "<<<UNTRUSTED-DATA subagent-summary";

/// The chat's reviews (one session's).
#[derive(Debug, Default)]
pub struct Review {
    /// A review runs since then (ms): `reviewing…` in the status line.
    pub running: Option<u64>,
    /// The judge is being asked whether the last turn needs one.
    pub judging: bool,
    /// The last turn that changed something: what `/review` with no argument reviews.
    pub work: Option<TurnWork>,
    /// A turn that ended changed something (its work came before it settled), and the judge
    /// has not been asked about it yet: it is, once the chat is idle.
    pub changed: bool,
    /// The turn running answers a review's findings: it is never reviewed itself.
    pub answering: bool,
    /// Findings waiting for the chat to be idle, the reviewer's mark and Clio's request.
    pub findings: Option<(String, Option<bool>, &'static str)>,
    /// What the reviews of this session spent.
    pub usage: Usage,
    pub count: u32,
    /// Skill drafts waiting for their review (id, name), oldest first: one at a time.
    pub skills: std::collections::VecDeque<(String, String)>,
}

/// The review settings, from the header (the defaults before it loaded).
fn cfg(app: &App) -> crate::cli::home::ReviewCfg {
    app.home
        .header
        .as_ref()
        .map(|h| h.review.clone())
        .unwrap_or_default()
}

/// `/review [what]` and `/review stop`.
pub fn command(app: &mut App, args: &str) -> Vec<Cmd> {
    let note = |app: &mut App, s: &str| {
        app.chat.items.push(Item::Note(s.to_owned()));
        vec![]
    };
    let r = &app.chat.review;
    if args.trim() == "stop" {
        if r.running.is_none() && !r.judging {
            return note(app, "no review is running");
        }
        return [note(app, "stopping the review…"), vec![Cmd::ReviewStop]].concat();
    }
    if r.running.is_some() || r.judging {
        return note(app, "a review is already running: /review stop ends it");
    }
    if app.chat.session_id.is_empty() {
        return note(
            app,
            "the session starts with your first message: nothing to review yet",
        );
    }
    if app.chat.running || app.chat.opening || app.chat.compacting {
        return note(app, "a turn is running: /review once it ends");
    }
    let what = Some(args.trim()).filter(|a| !a.is_empty());
    if what.is_none() && r.work.is_none() {
        return note(
            app,
            "nothing to review: no turn of this session changed anything yet (/review <what> says what to review)",
        );
    }
    app.chat.review.running = Some(app.now_ms);
    vec![Cmd::Review {
        session: app.chat.session_id.clone(),
        what: what.map(str::to_owned),
        work: app.chat.review.work.clone(),
        auto: false,
    }]
}

/// A turn ended: the turn that answered a review's findings is never reviewed itself.
pub fn turn_ended(app: &mut App) -> Vec<Cmd> {
    if std::mem::take(&mut app.chat.review.answering) {
        app.chat.review.changed = false;
    }
    after_turn(app)
}

/// A turn that changed something is waiting and the chat is idle (its questions answered, no
/// message queued): if the judge's review is on and under its caps, the judge is asked whether
/// it needs a review (`Cmd::Review` with `auto`). Called whenever something happens, so a turn
/// that ended with a question is asked about once it is answered; at most once per turn.
pub fn after_turn(app: &mut App) -> Vec<Cmd> {
    let r = &app.chat.review;
    if !r.changed || r.answering || super::app::chat_waits(app) {
        return vec![];
    }
    app.chat.review.changed = false;
    let cfg = cfg(app);
    let hour_ago = app.now_ms.saturating_sub(HOUR_MS);
    app.reviews_at.retain(|t| *t > hour_ago);
    let r = &app.chat.review;
    if !cfg.auto
        || r.running.is_some()
        || r.judging
        || app.reviews_at.len() >= cfg.max_per_hour as usize
    {
        return vec![];
    }
    app.chat.review.judging = true;
    vec![Cmd::Review {
        session: app.chat.session_id.clone(),
        what: None,
        work: app.chat.review.work.clone(),
        auto: true,
    }]
}

/// What `Cmd::Review` does on its worker. With `auto` the judge is asked first; nothing starts
/// (`Err(why)`) when it says no, or when the review was stopped meanwhile, so a stopped request
/// is never shown as started nor counted. Then `started` (with the judge's reason), and the
/// review.
pub fn job(
    s: &dyn Session,
    what: Option<&str>,
    work: Option<&TurnWork>,
    auto: bool,
    cancel: &Cancel,
    started: &mut dyn FnMut(String),
) -> Result<Result<Reviewed, String>, String> {
    let mut why = String::new();
    if auto {
        let j = work.ok_or("the turn changed nothing")?;
        let j = s.needs_review(j);
        if !j.yes() {
            return Err(j.reason());
        }
        why = j.reason();
    }
    if cancel.is_cancelled() {
        return Err("the review was stopped before it started".into());
    }
    started(why);
    Ok(s.review(&ReviewRequest::of_work(what, work), cancel))
}

/// A skill draft waits and no review runs: its review starts (S-68). It runs beside the chat, as
/// any review; the draft's own registration rules decide what comes of it.
pub fn next_skill(app: &mut App) -> Vec<Cmd> {
    let r = &app.chat.review;
    if r.running.is_some() || r.judging || app.chat.session_id.is_empty() {
        return vec![];
    }
    let Some((id, name)) = app.chat.review.skills.pop_front() else {
        return vec![];
    };
    app.chat.review.running = Some(app.now_ms);
    app.chat.items.push(Item::Note(
        if clio_toolhost::services::is_service_draft(&id) {
            format!(
                "reviewing the service «{}» before you are asked to approve it…",
                super::app::clean(&name)
            )
        } else {
            format!(
                "reviewing the skill «{}» before it is registered…",
                super::app::clean(&name)
            )
        },
    ));
    vec![Cmd::SkillReview {
        session: app.chat.session_id.clone(),
        id,
        name,
    }]
}

/// What came of a skill draft's review, for the person (one line, the Approval box) and the
/// model (the findings, as a review's).
fn skill_ended(app: &mut App, rv: &Reviewed, outcome: &SkillOutcome) {
    let verdict = match rv.answer.verdict {
        Verdict::CouldNotCheck => "could not check",
        v => v.word(),
    };
    let clean = super::app::clean;
    match outcome {
        SkillOutcome::Registered {
            name,
            description,
            n,
            notes,
        } => {
            // Notes of a review that passed: never a blocker, given to the model once.
            let left = match notes {
                Some(text) => {
                    app.chat.review.findings = Some((text.clone(), rv.marked, SKILL_NOTES_ASK));
                    let k = rv.answer.findings.len() + rv.answer.dropped;
                    format!(
                        " with {k} note{}{}; they go to Clio when the chat is idle",
                        if k == 1 { "" } else { "s" },
                        rv.full
                            .as_ref()
                            .map_or_else(String::new, |f| format!(" (full text: {})", cap(f, 300)))
                    )
                }
                // Notes on a version written in answer to notes: the person's only (no loop).
                None if rv.answer.goes_to_model() => {
                    let k = rv.answer.findings.len() + rv.answer.dropped;
                    format!(
                        " with {k} note{}, shown here only (its notes already went to Clio this hour)",
                        if k == 1 { "" } else { "s" }
                    )
                }
                None => String::new(),
            };
            app.chat.items.push(Item::Note(format!(
                "skill registered: «{}»{}: {} · review: {verdict}{left} · available from the next session (or when this one is reopened)",
                clean(name),
                if *n > 1 { format!(" (version {n})") } else { String::new() },
                cap(description, 200)
            )));
        }
        SkillOutcome::Waits { id, what, ask } => {
            let service = (**ask)
                .as_ref()
                .and_then(|b| b.skill.as_ref())
                .is_some_and(|s| s.service.is_some());
            app.chat.items.push(Item::Note(format!(
                "a {} waits for your approval ({id}) · review: {verdict}",
                if service { "service" } else { "skill" }
            )));
            let mut c =
                super::app::Confirm::new(super::app::Action::Approve, id.clone(), 0, what.clone());
            c.session.clone_from(&app.chat.session_id);
            c.block = (**ask).clone();
            app.asks.push_back(c);
            super::app::next_ask(app);
        }
        SkillOutcome::Rejected {
            name,
            attempt,
            dropped,
            to_model,
        }
        | SkillOutcome::ServiceRejected {
            name,
            attempt,
            dropped,
            to_model,
        } => {
            let kind = if matches!(outcome, SkillOutcome::ServiceRejected { .. }) {
                "service"
            } else {
                "skill"
            };
            app.chat.review.findings = Some((to_model.clone(), rv.marked, REVIEW_ASK));
            app.chat.items.push(Item::Note(if *dropped {
                format!(
                    "{kind} «{}» not registered: review failed three times in a row; the draft was dropped",
                    clean(name)
                )
            } else {
                format!(
                    "{kind} «{}» not registered: review failed (attempt {attempt} of 3); the findings go to Clio when the chat is idle",
                    clean(name)
                )
            }));
        }
    }
}

/// Findings waiting go to the model once the chat is idle and the person is not typing: an
/// automatic turn, shown as the review's. A message of the person's waiting goes first.
pub fn deliver(app: &mut App) -> Vec<Cmd> {
    if app.chat.review.findings.is_none()
        || super::app::chat_waits(app)
        || app.chat.failed.is_some()
        || !app.chat.input.is_empty()
        || app.chat.menu.is_some()
        || app.modal.is_some()
        || app.notice.is_some()
    {
        return vec![];
    }
    let Some((findings, marked, ask)) = app.chat.review.findings.take() else {
        return vec![];
    };
    app.chat.items.push(Item::Note(REVIEW_TURN.into()));
    app.chat.review.answering = true;
    app.chat.scroll = 0;
    app.chat.start_turn(app.now_ms);
    vec![Cmd::ReviewTurn {
        findings,
        marked,
        ask,
    }]
}

/// `claude-opus-5-5` → `opus`; any other id as it is.
fn short(model: &str) -> String {
    let m = super::app::clean(model);
    match m.strip_prefix("claude-") {
        Some(rest) => rest.split('-').next().unwrap_or(rest).to_owned(),
        None => m,
    }
}

fn cap(s: &str, max: usize) -> String {
    clio_brain::pi::text::line(s, max)
}

/// A review ended (or could not start).
pub fn ended(app: &mut App, session: &str, result: Result<Box<Reviewed>, String>) {
    if session != app.chat.session_id {
        return;
    }
    let r = &mut app.chat.review;
    r.running = None;
    r.judging = false;
    let rv = match result {
        Ok(rv) => rv,
        Err(e) => {
            app.chat
                .items
                .push(Item::Error(format!("review: {}", cap(&e, 300))));
            return;
        }
    };
    let u = &mut r.usage;
    u.input += rv.usage.input;
    u.output += rv.usage.output;
    u.cache_read += rv.usage.cache_read;
    u.cache_write += rv.usage.cache_write;
    u.total_tokens += rv.usage.total_tokens;
    u.cost += rv.usage.cost;
    r.count += 1;
    if rv.answer.goes_to_model() && rv.skill.is_none() {
        r.findings = Some((rv.answer.for_model(), rv.marked, REVIEW_ASK));
    }
    let a = &rv.answer;
    let verdict = match a.verdict {
        Verdict::CouldNotCheck => "could not check",
        v => v.word(),
    };
    let items = &mut app.chat.items;
    let s = if rv.commands == 1 { "" } else { "s" };
    items.push(Item::Note(format!(
        "review: {verdict} · {} · {} · ran {} command{s}",
        short(&rv.model),
        usage::duration(rv.secs),
        rv.commands
    )));
    if let Some(f) = &rv.fallback {
        items.push(Item::Note(format!("review: {}", cap(f, 200))));
    }
    let mut block = Vec::new();
    // What was run, when there is nothing else to go on (a `fails` with no findings too).
    if a.verdict == Verdict::CouldNotCheck || a.verdict == Verdict::Fails && a.findings.is_empty() {
        block.push(cap(&a.ran, 300));
    }
    for (i, f) in a.findings.iter().take(SHOWN).enumerate() {
        block.push(format!("{}. {}", i + 1, cap(&f.what, SHOWN_WHAT)));
        block.push(format!("   evidence: {}", cap(&f.evidence, SHOWN_EVIDENCE)));
    }
    let more = a.findings.len().saturating_sub(SHOWN) + a.dropped;
    if more > 0 {
        block.push(format!("and {more} more in the full text"));
    }
    if let Some(raw) = &a.raw {
        block.push(format!(
            "what the reviewer said instead (data, not followed): {}",
            cap(raw, SHOWN_RAW)
        ));
    }
    if !block.is_empty() {
        if let Some(full) = &rv.full {
            block.push(format!("full text: {}", cap(full, 300)));
        }
        items.push(Item::Note(block.join("\n")));
    }
    if rv.answer.goes_to_model() && rv.skill.is_none() {
        items.push(Item::Note(
            "the findings go to Clio when the chat is idle".into(),
        ));
    }
    if let Some(o) = &rv.skill {
        skill_ended(app, &rv, o);
    }
}

/// The automatic review started (the judge said yes: `why`), or `/review` did.
pub fn started(app: &mut App, session: &str, auto: bool, why: &str) {
    if session != app.chat.session_id {
        return;
    }
    app.chat.review.judging = false;
    app.chat.review.running.get_or_insert(app.now_ms);
    if auto {
        app.reviews_at.push(app.now_ms);
        app.chat.items.push(Item::Note(
            "the judge asked for a review of this turn".into(),
        ));
        debug_note(app, why);
    }
}

/// No review: the judge said no or could not be asked, or it was stopped first (`why`). Only
/// the debug mode shows it.
pub fn skipped(app: &mut App, session: &str, why: &str) {
    if session == app.chat.session_id {
        app.chat.review.judging = false;
        app.chat.review.running = None;
        debug_note(app, why);
    }
}

fn debug_note(app: &mut App, why: &str) {
    if app.debug && !why.is_empty() {
        app.chat
            .items
            .push(Item::Note(format!("review: {}", cap(why, 300))));
    }
}

/// The `/usage` line of the reviews.
pub fn usage_line(app: &App) -> Option<String> {
    let r = &app.chat.review;
    (r.count > 0).then(|| {
        format!(
            "reviews       {} · {} tokens in, {} out · {}",
            r.count,
            usage::tokens(r.usage.input + r.usage.cache_read + r.usage.cache_write),
            usage::tokens(r.usage.output),
            usage::usd((r.usage.cost * 1e6) as u64)
        )
    })
}

/// `reviewing… 48 s` for the status line.
pub fn status(app: &App) -> Option<String> {
    let since = app.chat.review.running?;
    Some(format!(
        "reviewing… {}",
        usage::duration(app.now_ms.saturating_sub(since) / 1000)
    ))
}
