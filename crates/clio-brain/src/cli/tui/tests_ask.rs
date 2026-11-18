//! What Clio asks the person: the `ask_person` question (Pi's `select`, then `input` after
//! `Other…`), its quiet second, its clicks, and its turn with the approvals.

#![allow(clippy::unwrap_used)]

use clio_brain::pi::rpc::Event;
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::app::{Action, App, Bg, Cmd, Input};
use super::ask::{OTHER, Prompt};
use super::tests::{ch, feed, key, screen_text};

pub(super) fn chat() -> App {
    let mut a = super::tests_gallery::pi(false);
    a.chat = super::chat::Chat::opening(1, 0);
    a.chat.opened("sid".into(), String::new(), &[], None, 0);
    a.chat.running = true;
    a
}

pub(super) fn select(id: &str, question: &str, options: &[&str]) -> Input {
    let mut o: Vec<String> = options.iter().map(|s| (*s).to_owned()).collect();
    o.push(OTHER.into());
    Input::Bg(Bg::ChatEvent(Event::ExtensionUiRequest {
        id: id.into(),
        method: "select".into(),
        title: question.into(),
        options: o,
        timeout_ms: Some(600_000),
        widget_key: String::new(),
        widget_lines: None,
    }))
}

fn input(id: &str) -> Input {
    Input::Bg(Bg::ChatEvent(Event::ExtensionUiRequest {
        id: id.into(),
        method: "input".into(),
        title: "Which database?".into(),
        options: vec![],
        timeout_ms: Some(500_000),
        widget_key: String::new(),
        widget_lines: None,
    }))
}

fn ui(id: &str, value: Option<&str>) -> Cmd {
    Cmd::ChatUi {
        id: id.into(),
        value: value.map(str::to_owned),
    }
}

pub(super) const OPTIONS: [&str; 3] = [
    "Postgres — what the other services use",
    "SQLite — one file, nothing to run",
    "Keep both",
];

#[test]
fn a_question_takes_keys_only_after_the_quiet_second_and_answers_with_the_option_string() {
    let (a, cmds) = feed(
        chat(),
        vec![Input::Tick(100), select("q1", "Which database?", &OPTIONS)],
    );
    assert!(cmds.is_empty());
    let screen = screen_text(&a, 90, 30);
    assert!(
        screen.contains("Clio asks")
            && !screen.contains("Visto bueno")
            && screen.contains("Which database?")
            && screen.contains("1  Postgres (recommended)")
            && screen.contains("what the other services use")
            && screen.contains("4  Other…")
            && screen.contains("Esc skip the question"),
        "{screen}"
    );
    // Typing on, still in the chat's rhythm: nothing is answered, nothing moves.
    let (a, cmds) = feed(
        a,
        vec![
            Input::Tick(300),
            ch('2'),
            Input::Tick(600),
            key(KeyCode::Enter),
            Input::Tick(900),
            key(KeyCode::Esc),
        ],
    );
    assert!(cmds.is_empty(), "{cmds:?}");
    let Some(Prompt::Question(q)) = &a.prompt else {
        panic!("the question went")
    };
    assert_eq!((q.sel, q.ready), (0, false));
    // A second later: ↓ and Enter pick the second answer; Pi gets the string it sent.
    let (a, cmds) = feed(
        a,
        vec![Input::Tick(2_000), key(KeyCode::Down), key(KeyCode::Enter)],
    );
    assert_eq!(cmds, vec![ui("q1", Some(OPTIONS[1]))]);
    assert!(a.prompt.is_none());
    // The digit answers at once; Esc skips (Pi hears it as cancelled), and the turn goes on.
    let (a, cmds) = feed(
        a,
        vec![
            select("q2", "Which database?", &OPTIONS),
            Input::Tick(4_000),
            ch('3'),
        ],
    );
    assert_eq!(cmds, vec![ui("q2", Some(OPTIONS[2]))]);
    let (a, cmds) = feed(
        a,
        vec![
            select("q3", "Which database?", &OPTIONS),
            Input::Tick(6_000),
            key(KeyCode::Esc),
        ],
    );
    assert_eq!(cmds, vec![ui("q3", None)]);
    assert!(a.chat.running && a.prompt.is_none());
}

