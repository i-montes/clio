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
