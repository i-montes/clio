//! TUI tests with no terminal: keys against `update`, drawing against `TestBackend`, and hostile
//! text against both. The tests of the context, usage and debug views are in `tests_views`.

#![allow(clippy::unwrap_used)]

use std::cell::Cell;

use clio_brain::agent::Event;
use clio_core::Level as TaskLevel;
use clio_gateway::Outcome;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::app::{Action, App, Bg, Cmd, Input, Planned, Screen, draw, update};
use super::detail::{Detail, Row};
use super::doctor::DoctorView;
use super::home::{Header, KeyState};
use super::new_run::Preview;
use super::theme::Theme;
use crate::cli::doctor::{Check, Report};
use crate::cli::runs::{RunInfo, RunSummary, Status};
use crate::cli::ui::Level;
use crate::cli::usage::{RunLimits, RunUsage};

// ------------------------------------------------------------ helpers

pub(super) fn key(c: KeyCode) -> Input {
    Input::Key(KeyEvent::new(c, KeyModifiers::NONE))
}
pub(super) fn ch(c: char) -> Input {
    key(KeyCode::Char(c))
}
pub(super) fn ctrl(c: char) -> Input {
    Input::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

pub(super) const THEME: Theme = Theme {
    color: true,
    unicode: true,
    rgb: false,
};

pub(super) fn app() -> App {
    App::new("/home/ivan/project", THEME)
}

/// Feeds inputs one by one, collecting every command.
pub(super) fn feed(mut a: App, inputs: Vec<Input>) -> (App, Vec<Cmd>) {
    let mut all = Vec::new();
    for i in inputs {
        let (n, c) = update(a, i);
        a = n;
        all.extend(c);
    }
    (a, all)
}

pub(super) fn summary(id: &str, ending: &str, files: &[&str]) -> RunSummary {
    RunSummary {
        id: id.into(),
        workspace: "/home/ivan/project".into(),
        instruction: "fix parse_date".into(),
        ending: ending.into(),
        level: "Simple".into(),
        steps: 7,
        elapsed_ms: 12_300,
        input_tokens: 12_400,
        output_tokens: 3_100,
        modified: files.iter().map(|s| (*s).to_owned()).collect(),
        final_message: Some("Fixed parse_date.\nCould not run the tests.".into()),
        ..RunSummary::default()
    }
}

pub(super) fn info(id: &str, status: Status, instr: &str, files: &[&str]) -> RunInfo {
    RunInfo {
        id: id.into(),
        status,
        instruction: instr.into(),
        workspace: "/home/ivan/project".into(),
        created_ms: Some(0),
        summary: Some(summary(id, "Final", files)),
    }
}

pub(super) const ID1: &str = "20261031-181646-5a67";
pub(super) const ID2: &str = "20261030-090000-0b1c";
pub(super) const ID3: &str = "20261029-120000-ffff";

pub(super) const LIMITS: RunLimits = RunLimits {
    max_steps: 40,
    max_seconds: 600,
    max_cost_micro: 100_000,
};

pub(super) fn with_runs() -> App {
    let mut a = app();
    a.home.header = Some(Header {
        model: "beam-free".into(),
        thinking: "low".into(),
        key: KeyState::Present,
        connector: "opencode".into(),
        vault: "not created (optional)".into(),
        config_error: None,
        limits: LIMITS,
        window: 128_000,
        window_known: true,
        context_cap: 200_000,
        review: crate::cli::home::ReviewCfg::default(),
    });
    a.home.doctor = Some((12, 3, 0));
    a.home.last_usage = super::tests_views::usage_with_context();
    a.home.runs = vec![
        info(
            ID1,
            Status::Pending,
            "fix parse_date in dates.py",
            &["dates.py"],
        ),
        info(
            ID2,
            Status::Applied,
            "add a README",
            &["README.md", "docs/a.md"],
        ),
        info(ID3, Status::Failed, "refactor everything", &[]),
    ];
    a
}

pub(super) fn row(kind: char, rel: &str, diff: &str) -> Row {
    Row {
        kind,
        rel: rel.into(),
        diff: diff.into(),
    }
}

const DIFF: &str = "--- a/dates.py\n+++ b/dates.py\n@@ -1,3 +1,3 @@\n def parse(s):\n-    return int(s)\n+    return None\n x = 1\n";

pub(super) fn detail(id: &str, status: Status) -> Detail {
    Detail {
        info: info(id, status, "fix parse_date", &["dates.py", "docs/new.md"]),
        rows: vec![
            row('M', "dates.py", DIFF),
            row(
                'A',
                "docs/new.md",
                "--- /dev/null\n+++ b/docs/new.md\n@@ -0,0 +1,1 @@\n+hola\n",
            ),
        ],
        sel: 0,
        in_diff: false,
        scroll: 0,
        usage: super::tests_views::usage_with_context(),
        view_h: Cell::new(10),
        mem_sel: 0,
    }
}

pub(super) fn open_detail(a: App, d: Detail, diff: bool) -> App {
    update(
        a,
        Input::Bg(Bg::Detail {
            detail: Ok(Box::new(d)),
            diff,
        }),
    )
    .0
}

pub(super) fn screen_text(a: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, a)).unwrap();
    let buf = t.backend().buffer();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

/// No cell of the screen may hold a control character or a deceptive format character.
pub(super) fn assert_clean(screen: &str) {
    for c in screen.chars() {
        let bad = (c.is_control() && c != '\n')
            || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{FEFF}');
        assert!(
            !bad,
            "a control or bidi character reached the screen: {c:?}\n{screen}"
        );
    }
}

/// The sizes every screen is drawn at: the minimum, the usual one, a wide one and a huge one.
pub(super) const SIZES: [(u16, u16); 4] = [(60, 16), (80, 24), (120, 30), (200, 50)];

/// The text has the exact shape of the terminal: nothing wider or taller was drawn.
pub(super) fn assert_fits(t: &str, w: u16, h: u16) {
    assert_eq!(t.lines().count(), usize::from(h), "{t}");
    assert!(
        t.lines().all(|l| l.chars().count() == usize::from(w)),
        "{t}"
    );
}

fn sizes() -> [(u16, u16); 5] {
    [(60, 16), (80, 24), (120, 30), (200, 50), (42, 11)]
}

// ------------------------------------------------------------ update: keys

#[test]
fn home_navigation_clamps_and_opens_the_selected_run() {
    let (a, cmds) = feed(with_runs(), vec![ch('j'), ch('j'), ch('j'), ch('j')]);
    assert_eq!(a.home.sel, 2, "clamped at the last run");
    assert!(cmds.is_empty());
    let (a, _) = feed(a, vec![ch('k'), key(KeyCode::Up), key(KeyCode::Up)]);
    assert_eq!(a.home.sel, 0);
    let (a, cmds) = feed(a, vec![ch('G')]);
    assert_eq!(a.home.sel, 2);
    assert!(cmds.is_empty());
    let (a, cmds) = feed(a, vec![key(KeyCode::Home), key(KeyCode::Enter)]);
    assert_eq!(
        cmds,
        vec![Cmd::LoadDetail {
            id: ID1.into(),
            diff: false
        }]
    );
    let (_, cmds) = feed(a, vec![ch('d')]);
    assert_eq!(
        cmds,
        vec![Cmd::LoadDetail {
            id: ID1.into(),
            diff: true
        }]
    );
}

#[test]
fn home_keys_new_refresh_doctor_help_quit() {
    let (a, cmds) = feed(with_runs(), vec![ch('n')]);
    assert_eq!(a.screen, Screen::NewRun);
    assert_eq!(
        cmds,
        vec![Cmd::Check {
            dir: "/home/ivan/project".into(),
            seq: 1
        }]
    );
    let (a, cmds) = feed(a, vec![key(KeyCode::Esc)]);
    assert_eq!((a.screen, cmds), (Screen::Home, vec![Cmd::LoadHome]));
    let (a, cmds) = feed(a, vec![ch('r')]);
    assert_eq!(cmds, vec![Cmd::LoadHome]);
    assert!(a.flash.is_some());
    let (a, cmds) = feed(a, vec![ch('D')]);
    assert_eq!((a.screen, cmds), (Screen::Doctor, vec![Cmd::RunDoctor]));
    let (a, _) = feed(a, vec![key(KeyCode::Esc), ch('?')]);
    assert_eq!(a.screen, Screen::Help);
    let (a, _) = feed(a, vec![ch('x')]);
    assert_eq!(
        a.screen,
        Screen::Help,
        "a key the help does not use changes nothing"
    );
    let (a, _) = feed(a, vec![key(KeyCode::Esc)]);
    assert_eq!(a.screen, Screen::Home);
    assert!(!a.quit);
    let (a, _) = feed(a, vec![key(KeyCode::Esc)]);
    assert!(!a.quit, "Esc at home does not leave: q does");
    assert!(feed(a, vec![ch('q')]).0.quit);
    assert!(feed(with_runs(), vec![ctrl('c')]).0.quit);
}

