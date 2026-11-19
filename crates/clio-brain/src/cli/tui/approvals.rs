//! When Clio opens the approval boxes by herself: at most once every `approvals.every_hours`,
//! checked when a chat opens and whenever it becomes idle, never in the middle of a turn. The
//! rest of the time what waits stays in `/pending`, the chat says so in one quiet line and the
//! status line counts it.

use super::app::{App, Cmd, Screen, chat_waits};
use super::chat::Item;

/// The chat's approvals (one session's).
#[derive(Debug, Default)]
pub struct Approvals {
    /// The window is checked once the chat is idle: it opened, a turn ended, a draft waits.
    pub check: bool,
    /// What waits for the person in this session, as of the last check, less what was answered
    /// since: the status line.
    pub waiting: usize,
    /// The person answered a box since the last turn: once nothing else waits, a turn gives the
    /// model what came of them.
    pub answered: bool,
}

/// The note of a turn the approvals started.
pub const APPROVALS_TURN: &str = "turn started by the approvals: Clio says what came of them";

/// The person answered what waited and the chat is idle: one turn, with no words of the person,
/// gives the model what came of each answer (the runtime adds Clio's request). Nothing typed,
/// no menu, box or notice in the way, as for the review's turn, and the keys in the chat: one
/// who went to the sessions may be leaving, and the outcomes then wait for this session.
pub fn deliver(app: &mut App) -> Vec<Cmd> {
    if !app.chat.approvals.answered
        || app.screen != Screen::Chat
        || chat_waits(app)
        || app.chat.failed.is_some()
        || !app.chat.input.is_empty()
        || app.chat.menu.is_some()
        || app.modal.is_some()
        || app.notice.is_some()
    {
        return vec![];
    }
    app.chat.items.push(Item::Note(APPROVALS_TURN.into()));
    app.chat.scroll = 0;
    app.chat.start_turn(app.now_ms);
    vec![Cmd::ChatSend {
        text: String::new(),
    }]
}

/// The chat is idle and something asked for a check: the runtime looks at what waits and at the
/// window (`Cmd::CheckApprovals`), and opens the boxes when it may.
pub fn check(app: &mut App) -> Vec<Cmd> {
    if !app.chat.approvals.check || chat_waits(app) {
        return vec![];
    }
    app.chat.approvals.check = false;
    vec![Cmd::CheckApprovals {
        session: app.chat.session_id.clone(),
    }]
}

/// What the check found: `waiting` items in all, the boxes `opened` or not. Not opened, a count
/// that grew is said once, quietly.
pub fn checked(app: &mut App, session: &str, waiting: usize, opened: bool) {
    if session != app.chat.session_id {
        return;
    }
    let before = std::mem::replace(&mut app.chat.approvals.waiting, waiting);
    if !opened && waiting > before {
        let (s, verb) = if waiting == 1 {
            ("", "waits")
        } else {
            ("s", "wait")
        };
        app.chat.items.push(Item::Note(format!(
            "{waiting} action{s} {verb} for your approval · /pending to answer now"
        )));
    }
}

