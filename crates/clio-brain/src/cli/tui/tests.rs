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
};

pub(super) fn app() -> App {
    App::new("/home/ivan/proyecto", THEME)
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
        workspace: "/home/ivan/proyecto".into(),
        instruction: "arregla parse_date".into(),
        ending: ending.into(),
        level: "Simple".into(),
        steps: 7,
        elapsed_ms: 12_300,
        input_tokens: 12_400,
        output_tokens: 3_100,
        modified: files.iter().map(|s| (*s).to_owned()).collect(),
        final_message: Some("Arreglé parse_date.\nNo pude correr las pruebas.".into()),
        ..RunSummary::default()
    }
}

pub(super) fn info(id: &str, status: Status, instr: &str, files: &[&str]) -> RunInfo {
    RunInfo {
        id: id.into(),
        status,
        instruction: instr.into(),
        workspace: "/home/ivan/proyecto".into(),
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
        key: KeyState::Present,
        vault: "sin crear (opcional)".into(),
        config_error: None,
        limits: LIMITS,
        window: 128_000,
        window_known: true,
    });
    a.home.doctor = Some((12, 3, 0));
    a.home.last_usage = super::tests_views::usage_with_context();
    a.home.runs = vec![
        info(
            ID1,
            Status::Pending,
            "arregla parse_date en dates.py",
            &["dates.py"],
        ),
        info(
            ID2,
            Status::Applied,
            "agrega un README",
            &["README.md", "docs/a.md"],
        ),
        info(ID3, Status::Failed, "refactoriza todo", &[]),
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
        info: info(
            id,
            status,
            "arregla parse_date",
            &["dates.py", "docs/new.md"],
        ),
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
            dir: "/home/ivan/proyecto".into(),
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
        assert!(
            b.debug
                && b.flash
                    .as_deref()
                    .is_some_and(|f| f.contains("debug activado"))
        );
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
        vec![Input::Bg(Bg::Planned(Ok(Planned {
            id: ID1.into(),
            files: 2,
            workspace: "/home/ivan/proyecto".into(),
        })))],
    );
    let c = a.confirm.clone().unwrap();
    assert_eq!((c.action, c.files), (Action::Apply, 2));
    let text = screen_text(&a, 80, 24);
    assert!(
        text.contains("Aplicar 2 archivo(s) en") && text.contains("/home/ivan/proyecto"),
        "{text}"
    );
    // Anything but y cancels and nothing is applied.
    let (b, cmds) = feed(a.clone_for_test(), vec![key(KeyCode::Enter)]);
    assert!(cmds.is_empty() && b.confirm.is_none());
    let (_, cmds) = feed(a, vec![ch('y')]);
    assert_eq!(cmds, vec![Cmd::Apply { id: ID1.into() }]);
}

#[test]
fn a_refused_plan_shows_the_reasons_and_applies_nothing() {
    let (a, cmds) = feed(
        with_runs(),
        vec![Input::Bg(Bg::Planned(Err(
            "rechazado, no se aplicó nada:\n  .git/hooks/x: ruta protegida".into(),
        )))],
    );
    assert!(cmds.is_empty() && a.confirm.is_none());
    let text = screen_text(&a, 80, 24);
    assert!(
        text.contains("ruta protegida") && text.contains("Aviso"),
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
    let (a, _) = feed(a, "arregla algo ".chars().map(ch).collect());
    // The check answers for the current sequence (1); an old answer is ignored.
    let ok = Preview {
        workspace: "/home/ivan/proyecto".into(),
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
            dir: "/home/ivan/proyecto".into(),
            instruction: "arregla algo".into()
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
            result: Err("rechazo usar / como carpeta de trabajo".into()),
        })],
    );
    assert!(cmds.is_empty());
    let text = screen_text(&a, 80, 24);
    assert!(text.contains("rechazo usar /"), "{text}");
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
    assert_eq!(a.new_run.dir, "/home/ivan/proyecto/tmpx");
    assert!(matches!(cmds[0], Cmd::Check { .. }));
}

pub(super) fn started() -> Bg {
    Bg::Started {
        id: ID1.into(),
        files: 3,
        workspace: "/home/ivan/proyecto".into(),
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
                t.contains("read_file") && t.contains("permitido") && t.contains("negado"),
                "{t}"
            );
            assert!(t.contains("S-16") && t.contains("1234 car."), "{t}");
            assert!(
                t.contains("2/40") && t.contains("5 s/10 min") && t.contains("externo"),
                "{t}"
            );
            assert!(
                !t.contains("contaminada") && !t.contains("leyó archivos"),
                "the long warning is gone; `externo` says it"
            );
        }
    }
    let (a, cmds) = feed(a, vec![key(KeyCode::Esc)]);
    assert_eq!(cmds, vec![Cmd::Cancel]);
    assert!(a.live.cancelling && !a.quit);
    assert!(screen_text(&a, 80, 24).contains("cancelando"));
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
    assert!(screen_text(&a, 80, 24).contains("terminó"));
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
    assert!(screen_text(&a, 80, 24).contains("cancelado"));
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
    assert!(t.contains("línea 1/63"), "{t}");
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
    assert!(screen_text(&a, 80, 24).contains("Deshacer lo aplicado (2 archivo(s))"));
    let (_, cmds) = feed(a, vec![ch('y')]);
    assert_eq!(cmds, vec![Cmd::Undo { id: ID2.into() }]);
}