#[test]
fn help_returns_to_where_it_was_opened_from() {
    for open in [ch('n'), ch('D')] {
        let (a, _) = feed(with_runs(), vec![open]);
        // The new job takes F1 (a `?` is text there); the doctor takes `?`.
        let help = if a.screen == Screen::NewRun {
            key(KeyCode::F(1))
        } else {
            ch('?')
        };
        let from = a.screen;
        let (a, _) = feed(a, vec![help]);
        assert_eq!(a.screen, Screen::Help);
        let (a, _) = feed(a, vec![ch('?')]);
        assert_eq!(a.screen, from);
    }
}

#[test]
fn ctrl_l_asks_for_a_redraw_and_ctrl_d_flips_debug_on_every_screen() {
    let mut screens = vec![with_runs()];
    screens.push(feed(with_runs(), vec![ch('n')]).0);
    screens.push(feed(with_runs(), vec![ch('D')]).0);
    screens.push(open_detail(
        with_runs(),
        detail(ID1, Status::Pending),
        false,
    ));
    screens.push(open_detail(with_runs(), detail(ID1, Status::Pending), true));
    screens.push(feed(with_runs(), vec![ch('c')]).0);
    screens.push(feed(with_runs(), vec![ch('?')]).0);
    let mut live = with_runs();
    live.screen = Screen::Live;
    screens.push(live);
    for a in screens {
        let screen = a.screen;
        let (b, cmds) = feed(a, vec![ctrl('l')]);
        assert_eq!(cmds, vec![Cmd::Redraw], "{screen:?}");
        assert_eq!(b.screen, screen, "redrawing changes no screen");
        let (b, cmds) = feed(b, vec![ctrl('d')]);
        assert_eq!(cmds, vec![Cmd::Debug(true)], "{screen:?}");
        assert!(b.debug && b.flash.as_deref().is_some_and(|f| f.contains("debug on")));
        assert_eq!(b.screen, screen, "toggling debug changes no screen");
        let (b, cmds) = feed(b, vec![ctrl('d')]);
        assert_eq!(cmds, vec![Cmd::Debug(false)]);
        assert!(!b.debug);
    }
}

#[test]
fn ctrl_d_is_debug_and_a_plain_d_is_still_the_diff() {
    let (a, cmds) = feed(with_runs(), vec![ctrl('d')]);
    assert!(
        a.debug && cmds == vec![Cmd::Debug(true)],
        "no diff was opened"
    );
    let (_, cmds) = feed(a, vec![ch('d')]);
    assert_eq!(
        cmds,
        vec![Cmd::LoadDetail {
            id: ID1.into(),
            diff: true
        }]
    );
    // In the text fields Ctrl-D is still debug and never types a `d`.
    let (a, _) = feed(with_runs(), vec![ch('n'), ctrl('d')]);
    assert!(a.debug && a.new_run.instruction.is_empty());
}

#[test]
fn apply_needs_a_plan_then_a_y() {
    let (a, cmds) = feed(with_runs(), vec![ch('a')]);
    assert_eq!(cmds, vec![Cmd::PlanApply { id: ID1.into() }]);
    // The checks passed: the dialog repeats how many files and where.
    let (a, _) = feed(
        a,
        vec![Input::Bg(Bg::Planned(Ok(planned(ID1, Some(false)))))],
    );
    let c = a.confirm.clone().unwrap();
    assert_eq!((c.action, c.files), (Action::Apply, 2));
    let text = screen_text(&a, 80, 24);
    assert!(
        text.contains("Apply 2 file(s) in") && text.contains("/home/ivan/project"),
        "{text}"
    );
    // Anything but y cancels and nothing is applied.
    let (b, cmds) = feed(a.clone_for_test(), vec![key(KeyCode::Enter)]);
    assert!(cmds.is_empty() && b.confirm.is_none());
    let (_, cmds) = feed(a, vec![ch('y')]);
    assert_eq!(cmds, vec![Cmd::Apply { id: ID1.into() }]);
}

fn planned(id: &str, tainted: Option<bool>) -> Planned {
    Planned {
        id: id.into(),
        files: 2,
        workspace: "/home/ivan/project".into(),
        names: vec!["M src/dates.py".into(), "+ notas\u{1b}[2J.md".into()],
        tainted,
    }
}

fn alt(c: char) -> Input {
    Input::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT))
}

// S-36 (second audit, finding 3a): the answer is a plain `y`. Ctrl-S (save, by habit), Ctrl-Y and
// Alt-S are not a yes.
#[test]
fn only_a_plain_y_or_s_confirms_an_apply() {
    let (asked, _) = feed(
        with_runs(),
        vec![
            ch('a'),
            Input::Bg(Bg::Planned(Ok(planned(ID1, Some(false))))),
        ],
    );
    for no in [ctrl('s'), ctrl('y'), alt('s'), alt('y')] {
        let (b, cmds) = feed(asked.clone_for_test(), vec![no]);
        assert!(cmds.is_empty() && b.confirm.is_none(), "{cmds:?}");
    }
    for yes in [ch('y'), ch('Y'), ch('s'), ch('S')] {
        let (_, cmds) = feed(asked.clone_for_test(), vec![yes]);
        assert_eq!(cmds, vec![Cmd::Apply { id: ID1.into() }]);
    }
}

// S-36 (finding 3b): the plan takes seconds; the dialog must not pop up over whatever the person
// is doing by then, where the next `s` they type (the commonest letter in Spanish) would apply.
#[test]
fn a_plan_that_arrives_after_the_person_moved_on_does_not_ask() {
    // They left for a new run and are typing the instruction.
    let (a, _) = feed(with_runs(), vec![ch('a'), ch('n')]);
    let (a, cmds) = feed(
        a,
        vec![
            Input::Bg(Bg::Planned(Ok(planned(ID1, Some(false))))),
            ch('s'),
        ],
    );
    assert!(a.confirm.is_none(), "the dialog opened over the new run");
    assert!(cmds.is_empty(), "{cmds:?}");
    assert_eq!(
        a.new_run.instruction, "s",
        "the key was text, not an answer"
    );
    // Still on the list, but the plan is for another run than the selected one.
    let (a, cmds) = feed(
        with_runs(),
        vec![
            Input::Bg(Bg::Planned(Ok(planned(ID2, Some(false))))),
            ch('y'),
        ],
    );
    assert!(a.confirm.is_none() && cmds.is_empty(), "{cmds:?}");
}

// S-36 (finding 3c): what is about to be written is named, not only counted, and the dialog says
// when the agent read outside content (or that nothing says so).
#[test]
fn the_apply_dialog_names_the_files_and_the_taint() {
    for (tainted, note) in [(Some(true), "read outside content"), (None, "No record")] {
        let (a, _) = feed(
            with_runs(),
            vec![Input::Bg(Bg::Planned(Ok(planned(ID1, tainted))))],
        );
        let text = screen_text(&a, 90, 24);
        assert!(
            text.contains("src/dates.py") && text.contains("notas"),
            "{text}"
        );
        assert!(text.contains(note), "{text}");
        assert!(!text.contains('\u{1b}'), "{text}");
    }
    let (a, _) = feed(
        with_runs(),
        vec![Input::Bg(Bg::Planned(Ok(planned(ID1, Some(false)))))],
    );
    let text = screen_text(&a, 90, 24);
    assert!(
        !text.contains("read outside content") && !text.contains("No record"),
        "{text}"
    );
}

#[test]
fn a_refused_plan_shows_the_reasons_and_applies_nothing() {
    let (a, cmds) = feed(
        with_runs(),
        vec![Input::Bg(Bg::Planned(Err(
            "rejected, nothing was applied:\n  .git/hooks/x: protected path".into(),
        )))],
    );
    assert!(cmds.is_empty() && a.confirm.is_none());
    let text = screen_text(&a, 80, 24);
    assert!(
        text.contains("protected path") && text.contains("Notice"),
        "{text}"
    );
    let (a, cmds) = feed(a, vec![ch('y')]);
    assert!(cmds.is_empty(), "the key only closes the notice");
    assert!(a.notice.is_none());
}

#[test]
fn undo_only_for_applied_runs_and_it_asks() {
    let (a, cmds) = feed(with_runs(), vec![ch('u')]);
    assert!(
        cmds.is_empty() && a.confirm.is_none() && a.notice.is_some(),
        "run 1 is not applied"
    );
    let (a, cmds) = feed(a, vec![ch('x'), ch('j'), ch('u')]);
    assert!(cmds.is_empty());
    assert_eq!(a.confirm.as_ref().unwrap().action, Action::Undo);
    let (_, cmds) = feed(a, vec![ch('y')]);
    assert_eq!(cmds, vec![Cmd::Undo { id: ID2.into() }]);
}