#[test]
fn other_sends_the_choice_then_the_typed_line_to_the_input_that_follows() {
    let (a, cmds) = feed(
        chat(),
        vec![
            select("q1", "Which database?", &OPTIONS),
            Input::Tick(2_000),
            ch('4'),
        ],
    );
    assert_eq!(cmds, vec![ui("q1", Some(OTHER))]);
    // The person types before Pi's `input` comes: the answer waits for it.
    let typed: Vec<Input> = "MariaDB"
        .chars()
        .map(ch)
        .chain([
            Input::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('x'),
                KeyModifiers::CONTROL,
            )),
            Input::Paste("pasted".into()),
        ])
        .collect();
    let (a, cmds) = feed(a, typed);
    assert!(cmds.is_empty());
    let screen = screen_text(&a, 90, 30);
    assert!(
        screen.contains("MariaDB") && !screen.contains("pasted"),
        "{screen}"
    );
    let (a, cmds) = feed(a, vec![key(KeyCode::Enter)]);
    assert!(
        cmds.is_empty(),
        "nothing goes before Pi asks for it: {cmds:?}"
    );
    let (a, cmds) = feed(a, vec![input("q1b")]);
    assert_eq!(cmds, vec![ui("q1b", Some("MariaDB"))]);
    assert!(a.prompt.is_none());
    // Once the field is there, Enter sends; an empty line sends nothing; Esc skips.
    let (a, _) = feed(
        a,
        vec![
            select("q2", "Which?", &OPTIONS),
            Input::Tick(4_000),
            ch('4'),
            input("q2b"),
            key(KeyCode::Enter),
        ],
    );
    assert!(a.prompt.is_some());
    let (_, cmds) = feed(a, vec![key(KeyCode::Esc)]);
    assert_eq!(cmds, vec![ui("q2b", None)]);
}

#[test]
fn a_question_and_an_approval_wait_for_each_other_and_a_turn_end_drops_the_question() {
    let mut a = chat();
    a.host = true;
    let (a, _) = feed(
        a,
        vec![
            select("q1", "Which?", &OPTIONS),
            Input::Bg(Bg::ChatAsks(vec![("a1".into(), "«borrar» rm x".into())])),
        ],
    );
    assert!(a.confirm.is_none() && a.asks.len() == 1);
    let (a, cmds) = feed(a, vec![Input::Tick(2_000), ch('1')]);
    assert_eq!(cmds, vec![ui("q1", Some(OPTIONS[0]))]);
    assert_eq!(a.confirm.as_ref().unwrap().action, Action::Approve);
    // And the reverse: a question behind the approval on screen.
    let (a, _) = feed(a, vec![select("q2", "Which?", &OPTIONS)]);
    assert!(a.prompt.is_none() && a.prompts.len() == 1);
    let (a, _) = feed(a, vec![Input::Tick(4_000), key(KeyCode::Esc)]);
    assert!(a.confirm.is_none() && a.prompt.is_some());
    // The digit of a question never approves: the approval's keys mean nothing here.
    let (a, cmds) = feed(a, vec![Input::Tick(6_000), ch('s'), key(KeyCode::Enter)]);
    assert_eq!(cmds, vec![ui("q2", Some(OPTIONS[0]))]);
    // A question still open when the turn settles is gone: Pi already answered it.
    let (a, _) = feed(a, vec![select("q3", "Which?", &OPTIONS)]);
    let (a, _) = feed(
        a,
        vec![Input::Bg(Bg::ChatSettled {
            result: Err("x".into()),
            refresh: None,
        })],
    );
    assert!(a.prompt.is_none() && a.prompts.is_empty());
}