#[test]
fn after_applying_the_lists_reload() {
    let a = open_detail(with_runs(), detail(ID1, Status::Pending), false);
    let (a, cmds) = feed(
        a,
        vec![Input::Bg(Bg::Done(Ok("2 archivo(s) aplicados".into())))],
    );
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
    assert_eq!(a.flash.as_deref(), Some("2 archivo(s) aplicados"));
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
    assert!(t.contains("/readme") && t.contains("1 de 3"), "{t}");
    assert!(
        t.contains("agrega un README") && !t.contains("arregla parse_date en"),
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
    let (a, _) = feed(a, "fallido".chars().map(ch).collect());
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
    assert!(
        t.contains("Ningún run coincide") && t.contains("0 de 3"),
        "{t}"
    );
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
                msg: "/home/ivan/.config/clio es privada (700)".into(),
                hint: None,
            },
            Check {
                level: Level::Warn,
                msg: "no hay clave del modelo".into(),
                hint: Some(
                    "Ponla en OPENCODE_API_KEY o en ~/.config/clio/opencode-api-key (chmod 600)."
                        .into(),
                ),
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
            assert!(t.contains("clio · proyecto · beam-free"), "{t}");
            assert!(t.contains("clave presente"), "{t}");
            assert!(t.contains("doctor: 12 ok · 3 avisos · 0 fallos"), "{t}");
            assert!(
                t.contains("1031-1816-5a67") && t.contains("pendiente de revisar"),
                "{t}"
            );
            assert!(t.contains("aplicado") && t.contains("fallido"), "{t}");
            assert!(t.contains("arregla parse_date en dates.py"), "{t}");
            assert!(
                t.contains("n nuevo") && t.contains("/ filtrar") && t.contains("q salir"),
                "{t}"
            );
        }
    }
    let t = screen_text(&app(), 80, 24);
    assert!(
        t.contains("Todavía no hay runs") && t.contains("cargando"),
        "{t}"
    );
}

#[test]
fn home_reports_a_missing_key_and_a_broken_config() {
    let mut a = with_runs();
    a.home.header.as_mut().unwrap().key = KeyState::Missing;
    assert!(screen_text(&a, 80, 24).contains("falta la clave"));
    a.home.header.as_mut().unwrap().config_error =
        Some("config.json: no existe (ejecuta `clio init` primero)".into());
    assert!(screen_text(&a, 100, 24).contains("ejecuta `clio init` primero"));
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
    a.new_run.instruction = "arregla algo".into();
    a.new_run.check = Some(Ok(Preview {
        workspace: "/home/ivan/proyecto".into(),
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
                t.contains("Carpeta") && t.contains("/home/ivan/proyecto"),
                "{t}"
            );
            assert!(
                t.contains("arregla algo") && t.contains("se copiarán 12 archivos (3,0 MB)"),
                "{t}"
            );
            assert!(t.contains("parecen secretos: .env, id_rsa"), "{t}");
            assert!(
                t.contains(".git, node_modules") && t.contains("enlaces simbólicos: link"),
                "{t}"
            );
        }
    }
    a.new_run.check = Some(Err("rechazo usar tu carpeta personal".into()));
    assert!(screen_text(&a, 80, 24).contains("rechazo usar tu carpeta personal"));
}

#[test]
fn detail_draws_summary_message_files_and_a_colored_diff() {
    let a = open_detail(with_runs(), detail(ID1, Status::Pending), false);
    for (w, h) in sizes() {
        let t = screen_text(&a, w, h);
        assert_clean(&t);
        if w >= 80 {
            assert!(
                t.contains("pendiente de revisar") && t.contains("arregla parse_date"),
                "{t}"
            );
            assert!(
                t.contains("Arreglé parse_date.") && t.contains("No pude correr las pruebas."),
                "{t}"
            );
            assert!(
                t.contains("archivos cambiados (2)")
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
                t.contains("✓  /home") && t.contains("!  no hay clave") && t.contains("✗  /x/y"),
                "{t}"
            );
            assert!(
                t.contains("→ Ponla en OPENCODE_API_KEY") && t.contains("→ chmod 700 /x/y"),
                "{t}"
            );
            assert!(t.contains("doctor: 1 ok · 1 aviso · 1 fallo"), "{t}");
        }
    }
}

#[test]
fn help_lists_this_screens_keys_in_one_short_view() {
    let (mut a, _) = feed(with_runs(), vec![ch('?')]);
    let t = screen_text(&a, 80, 24);
    assert_fits(&t, 80, 24);
    for want in [
        "atajos",
        "Enter",
        "abrir",
        "filtrar",
        "contexto",
        "uso",
        "refrescar",
        "doctor",
        "salir",
        "Ctrl-D",
        "modo debug",
        "Ctrl-L",
        "redibujar",
        "aplicar",
        "deshacer",
    ] {
        assert!(t.contains(want), "{want}\n{t}");
    }
    // Another screen has another list.
    a.screen = Screen::Help;
    a.before_help = Screen::Live;
    let t = screen_text(&a, 80, 24);
    assert!(
        t.contains("cancelar") && t.contains("tiempo / debug"),
        "{t}"
    );
    assert!(!t.contains("filtrar"), "{t}");
    // At the minimum size every key is still there (two columns), and a long list would scroll.
    let t = screen_text(&a, 60, 16);
    assert_fits(&t, 60, 16);
    for want in ["Ctrl-C", "Tab", "Ctrl-B", "Ctrl-D", "Ctrl-L", "redibujar"] {
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
            assert!(t.contains("Terminal muy pequeña") || h < 4, "{w}x{h}\n{t}");
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
                    "--- a/{n}\n+++ b/{n}\n@@ -1,1 +1,1 @@\n-viejo\u{1b}[2J\n+nuevo\u{1b}]0;pwned-title\u{7}\r+FAKE\n \u{202E}reversed\n"
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
        let mut b = App::new("/home/ivan/proyecto", self.theme);
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