/// `3 waiting for approval` for the status line, while anything waits.
pub fn status(app: &App) -> Option<String> {
    let n = app.chat.approvals.waiting;
    (n > 0).then(|| format!("{n} waiting for approval"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use crossterm::event::KeyCode;

    use super::super::app::{Action, App, Bg, Cmd, Input, Screen};
    use super::super::ask::Prompt;
    use super::super::commands::{Modal, Pick};
    use super::super::tests::{feed, key};
    use super::super::tests_continuous::settled;
    use super::super::tests_gallery::{screen_text, skill_ask, skill_md};
    use super::super::tests_review::idle;

    fn found(waiting: usize, opened: bool) -> Input {
        Input::Bg(Bg::Approvals {
            session: "sid".into(),
            waiting,
            opened,
        })
    }

    fn notes(a: &App) -> Vec<String> {
        a.chat
            .items
            .iter()
            .filter_map(|i| match i {
                super::Item::Note(n) => Some(n.clone()),
                _ => None,
            })
            .collect()
    }

    // Not yet time to ask: one quiet line when the count grows, the count in the status line,
    // and nothing said again while it does not grow. Asked: no line, the boxes say it.
    #[test]
    fn what_waits_outside_the_window_is_said_once_and_counted() {
        let line = "2 actions wait for your approval · /pending to answer now";
        let (a, _) = feed(idle(), vec![found(2, false)]);
        assert_eq!(notes(&a), [line]);
        assert!(screen_text(&a, 100, 20).contains("2 waiting for approval"));
        let (a, _) = feed(a, vec![found(2, false), found(1, false)]);
        assert_eq!(notes(&a), [line], "said once; fewer is not news");
        assert!(screen_text(&a, 100, 20).contains("1 waiting for approval"));
        let (a, _) = feed(a, vec![found(0, false)]);
        assert!(!screen_text(&a, 100, 20).contains("waiting for approval"));
        let (a, _) = feed(idle(), vec![found(3, true)]);
        assert!(notes(&a).is_empty());
        // Another session's answer changes nothing here.
        let (mut a, _) = feed(idle(), vec![]);
        a.chat.session_id = "other".into();
        let (a, _) = feed(a, vec![found(2, false)]);
        assert!(notes(&a).is_empty());
    }

    // The window is checked when the chat is idle, never in the middle of a turn.
    #[test]
    fn the_window_is_checked_only_when_idle() {
        let mut a = idle();
        a.chat.approvals.check = true;
        a.chat.start_turn(a.now_ms);
        let (mut a, cmds) = feed(a, vec![key(KeyCode::Down)]);
        assert!(!cmds.iter().any(|c| matches!(c, Cmd::CheckApprovals { .. })));
        a.chat.running = false;
        let (_, cmds) = feed(a, vec![key(KeyCode::Down)]);
        assert!(cmds.contains(&Cmd::CheckApprovals {
            session: "sid".into()
        }));
    }

    // `/pending` answers every class: Enter on a secret opens its own box, on a skill the whole
    // skill, on an action its plain box.
    #[test]
    fn pending_opens_the_right_box_for_every_class() {
        let modal = |p: Pick| {
            let mut a = idle();
            let mut m = Modal::text("Pending", vec![p.what.clone()]);
            m.picks = vec![p];
            a.modal = Some(m);
            feed(a, vec![key(KeyCode::Enter)]).0
        };
        let mut p = Pick::new("k1", "«secret» LINEAR_TOKEN");
        p.secret = Some((
            "LINEAR_TOKEN".into(),
            "api.linear.app".into(),
            "to file the issue".into(),
        ));
        let a = modal(p);
        assert!(a.modal.is_none() && a.confirm.is_none());
        let Some(Prompt::Secret(s)) = &a.prompt else {
            panic!("no secret box")
        };
        assert_eq!((s.id.as_str(), s.name.as_str()), ("k1", "LINEAR_TOKEN"));

        let mut p = Pick::new("e5f6", "«skill» register the skill «disk-report»");
        p.block = Some(skill_ask(&skill_md("1. Say hello."), &[], "outside", false));
        let a = modal(p);
        let c = a.confirm.as_ref().unwrap();
        assert_eq!((c.action, c.id.as_str()), (Action::Approve, "e5f6"));
        assert!(c.block.as_ref().unwrap().skill.is_some());

        let a = modal(Pick::new("a1b2", "«delete» rm -r /srv/old"));
        let c = a.confirm.as_ref().unwrap();
        assert_eq!((c.id.as_str(), c.block.is_none()), ("a1b2", true));
    }

    fn answered(r: Result<&str, &str>) -> Input {
        Input::Bg(Bg::ChatAnswered(
            r.map(str::to_owned).map_err(str::to_owned),
        ))
    }

    fn approvals_turns(cmds: &[Cmd]) -> usize {
        cmds.iter()
            .filter(|c| {
                **c == Cmd::ChatSend {
                    text: String::new(),
                }
            })
            .count()
    }

    // Two boxes answered: once the last answer is back and the chat is idle, ONE turn gives the
    // model what came of them, and the chat says who started it.
    #[test]
    fn one_turn_after_a_batch_of_answers() {
        let mut a = idle();
        a.answering = 2;
        let (a, cmds) = feed(a, vec![answered(Ok("approved a1b2: done"))]);
        assert_eq!(approvals_turns(&cmds), 0, "one answer is still on its way");
        let (a, cmds) = feed(a, vec![answered(Ok("rejected c3d4: it is not run"))]);
        assert_eq!(approvals_turns(&cmds), 1, "{cmds:?}");
        assert!(a.chat.running);
        assert_eq!(
            notes(&a).last().map(String::as_str),
            Some(super::APPROVALS_TURN)
        );
        let (a, cmds) = feed(a, vec![settled(Some(1000)), key(KeyCode::Down)]);
        assert_eq!(approvals_turns(&cmds), 0, "once per batch");
        assert_eq!(
            notes(&a)
                .iter()
                .filter(|n| *n == super::APPROVALS_TURN)
                .count(),
            1
        );
    }

    // The person's own message goes first and carries what came of the answers: no turn after.
    // An answer that failed is no outcome.
    #[test]
    fn a_message_of_the_person_carries_the_outcomes_and_a_failure_is_none() {
        let mut a = idle();
        a.answering = 1;
        a.chat.set_input("and now?".into());
        let (a, cmds) = feed(a, vec![answered(Ok("approved a1b2: done"))]);
        assert_eq!(approvals_turns(&cmds), 0, "something is typed");
        let (a, cmds) = feed(a, vec![key(KeyCode::Enter)]);
        assert!(cmds.contains(&Cmd::ChatSend {
            text: "and now?".into()
        }));
        let (_, cmds) = feed(a, vec![settled(Some(1000)), key(KeyCode::Down)]);
        assert_eq!(approvals_turns(&cmds), 0);

        let mut a = idle();
        a.answering = 1;
        let (_, cmds) = feed(a, vec![answered(Err("a1b2: gone"))]);
        assert_eq!(approvals_turns(&cmds), 0);

        // In the sessions the person may be leaving: the turn waits for the chat.
        let mut a = idle();
        a.answering = 1;
        a.screen = Screen::Sessions;
        let (mut a, cmds) = feed(a, vec![answered(Ok("approved a1b2: done"))]);
        assert_eq!(approvals_turns(&cmds), 0);
        a.screen = Screen::Chat;
        let (_, cmds) = feed(a, vec![key(KeyCode::Down)]);
        assert_eq!(approvals_turns(&cmds), 1);
    }
}