#[test]
fn a_question_past_its_time_closes_and_one_of_another_session_is_never_shown() {
    let (a, _) = feed(
        chat(),
        vec![Input::Tick(1_000), select("q1", "Which?", &OPTIONS)],
    );
    let (a, cmds) = feed(a, vec![Input::Tick(601_000)]);
    assert!(cmds.is_empty() && a.prompt.is_none());
    assert!(
        a.flash.as_deref().unwrap().contains("no answer"),
        "{:?}",
        a.flash
    );
    let mut a = a;
    a.chat.session_id = "otra".into();
    let mut b = chat();
    b.chat.session_id = "otra".into();
    let (mut b, _) = feed(b, vec![select("q2", "Which?", &OPTIONS)]);
    // Stamped with another session, then this one is shown: it is dropped.
    if let Some(Prompt::Question(x)) = &mut b.prompt {
        x.session = "sid".into();
    }
    a.prompts.push_back(b.prompt.unwrap());
    let (a, _) = feed(a, vec![Input::Bg(Bg::ChatAsks(vec![]))]);
    assert!(a.prompt.is_none() && a.prompts.is_empty());
}

#[test]
fn a_click_selects_only_the_row_drawn_there_and_never_answers() {
    let (a, _) = feed(chat(), vec![select("q1", "Which database?", &OPTIONS)]);
    let screen = screen_text(&a, 90, 30);
    let (y, line) = screen
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains("3  Keep both"))
        .unwrap();
    let x = line.chars().position(|c| c == 'K').unwrap();
    let click = |x: usize, y: usize| {
        Input::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: u16::try_from(x).unwrap(),
            row: u16::try_from(y).unwrap(),
            modifiers: KeyModifiers::NONE,
        })
    };
    let (a, cmds) = feed(a, vec![click(x, y)]);
    assert!(cmds.is_empty());
    let Some(Prompt::Question(q)) = &a.prompt else {
        panic!()
    };
    assert_eq!(q.sel, 2);
    // Another question now, never drawn: the old rows say nothing about it.
    let mut a = a;
    let (b, _) = feed(chat(), vec![select("q9", "Other", &["A", "B"])]);
    let mut fresh = b.prompt.unwrap();
    if let (Prompt::Question(n), Some(Prompt::Question(old))) = (&mut fresh, &a.prompt) {
        n.rows_at.replace(old.rows_at.borrow().clone());
    }
    a.prompt = Some(fresh);
    let (a, _) = feed(a, vec![click(x, y - 1)]);
    let Some(Prompt::Question(q)) = &a.prompt else {
        panic!()
    };
    assert_eq!(q.sel, 0);
}

pub(super) const LONG: [&str; 3] = [
    "Postgres — what the orders, billing and search services already use, with backups every night and a replica in the other zone",
    "SQLite — one file, nothing to run",
    "Keep both",
];

// A long description wraps onto dim rows under its label (three rows at most, then cut), and a
// click on any row of an answer selects that answer.
#[test]
fn a_long_description_wraps_under_its_answer_and_a_click_on_it_picks_that_answer() {
    let (a, _) = feed(chat(), vec![select("q1", "Which database?", &LONG)]);
    let screen = screen_text(&a, 60, 30);
    let rows: Vec<&str> = screen.lines().collect();
    let top = rows.iter().position(|l| l.contains("1  Postgres")).unwrap();
    assert!(
        rows[top + 1].contains("services") || rows[top + 1].contains("billing"),
        "{screen}"
    );
    assert!(rows[top + 2].contains('…'), "{screen}");
    assert!(rows[top + 3].contains("2  SQLite"), "{screen}");
    let click = |y: usize| {
        Input::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 20,
            row: u16::try_from(y).unwrap(),
            modifiers: KeyModifiers::NONE,
        })
    };
    let pick = |a: App, y: usize| {
        let (a, cmds) = feed(a, vec![click(y)]);
        assert!(cmds.is_empty());
        let Some(Prompt::Question(q)) = &a.prompt else {
            panic!()
        };
        let sel = q.sel;
        (a, sel)
    };
    let (a, sel) = pick(a, top + 3);
    assert_eq!(sel, 1);
    let (a, sel) = pick(a, top + 2);
    assert_eq!(sel, 0);
    let (_, sel) = pick(a, top + 4);
    assert_eq!(sel, 2);
}