#[test]
fn new_run_validates_before_launching() {
    let (a, _) = feed(with_runs(), vec![ch('n')]);
    // No instruction and no check yet: Enter does nothing.
    let (a, cmds) = feed(a, vec![key(KeyCode::Enter)]);
    assert!(cmds.is_empty() && a.screen == Screen::NewRun);
    let (a, _) = feed(a, "fix something ".chars().map(ch).collect());
    // The check answers for the current sequence (1); an old answer is ignored.
    let ok = Preview {
        workspace: "/home/ivan/project".into(),
        files: 3,
        bytes: 10,
        ..Preview::default()
    };
    let (a, _) = feed(
        a,
        vec![Input::Bg(Bg::Checked {
            seq: 99,
            result: Ok(ok.clone()),
        })],
    );
    assert!(a.new_run.check.is_none(), "stale answer ignored");
    let (a, cmds) = feed(a, vec![key(KeyCode::Enter)]);
    assert!(cmds.is_empty(), "still no valid folder");
    let (a, _) = feed(
        a,
        vec![Input::Bg(Bg::Checked {
            seq: 1,
            result: Ok(ok),
        })],
    );
    let (a, cmds) = feed(a, vec![key(KeyCode::Enter)]);
    assert_eq!(
        cmds,
        vec![Cmd::Start {
            dir: "/home/ivan/project".into(),
            instruction: "fix something".into()
        }]
    );
    assert_eq!(a.screen, Screen::Live);
}

#[test]
fn new_run_rejects_an_invalid_folder_and_edits_the_fields() {
    let (a, _) = feed(
        with_runs(),
        vec![ch('n'), key(KeyCode::Tab), ctrl('u'), ch('/')],
    );
    assert_eq!(a.new_run.dir, "/");
    let seq = a.check_seq;
    let (a, cmds) = feed(
        a,
        vec![Input::Bg(Bg::Checked {
            seq,
            result: Err("refused to use / as a working folder".into()),
        })],
    );
    assert!(cmds.is_empty());
    let text = screen_text(&a, 80, 24);
    assert!(text.contains("refused to use /"), "{text}");
    // Backspace edits the folder and asks again; Alt+Enter adds a line to the instruction.
    let (a, cmds) = feed(a, vec![key(KeyCode::Backspace)]);
    assert_eq!(a.new_run.dir, "");
    assert!(matches!(cmds[0], Cmd::Check { .. }));
    let (a, _) = feed(
        a,
        vec![
            key(KeyCode::Tab),
            ch('a'),
            Input::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)),
            ch('b'),
        ],
    );
    assert_eq!(a.new_run.instruction, "a\nb");
}

#[test]
fn letters_are_text_in_the_new_job_even_the_ones_that_are_shortcuts_elsewhere() {
    let (a, _) = feed(with_runs(), vec![ch('n')]);
    let (a, _) = feed(a, "qcUdau?/jk".chars().map(ch).collect());
    assert_eq!(a.new_run.instruction, "qcUdau?/jk");
    assert_eq!(a.screen, Screen::NewRun);
    assert!(!a.quit);
}

#[test]
fn pasted_text_is_cleaned_and_cannot_press_enter() {
    let (a, _) = feed(app(), vec![ch('n')]);
    let (a, cmds) = feed(
        a,
        vec![Input::Paste(
            "haz esto\u{1b}]0;pwned\u{7}\u{1b}[2J\nluego\rEVIL".into(),
        )],
    );
    assert!(
        cmds.is_empty(),
        "a paste into the instruction launches nothing"
    );
    assert_eq!(a.screen, Screen::NewRun);
    assert_eq!(a.new_run.instruction, "haz esto\nluego\u{FFFD}EVIL");
    let (a, cmds) = feed(
        a,
        vec![key(KeyCode::Tab), Input::Paste("/tmp\nx\u{1b}[31m".into())],
    );
    assert_eq!(a.new_run.dir, "/home/ivan/project/tmpx");
    assert!(matches!(cmds[0], Cmd::Check { .. }));
}

pub(super) fn started() -> Bg {
    Bg::Started {
        id: ID1.into(),
        files: 3,
        workspace: "/home/ivan/project".into(),
        usage: RunUsage::new(128_000, true, LIMITS),
    }
}

#[test]
fn live_run_shows_events_and_cancel_asks_twice() {
    let (a, _) = feed(with_runs(), vec![ch('n'), ch('x')]);
    let a = {
        let mut a = a;
        a.new_run.check = Some(Ok(Preview::default()));
        a
    };
    let (a, cmds) = feed(a, vec![key(KeyCode::Enter)]);
    assert!(matches!(cmds[0], Cmd::Start { .. }));
    let events = vec![
        started(),
        Bg::Agent(Event::Started {
            level: TaskLevel::Simple,
        }),
        Bg::Agent(Event::Step { n: 1 }),
        Bg::Agent(Event::Tool {
            name: "read_file".into(),
            target: "dates.py".into(),
            outcome: Outcome::Allow,
            rule: "step-8-reversible-in-sandbox".into(),
            ran: true,
            result_chars: 1234,
            tainted: true,
        }),
        Bg::Agent(Event::Step { n: 2 }),
        Bg::Agent(Event::Tool {
            name: "write_file".into(),
            target: ".git/hooks/pre-commit".into(),
            outcome: Outcome::Deny,
            rule: "S-16".into(),
            ran: false,
            result_chars: 60,
            tainted: true,
        }),
    ];
    let (a, _) = feed(a, events.into_iter().map(Input::Bg).collect());
    let (a, _) = feed(a, vec![Input::Tick(5_000)]);
    for (w, h) in sizes() {
        let t = screen_text(&a, w, h);
        assert_clean(&t);
        if w >= 80 {
            assert!(
                t.contains("read_file") && t.contains("allowed") && t.contains("denied"),
                "{t}"
            );
            assert!(t.contains("S-16") && t.contains("1234 ch."), "{t}");
            // The use of the run, shortened at 80 columns.
            assert!(
                t.contains("2/40")
                    && (t.contains("5 s/10 min") || t.contains("5s/10m"))
                    && t.contains("ext"),
                "{t}"
            );
            assert!(
                !t.contains("tainted") && !t.contains("read files"),
                "the long warning is gone; `externo` says it"
            );
        }
    }
    let (a, cmds) = feed(a, vec![key(KeyCode::Esc)]);
    assert_eq!(cmds, vec![Cmd::Cancel]);
    assert!(a.live.cancelling && !a.quit);
    assert!(screen_text(&a, 80, 24).contains("cancelling"));
    // The second press leaves right away (the run stays incomplete; the original was never touched).
    let (a, cmds) = feed(a, vec![ctrl('c')]);
    assert!(cmds.is_empty() && a.quit);
}

#[test]
fn ctrl_c_cancels_a_live_run_and_the_second_press_quits() {
    let mut a = app();
    a.screen = Screen::Live;
    let (a, cmds) = feed(a, vec![ctrl('c')]);
    assert_eq!(cmds, vec![Cmd::Cancel]);
    assert!(!a.quit);
    let (a, cmds) = feed(a, vec![ctrl('c')]);
    assert!(cmds.is_empty() && a.quit);
}

#[test]
fn a_finished_live_run_offers_the_detail() {
    let mut a = app();
    a.screen = Screen::Live;
    a.live.id = Some(ID1.into());
    let (a, cmds) = feed(
        a,
        vec![Input::Bg(Bg::Finished(Ok(Box::new(summary(
            ID1,
            "Final",
            &["a.py"],
        )))))],
    );
    assert_eq!(cmds, vec![Cmd::LoadHome]);
    assert!(screen_text(&a, 80, 24).contains("finished"));
    let (_, cmds) = feed(a, vec![key(KeyCode::Enter)]);
    assert_eq!(
        cmds,
        vec![Cmd::LoadDetail {
            id: ID1.into(),
            diff: false
        }]
    );
    // A cancelled run says so.
    let mut a = app();
    a.screen = Screen::Live;
    let (a, _) = feed(
        a,
        vec![Input::Bg(Bg::Finished(Ok(Box::new(summary(
            ID1,
            "Cancelled",
            &[],
        )))))],
    );
    assert!(screen_text(&a, 80, 24).contains("cancelled"));
    let (a, cmds) = feed(a, vec![key(KeyCode::Esc)]);
    assert_eq!((a.screen, cmds), (Screen::Home, vec![Cmd::LoadHome]));
}

#[test]
fn detail_selects_files_scrolls_the_diff_and_goes_back() {
    let a = open_detail(with_runs(), detail(ID1, Status::Pending), false);
    assert_eq!(a.screen, Screen::Detail);
    let (a, _) = feed(a, vec![ch('j'), ch('j')]);
    assert_eq!(a.detail.as_ref().unwrap().sel, 1, "clamped");
    let (a, _) = feed(a, vec![ch('k'), key(KeyCode::Enter)]);
    assert!(a.detail.as_ref().unwrap().in_diff);
    let (a, _) = feed(a, vec![ch('j'), ch('j')]);
    assert_eq!(
        a.detail.as_ref().unwrap().scroll,
        0,
        "a short diff cannot scroll past its end"
    );
    let (a, _) = feed(a, vec![ch(']')]);
    assert_eq!(a.detail.as_ref().unwrap().sel, 1);
    let (a, _) = feed(a, vec![key(KeyCode::Esc)]);
    assert!(!a.detail.as_ref().unwrap().in_diff && a.screen == Screen::Detail);
    // `d` opens the diff of the chosen file too.
    let (a, _) = feed(a, vec![ch('d')]);
    assert!(a.detail.as_ref().unwrap().in_diff);
    let (a, _) = feed(a, vec![ch('q'), key(KeyCode::Esc)]);
    assert_eq!(a.screen, Screen::Home);
}

