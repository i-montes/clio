//! One continuous conversation: `clio` reopens the last conversation, and the history of a
//! reopened session comes from its Pi file, a page at a time.

#![allow(clippy::unwrap_used)]

use clio_brain::pi::sessions::History;
use crossterm::event::KeyCode;
use serde_json::{Value, json};

use super::app::{App, Bg, ChatInfo, Cmd, Input, NOTHING_TO_REOPEN, OpenTarget, Screen};
use super::chat::{EARLIER, Item, SUMMARISED};
use super::tests::{ch, feed, key};
use super::tests_gallery::{pi, screen_text};

pub(super) fn message(role: &str, text: &str) -> Value {
    json!({"type": "message", "message": {"role": role, "content": [{"type": "text", "text": text}]}})
}

pub(super) fn page(entries: Vec<Value>, first: usize, more: bool) -> History {
    History {
        entries,
        first,
        more,
    }
}

pub(super) fn opened(id: &str, history: Vec<Value>) -> Box<ChatInfo> {
    Box::new(ChatInfo {
        id: id.into(),
        workspace: "/home/ivan".into(),
        history,
        refresh: None,
        presentation: false,
        notes: vec![],
        subagents: vec![],
    })
}

/// Opens the drawer's second row (`/s/2_b.jsonl`): the command, and the chat waiting for it.
fn open_second() -> (App, u64) {
    let mut a = pi(true);
    a.screen = Screen::Sessions;
    a.sessions.sel = 2;
    let (a, cmds) = feed(a, vec![key(KeyCode::Enter)]);
    let seq = match &cmds[..] {
        [
            Cmd::ChatOpen {
                seq,
                target: OpenTarget::Existing { reference },
            },
        ] if reference == "/s/2_b.jsonl" => *seq,
        _ => panic!("{cmds:?}"),
    };
    assert_eq!(a.chat.file.as_deref(), Some("/s/2_b.jsonl"));
    (a, seq)
}

fn texts(a: &App) -> Vec<String> {
    a.chat
        .items
        .iter()
        .map(|i| match i {
            Item::User(t) | Item::Note(t) => t.clone(),
            Item::Assistant { text, .. } => text.clone(),
            other => format!("{other:?}"),
        })
        .collect()
}

// The chat shows what the session's file recorded, not Pi's copy (after a compaction only the
// summary and the tail): every message, the person's without Clio's envelope, a quiet line where
// the file has a compaction and one on top while there is more. At the very top the page before
// is read, once, and the view stays where it was.
#[test]
fn a_reopened_session_shows_its_file_and_reads_the_page_before_at_the_top() {
    let (a, seq) = open_second();
    let wrapped = clio_brain::pi::envelope::wrap("2026-11-18 10:00 -05:00", "q2");
    let first = page(
        vec![
            message("user", "q1"),
            message("assistant", "a1"),
            json!({"type": "compaction", "summary": "s", "firstKeptEntryId": "x"}),
            message("user", &wrapped),
            message("assistant", "a2"),
        ],
        40,
        true,
    );
    let (a, _) = feed(
        a,
        vec![Input::Bg(Bg::History {
            seq,
            file: "/s/2_b.jsonl".into(),
            page: Ok(first),
        })],
    );
    let shown = [EARLIER, "q1", "a1", SUMMARISED, "q2", "a2"];
    assert_eq!(texts(&a), shown);
    // On screen while Pi starts, under the session's title.
    assert!(a.chat.opening);
    let s = screen_text(&a, 100, 30);
    assert!(
        s.contains("earlier messages · scroll up") && s.contains("What weighs"),
        "{s}"
    );
    // Pi is up: its own copy is not drawn over it.
    let (a, _) = feed(
        a,
        vec![Input::Bg(Bg::ChatOpened {
            seq,
            result: Ok(opened(
                "b",
                vec![json!({"role": "user", "content": "tail"})],
            )),
        })],
    );
    assert_eq!(texts(&a), shown);
    // Enough turns to scroll: up to the very top asks for the page before, once.
    let mut a = a;
    for n in 0..30 {
        a.chat.items.push(Item::User(format!("later {n}")));
    }
    let _ = screen_text(&a, 100, 30);
    let (a, cmds) = feed(a, vec![key(KeyCode::PageUp)]);
    assert!(cmds.is_empty(), "not at the top yet: {cmds:?}");
    let mut a = a;
    let mut asked = vec![];
    for _ in 0..20 {
        let _ = screen_text(&a, 100, 30);
        let (next, cmds) = feed(a, vec![key(KeyCode::PageUp)]);
        a = next;
        asked.extend(cmds);
    }
    assert_eq!(
        asked,
        vec![Cmd::LoadHistory {
            seq,
            file: "/s/2_b.jsonl".into(),
            before: 40,
        }]
    );
    let before = screen_text(&a, 100, 30);
    let row = |s: &str| s.lines().position(|l| l.contains("▌ q1")).unwrap();
    let (a, _) = feed(
        a,
        vec![Input::Bg(Bg::History {
            seq,
            file: "/s/2_b.jsonl".into(),
            page: Ok(page(
                vec![message("user", "q0"), message("assistant", "a0")],
                2,
                false,
            )),
        })],
    );
    assert_eq!(texts(&a)[..4], ["q0", "a0", "q1", "a1"]);
    assert_eq!(a.chat.earlier, None);
    let after = screen_text(&a, 100, 30);
    assert_eq!(row(&before), row(&after), "{before}\n{after}");
    // Nothing more to read.
    let (_, cmds) = feed(a, vec![key(KeyCode::PageUp)]);
    assert!(cmds.is_empty(), "{cmds:?}");
}