#[test]
fn the_other_field_says_never_to_type_a_secret_there() {
    let (a, _) = feed(
        chat(),
        vec![
            select("q1", "Which database?", &OPTIONS),
            Input::Tick(2_000),
            ch('4'),
        ],
    );
    let screen = screen_text(&a, 90, 30);
    assert!(screen.contains(super::ask::NO_SECRETS), "{screen}");
    let (b, _) = feed(chat(), vec![select("q1", "Which database?", &OPTIONS)]);
    assert!(!screen_text(&b, 90, 30).contains("Never type a secret"));
}

#[test]
fn hostile_question_text_is_one_clean_line_and_capped() {
    let long = "x".repeat(5_000);
    let evil = format!("Pick\u{1b}[2J\u{202e}one\n# not a heading {long}");
    let (a, _) = feed(
        chat(),
        vec![select(
            "q1",
            &evil,
            &["**bold** — \u{1b}]52;c;aGk=\u{7}desc", "/quit", "B"],
        )],
    );
    let Some(Prompt::Question(q)) = &a.prompt else {
        panic!()
    };
    assert!(q.text.chars().count() <= super::ask::MAX_QUESTION);
    assert!(!q.text.contains('\u{1b}') && !q.text.contains('\n'));
    let screen = screen_text(&a, 90, 30);
    super::tests::assert_clean(&screen);
    assert!(
        screen.contains("**bold**") && screen.contains("/quit"),
        "{screen}"
    );
}

// ---------------------------------------------------------------- the secret box

const VALUE: &str = "lin_api_Zq8vX2mN4pR7tY1wK9sB3dF6";

pub(super) fn secrets(suspect: bool) -> Input {
    Input::Bg(Bg::ChatSecrets(
        vec![(
            "ab12cd34".into(),
            (
                "linear".into(),
                "http_fetch:api.linear.app".into(),
                "to read the issues of the sprint".into(),
            ),
        )],
        suspect,
    ))
}

/// No 8 bytes in a row of `VALUE` are in `hay`.
fn none_of_the_value(hay: &str) -> bool {
    VALUE
        .as_bytes()
        .windows(8)
        .all(|w| !hay.as_bytes().windows(8).any(|h| h == w))
}