#[test]
fn detail_diff_scroll_follows_the_viewport() {
    let long: String = (0..60).map(|i| format!("+line {i}\n")).collect();
    let mut d = detail(ID1, Status::Pending);
    d.rows[0].diff = format!("--- a/x\n+++ b/x\n@@ -0,0 +1,60 @@\n{long}");
    let a = open_detail(with_runs(), d, true);
    assert!(a.detail.as_ref().unwrap().in_diff);
    let t = screen_text(&a, 80, 24); // draw sets the viewport height
    assert!(t.contains("line 1/63"), "{t}");
    let (a, _) = feed(a, vec![ch('G')]);
    let t = screen_text(&a, 80, 24);
    assert!(t.contains("+line 59"), "{t}");
    let (a, _) = feed(a, vec![ctrl('b')]);
    assert!(a.detail.as_ref().unwrap().scroll < 63 - 10);
    let (a, _) = feed(a, vec![key(KeyCode::PageDown)]);
    let page_down = a.detail.as_ref().unwrap().scroll;
    let (a, _) = feed(a, vec![key(KeyCode::PageUp)]);
    assert!(a.detail.as_ref().unwrap().scroll < page_down);
    let (a, _) = feed(a, vec![ch('g')]);
    assert_eq!(a.detail.as_ref().unwrap().scroll, 0);
    let (a, _) = feed(a, vec![ctrl('f')]);
    assert!(a.detail.as_ref().unwrap().scroll > 0, "Ctrl-F pages down");
}

#[test]
fn detail_apply_and_undo_ask_first() {
    let a = open_detail(with_runs(), detail(ID1, Status::Pending), false);
    let (_, cmds) = feed(a.clone_for_test(), vec![ch('a')]);
    assert_eq!(cmds, vec![Cmd::PlanApply { id: ID1.into() }]);
    let (b, cmds) = feed(a, vec![ch('u')]);
    assert!(
        cmds.is_empty() && b.notice.is_some(),
        "not applied yet, nothing to undo"
    );
    let a = open_detail(with_runs(), detail(ID2, Status::Applied), false);
    let (a, cmds) = feed(a, vec![ch('u')]);
    assert!(cmds.is_empty());
    let c = a.confirm.clone().unwrap();
    assert_eq!((c.action, c.files), (Action::Undo, 2));
    assert!(screen_text(&a, 80, 24).contains("Undo what was applied (2 file(s))"));
    let (_, cmds) = feed(a, vec![ch('y')]);
    assert_eq!(cmds, vec![Cmd::Undo { id: ID2.into() }]);
}

#[test]
fn after_applying_the_lists_reload() {
    let a = open_detail(with_runs(), detail(ID1, Status::Pending), false);
    let (a, cmds) = feed(a, vec![Input::Bg(Bg::Done(Ok("2 file(s) applied".into())))]);
    assert_eq!(
        cmds,
        vec![
            Cmd::LoadHome,
            Cmd::LoadDetail {
                id: ID1.into(),
                diff: false
            }
        ]
    );
    assert_eq!(a.flash.as_deref(), Some("2 file(s) applied"));
}

#[test]
fn doctor_screen_scrolls_and_repeats() {
    let (a, _) = feed(with_runs(), vec![ch('D')]);
    let (a, _) = feed(a, vec![Input::Bg(Bg::Doctor(report()))]);
    assert_eq!(a.home.doctor, Some((1, 1, 1)));
    let (a, cmds) = feed(a, vec![ch('j'), ch('r')]);
    assert_eq!((a.doctor.scroll, cmds), (1, vec![Cmd::RunDoctor]));
}

// ------------------------------------------------------------ the list filter

#[test]
fn slash_filters_the_list_and_the_selection_follows_the_filter() {
    let (a, _) = feed(with_runs(), vec![ch('/')]);
    assert!(a.home.filtering);
    // In the filter the letters are text, not shortcuts.
    let (a, cmds) = feed(a, "readme".chars().map(ch).collect());
    assert!(cmds.is_empty() && a.home.filter == "readme");
    assert_eq!(a.home.visible(), vec![1]);
    let t = screen_text(&a, 80, 24);
    assert!(t.contains("/readme") && t.contains("1 of 3"), "{t}");
    assert!(
        t.contains("add a README") && !t.contains("fix parse_date en"),
        "{t}"
    );
    // Enter leaves the typing and keeps the filter; the keys are shortcuts again.
    let (a, _) = feed(a, vec![key(KeyCode::Enter)]);
    assert!(!a.home.filtering && a.home.filter == "readme");
    let (a2, cmds) = feed(a.clone_for_test_filter(), vec![key(KeyCode::Enter)]);
    assert_eq!(
        cmds,
        vec![Cmd::LoadDetail {
            id: ID2.into(),
            diff: false
        }],
        "the run opened is the one the filter shows"
    );
    assert_eq!(a2.home.sel, 0);
    // Esc clears it.
    let (a, _) = feed(a, vec![key(KeyCode::Esc)]);
    assert!(a.home.filter.is_empty() && a.home.visible().len() == 3);
}

#[test]
fn the_filter_matches_status_and_id_and_says_when_nothing_matches() {
    let (a, _) = feed(with_runs(), vec![ch('/')]);
    let (a, _) = feed(a, "failed".chars().map(ch).collect());
    assert_eq!(a.home.visible(), vec![2]);
    let (a, _) = feed(a, vec![key(KeyCode::Esc)]);
    assert!(
        !a.home.filtering && a.home.filter.is_empty(),
        "Esc while typing clears and leaves"
    );
    let (a, _) = feed(a, vec![ch('/')]);
    let (a, _) = feed(a, "0b1c".chars().map(ch).collect());
    assert_eq!(a.home.visible(), vec![1]);
    let (a, _) = feed(a, "zzz".chars().map(ch).collect());
    assert!(a.home.visible().is_empty());
    let t = screen_text(&a, 80, 24);
    assert!(t.contains("No run matches") && t.contains("0 of 3"), "{t}");
    let (a, cmds) = feed(a, vec![key(KeyCode::Enter), key(KeyCode::Enter)]);
    assert!(cmds.is_empty() && a.flash.is_some(), "nothing to open");
    // Backspace edits the filter.
    let (a, _) = feed(
        a,
        vec![
            ch('/'),
            key(KeyCode::Backspace),
            key(KeyCode::Backspace),
            key(KeyCode::Backspace),
        ],
    );
    assert_eq!(a.home.filter, "0b1c");
    assert_eq!(a.home.visible(), vec![1]);
}

#[test]
fn a_filter_with_hostile_text_is_drawn_clean() {
    let (a, _) = feed(with_runs(), vec![ch('/')]);
    let (a, _) = feed(a, vec![ch('a'), ch('\u{202E}'), ch('b')]);
    let t = screen_text(&a, 80, 24);
    assert_clean(&t);
}

// ------------------------------------------------------------ drawing

pub(super) fn report() -> Report {
    Report {
        checks: vec![
            Check {
                level: Level::Pass,
                msg: "/home/ivan/.config/clio is private (700)".into(),
                hint: None,
            },
            Check {
                level: Level::Warn,
                msg: "connector opencode: the key is missing from the vault".into(),
                hint: Some("With the vault running: clio connect opencode".into()),
            },
            Check {
                level: Level::Fail,
                msg: "/x/y tiene modo 755, debe ser 700 (S-23)".into(),
                hint: Some("chmod 700 /x/y".into()),
            },
        ],
        host_script: None,
        jail: None,
    }
}

#[test]
fn home_draws_state_and_runs_without_overflow() {
    for (w, h) in sizes() {
        let t = screen_text(&with_runs(), w, h);
        assert_clean(&t);
        assert_fits(&t, w, h);
        if w >= 80 {
            assert!(
                t.contains("clio · home · project") && t.contains("beam-free"),
                "{t}"
            );
            assert!(t.contains("key present"), "{t}");
            assert!(t.contains("doctor: 12 ok · 3 warnings · 0 failures"), "{t}");
            assert!(
                t.contains("1031-1816-5a67") && t.contains("pending review"),
                "{t}"
            );
            assert!(t.contains("applied") && t.contains("failed"), "{t}");
            assert!(t.contains("fix parse_date in dates.py"), "{t}");
            assert!(t.contains("q quit") && t.contains("? help"), "{t}");
            if w >= 120 {
                assert!(t.contains("/ filter") && t.contains("n new"), "{t}");
            }
        }
    }
    let t = screen_text(&app(), 80, 24);
    assert!(t.contains("No runs yet") && t.contains("loading"), "{t}");
}

#[test]
fn home_reports_a_missing_key_and_a_broken_config() {
    let mut a = with_runs();
    a.home.header.as_mut().unwrap().key = KeyState::Missing;
    let t = screen_text(&a, 80, 24);
    assert!(
        t.contains("model key missing: clio connect opencode"),
        "{t}"
    );
    // A config with no connector still says where to start.
    a.home.header.as_mut().unwrap().connector = String::new();
    let t = screen_text(&a, 80, 24);
    assert!(t.contains("model key missing: clio connect"), "{t}");
    assert!(!t.contains("clio connect opencode"), "{t}");
    a.home.header.as_mut().unwrap().config_error =
        Some("config.json: does not exist (run `clio init` first)".into());
    assert!(screen_text(&a, 100, 24).contains("run `clio init` first"));
}