// A file that cannot be read: Pi's own copy, with a quiet line that says why.
#[test]
fn a_session_file_that_cannot_be_read_falls_back_to_pis_copy() {
    let (a, seq) = open_second();
    let (a, _) = feed(
        a,
        vec![
            Input::Bg(Bg::History {
                seq,
                file: "/s/2_b.jsonl".into(),
                page: Err("Permission denied".into()),
            }),
            Input::Bg(Bg::ChatOpened {
                seq,
                result: Ok(opened(
                    "b",
                    vec![json!({"role": "user", "content": "tail"})],
                )),
            }),
        ],
    );
    let t = texts(&a);
    assert_eq!(t[0], "tail");
    assert!(
        t[1].contains("Permission denied") && t[1].contains("what Pi keeps"),
        "{t:?}"
    );
    // A page for a chat the person already left is dropped.
    let (a, _) = feed(
        a,
        vec![Input::Bg(Bg::History {
            seq: seq + 7,
            file: "/s/x.jsonl".into(),
            page: Ok(page(vec![message("user", "other")], 0, false)),
        })],
    );
    assert_eq!(texts(&a), t);
}

/// `clio` alone, as `run` starts it: the drawer's session, opened by itself.
fn resuming() -> (App, u64) {
    let mut a = pi(true);
    let cmds = super::sessions::resume(&mut a);
    let seq = match &cmds[..] {
        [
            Cmd::ChatOpen {
                seq,
                target: OpenTarget::Last { project },
            },
        ] if project == "/home/ivan/project" => *seq,
        _ => panic!("{cmds:?}"),
    };
    assert!(a.chat.opening && a.chat.resume && !a.chat.is_draft());
    (a, seq)
}

fn typed(text: &str) -> Vec<Input> {
    text.chars().map(ch).collect()
}

// `clio` reopens the last conversation: its history is on screen at once, the composer takes keys
// while Pi starts, and a message sent meanwhile goes out once it is up, after the history.
#[test]
fn clio_reopens_the_last_conversation_and_sends_what_was_typed_meanwhile() {
    let (a, seq) = resuming();
    let mut inputs = typed("and now /home");
    inputs.push(key(KeyCode::Enter));
    inputs.extend(typed("draft"));
    let (a, cmds) = feed(a, inputs);
    assert!(cmds.is_empty(), "{cmds:?}");
    assert_eq!(a.chat.queued.as_deref(), Some("and now /home"));
    assert_eq!(a.chat.input, "draft");
    let (a, cmds) = feed(
        a,
        vec![
            Input::Bg(Bg::History {
                seq,
                file: "/s/1_sid.jsonl".into(),
                page: Ok(page(
                    vec![message("user", "q1"), message("assistant", "a1")],
                    1,
                    false,
                )),
            }),
            Input::Bg(Bg::ChatOpened {
                seq,
                result: Ok(opened("sid", vec![])),
            }),
        ],
    );
    assert_eq!(texts(&a), ["q1", "a1", "and now /home"]);
    assert_eq!(
        cmds,
        vec![Cmd::ChatSend {
            text: "and now /home".into()
        }]
    );
    assert!(a.chat.running && a.chat.input == "draft");
    // The title is the session's, and the drawer marks it open.
    assert_eq!(super::sessions::open_pos(&a), Some(1));
    assert_eq!(a.chat.file.as_deref(), Some("/s/1_sid.jsonl"));
}