#[test]
fn the_secret_box_says_where_the_value_may_go_and_shows_only_dots() {
    let (a, _) = feed(chat(), vec![Input::Tick(100), secrets(false)]);
    let screen = screen_text(&a, 90, 30);
    assert!(
        screen.contains("Clio asks for a secret")
            && screen.contains("linear")
            && screen.contains("it can only be sent to http_fetch:api.linear.app")
            && screen.contains("why: to read the issues of the sprint")
            && screen.contains("Esc decline")
            && !screen.contains("Visto bueno")
            && !screen.contains("tried to give orders"),
        "{screen}"
    );
    // Typed within the quiet second: nothing.
    let (a, _) = feed(
        a,
        vec![Input::Tick(300), ch('x'), Input::Paste("yy".into())],
    );
    let Some(Prompt::Secret(s)) = &a.prompt else {
        panic!()
    };
    assert!(s.value.is_empty());
    // After it: typed and pasted (line ends dropped), each a dot on screen.
    let mut inputs = vec![Input::Tick(2_000)];
    inputs.extend(VALUE[..10].chars().map(ch));
    inputs.push(Input::Paste(format!("{}\n", &VALUE[10..])));
    inputs.extend([ch('!'), key(KeyCode::Backspace)]);
    let (mut a, cmds) = feed(a, inputs);
    assert!(cmds.is_empty());
    let Some(Prompt::Secret(s)) = &a.prompt else {
        panic!()
    };
    assert_eq!(s.value.expose(), VALUE);
    let dots = "•".repeat(VALUE.chars().count());
    for debug in [false, true] {
        a.debug = debug;
        let screen = screen_text(&a, 120, 40);
        assert!(screen.contains(&dots), "{screen}");
        assert!(none_of_the_value(&screen), "{screen}");
    }
    // A turn that ends does not take it away; Enter sends the value, and only in its command.
    let (a, _) = feed(
        a,
        vec![Input::Bg(Bg::ChatSettled {
            result: Err("x".into()),
            refresh: None,
        })],
    );
    assert!(a.prompt.is_some());
    let (a, cmds) = feed(a, vec![key(KeyCode::Enter)]);
    assert!(a.prompt.is_none() && a.answering == 1);
    let [Cmd::ChatSecret { id, name, value }] = &cmds[..] else {
        panic!("{cmds:?}")
    };
    assert_eq!((id.as_str(), name.as_str()), ("ab12cd34", "linear"));
    assert_eq!(value.as_ref().unwrap().0.expose(), VALUE);
    // What can be printed of the app's state and its commands holds none of it.
    let printed = format!("{cmds:?} {:?}", a.chat.items);
    assert!(none_of_the_value(&printed), "{printed}");
    let boxed = value.clone();
    let panic = std::panic::catch_unwind(|| panic!("{boxed:?}")).unwrap_err();
    assert!(none_of_the_value(panic.downcast_ref::<String>().unwrap()));
}

#[test]
fn esc_declines_a_secret_and_a_suspect_session_says_so() {
    let (a, _) = feed(chat(), vec![secrets(true)]);
    let screen = screen_text(&a, 90, 30);
    assert!(screen.contains("tried to give orders"), "{screen}");
    let (a, cmds) = feed(a, vec![Input::Tick(2_000), key(KeyCode::Esc)]);
    assert_eq!(
        cmds,
        vec![Cmd::ChatSecret {
            id: "ab12cd34".into(),
            name: "linear".into(),
            value: None
        }]
    );
    assert!(a.prompt.is_none());
    // Paste goes nowhere in a question.
    let (a, _) = feed(
        a,
        vec![
            select("q1", "Which?", &OPTIONS),
            Input::Tick(5_000),
            Input::Paste("hello".into()),
        ],
    );
    let Some(Prompt::Question(q)) = &a.prompt else {
        panic!()
    };
    assert!(q.typing.is_none() && !q.ready);
}

// A store the vault refused: the worker offers the box again (the request still waits) and the
// error goes to the chat; the box is empty, ready for the value again.
#[test]
fn a_secret_the_vault_refused_comes_back_with_the_error_in_the_chat() {
    let (a, _) = feed(chat(), vec![secrets(false), Input::Tick(2_000)]);
    let mut inputs: Vec<Input> = VALUE.chars().map(ch).collect();
    inputs.push(key(KeyCode::Enter));
    let (a, cmds) = feed(a, inputs);
    assert!(a.prompt.is_none() && cmds.len() == 1);
    let (a, _) = feed(
        a,
        vec![
            secrets(false),
            Input::Bg(Bg::ChatAnswered(Err("linear: the vault is locked".into()))),
        ],
    );
    let Some(Prompt::Secret(s)) = &a.prompt else {
        panic!("the box did not come back")
    };
    assert!(s.value.is_empty() && a.answering == 0);
    let screen = screen_text(&a, 90, 30);
    assert!(screen.contains("Clio asks for a secret"), "{screen}");
    assert!(
        a.chat
            .items
            .iter()
            .any(|i| matches!(i, super::chat::Item::Error(e) if e.contains("vault is locked")))
    );
}