#[test]
fn there_are_no_boxes_except_in_dialogs() {
    let boxes = ['╭', '╮', '╰', '╯', '│', '┌', '┐', '└', '┘', '╔', '═'];
    let (new, _) = feed(with_runs(), vec![ch('n')]);
    let screens = [
        with_runs(),
        new,
        open_detail(with_runs(), detail(ID1, Status::Pending), false),
        open_detail(with_runs(), detail(ID1, Status::Pending), true),
        feed(with_runs(), vec![ch('c')]).0,
        feed(with_runs(), vec![ch('U')]).0,
        feed(with_runs(), vec![ch('?')]).0,
        feed(with_runs(), vec![ch('D')]).0,
    ];
    for a in &screens {
        for (w, h) in SIZES {
            let t = screen_text(a, w, h);
            assert!(
                !t.chars().any(|c| boxes.contains(&c)),
                "{:?}\n{t}",
                a.screen
            );
        }
    }
}

#[test]
fn new_run_draws_the_checks_and_the_exclusions() {
    let (mut a, _) = feed(with_runs(), vec![ch('n')]);
    a.new_run.instruction = "fix something".into();
    a.new_run.check = Some(Ok(Preview {
        workspace: "/home/ivan/project".into(),
        files: 12,
        bytes: 3 * 1024 * 1024,
        secrets: vec![".env".into(), "id_rsa".into()],
        skipped: vec![".git".into(), "node_modules".into()],
        symlinks: vec!["link".into()],
    }));
    for (w, h) in sizes() {
        let t = screen_text(&a, w, h);
        assert_clean(&t);
        if w >= 80 {
            assert!(
                t.contains("Folder") && t.contains("/home/ivan/project"),
                "{t}"
            );
            assert!(
                t.contains("fix something") && t.contains("12 files will be copied (3.0 MB)"),
                "{t}"
            );
            assert!(t.contains("look like secrets: .env, id_rsa"), "{t}");
            assert!(
                t.contains(".git, node_modules") && t.contains("symbolic links: link"),
                "{t}"
            );
        }
    }
    a.new_run.check = Some(Err("refused to use your home folder".into()));
    assert!(screen_text(&a, 80, 24).contains("refused to use your home folder"));
}

#[test]
fn detail_draws_summary_message_files_and_a_colored_diff() {
    let a = open_detail(with_runs(), detail(ID1, Status::Pending), false);
    for (w, h) in sizes() {
        let t = screen_text(&a, w, h);
        assert_clean(&t);
        if w >= 80 {
            assert!(
                t.contains("pending review") && t.contains("fix parse_date"),
                "{t}"
            );
            assert!(
                t.contains("Fixed parse_date.") && t.contains("Could not run the tests."),
                "{t}"
            );
            assert!(
                t.contains("changed files (2)")
                    && t.contains("M dates.py")
                    && t.contains("A docs/new.md"),
                "{t}"
            );
        }
    }
    let a = open_detail(with_runs(), detail(ID1, Status::Pending), true);
    for (w, h) in sizes() {
        assert_clean(&screen_text(&a, w, h));
    }
    let t = screen_text(&a, 80, 24);
    assert!(
        t.contains("-    return int(s)")
            && t.contains("+    return None")
            && t.contains("@@ -1,3 +1,3 @@"),
        "{t}"
    );
    // Colors: + green, - red; the hunk header is dim (cyan is only for the selection).
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    term.draw(|f| draw(f, &a)).unwrap();
    let buf = term.backend().buffer();
    let mut seen = std::collections::BTreeMap::new();
    for y in 0..24 {
        let line: String = (0..80).map(|x| buf[(x, y)].symbol()).collect();
        for (tag, color) in [
            ("+    return None", ratatui::style::Color::Green),
            ("-    return int(s)", ratatui::style::Color::Red),
        ] {
            if let Some(x) = line.find(tag) {
                let col = line[..x].chars().count() as u16;
                seen.insert(tag, buf[(col, y)].fg == color);
            }
        }
        if let Some(x) = line.find("@@ -1,3") {
            let col = line[..x].chars().count() as u16;
            seen.insert(
                "hunk",
                buf[(col, y)]
                    .modifier
                    .contains(ratatui::style::Modifier::DIM)
                    && buf[(col, y)].fg != ratatui::style::Color::Cyan,
            );
        }
    }
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert!(seen.values().all(|ok| *ok), "{seen:?}");
}

#[test]
fn doctor_draws_marks_and_one_hint_per_warning() {
    let mut a = with_runs();
    a.screen = Screen::Doctor;
    a.doctor = DoctorView::default();
    a.doctor.set(report());
    for (w, h) in sizes() {
        let t = screen_text(&a, w, h);
        assert_clean(&t);
        if w >= 80 {
            assert!(
                t.contains("✓  /home")
                    && t.contains("!  connector opencode: the key")
                    && t.contains("✗  /x/y"),
                "{t}"
            );
            assert!(
                t.contains("→ With the vault running: clio connect opencode")
                    && t.contains("→ chmod 700 /x/y"),
                "{t}"
            );
            assert!(t.contains("doctor: 1 ok · 1 warning · 1 failure"), "{t}");
        }
    }
}

#[test]
fn help_lists_this_screens_keys_in_one_short_view() {
    let (mut a, _) = feed(with_runs(), vec![ch('?')]);
    let t = screen_text(&a, 80, 24);
    assert_fits(&t, 80, 24);
    for want in [
        "keys",
        "Enter",
        "open",
        "filter",
        "context",
        "usage",
        "refresh",
        "doctor",
        "quit",
        "Ctrl-D",
        "debug mode",
        "Ctrl-L",
        "redraw",
        "apply",
        "undo",
    ] {
        assert!(t.contains(want), "{want}\n{t}");
    }
    // Another screen has another list.
    a.screen = Screen::Help;
    a.before_help = Screen::Live;
    let t = screen_text(&a, 80, 24);
    assert!(
        t.contains("cancel") && t.contains("timeline / debug"),
        "{t}"
    );
    assert!(!t.contains("filter"), "{t}");
    // At the minimum size every key is still there (two columns), and a long list would scroll.
    let t = screen_text(&a, 60, 16);
    assert_fits(&t, 60, 16);
    for want in ["Ctrl-C", "Tab", "Ctrl-B", "Ctrl-D", "Ctrl-L", "redraw"] {
        assert!(t.contains(want), "{want}\n{t}");
    }
    a.before_help = Screen::Home;
    let t = screen_text(&a, 60, 16);
    assert_fits(&t, 60, 16);
    let (b, _) = feed(a, vec![ch('G')]);
    assert_fits(&screen_text(&b, 60, 16), 60, 16);
}

#[test]
fn tiny_terminals_get_a_message_and_nothing_panics() {
    let mut a = with_runs();
    for (w, h) in [(59, 16), (60, 15), (30, 8), (1, 1)] {
        let t = screen_text(&a, w, h);
        if w > 10 {
            assert!(t.contains("Terminal too small") || h < 4, "{w}x{h}\n{t}");
        }
    }
    let t = screen_text(&a, 59, 16);
    assert!(t.contains("60x16"), "{t}");
    // No size may panic, whatever the screen.
    for s in [
        Screen::Home,
        Screen::NewRun,
        Screen::Live,
        Screen::Doctor,
        Screen::Help,
        Screen::Context,
        Screen::Usage,
    ] {
        a.screen = s;
        for (w, h) in [
            (1, 1),
            (10, 3),
            (40, 10),
            (60, 12),
            (120, 8),
            (60, 16),
            (61, 17),
        ] {
            screen_text(&a, w, h);
        }
    }
    a = open_detail(a, detail(ID1, Status::Pending), true);
    for (w, h) in [(40, 10), (60, 12), (100, 30), (60, 16)] {
        screen_text(&a, w, h);
    }
}

#[test]
fn without_color_the_theme_uses_no_colors_and_no_unicode_has_ascii_stand_ins() {
    let t = Theme {
        color: false,
        unicode: false,
        rgb: false,
    };
    for s in [
        t.ok(),
        t.warn(),
        t.fail(),
        t.add(),
        t.del(),
        t.hunk(),
        t.accent(),
        t.key(),
        t.selected(),
        t.level(95),
    ] {
        assert!(s.fg.is_none(), "{s:?}");
    }
    for sym in [
        t.dot(),
        t.sel(),
        t.check(),
        t.bang(),
        t.cross(),
        t.up(),
        t.down(),
        t.rule(),
        t.ellipsis(),
        t.arrow(),
        t.cursor(),
        t.rule_v(),
        t.spinner(0),
        &t.bar(50, 10),
    ] {
        assert!(sym.is_ascii(), "{sym:?}");
    }
    let c = Theme {
        color: true,
        unicode: true,
        rgb: false,
    };
    assert_eq!(c.ok().fg, Some(ratatui::style::Color::Green));
    // Only the basic palette: nothing indexed or RGB, so the terminal's theme decides.
    for s in [
        c.ok(),
        c.warn(),
        c.fail(),
        c.add(),
        c.del(),
        c.hunk(),
        c.accent(),
        c.key(),
        c.selected(),
    ] {
        assert!(!matches!(
            s.fg,
            Some(ratatui::style::Color::Rgb(..) | ratatui::style::Color::Indexed(_))
        ));
    }
    // One accent: only the selection and the focus are cyan.
    assert_eq!(c.accent().fg, Some(ratatui::style::Color::Cyan));
    assert_eq!(c.selected().fg, Some(ratatui::style::Color::Cyan));
    assert_eq!(c.key().fg, None);
    assert_eq!(c.hunk().fg, None);
}

