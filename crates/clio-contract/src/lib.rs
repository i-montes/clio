//! Task contract (levels 2 and 3): fields, a state machine that rejects invalid transitions and a
//! durable JSON record. Only the person changes the objective, scope, role and policy (S-07);
//! the agent proposes amendments. Budget is enforced here (S-47).

use clio_core::{Clock, Level};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Actor {
    Person,
    Agent,
    /// Anything read from outside: files, pages, tool results. It can never change the contract.
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    Person,
    Scheduler,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum State {
    Draft,
    Approved,
    InProgress,
    Blocked,
    ClosedOk,
    ClosedFail,
    Cancelled,
}

impl State {
    fn can_go(self, to: State) -> bool {
        use State::*;
        matches!(
            (self, to),
            (Draft, Approved | Cancelled)
                | (Approved, InProgress | Cancelled)
                | (InProgress, Blocked | ClosedOk | ClosedFail | Cancelled)
                | (Blocked, InProgress | ClosedFail | Cancelled)
        )
    }
    pub fn is_closed(self) -> bool {
        matches!(self, State::ClosedOk | State::ClosedFail | State::Cancelled)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("only the person can do this")]
    NotPerson,
    #[error("invalid transition {0:?} -> {1:?}")]
    BadTransition(State, State),
    #[error("a criterion has no declared way to check it")]
    UncheckableCriterion,
    #[error("no criteria")]
    NoCriteria,
    #[error("scope is empty: say what is inside")]
    EmptyScope,
    #[error("the contract requires a spec first")]
    SpecRequired,
    #[error("level 3 needs approval bound to the contract digest")]
    ApprovalMissing,
    #[error("contracts are level 2 or 3")]
    LevelTooLow,
    #[error("a question is already open")]
    QuestionOpen,
    #[error("evidence is required")]
    NoEvidence,
    #[error("not found")]
    NotFound,
    #[error("not in progress")]
    NotInProgress,
    #[error("budget exhausted: contract closed with a report")]
    BudgetExhausted,
    #[error("cannot close as success: criteria or tasks are not done")]
    NotDone,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckKind {
    Test,
    Command,
    HumanInspection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub kind: CheckKind,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// What was observed: test output, a hash, a screenshot path.
    pub observed: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Criterion {
    pub text: String,
    /// `None` (or an empty detail) means there is no way to check it: not accepted.
    pub check: Option<Check>,
    pub attempts: u32,
    pub met: Option<Evidence>,
}

impl Criterion {
    pub fn new(text: impl Into<String>, check: Option<Check>) -> Self {
        Self {
            text: text.into(),
            check,
            attempts: 0,
            met: None,
        }
    }
    fn checkable(&self) -> bool {
        self.check
            .as_ref()
            .is_some_and(|c| !c.detail.trim().is_empty())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub included: Vec<String>,
    pub excluded: Vec<String>,
}

impl Scope {
    /// What is not inside is outside.
    pub fn contains(&self, item: &str) -> bool {
        let hit = |l: &[String]| {
            l.iter()
                .any(|p| item == p || item.starts_with(&format!("{}/", p.trim_end_matches('/'))))
        };
        hit(&self.included) && !hit(&self.excluded)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    pub max_steps: u64,
    pub max_ms: i64,
    pub max_cost_micro_cents: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spent {
    pub steps: u64,
    pub cost_micro_cents: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: u32,
    pub title: String,
    pub evidence: Option<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Change {
    AddCriterion(Criterion),
    SetBudget(Budget),
    SetScope(Scope),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AmendmentStatus {
    Proposed,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Amendment {
    pub version: u32,
    pub proposed_by: Actor,
    pub change: Change,
    pub status: AmendmentStatus,
}

/// The fixed closing report. `unfinished` is always there, empty or not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub verified: Vec<String>,
    pub unfinished: Vec<String>,
    pub next_step: String,
}

/// One state change, for the trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub contract_id: String,
    pub trace_id: String,
    pub from: State,
    pub to: State,
    pub at_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Telemetry {
    pub ms_by_phase: BTreeMap<State, i64>,
    pub criteria_first_try: u32,
    pub amendments: u32,
    pub closes: BTreeMap<State, u32>,
    pub tasks_reopened: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contract {
    id: String,
    level: Level,
    objective: String,
    role: Option<String>,
    scope: Scope,
    requires_spec: bool,
    spec: Option<String>,
    criteria: Vec<Criterion>,
    budget: Budget,
    spent: Spent,
    policy: Option<String>,
    nodes: Vec<String>,
    tasks: Vec<Task>,
    open_question: Option<String>,
    origin: Origin,
    state: State,
    trace_id: String,
    amendments: Vec<Amendment>,
    report: Option<Report>,
    events: Vec<Event>,
    telemetry: Telemetry,
    entered_at_ms: i64,
    started_at_ms: Option<i64>,
}

impl Contract {
    /// A draft. `objective` must come from the person's message (S-07). Levels 0 and 1 carry no contract.
    pub fn draft(
        id: impl Into<String>,
        level: Level,
        objective: impl Into<String>,
        origin: Origin,
        trace_id: impl Into<String>,
        clock: &dyn Clock,
    ) -> Result<Self> {
        if level < Level::Full {
            return Err(Error::LevelTooLow);
        }
        Ok(Self {
            id: id.into(),
            level,
            objective: objective.into(),
            role: None,
            scope: Scope::default(),
            requires_spec: false,
            spec: None,
            criteria: Vec::new(),
            budget: Budget::default(),
            spent: Spent::default(),
            policy: None,
            nodes: Vec::new(),
            tasks: Vec::new(),
            open_question: None,
            origin,
            state: State::Draft,
            trace_id: trace_id.into(),
            amendments: Vec::new(),
            report: None,
            events: Vec::new(),
            telemetry: Telemetry::default(),
            entered_at_ms: clock.now_ms(),
            started_at_ms: None,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn level(&self) -> Level {
        self.level
    }
    pub fn objective(&self) -> &str {
        &self.objective
    }
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn criteria(&self) -> &[Criterion] {
        &self.criteria
    }
    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }
    pub fn report(&self) -> Option<&Report> {
        self.report.as_ref()
    }
    pub fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }
    pub fn amendments(&self) -> &[Amendment] {
        &self.amendments
    }
    pub fn open_question(&self) -> Option<&str> {
        self.open_question.as_deref()
    }
    pub fn spent(&self) -> &Spent {
        &self.spent
    }
    /// Events not yet handed to the trace.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
    /// Writes (to the sandbox or outside it) are allowed only while in progress.
    pub fn can_write(&self) -> bool {
        self.state == State::InProgress
    }

    fn person(by: Actor) -> Result<()> {
        if by == Actor::Person {
            Ok(())
        } else {
            Err(Error::NotPerson)
        }
    }

    // --- fields only the person changes (S-07) ---

    pub fn set_objective(&mut self, by: Actor, text: impl Into<String>) -> Result<()> {
        Self::person(by)?;
        self.objective = text.into();
        Ok(())
    }
    pub fn set_scope(&mut self, by: Actor, scope: Scope) -> Result<()> {
        Self::person(by)?;
        self.scope = scope;
        Ok(())
    }
    pub fn set_role(&mut self, by: Actor, role: Option<String>) -> Result<()> {
        Self::person(by)?;
        self.role = role;
        Ok(())
    }
    pub fn set_policy(
        &mut self,
        by: Actor,
        policy: Option<String>,
        nodes: Vec<String>,
    ) -> Result<()> {
        Self::person(by)?;
        self.policy = policy;
        self.nodes = nodes;
        Ok(())
    }
    pub fn set_budget(&mut self, by: Actor, budget: Budget) -> Result<()> {
        Self::person(by)?;
        self.budget = budget;
        Ok(())
    }
    pub fn require_spec(&mut self, by: Actor, yes: bool) -> Result<()> {
        Self::person(by)?;
        self.requires_spec = yes;
        Ok(())
    }

    /// The level comes from triage and only goes up.
    pub fn raise_level(&mut self, to: Level) {
        self.level = self.level.max(to);
    }

    /// Attach the spec; its acceptance tests become success criteria.
    pub fn attach_spec(&mut self, spec: impl Into<String>, acceptance: Vec<Criterion>) {
        self.spec = Some(spec.into());
        self.criteria.extend(acceptance);
    }
    pub fn add_criterion(&mut self, c: Criterion) {
        self.criteria.push(c);
    }
    pub fn add_task(&mut self, title: impl Into<String>) -> u32 {
        let id = self.tasks.len() as u32 + 1;
        self.tasks.push(Task {
            id,
            title: title.into(),
            evidence: None,
        });
        id
    }

    // --- one open question at a time ---

    pub fn ask(&mut self, question: impl Into<String>) -> Result<()> {
        if self.open_question.is_some() {
            return Err(Error::QuestionOpen);
        }
        self.open_question = Some(question.into());
        Ok(())
    }
    pub fn answer(&mut self) {
        self.open_question = None;
    }

    // --- state machine ---

    fn go(&mut self, to: State, clock: &dyn Clock) -> Result<()> {
        if !self.state.can_go(to) {
            return Err(Error::BadTransition(self.state, to));
        }
        let now = clock.now_ms();
        *self.telemetry.ms_by_phase.entry(self.state).or_default() += now - self.entered_at_ms;
        self.events.push(Event {
            contract_id: self.id.clone(),
            trace_id: self.trace_id.clone(),
            from: self.state,
            to,
            at_ms: now,
        });
        self.state = to;
        self.entered_at_ms = now;
        if to.is_closed() {
            *self.telemetry.closes.entry(to).or_default() += 1;
        }
        Ok(())
    }

    /// Digest of what the person approves (objective, scope, criteria); level 3 approval is bound to it (S-14).
    pub fn digest(&self) -> String {
        let body =
            serde_json::to_vec(&(&self.objective, &self.scope, &self.criteria, &self.budget))
                .unwrap_or_default();
        Sha256::digest(&body)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Draft -> Approved. Checks every criterion, scope and spec; level 3 needs the digest.
    pub fn approve(
        &mut self,
        by: Actor,
        passkey_digest: Option<&str>,
        clock: &dyn Clock,
    ) -> Result<()> {
        Self::person(by)?;
        if self.criteria.is_empty() {
            return Err(Error::NoCriteria);
        }
        if !self.criteria.iter().all(Criterion::checkable) {
            return Err(Error::UncheckableCriterion);
        }
        if self.scope.included.is_empty() {
            return Err(Error::EmptyScope);
        }
        if self.requires_spec && self.spec.is_none() {
            return Err(Error::SpecRequired);
        }
        if self.level == Level::Long && passkey_digest != Some(self.digest().as_str()) {
            return Err(Error::ApprovalMissing);
        }
        self.go(State::Approved, clock)
    }

    /// Approved -> InProgress. The record is saved atomically first: no write before the durable record.
    pub fn start(&mut self, path: &Path, clock: &dyn Clock) -> Result<()> {
        if self.state != State::Approved {
            return Err(Error::BadTransition(self.state, State::InProgress));
        }
        self.go(State::InProgress, clock)?;
        self.started_at_ms = Some(clock.now_ms());
        if let Err(e) = self.save(path) {
            self.state = State::Approved; // not durable: do not write
            return Err(e);
        }
        Ok(())
    }

    pub fn block(&mut self, clock: &dyn Clock) -> Result<()> {
        self.go(State::Blocked, clock)
    }
    pub fn unblock(&mut self, clock: &dyn Clock) -> Result<()> {
        self.go(State::InProgress, clock)
    }
    pub fn cancel(&mut self, by: Actor, next_step: &str, clock: &dyn Clock) -> Result<()> {
        Self::person(by)?;
        self.close(State::Cancelled, next_step, clock)
    }

    // --- evidence ---

    fn evidence(e: Evidence) -> Result<Evidence> {
        if e.observed.trim().is_empty() {
            Err(Error::NoEvidence)
        } else {
            Ok(e)
        }
    }

    /// A task is done only with observed evidence.
    pub fn complete_task(&mut self, id: u32, e: Evidence) -> Result<()> {
        if self.state != State::InProgress {
            return Err(Error::NotInProgress);
        }
        let e = Self::evidence(e)?;
        self.tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or(Error::NotFound)?
            .evidence = Some(e);
        Ok(())
    }
    pub fn reopen_task(&mut self, id: u32) -> Result<()> {
        let t = self
            .tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or(Error::NotFound)?;
        if t.evidence.take().is_some() {
            self.telemetry.tasks_reopened += 1;
        }
        Ok(())
    }
    /// Record one check of a criterion. It is met only with evidence.
    pub fn check_criterion(&mut self, idx: usize, passed: Option<Evidence>) -> Result<()> {
        let ev = passed.map(Self::evidence).transpose()?;
        let c = self.criteria.get_mut(idx).ok_or(Error::NotFound)?;
        c.attempts += 1;
        if let Some(ev) = ev {
            if c.attempts == 1 {
                self.telemetry.criteria_first_try += 1;
            }
            c.met = Some(ev);
        }
        Ok(())
    }

    // --- budget (S-47) ---

    /// Charge steps and cost; checks time against the clock. At the cap the contract closes with a report.
    pub fn spend(&mut self, steps: u64, cost_micro_cents: u64, clock: &dyn Clock) -> Result<()> {
        if self.state != State::InProgress {
            return Err(Error::NotInProgress);
        }
        self.spent.steps += steps;
        self.spent.cost_micro_cents += cost_micro_cents;
        let elapsed = clock.now_ms() - self.started_at_ms.unwrap_or(0);
        let b = &self.budget;
        if self.spent.steps > b.max_steps
            || self.spent.cost_micro_cents > b.max_cost_micro_cents
            || elapsed > b.max_ms
        {
            self.close(
                State::ClosedFail,
                "budget exhausted: ask the person for an amendment or a new contract",
                clock,
            )?;
            return Err(Error::BudgetExhausted);
        }
        Ok(())
    }

    // --- closing ---

    fn build_report(&self, next_step: &str) -> Report {
        let verified = self
            .criteria
            .iter()
            .filter(|c| c.met.is_some())
            .map(|c| c.text.clone())
            .collect();
        let mut unfinished: Vec<String> = self
            .criteria
            .iter()
            .filter(|c| c.met.is_none())
            .map(|c| format!("criterion: {}", c.text))
            .collect();
        unfinished.extend(
            self.tasks
                .iter()
                .filter(|t| t.evidence.is_none())
                .map(|t| format!("task: {}", t.title)),
        );
        Report {
            verified,
            unfinished,
            next_step: next_step.to_owned(),
        }
    }

    fn close(&mut self, to: State, next_step: &str, clock: &dyn Clock) -> Result<()> {
        self.go(to, clock)?;
        self.report = Some(self.build_report(next_step));
        Ok(())
    }

    /// Success needs every criterion met and every task done; otherwise use `close_failed`.
    pub fn close_ok(&mut self, next_step: &str, clock: &dyn Clock) -> Result<()> {
        if self.criteria.iter().any(|c| c.met.is_none())
            || self.tasks.iter().any(|t| t.evidence.is_none())
        {
            return Err(Error::NotDone);
        }
        self.close(State::ClosedOk, next_step, clock)
    }
    pub fn close_failed(&mut self, next_step: &str, clock: &dyn Clock) -> Result<()> {
        self.close(State::ClosedFail, next_step, clock)
    }

    // --- amendments: the agent proposes, only the person accepts ---

    pub fn propose(&mut self, by: Actor, change: Change) -> Result<u32> {
        if by == Actor::External {
            return Err(Error::NotPerson);
        }
        let version = self.amendments.len() as u32 + 1;
        self.amendments.push(Amendment {
            version,
            proposed_by: by,
            change,
            status: AmendmentStatus::Proposed,
        });
        Ok(version)
    }

    pub fn accept(&mut self, by: Actor, version: u32) -> Result<()> {
        Self::person(by)?;
        let a = self
            .amendments
            .iter_mut()
            .find(|a| a.version == version)
            .ok_or(Error::NotFound)?;
        if let Change::AddCriterion(c) = &a.change
            && !c.checkable()
        {
            return Err(Error::UncheckableCriterion);
        }
        a.status = AmendmentStatus::Accepted;
        match a.change.clone() {
            Change::AddCriterion(c) => self.criteria.push(c),
            Change::SetBudget(b) => self.budget = b,
            Change::SetScope(s) => self.scope = s,
        }
        self.telemetry.amendments += 1;
        Ok(())
    }

    // --- durable record ---

    /// Atomic write: temp file in the same directory, fsync, rename.
    pub fn save(&self, path: &Path) -> Result<()> {
        use std::io::Write;
        let tmp = path.with_extension("json.tmp");
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(self)?)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }
}

/// Mailbox lanes while a task runs: stop/cancel and amendments are immediate, the rest waits for the close.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Immediate,
    Normal,
}

/// A rule, not Jev, picks the lane.
pub fn lane(message: &str) -> Lane {
    let m = message.trim().to_lowercase();
    let immediate = ["para", "cancela", "cambia", "no toques"];
    if immediate
        .iter()
        .any(|p| m == *p || m.starts_with(&format!("{p} ")))
    {
        Lane::Immediate
    } else {
        Lane::Normal
    }
}

#[derive(Debug, Default)]
pub struct Inbox {
    immediate: Vec<String>,
    normal: Vec<String>,
}

impl Inbox {
    pub fn push(&mut self, message: &str) -> Lane {
        let l = lane(message);
        match l {
            Lane::Immediate => self.immediate.push(message.to_owned()),
            Lane::Normal => self.normal.push(message.to_owned()),
        }
        l
    }
    pub fn take_immediate(&mut self) -> Vec<String> {
        std::mem::take(&mut self.immediate)
    }
    /// At the close of the running task: the whole queue at once.
    pub fn drain_normal(&mut self) -> Vec<String> {
        std::mem::take(&mut self.normal)
    }
}