// ---------------------------------------------------------------- the checklist

pub(super) fn widget(key: &str, lines: Option<&[&str]>) -> Input {
    Input::Bg(Bg::ChatEvent(Event::ExtensionUiRequest {
        id: "w".into(),
        method: "setWidget".into(),
        title: String::new(),
        options: vec![],
        timeout_ms: None,
        widget_key: key.into(),
        widget_lines: lines.map(|l| l.iter().map(|s| (*s).to_owned()).collect()),
    }))
}

#[test]
fn the_checklist_is_pinned_above_the_status_line_and_only_its_widget_is_read() {
    let lines = [
        "[x] read the config",
        "[>] fix the parser",
        "[ ] run the tests",
        "[ ] four",
        "[ ] five",
        "[ ] six",
        "[ ] seven",
        "[ ] eight",
    ];
    let (a, cmds) = feed(
        chat(),
        vec![
            widget("other-ext", Some(&["[ ] not ours"])),
            widget(super::todo::KEY, Some(&lines)),
        ],
    );
    assert!(cmds.is_empty());
    assert_eq!(a.chat.todo.len(), 8);
    let screen = screen_text(&a, 90, 30);
    assert!(!screen.contains("not ours"), "{screen}");
    let rows: Vec<&str> = screen.lines().collect();
    let first = rows
        .iter()
        .position(|l| l.contains("read the config"))
        .unwrap();
    assert!(rows[first].contains('✓') && rows[first + 1].contains("fix the parser"));
    assert!(rows[first + 5].contains("+3 more"), "{screen}");
    // The status line comes right under it, then the composer.
    assert!(rows[first + 7].contains('╭'), "{screen}");
    // Cleared: the rows go.
    let (a, _) = feed(a, vec![widget(super::todo::KEY, None)]);
    assert!(a.chat.todo.is_empty());
    assert!(!screen_text(&a, 90, 30).contains("read the config"));
    // A turn that settles with one item left keeps it pinned; all done, the list goes.
    let settled = || {
        Input::Bg(Bg::ChatSettled {
            result: Err("x".into()),
            refresh: None,
        })
    };
    let (a, _) = feed(
        a,
        vec![
            widget(super::todo::KEY, Some(&["[x] one", "[ ] two"])),
            settled(),
        ],
    );
    assert_eq!(a.chat.todo.len(), 2);
    let (a, _) = feed(
        a,
        vec![
            widget(super::todo::KEY, Some(&["[x] one", "[x] two"])),
            settled(),
        ],
    );
    assert!(a.chat.todo.is_empty());
    assert!(!screen_text(&a, 90, 30).contains("two"));
}

#[test]
fn a_turn_that_ends_with_jobs_running_says_so_quietly_and_the_status_line_counts_them() {
    let settled = |jobs| {
        Input::Bg(Bg::ChatSettled {
            result: Err("x".into()),
            refresh: Some(crate::cli::chat::Refresh {
                stats: clio_brain::pi::rpc::SessionStats::default(),
                tainted: false,
                egress_denied: vec![],
                masked: 0,
                withheld: vec![],
                judge_silent: false,
                jobs,
            }),
        })
    };
    let (a, _) = feed(chat(), vec![settled(2)]);
    assert_eq!(a.chat.jobs, 2);
    let s = screen_text(&a, 100, 30);
    assert!(s.contains("2 jobs"), "{s}");
    assert!(s.contains("2 background commands are still running"), "{s}");
    // None left: no mark, and no new note.
    let notes = a.chat.items.len();
    let (a, _) = feed(a, vec![settled(0)]);
    let s = screen_text(&a, 100, 30);
    assert!(!s.contains("jobs"), "{s}");
    assert_eq!(a.chat.items.len(), notes + 1); // the error line only
}