#[test]
fn every_screen_is_plain_ascii_without_unicode() {
    let ascii = Theme {
        color: false,
        unicode: false,
        rgb: false,
    };
    let mut screens = vec![with_runs(), super::tests_views::live_app()];
    screens.push(open_detail(
        with_runs(),
        detail(ID1, Status::Pending),
        false,
    ));
    screens.push(feed(with_runs(), vec![ch('c')]).0);
    screens.push(feed(with_runs(), vec![ch('U')]).0);
    for mut a in screens {
        a.theme = ascii;
        for (w, h) in [(80, 24), (120, 30)] {
            let t = screen_text(&a, w, h);
            // The names of runs and files are Spanish text; the interface's own symbols are ASCII.
            let odd: Vec<char> = t
                .chars()
                .filter(|c| !c.is_ascii() && !"áéíóúñÁÉÍÓÚÑü¿¡…·".contains(*c))
                .collect();
            assert!(odd.is_empty(), "{:?} {odd:?}\n{t}", a.screen);
        }
    }
}

// ------------------------------------------------------------ hostile text

pub(super) const HOSTILE: &[&str] = &[
    "evil\u{1b}[2Jname.txt",
    "title\u{1b}]0;pwned-title\u{7}.txt",
    "ok\rOVERWRITTEN.txt",
    "report\u{202E}txt.exe",
    "link\u{1b}]8;;http://evil.example\u{1b}\\click\u{1b}]8;;\u{1b}\\.md",
    "bell\u{7}\u{8}\u{0}.rs",
];

fn hostile_detail() -> Detail {
    let mut d = detail(ID1, Status::Pending);
    d.rows = HOSTILE
        .iter()
        .map(|n| {
            row(
                'M',
                n,
                &format!(
                    "--- a/{n}\n+++ b/{n}\n@@ -1,1 +1,1 @@\n-viejo\u{1b}[2J\n+new\u{1b}]0;pwned-title\u{7}\r+FAKE\n \u{202E}reversed\n"
                ),
            )
        })
        .collect();
    d.info.summary.as_mut().unwrap().final_message =
        Some("Listo.\u{1b}[2J\u{1b}[HAll files verified\u{1b}]0;pwned-title\u{7}\rFAKE-OK \u{202E}gnp.exe".into());
    d.info.instruction = "haz\u{1b}[2J algo\u{202E}".into();
    d
}

#[test]
fn hostile_file_names_diffs_and_messages_never_reach_the_screen_raw() {
    let mut d = hostile_detail();
    for diff_mode in [false, true] {
        for sel in 0..d.rows.len() {
            d.sel = sel;
            d.in_diff = diff_mode;
            let mut a = open_detail(
                with_runs(),
                Detail {
                    info: d.info.clone(),
                    rows: d
                        .rows
                        .iter()
                        .map(|r| row(r.kind, &r.rel, &r.diff))
                        .collect(),
                    sel,
                    in_diff: diff_mode,
                    scroll: 0,
                    usage: RunUsage::default(),
                    view_h: Cell::new(10),
                    mem_sel: 0,
                },
                diff_mode,
            );
            for (w, h) in sizes() {
                let t = screen_text(&a, w, h);
                assert_clean(&t);
                assert!(
                    !t.contains("pwned-title"),
                    "the OSC payload must be gone\n{t}"
                );
                assert!(!t.contains("http://evil.example"), "{t}");
            }
            a.screen = Screen::Detail;
        }
    }
    // The visible parts are still there, with a mark where the control characters were.
    let a = open_detail(with_runs(), hostile_detail(), false);
    let t = screen_text(&a, 120, 40);
    assert!(t.contains("evilname.txt"), "{t}");
    assert!(
        t.contains("ok\u{FFFD}OVERWRITTEN.txt"),
        "\\r became a visible mark\n{t}"
    );
    assert!(
        t.contains("report\u{FFFD}txt.exe"),
        "U+202E became a visible mark\n{t}"
    );
    assert!(t.contains("Listo.All files verified\u{FFFD}FAKE-OK"), "{t}");
}

#[test]
fn hostile_instruction_and_live_timeline_are_cleaned() {
    let mut a = with_runs();
    a.home.runs[0].instruction = "haz\u{1b}[2J esto\u{1b}]0;pwned-title\u{7}\r\u{202E}".into();
    let t = screen_text(&a, 120, 30);
    assert_clean(&t);
    assert!(t.contains("haz esto") && !t.contains("pwned-title"), "{t}");
    let mut a = app();
    a.screen = Screen::Live;
    a.live.instruction = "mira\u{1b}[2J".into();
    for name in HOSTILE {
        a.live.on_event(&Event::Tool {
            name: "read_file".into(),
            target: (*name).into(),
            outcome: Outcome::Allow,
            rule: "r\u{1b}[2J".into(),
            ran: true,
            result_chars: 1,
            tainted: true,
        });
    }
    a.live.on_event(&Event::ProtocolError {
        reason: "no\u{1b}]0;pwned-title\u{7} json\u{202E}".into(),
    });
    for (w, h) in sizes() {
        let t = screen_text(&a, w, h);
        assert_clean(&t);
        assert!(!t.contains("pwned-title"), "{t}");
    }
}

#[test]
fn hostile_doctor_and_notice_text_is_cleaned() {
    let mut a = with_runs();
    a.screen = Screen::Doctor;
    a.doctor.set(Report {
        checks: vec![Check {
            level: Level::Warn,
            msg: "x\u{1b}[2J y\u{202E}".into(),
            hint: Some("h\u{1b}]0;pwned-title\u{7}".into()),
        }],
        host_script: None,
        jail: None,
    });
    a.notice =
        Some("rechazado:\n  \u{1b}[2J.git/x\u{1b}]0;pwned-title\u{7}: ruta protegida\rFAKE".into());
    let t = screen_text(&a, 100, 24);
    assert_clean(&t);
    assert!(
        !t.contains("pwned-title") && t.contains("ruta protegida"),
        "{t}"
    );
}

// ------------------------------------------------------------ terminal restoration

#[test]
fn restoring_the_terminal_twice_is_harmless() {
    // Without a terminal the calls fail quietly; the point is that they never panic.
    super::restore_terminal();
    super::restore_terminal();
}

impl App {
    /// A second copy of the pieces a test needs (the app is not `Clone`: it holds a channel-free
    /// but large state, and cloning it for real use is never wanted).
    pub(super) fn clone_for_test(&self) -> App {
        let mut b = App::new("/home/ivan/project", self.theme);
        b.screen = self.screen;
        b.home.runs = self.home.runs.clone();
        b.home.sel = self.home.sel;
        b.confirm = self.confirm.clone();
        b.detail = self.detail.as_ref().map(|d| Detail {
            info: d.info.clone(),
            rows: d
                .rows
                .iter()
                .map(|r| row(r.kind, &r.rel, &r.diff))
                .collect(),
            sel: d.sel,
            in_diff: d.in_diff,
            scroll: d.scroll,
            usage: d.usage.clone(),
            view_h: Cell::new(d.view_h.get()),
            mem_sel: 0,
        });
        b
    }

    /// The same, keeping the list filter.
    fn clone_for_test_filter(&self) -> App {
        let mut b = self.clone_for_test();
        b.home.filter.clone_from(&self.home.filter);
        b
    }
}

// Second audit: run folders are listed by name and a name can be anything. One whose `-` sit where
// a run id has them but with a multi-byte character before them must not take the screen down.
#[test]
fn short_id_never_panics_on_a_non_ascii_name() {
    use super::home::short_id;
    // `-` at bytes 8 and 15, and `é` straddling the cut at byte 4.
    let odd = "abcéfgh-123456-xyz01";
    assert_eq!(odd.as_bytes()[8], b'-');
    assert_eq!(odd.as_bytes()[15], b'-');
    assert!(!short_id(odd).is_empty());
    // A real id is still shortened as before.
    assert_eq!(short_id("20261031-181646-5a67"), "1031-1816-5a67");
}