// No conversation to reopen: a draft, silently, and a message sent meanwhile starts the new one.
// One that cannot be reopened: a draft with a note that says why, and the message back in the
// composer, in front of what was typed since.
#[test]
fn with_none_or_a_failure_clio_opens_a_draft() {
    let (a, seq) = resuming();
    let (a, cmds) = feed(
        a,
        vec![Input::Bg(Bg::ChatOpened {
            seq,
            result: Err(NOTHING_TO_REOPEN.into()),
        })],
    );
    assert!(cmds.is_empty() && a.chat.is_draft() && a.chat.items.is_empty());
    let (a, seq) = resuming();
    let mut inputs = typed("hola");
    inputs.push(key(KeyCode::Enter));
    let (a, _) = feed(a, inputs);
    let (a, cmds) = feed(
        a,
        vec![Input::Bg(Bg::ChatOpened {
            seq,
            result: Err(NOTHING_TO_REOPEN.into()),
        })],
    );
    assert!(
        matches!(
            &cmds[..],
            [Cmd::ChatOpen {
                target: OpenTarget::New { .. },
                ..
            }]
        ),
        "{cmds:?}"
    );
    assert_eq!(a.chat.queued.as_deref(), Some("hola"));
    let (a, seq) = resuming();
    let mut inputs = typed("hola");
    inputs.push(key(KeyCode::Enter));
    inputs.extend(typed("y"));
    let (a, _) = feed(a, inputs);
    let (a, cmds) = feed(
        a,
        vec![
            Input::Bg(Bg::History {
                seq,
                file: "/s/1_sid.jsonl".into(),
                page: Ok(page(vec![message("user", "old")], 0, false)),
            }),
            Input::Bg(Bg::ChatOpened {
                seq,
                result: Err("Pi exited while starting (code 1)".into()),
            }),
        ],
    );
    assert!(cmds.is_empty() && a.chat.is_draft(), "{cmds:?}");
    let t = texts(&a);
    assert_eq!(t.len(), 1, "{t:?}");
    assert!(t[0].contains("could not reopen the last conversation") && t[0].contains("code 1"));
    assert_eq!(a.chat.input, "hola\ny");
}

// The last conversation is open in another terminal: `clio` does not fall back to an older one; it
// opens a draft and says why in one note.
#[test]
fn a_last_conversation_open_elsewhere_gives_a_new_one() {
    let (a, seq) = resuming();
    let (a, cmds) = feed(
        a,
        vec![Input::Bg(Bg::ChatOpened {
            seq,
            result: Err(crate::cli::chat::local::OPEN_ELSEWHERE.into()),
        })],
    );
    assert!(cmds.is_empty() && a.chat.is_draft(), "{cmds:?}");
    assert_eq!(
        texts(&a),
        ["your last conversation is open in another terminal: this is a new one"]
    );
}

// A note from opening (the session file changed after Clio closed it) is shown in the chat.
#[test]
fn a_note_from_opening_is_shown() {
    let (a, seq) = resuming();
    let mut info = opened("sid", vec![]);
    info.notes = vec!["the session file is not as Clio left it".into()];
    let (a, _) = feed(
        a,
        vec![Input::Bg(Bg::ChatOpened {
            seq,
            result: Ok(info),
        })],
    );
    assert_eq!(last_note(&a), "the session file is not as Clio left it");
}