// Host mode: at the end of a turn, each action the session set aside is one question; `s` and then
// Enter runs it, `n` or Esc rejects it, and the next one comes. The question opens while the
// person may be typing: it hears no key until a second passed without one, and a lone Enter or an
// `s` followed by another key decides nothing.
#[test]
fn what_a_turn_set_aside_is_asked_one_by_one_and_only_s_then_enter_runs_it() {
    let asks = vec![
        ("a1".to_owned(), "«delete» rm -r datos".to_owned(), None),
        ("b2".to_owned(), "«send» git push".to_owned(), None),
    ];
    let (a, cmds) = feed(app(), vec![Input::Bg(Bg::ChatAsks(asks))]);
    assert!(cmds.is_empty());
    let c = a.confirm.as_ref().unwrap();
    assert_eq!((c.action, c.id.as_str()), (Action::Approve, "a1"));
    let screen = screen_text(&a, 90, 30);
    assert!(
        screen.contains("Run it?")
            && screen.contains("y then Enter")
            && screen.contains("n or Esc")
            && screen.contains("rm -r datos"),
        "{screen}"
    );
    // Its own title, and the class on a line of its own above the command.
    assert!(
        screen.contains("Approval") && !screen.contains("Confirm"),
        "{screen}"
    );
    let class = screen.lines().find(|l| l.contains("«delete»")).unwrap();
    assert!(!class.contains("rm -r"), "{screen}");
    // Typing on: nothing is answered, and an `s` followed by another key is undone.
    let mut typing = Vec::new();
    for (i, k) in [
        ch('h'),
        key(KeyCode::Enter),
        ch('s'),
        ch('o'),
        key(KeyCode::Enter),
        ch('s'),
        ch('s'),
    ]
    .into_iter()
    .enumerate()
    {
        typing.extend([Input::Tick(100 * (i as u64 + 1)), k]);
    }
    let (a, cmds) = feed(a, typing);
    assert!(cmds.is_empty(), "{cmds:?}");
    assert_eq!(a.confirm.as_ref().unwrap().id, "a1");
    assert!(!a.confirm.as_ref().unwrap().armed);
    let (a, cmds) = feed(
        a,
        vec![
            Input::Tick(2_000),
            ch('s'),
            Input::Tick(3_000),
            key(KeyCode::Enter),
        ],
    );
    assert_eq!(
        cmds,
        vec![Cmd::ChatAnswer {
            id: "a1".into(),
            yes: true
        }]
    );
    assert_eq!(a.confirm.as_ref().unwrap().id, "b2");
    let (a, cmds) = feed(a, vec![Input::Tick(4_000), key(KeyCode::Esc)]);
    assert_eq!(
        cmds,
        vec![Cmd::ChatAnswer {
            id: "b2".into(),
            yes: false
        }]
    );
    assert!(a.confirm.is_none());
    assert_eq!(a.answering, 2);
    let (a, _) = feed(
        a,
        vec![Input::Bg(Bg::ChatAnswered(Ok(
            "approved a1: rm -r datos".into()
        )))],
    );
    assert!(matches!(
        a.chat.items.last(),
        Some(super::chat::Item::Note(m)) if m.contains("approved a1")
    ));
    assert_eq!(a.answering, 1);
}

/// The Pi screen in host mode with a session open (`sid`) and two more listed.
fn host_chat() -> App {
    use crate::cli::chat::{Row, RowKind};
    let mut a = app();
    a.use_pi();
    a.host = true;
    a.chat = super::chat::Chat::opening(1, 0);
    a.chat.opened("sid".into(), String::new(), &[], None, 0);
    let row = |title: &str, reference: &str| Row {
        kind: RowKind::Session,
        title: title.into(),
        detail: String::new(),
        messages: 2,
        last_ms: None,
        reference: reference.into(),
        unavailable: None,
    };
    a.sessions.set(vec![
        row("abierta", "/s/1_sid.jsonl"),
        row("otra", "/s/2_otra.jsonl"),
    ]);
    a
}

fn typed_at(t0: u64, text: &str) -> Vec<Input> {
    text.chars()
        .enumerate()
        .flat_map(|(i, c)| [Input::Tick(t0 + 50 * i as u64), ch(c)])
        .collect()
}

// Review of the sessions bar: the question opened while the person typed «muéstrame los
// permisos⏎», and the tail `s` then Enter ran the action. Now the question hears nothing until a
// second passed with no key, counting the keys typed before it opened, and says so.
#[test]
fn the_tail_of_what_was_typed_never_answers_a_set_aside_question() {
    let mut inputs = typed_at(1_000, "muéstrame los permi");
    inputs.push(Input::Bg(Bg::ChatAsks(vec![(
        "a1".into(),
        "«delete» rm -r x".into(),
        None,
    )])));
    inputs.extend(typed_at(2_000, "sos"));
    inputs.extend([Input::Tick(2_150), key(KeyCode::Enter)]);
    let (a, cmds) = feed(host_chat(), inputs);
    assert!(cmds.is_empty(), "{cmds:?}");
    assert_eq!(a.chat.input, "muéstrame los permi");
    let c = a.confirm.as_ref().unwrap();
    assert!(c.id == "a1" && !c.armed);
    assert!(screen_text(&a, 100, 30).contains("wait a second"));
    let mut help = host_chat();
    help.open_help();
    assert!(screen_text(&help, 140, 60).contains("a second without typing"));
    // `n` and Esc too: no answer while the keys still come, then `n` rejects.
    let (a, cmds) = feed(a, vec![Input::Tick(2_900), ch('n')]);
    assert!(cmds.is_empty() && a.confirm.is_some(), "{cmds:?}");
    let (a, _) = feed(a, vec![Input::Tick(3_899)]);
    assert!(screen_text(&a, 100, 30).contains("wait a second"));
    let (a, _) = feed(a, vec![Input::Tick(3_900)]);
    assert!(!screen_text(&a, 100, 30).contains("wait a second"));
    // `s` arms once the second passed, and Enter right after it runs it: no second wait.
    let armed_at = |a: App| {
        let (a, cmds) = feed(a, vec![ch('s')]);
        assert!(
            cmds.is_empty() && a.confirm.as_ref().unwrap().armed,
            "{cmds:?}"
        );
        a
    };
    let a = armed_at(a);
    let armed_screen = screen_text(&a, 100, 30);
    assert!(
        !armed_screen.contains("wait a second") && armed_screen.contains("Enter yes"),
        "{armed_screen}"
    );
    // Any other key while armed disarms it, and the quiet second starts again: an Enter or an
    // `s` right after decides nothing.
    let (a, cmds) = feed(
        a,
        vec![
            Input::Tick(4_000),
            ch('x'),
            Input::Tick(4_100),
            key(KeyCode::Enter),
            Input::Tick(4_200),
            ch('s'),
            Input::Tick(4_300),
            key(KeyCode::Enter),
        ],
    );
    assert!(
        cmds.is_empty() && !a.confirm.as_ref().unwrap().armed,
        "{cmds:?}"
    );
    let (a, _) = feed(a, vec![Input::Tick(5_300)]);
    let (_, cmds) = feed(armed_at(a), vec![key(KeyCode::Enter)]);
    assert_eq!(
        cmds,
        vec![Cmd::ChatAnswer {
            id: "a1".into(),
            yes: true
        }]
    );
}

// Review: the quiet second counted only from the last key, so a question that opened during a
// pause took `s⏎` typed as a chat message before the person ever saw it. It also waits a second
// from when it came on screen; a queued one, from when it is shown, not when it was queued.
#[test]
fn a_question_that_opens_in_a_pause_waits_its_own_quiet_second() {
    let ask = |id: &str| {
        Input::Bg(Bg::ChatAsks(vec![(
            id.into(),
            "«delete» rm -r x".into(),
            None,
        )]))
    };
    let mut inputs = typed_at(1_000, "x");
    inputs.extend([Input::Tick(3_500), ask("a1")]);
    inputs.extend([
        Input::Tick(3_501),
        ch('s'),
        Input::Tick(3_502),
        key(KeyCode::Enter),
    ]);
    let (a, cmds) = feed(host_chat(), inputs);
    assert!(cmds.is_empty(), "{cmds:?}");
    assert!(!a.confirm.as_ref().unwrap().armed);
    assert!(screen_text(&a, 100, 30).contains("wait a second"));
    let (_, cmds) = feed(a, vec![Input::Tick(4_502), ch('s'), key(KeyCode::Enter)]);
    assert_eq!(
        cmds,
        vec![Cmd::ChatAnswer {
            id: "a1".into(),
            yes: true
        }]
    );

    // Queued behind another dialog at 1 000, shown at 5 000 when that one closes.
    let mut a = host_chat();
    a.screen = Screen::Sessions;
    a.sessions.sel = 2;
    let (a, _) = feed(
        a,
        vec![
            Input::Tick(1_000),
            ch('x'),
            Input::Bg(Bg::SessionPlan(Ok((
                "/s/2_otra.jsonl".into(),
                "otra".into(),
                "the conversation only".into(),
            )))),
            ask("b2"),
            Input::Tick(5_000),
            ch('n'),
        ],
    );
    let c = a.confirm.as_ref().unwrap();
    assert_eq!((c.id.as_str(), c.shown_ms), ("b2", 5_000));
    let (a, _) = feed(a, vec![Input::Tick(5_600), ch('s')]);
    assert!(!a.confirm.as_ref().unwrap().armed);
    let (_, cmds) = feed(a, vec![Input::Tick(6_700), ch('s'), key(KeyCode::Enter)]);
    assert_eq!(
        cmds,
        vec![Cmd::ChatAnswer {
            id: "b2".into(),
            yes: true
        }]
    );
}

// Review of the sessions bar: with a turn running, Tab and `x` in the list asked to delete another
// session, its question replaced the one of a set-aside action, and `s` deleted the session.
#[test]
fn a_delete_never_comes_over_a_set_aside_question_nor_while_the_chat_is_busy() {
    let plan = || {
        Input::Bg(Bg::SessionPlan(Ok((
            "/s/2_otra.jsonl".into(),
            "otra".into(),
            "the conversation only".into(),
        ))))
    };
    // No delete while a turn runs, a session opens or an approved action runs.
    for busy in 0..3 {
        let mut a = host_chat();
        match busy {
            0 => a.chat.running = true,
            1 => a.chat = super::chat::Chat::opening(2, 0),
            _ => a.answering = 1,
        }
        a.screen = Screen::Sessions;
        a.sessions.sel = 2;
        let (a, cmds) = feed(a, vec![ch('x')]);
        assert!(cmds.is_empty() && a.flash.is_some(), "{busy}: {cmds:?}");
        // A plan asked before it began is dropped when it comes.
        let (a, _) = feed(a, vec![plan()]);
        assert!(a.confirm.is_none(), "{busy}");
    }
    // A plan that comes late never replaces the question on screen.
    let mut a = host_chat();
    a.screen = Screen::Sessions;
    let (a, _) = feed(
        a,
        vec![Input::Bg(Bg::ChatAsks(vec![(
            "a1".into(),
            "«delete» rm -r x".into(),
            None,
        )]))],
    );
    let (a, _) = feed(a, vec![plan()]);
    assert_eq!(a.confirm.as_ref().unwrap().action, Action::Approve);
    assert!(a.flash.is_some());
    let (_, cmds) = feed(a, vec![ch('s'), Input::Tick(5_000), ch('y')]);
    assert!(
        !cmds.iter().any(|c| matches!(c, Cmd::DeleteSession { .. })),
        "{cmds:?}"
    );
}

// Review of the sessions bar: a set-aside question that came while another dialog was open waited
// forever. It shows when that dialog closes; one of a session no longer open says where it waits.
#[test]
fn a_question_queued_behind_another_dialog_shows_when_it_closes() {
    let mut a = host_chat();
    a.screen = Screen::Sessions;
    a.sessions.sel = 2;
    let (a, _) = feed(
        a,
        vec![
            ch('x'),
            Input::Bg(Bg::SessionPlan(Ok((
                "/s/2_otra.jsonl".into(),
                "otra".into(),
                "the conversation only".into(),
            )))),
            Input::Bg(Bg::ChatAsks(vec![(
                "a1".into(),
                "«delete» rm -r x".into(),
                None,
            )])),
        ],
    );
    assert_eq!(a.confirm.as_ref().unwrap().action, Action::DeleteSession);
    let (a, cmds) = feed(a, vec![Input::Tick(5_000), ch('n')]);
    assert!(cmds.is_empty());
    let c = a.confirm.as_ref().unwrap();
    assert_eq!((c.action, c.id.as_str()), (Action::Approve, "a1"));
    // A question stamped with another session is not answered here.
    let mut a = host_chat();
    let mut c = super::app::Confirm::new(Action::Approve, "b2".into(), 0, "«send» x".into());
    c.session = "otra".into();
    a.asks.push_back(c);
    a.confirm = Some(super::app::Confirm::new(
        Action::Promote,
        "7".into(),
        0,
        "x".into(),
    ));
    let (a, _) = feed(a, vec![Input::Tick(5_000), ch('n')]);
    assert!(a.confirm.is_none() && a.asks.is_empty());
    assert!(
        a.flash.as_deref().unwrap().contains("clio pending"),
        "{:?}",
        a.flash
    );
}

// Review of the sessions bar: an approved action runs on a worker; quitting killed it halfway and
// switching showed its result in another session. While it runs the chat counts as busy.
#[test]
fn while_an_approved_action_runs_the_chat_cannot_be_left() {
    let mut a = host_chat();
    a.answering = 1;
    let (a, _) = feed(a, vec![ctrl('c')]);
    assert!(!a.quit && a.flash.as_deref().unwrap().contains("approved"));
    let mut a = App {
        screen: Screen::Sessions,
        ..a
    };
    a.sessions.sel = 2;
    for k in [key(KeyCode::Enter), ch('n'), ch('q')] {
        let (next, cmds) = feed(a, vec![k]);
        a = next;
        assert!(cmds.is_empty() && !a.quit, "{cmds:?}");
        assert!(a.flash.as_deref().unwrap().contains("approved"));
        assert_eq!(a.chat.session_id, "sid");
    }
    let (a, _) = feed(
        a,
        vec![Input::Bg(Bg::ChatAnswered(Ok("approved a1".into())))],
    );
    let (a, cmds) = feed(a, vec![key(KeyCode::Enter)]);
    assert!(matches!(&cmds[..], [Cmd::ChatOpen { .. }]), "{cmds:?}");
    assert!(!a.chat.session_id.contains("sid"));
}

// Review of the sessions bar: the first message of a new session was lost by switching, `n` or
// quitting while it opened. Now those wait, and if it cannot open the message comes back in front
// of whatever was typed meanwhile.
#[test]
fn the_first_message_survives_a_session_that_is_still_opening() {
    let mut a = app();
    a.use_pi();
    a.sessions.set(host_chat().sessions.rows);
    let mut inputs = typed_at(0, "hola");
    inputs.push(key(KeyCode::Enter));
    let (a, cmds) = feed(a, inputs);
    let seq = match &cmds[..] {
        [Cmd::ChatOpen { seq, .. }] => *seq,
        _ => panic!("{cmds:?}"),
    };
    let (a, _) = feed(a, vec![ctrl('c')]);
    assert!(!a.quit && a.flash.as_deref().unwrap().contains("opening"));
    let mut a = App {
        screen: Screen::Sessions,
        ..a
    };
    a.sessions.sel = 2;
    for k in [key(KeyCode::Enter), ch('n'), ch('q')] {
        let (next, cmds) = feed(a, vec![k]);
        a = next;
        assert!(cmds.is_empty() && !a.quit, "{cmds:?}");
        assert_eq!(a.chat.queued.as_deref(), Some("hola"));
    }
    let (a, _) = feed(a, vec![key(KeyCode::Esc)]);
    let (a, _) = feed(a, typed_at(1_000, "y además"));
    let (a, _) = feed(
        a,
        vec![Input::Bg(Bg::ChatOpened {
            seq,
            result: Err("sin llave".into()),
        })],
    );
    assert_eq!(a.chat.input, "hola\ny además");
    assert!(a.chat.is_draft());
}

// Review of the sessions bar: the terminal reports every move of the pointer, and each one drew the
// whole screen. A move, a drag or a release is not an input; a click and the wheel are.
#[test]
fn pointer_moves_are_not_inputs() {
    use crossterm::event::{Event, MouseButton, MouseEvent, MouseEventKind as M};
    let m = |kind| {
        Event::Mouse(MouseEvent {
            kind,
            column: 3,
            row: 4,
            modifiers: KeyModifiers::NONE,
        })
    };
    for k in [
        M::Moved,
        M::Drag(MouseButton::Left),
        M::Up(MouseButton::Left),
    ] {
        assert!(super::input_of(m(k)).is_none(), "{k:?}");
    }
    for k in [M::Down(MouseButton::Left), M::ScrollDown, M::ScrollUp] {
        assert!(super::input_of(m(k)).is_some(), "{k:?}");
    }
}

// The sessions drawer shows ages that go stale: it is redrawn at least every 30 s while open;
// the chat waits for something to come in.
#[test]
fn the_sessions_drawer_is_redrawn_while_it_is_open() {
    let mut a = host_chat();
    a.screen = Screen::Sessions;
    assert!(
        a.stale_after()
            .is_some_and(|t| t <= std::time::Duration::from_secs(30))
    );
    a.screen = Screen::Chat;
    assert_eq!(a.stale_after(), None);
}

// The question shows `y`; `s` (the Spanish sí) is still accepted, silently. Same timing for both.
#[test]
fn y_and_the_old_s_both_arm_the_question_after_its_quiet_second() {
    for yes in ['y', 's'] {
        let mut inputs = typed_at(1_000, "x");
        inputs.extend([
            Input::Tick(3_500),
            Input::Bg(Bg::ChatAsks(vec![(
                "a1".into(),
                "«delete» rm -r x".into(),
                None,
            )])),
            Input::Tick(3_501),
            ch(yes),
            Input::Tick(3_502),
            key(KeyCode::Enter),
        ]);
        let (a, cmds) = feed(host_chat(), inputs);
        assert!(
            cmds.is_empty() && !a.confirm.as_ref().unwrap().armed,
            "{yes}"
        );
        let screen = screen_text(&a, 100, 30);
        assert!(screen.contains("y then Enter") && !screen.contains("s y Enter"));
        let (a, cmds) = feed(a, vec![Input::Tick(4_502), ch(yes)]);
        assert!(
            cmds.is_empty() && a.confirm.as_ref().unwrap().armed,
            "{yes}"
        );
        let (_, cmds) = feed(a, vec![key(KeyCode::Enter)]);
        assert_eq!(
            cmds,
            vec![Cmd::ChatAnswer {
                id: "a1".into(),
                yes: true
            }],
            "{yes}"
        );
    }
}