// A session opened from the drawer that fails: the message sent meanwhile is back in the
// composer, and the chat says it could not open (it is not a new session to retry with Enter).
#[test]
fn a_drawer_session_that_fails_to_open_gives_the_message_back() {
    let (a, seq) = open_second();
    let mut inputs = typed("hola");
    inputs.push(key(KeyCode::Enter));
    let (a, _) = feed(a, inputs);
    let (a, _) = feed(
        a,
        vec![Input::Bg(Bg::ChatOpened {
            seq,
            result: Err("no key".into()),
        })],
    );
    assert_eq!(a.chat.input, "hola");
    assert!(a.chat.failed.is_some() && !a.chat.is_draft());
    assert!(!a.chat.items.contains(&Item::User("hola".into())));
}

/// A Pi session open on `sid`, a turn running.
fn turn_running() -> App {
    let mut a = pi(true);
    a.chat = super::chat::Chat::opening(1, 0);
    a.chat.opened("sid".into(), "/w".into(), &[], None, 0);
    a.chat.running = true;
    a
}

/// The turn ends, Pi's context then holding `tokens` (none right after Pi compacted).
pub(super) fn settled(tokens: Option<u64>) -> Input {
    use clio_brain::pi::rpc::{ContextUsage, SessionStats};
    Input::Bg(Bg::ChatSettled {
        result: Ok(clio_brain::pi::rpc::RunSummary::default()),
        refresh: Some(crate::cli::chat::Refresh {
            stats: SessionStats {
                context: Some(ContextUsage {
                    tokens,
                    window: 1_000_000,
                    percent: None,
                }),
                ..SessionStats::default()
            },
            tainted: false,
            egress_denied: vec![],
            masked: 0,
            withheld: vec![],
            judge_silent: false,
            jobs: 0,
        }),
    })
}

fn last_note(a: &App) -> String {
    match a.chat.items.last() {
        Some(Item::Note(n)) => n.clone(),
        other => panic!("{other:?}"),
    }
}

// Past the cap, the turn that ends is followed by the compaction `/compact` sends, with a quiet
// note; what the person types meanwhile stays in the composer. One that leaves it still over the
// cap says so once, and nothing tries again before another turn ends.
#[test]
fn a_context_over_the_cap_is_compacted_when_the_turn_ends() {
    let (a, cmds) = feed(turn_running(), vec![settled(Some(150_000))]);
    assert_eq!(cmds, vec![Cmd::LoadSessions, check()]);
    assert!(!a.chat.compacting);
    let mut a = a;
    a.chat.running = true;
    let (a, cmds) = feed(a, vec![settled(Some(240_000))]);
    assert_eq!(cmds, vec![Cmd::LoadSessions, Cmd::Compact(String::new())]);
    assert!(a.chat.compacting && a.chat.auto_compact);
    let n = last_note(&a);
    assert!(
        n.contains("240k") && n.contains("200k") && n.contains("automatically"),
        "{n}"
    );
    // Keys still go to the composer, at once.
    let (a, cmds) = feed(a, typed("sigo"));
    assert!(cmds.is_empty());
    assert_eq!(a.chat.input, "sigo");
    let (a, cmds) = feed(
        a,
        vec![Input::Bg(Bg::Compacted(
            Ok("context compacted: from 240k to about 210k tokens".into()),
            Some(210_000),
            None,
        ))],
    );
    // Idle again: now whether to ask about what waits is checked.
    assert_eq!(cmds, vec![check()]);
    assert!(!a.chat.compacting && !a.chat.auto_compact);
    let n = last_note(&a);
    assert!(
        n.contains("still over the cap") && n.contains("/compact N"),
        "{n}"
    );
    assert_eq!(a.chat.input, "sigo");
    // The person's own `/compact`, over the cap or not, says only what it did.
    let mut a = a;
    a.chat.compacting = true;
    let (a, _) = feed(
        a,
        vec![Input::Bg(Bg::Compacted(
            Ok("context compacted".into()),
            Some(210_000),
            None,
        ))],
    );
    assert_eq!(last_note(&a), "context compacted");
}

// "Still over the cap" is said once, not after every turn's compaction; once the context is back
// under the cap, a later one says it again.
#[test]
fn still_over_the_cap_is_said_once() {
    let compacted = || {
        Input::Bg(Bg::Compacted(
            Ok("context compacted".into()),
            Some(210_000),
            None,
        ))
    };
    let said = |a: &App| {
        a.chat
            .items
            .iter()
            .filter(|i| matches!(i, Item::Note(n) if n.contains("still over the cap")))
            .count()
    };
    let (mut a, _) = feed(turn_running(), vec![settled(Some(240_000)), compacted()]);
    assert_eq!(said(&a), 1);
    a.chat.running = true;
    let (mut a, cmds) = feed(a, vec![settled(Some(230_000)), compacted()]);
    assert!(cmds.contains(&Cmd::Compact(String::new())), "{cmds:?}");
    assert_eq!(said(&a), 1, "not again");
    a.chat.running = true;
    let (mut a, _) = feed(a, vec![settled(Some(100_000))]);
    a.chat.running = true;
    let (a, _) = feed(a, vec![settled(Some(240_000)), compacted()]);
    assert_eq!(said(&a), 2, "back under the cap, then over again");
}

// An automatic compaction does not keep the person from quitting (Ctrl-C, /quit), and Esc stops
// it with a quiet note (Pi's `abort` cancels a compaction). A `/compact` the person asked for is
// still waited for, and Esc does nothing to it.
#[test]
fn an_automatic_compaction_can_be_left_or_stopped() {
    use crossterm::event::{KeyEvent, KeyModifiers};
    let ctrl_c = || Input::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    let (a, _) = feed(turn_running(), vec![settled(Some(240_000))]);
    assert!(a.chat.compacting && a.chat.auto_compact);
    let (q, _) = feed(a, vec![ctrl_c()]);
    assert!(q.quit, "Ctrl-C quits");
    let (a, _) = feed(turn_running(), vec![settled(Some(240_000))]);
    let mut inputs = typed("/quit");
    inputs.push(key(KeyCode::Enter));
    let (q, _) = feed(a, inputs);
    assert!(q.quit, "/quit quits");

    let (a, _) = feed(turn_running(), vec![settled(Some(240_000))]);
    let (a, cmds) = feed(a, vec![key(KeyCode::Esc)]);
    assert_eq!(cmds, vec![Cmd::CompactAbort]);
    assert_eq!(last_note(&a), "stopping the automatic compaction…");
    let (a, cmds) = feed(a, vec![key(KeyCode::Esc)]);
    assert!(cmds.is_empty(), "once");
    let (a, _) = feed(
        a,
        vec![Input::Bg(Bg::Compacted(
            Err("Compaction cancelled".into()),
            None,
            None,
        ))],
    );
    assert_eq!(last_note(&a), "the automatic compaction was stopped");
    assert!(!a.chat.compacting && !a.chat.aborting);

    // The person's own `/compact`: waited for.
    let (mut a, _) = feed(turn_running(), vec![settled(Some(100_000))]);
    a.chat.compacting = true;
    let (a, cmds) = feed(a, vec![key(KeyCode::Esc), ctrl_c()]);
    assert!(cmds.is_empty() && !a.quit, "{cmds:?}");
    assert!(a.flash.as_deref().is_some_and(|f| f.contains("compacting")));
}

// Never while something waits: a question or approval on screen, an approved action still
// running, a message held. Nor without Pi's number (right after it compacted by itself).
#[test]
fn nothing_is_compacted_by_itself_while_something_waits() {
    use super::app::{Action, Confirm};
    type Set = fn(&mut App);
    let cases: [(&str, Set); 4] = [
        ("approval", |a| {
            a.confirm = Some(Confirm::new(
                Action::UndoSession,
                "sid".into(),
                0,
                String::new(),
            ));
        }),
        ("answering", |a| a.answering = 1),
        ("asks", |a| {
            let mut c = Confirm::new(Action::UndoSession, "x".into(), 0, String::new());
            c.session = "sid".into();
            a.asks.push_back(c);
        }),
        ("held", |a| a.chat.queued = Some("hola".into())),
    ];
    for (what, set) in cases {
        let mut a = turn_running();
        set(&mut a);
        let (a, cmds) = feed(a, vec![settled(Some(900_000))]);
        assert_eq!(cmds, vec![Cmd::LoadSessions], "{what}");
        assert!(!a.chat.compacting, "{what}");
    }
    let (_, cmds) = feed(turn_running(), vec![settled(None)]);
    assert_eq!(cmds, vec![Cmd::LoadSessions, check()]);
}

/// The turn ended idle: whether to ask about what waits is checked.
fn check() -> Cmd {
    Cmd::CheckApprovals {
        session: "sid".into(),
    }
}
