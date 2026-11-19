//! The chat's states drawn as plain text, to look at them:
//! `cargo test -p clio-brain tui_gallery -- --ignored --nocapture`. Also the fixtures the chat's
//! drawing tests share.

#![allow(clippy::unwrap_used)]

use clio_brain::pi::rpc::Event;

use super::app::{Action, App, Confirm, Screen};
use super::chat::{Chat, Item, ToolState};
use super::home::{Header, KeyState};
use super::tests::THEME;
use crate::cli::chat::{Row, RowKind};
use crate::cli::usage::RunLimits;

pub(super) const ANSWER: &str = "## What weighs most\n\nAlmost all of it is in `/var/lib`:\n\n- **docker**: 56G, old images and *volumes*\n- containerd: 39G\n  - snapshots: 30G\n\n1. Check with `docker system df`.\n2. Read [the guide](https://docs.docker.com/config/pruning/).\n\n> Note: I deleted nothing.\n\n```sh\nsudo du -xh --max-depth=1 /var/lib 2>/dev/null | sort -rh | head -n 20\n```\n\nI like 日本語 and ❤\u{fe0f} without breaking the line.";

pub(super) const TABLE: &str = "The ones that use the most memory:\n\n| PID | Process | Memory (RSS) | What it is |\n|---|---|---|---|\n| 4071 | java | ~678 MB | the Minecraft server, with its world loaded in memory |\n| 812 | dockerd | 96 MB | Docker |\n| 1 | systemd | 12 MB | 日本語 ❤\u{fe0f} |\n\nI set the volume deletion aside: it waits for your approval.";

/// The Pi chat with a model and nothing wrong, as `clio` opens.
pub(super) fn pi(host: bool) -> App {
    let mut a = App::new("/home/ivan/project", THEME);
    a.use_pi();
    a.host = host;
    a.home.header = Some(Header {
        model: "claude-sonnet-5-5".into(),
        thinking: "low".into(),
        key: KeyState::Present,
        connector: "anthropic".into(),
        vault: "active".into(),
        config_error: None,
        limits: RunLimits::default(),
        window: 200_000,
        window_known: true,
        context_cap: 200_000,
        review: crate::cli::home::ReviewCfg::default(),
    });
    a.home.doctor = Some((36, 1, 0));
    a.sessions.set(rows());
    a
}

pub(super) fn rows() -> Vec<Row> {
    let now = crate::cli::now_ms();
    let row = |title: &str, reference: &str, mins: i64, n: u32| Row {
        kind: RowKind::Session,
        title: title.into(),
        detail: String::new(),
        messages: n,
        last_ms: Some(now - mins * 60_000),
        reference: reference.into(),
        unavailable: None,
    };
    vec![
        row(
            "Which root folders weigh most on this server?",
            "/s/1_sid.jsonl",
            3,
            4,
        ),
        row("What weighs the most on the disk?", "/s/2_b.jsonl", 26, 3),
        Row {
            unavailable: Some("not a Clio session".into()),
            ..row("a Pi session by hand", "/s/3_c.jsonl", 60 * 5, 9)
        },
    ]
}

/// A session open (`sid`) with a turn behind it: the person's message, a group of tools, the
/// agent's answer in markdown, and its usage.
pub(super) fn conversation(host: bool) -> App {
    let mut a = pi(host);
    a.now_ms = 200_000;
    a.chat = Chat::opening(1, 0);
    a.chat
        .opened("sid".into(), "/home/ivan".into(), &[], None, 0);
    let c = &mut a.chat;
    c.items.push(Item::User(
        "Which root folders weigh most on this server (for example under /var/lib)? Only look, do not delete or change anything.".into(),
    ));
    let tool = |id: &str, reason: &str, cmd: &str, state, ms| Item::Tool {
        id: id.into(),
        name: "bash".into(),
        summary: cmd.into(),
        reason: reason.into(),
        state,
        ms,
    };
    c.items.push(tool(
        "1",
        "Measure the heaviest folders",
        "sudo du -xh --max-depth=1 / 2>/dev/null | sort -rh | head -n 20",
        ToolState::Failed,
        Some(120_000),
    ));
    c.items.push(tool(
        "2",
        "Measure in parts",
        "sudo timeout 50 du -xh --max-depth=1 /var 2>/dev/null | sort -rh",
        ToolState::Ok,
        Some(80_000),
    ));
    c.items
        .push(tool("3", "", "docker system df", ToolState::Running, None));
    c.items.push(Item::Assistant {
        text: ANSWER.into(),
        live: false,
    });
    c.items.push(Item::Note(
        "request judged as level 1 · simple (by what you ask)".into(),
    ));
    c.on_event(&Event::MessageUpdate {
        usage: Some(clio_brain::pi::rpc::Usage {
            input: 40_000,
            output: 616,
            ..Default::default()
        }),
        kind: "text_delta".into(),
        delta: None,
    });
    a
}

fn approve(a: &mut App, quiet: bool) {
    let mut c = Confirm::new(
        Action::Approve,
        "a1b2".into(),
        0,
        "«delete» rm -r /var/lib/docker/volumes/old (deletes 3 folders)".into(),
    );
    c.session = "sid".into();
    a.confirm = Some(c);
    a.now_ms = 300_000;
    a.last_key_ms = Some(if quiet { 0 } else { 299_800 });
}

/// A change to one of the person's blocks a marked session set aside, as the box gets it.
pub(super) fn block_ask(args: serde_json::Value, suspect: bool) -> super::ask::BlockAsk {
    let p = clio_toolhost::pending::Pending {
        id: "c3d4e5f6".into(),
        class: clio_toolhost::ask::Class::Memory,
        tool: "memory_block_edit".into(),
        args,
        hash: String::new(),
        what: String::new(),
        run: "r1".into(),
        session: "sid".into(),
        created_ms: 0,
        expires_ms: 1,
        ask: true,
        state: clio_toolhost::pending::State::Waiting,
        reason: None,
    };
    super::ask::BlockAsk::new(&p, suspect).unwrap()
}

/// A skill set aside for the person (S-68), as the box gets it: `md` is SKILL.md whole.
pub(super) fn skill_ask(
    md: &str,
    files: &[(&str, &str)],
    reason: &str,
    suspect: bool,
) -> super::ask::BlockAsk {
    let files: Vec<serde_json::Value> = files
        .iter()
        .map(|(p, c)| serde_json::json!({"path": p, "content": c}))
        .collect();
    let p = clio_toolhost::pending::Pending {
        id: "e5f6a7b8".into(),
        class: clio_toolhost::ask::Class::Skill,
        tool: "skill_write".into(),
        args: serde_json::json!({"name": "disk-report", "description": SKILL_DESCRIPTION,
            "skill_md": md, "files": files, "update": false}),
        hash: String::new(),
        what: String::new(),
        run: "r1".into(),
        session: "sid".into(),
        created_ms: 0,
        expires_ms: 1,
        ask: true,
        state: clio_toolhost::pending::State::Waiting,
        reason: Some(reason.into()),
    };
    super::ask::BlockAsk::new(&p, suspect).unwrap()
}

/// A service set aside for the person (S-69), as the box gets it: its manifest composed from
/// `m`, its files, the review's verdict and notes, and why it waits.
pub(super) fn service_ask(
    m: &clio_toolhost::services::Manifest,
    files: &[(&str, &str)],
    verdict: &str,
    notes: &str,
    suspect: bool,
) -> super::ask::BlockAsk {
    let files: Vec<serde_json::Value> = files
        .iter()
        .map(|(p, c)| serde_json::json!({"path": p, "content": c}))
        .collect();
    let p = clio_toolhost::pending::Pending {
        id: "s1a2b3c4".into(),
        class: clio_toolhost::ask::Class::Service,
        tool: "service_write".into(),
        args: serde_json::json!({"name": m.name, "description": m.description,
            "skill_md": clio_toolhost::services::compose(m), "files": files, "update": false,
            "verdict": verdict, "notes": notes}),
        hash: String::new(),
        what: String::new(),
        run: "r1".into(),
        session: "sid".into(),
        created_ms: 0,
        expires_ms: 1,
        ask: true,
        state: clio_toolhost::pending::State::Waiting,
        reason: Some("a service always waits for your approval".into()),
    };
    super::ask::BlockAsk::new(&p, suspect).unwrap()
}

/// A service as `service_write` composes it: `disk-check`, every hour, no network.
pub(super) fn service_manifest() -> clio_toolhost::services::Manifest {
    clio_toolhost::services::Manifest {
        name: "disk-check".into(),
        description: "Checks the free space of / every hour and says when it is under 10%.".into(),
        schedule: clio_toolhost::services::Schedule {
            every: "1h".into(),
            at: None,
        },
        command: "bash check.sh".into(),
        verify: "bash check-run.sh . 'bash check.sh' | grep -q '^ok'".into(),
        network: vec![],
        secrets: vec![],
        writes: vec![],
        limits: clio_toolhost::services::DEFAULT_LIMITS,
    }
}

pub(super) const SERVICE_SCRIPT: &str = "#!/bin/bash\nused=$(df --output=pcent / | tail -1 | tr -dc 0-9)\necho \"$used\" > \"$CLIO_STATE_DIR/last\"\nif [ \"$used\" -gt 90 ]; then echo \"/ is $used% full\"; exit 10; fi\necho \"/ is $used% full\"\n";

pub(super) const SKILL_DESCRIPTION: &str =
    "Reports which folders take the disk. Use when the person asks what fills the disk.";

/// A skill as `skill_write` composes it, with these steps.
pub(super) fn skill_md(steps: &str) -> String {
    clio_toolhost::skills::compose(
        "disk-report",
        SKILL_DESCRIPTION,
        steps,
        "bash scripts/report.sh samples | grep -q big",
    )
}

/// A skill's approval box on screen, after the quiet second, scrolled `scroll` rows.
fn approve_skill(a: &mut App, b: super::ask::BlockAsk, scroll: usize) {
    approve_block(a, b);
    let c = a.confirm.as_mut().unwrap();
    c.id = "e5f6a7b8".into();
    c.workspace = "«skill» register the skill «disk-report» (2 files)".into();
    c.block.as_mut().unwrap().scroll = scroll;
}

/// Its approval box on screen, after the quiet second.
fn approve_block(a: &mut App, b: super::ask::BlockAsk) {
    let mut c = Confirm::new(
        Action::Approve,
        "c3d4e5f6".into(),
        0,
        "«memory» add in your block «vigilancia»".into(),
    );
    c.session = "sid".into();
    c.block = Some(b);
    a.confirm = Some(c);
    a.now_ms = 300_000;
    a.last_key_ms = Some(0);
}

/// A text of the longest kind a block entry takes: 1200 characters.
pub(super) const LONG_ENTRY: &str = "Watch the disk of / and of /var/lib/docker every hour; when either passes 90 % tell me at once, with the five folders that grew the most since the day before, how much each grew, and which service writes there. Do not delete anything to make room: propose what to delete and wait for my yes. Also watch the backups of the orders database: each night's copy must exist by 04:00, weigh within 20 % of the night before, and open with pg_restore --list; if any of those fails, tell me in the morning summary with the file name and the size. Watch the certificates of the three public domains: when one has fewer than 14 days left, tell me which one and whether certbot's timer ran in the last day. Watch the memory of the java process of the game server: above 3 GB for more than ten minutes, tell me with its uptime and the number of players. Watch failed logins over ssh: more than 30 from one address in an hour, tell me the address and whether fail2ban already banned it. Watch that the orders service answers on its health endpoint every five minutes; two failures in a row, tell me with the last lines of its log. Keep each warning to one short paragraph and never repeat one within six hours.";

/// Clio asks (`ask_person`): within the quiet second or after it, and with `Other…` typed.
fn question(a: &mut App, quiet: bool, typed: Option<&str>) {
    question_with(a, &super::tests_ask::OPTIONS, quiet, typed);
}

fn question_with(a: &mut App, answers: &[&str], quiet: bool, typed: Option<&str>) {
    let mut options: Vec<String> = answers.iter().map(|s| (*s).to_owned()).collect();
    options.push(super::ask::OTHER.into());
    let mut q = super::ask::Question::from_select(
        "q1",
        "sid",
        "The orders service needs a database. Which one should I set up?",
        &options,
        Some(600_000),
        299_900,
    )
    .unwrap();
    a.now_ms = 300_000;
    a.last_key_ms = Some(if quiet { 0 } else { 299_800 });
    if quiet {
        q.shown_ms = 0;
        q.sel = 1;
    }
    if let Some(t) = typed {
        q.ready = true;
        q.typing = Some(t.into());
    }
    a.prompt = Some(super::ask::Prompt::Question(q));
}

/// Every state, by name.
pub(super) fn states() -> Vec<(&'static str, App)> {
    let mut out = vec![
        ("empty draft, on this server", pi(true)),
        ("empty draft, in a copy", pi(false)),
        ("conversation", conversation(true)),
    ];
    let mut a = conversation(true);
    a.chat.start_turn(188_000);
    a.chat
        .items
        .push(Item::User("and also look at /home".into()));
    a.chat.items.push(Item::Assistant {
        text: "Looking at **/home** now; with `du -sh`".into(),
        live: true,
    });
    a.chat.set_input(
        "better only /home/ivan, and tell me how much each first-level folder weighs".into(),
    );
    out.push(("turn running, typing", a));
    let mut a = conversation(true);
    a.screen = Screen::Sessions;
    a.sessions.sel = 2;
    out.push(("sessions open", a));
    let mut a = conversation(true);
    approve(&mut a, false);
    out.push(("set-aside question, within the second", a));
    let mut a = conversation(true);
    approve(&mut a, true);
    a.confirm.as_mut().unwrap().armed = true;
    out.push(("set-aside question, after the second, armed", a));
    for (name, quiet, typed) in [
        ("Clio asks, within the second", false, None),
        ("Clio asks, second option picked", true, None),
        (
            "Clio asks, Other… typed",
            true,
            Some("MariaDB, the one ops already runs"),
        ),
    ] {
        let mut a = conversation(false);
        question(&mut a, quiet, typed);
        out.push((name, a));
    }
    let mut a = conversation(false);
    question_with(&mut a, &super::tests_ask::LONG, true, None);
    out.push(("Clio asks, a long description", a));
    let mut a = conversation(false);
    a.chat.start_turn(188_000);
    a.chat.todo = super::todo::parse(
        &[
            "[x] read the orders service config",
            "[x] find where the connection pool is built",
            "[>] move the pool size to the config file",
            "[ ] update the tests that build a pool",
            "[ ] run the whole test suite",
            "[ ] write down what changed",
            "[ ] tell the person what to restart",
        ]
        .map(String::from),
    );
    out.push(("checklist", a));
    let mut a = conversation(true);
    a.host = true;
    a.chat.jobs = 2;
    a.chat.items.push(super::chat::Item::Note(
        "2 background commands are still running; all stop when the session ends".into(),
    ));
    out.push(("background commands", a));
    // The reviewer: running, a pass, the findings of a failure, and the turn that answers them.
    let mut a = conversation(true);
    a.chat.review.running = Some(a.now_ms - 48_000);
    out.push(("reviewing in the background", a));
    let mut a = conversation(true);
    a.now_ms = 10 * 3_600_000;
    let (a, _) = super::tests::feed(
        a,
        vec![super::tests_review::reviewed(
            super::tests_review::PASSES,
            None,
        )],
    );
    out.push(("review passes", a));
    let fails = || {
        let mut a = conversation(true);
        a.now_ms = 10 * 3_600_000;
        a.chat.start_turn(a.now_ms);
        let r = super::tests_review::reviewed(super::tests_review::FAILS, None);
        super::tests::feed(a, vec![r]).0
    };
    out.push(("review fails, its findings", fails()));
    let (a, _) = super::tests::feed(fails(), vec![super::tests_continuous::settled(Some(1000))]);
    out.push(("the turn started by the review", a));
    for (name, typed, suspect) in [
        ("Clio asks for a secret, empty", 0, false),
        ("Clio asks for a secret, with characters", 23, true),
    ] {
        let mut a = conversation(false);
        let mut s = super::ask::SecretAsk::new(
            "ab12cd34",
            "sid",
            (
                "linear",
                "http_fetch:api.linear.app",
                "to read the open issues of this sprint",
            ),
            suspect,
        );
        a.now_ms = 300_000;
        a.last_key_ms = Some(0);
        s.shown_ms = 0;
        s.ready = typed > 0;
        (0..typed).for_each(|_| {
            s.value.push('x');
        });
        a.prompt = Some(super::ask::Prompt::Secret(s));
        out.push((name, a));
    }
    for (name, args, suspect) in [
        (
            "block change, add",
            serde_json::json!({"name": "vigilancia", "action": "add", "text": "Watch the disk of /: tell me when it passes 90 %."}),
            false,
        ),
        (
            "block change, replace (the old entry shown)",
            serde_json::json!({"name": "preferencias", "action": "replace", "id": 12, "text": "Call me Ivan, informally.", "old": "Address me formally, as Mr. Montes."}),
            false,
        ),
        (
            "block change, a long text",
            serde_json::json!({"name": "vigilancia", "action": "add", "text": LONG_ENTRY}),
            false,
        ),
        (
            "block change, remove, suspect session",
            serde_json::json!({"name": "vigilancia", "action": "remove", "id": 31, "old": "Watch the backups of the orders database."}),
            true,
        ),
    ] {
        let mut a = conversation(true);
        approve_block(&mut a, block_ask(args, suspect));
        out.push((name, a));
    }
    let short = skill_md(
        "1. Run `bash scripts/report.sh /`.\n2. Say the five biggest folders and their size.",
    );
    let long = skill_md(
        &(1..=40)
            .map(|i| format!("{i}. Step {i}: measure the folders of level {i} with `du -xh --max-depth=1` and keep the result."))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let script = [(
        "scripts/report.sh",
        "#!/bin/bash\ndu -xh --max-depth=1 \"$1\" | sort -rh | head -n 5\n",
    )];
    let outside = "this session read content from outside";
    for (name, md, reason, suspect, scroll) in [
        (
            "skill waits for approval, marked session",
            &short,
            outside,
            false,
            0,
        ),
        (
            "skill waits for approval, a long text scrolled",
            &long,
            outside,
            false,
            12,
        ),
        (
            "skill waits for approval, its check could not run",
            &short,
            "its check could not be run here: it needs the network, which a review does not have",
            false,
            0,
        ),
        (
            "skill waits for approval, suspect session",
            &short,
            "this session read content that tried to give orders",
            true,
            0,
        ),
    ] {
        let mut a = conversation(true);
        approve_skill(&mut a, skill_ask(md, &script, reason, suspect), scroll);
        out.push((name, a));
    }
    let mut a = conversation(true);
    super::review::ended(
        &mut a,
        "sid",
        Ok(Box::new(crate::cli::chat::Reviewed {
            answer: clio_brain::pi::review::parse(
                "```review\nverdict: passes\nran: bash scripts/report.sh samples | grep -q big (exit 0)\nfindings: none\n```",
            ),
            model: "claude-opus-5-5".into(),
            fallback: None,
            secs: 41,
            commands: 2,
            marked: None,
            full: None,
            usage: clio_brain::pi::rpc::Usage::default(),
            skill: Some(crate::cli::chat::SkillOutcome::Registered {
                name: "disk-report".into(),
                description: SKILL_DESCRIPTION.into(),
                n: 1,
                notes: None,
            }),
        })),
    );
    out.push(("skill registered", a));
    let mut a = conversation(true);
    let noted = "```review\nverdict: passes\nran: bash scripts/report.sh samples | grep -q big (exit 0)\nfindings:\n- what: a folder name with spaces is not quoted\n  evidence: scripts/report.sh line 2: du -sh $1\n- what: it does not say what happens on an empty folder\n  evidence: ran it on an empty mktemp -d: no output, exit 0\n```";
    let answer = clio_brain::pi::review::parse(noted);
    super::review::ended(
        &mut a,
        "sid",
        Ok(Box::new(crate::cli::chat::Reviewed {
            skill: Some(crate::cli::chat::SkillOutcome::Registered {
                name: "disk-report".into(),
                description: SKILL_DESCRIPTION.into(),
                n: 2,
                notes: Some(answer.for_model()),
            }),
            answer,
            model: "claude-opus-5-5".into(),
            fallback: None,
            secs: 63,
            commands: 4,
            marked: None,
            full: Some("~/.local/share/clio/runs/20261118-2210/reviews/3/answer.txt".into()),
            usage: clio_brain::pi::rpc::Usage::default(),
        })),
    );
    out.push(("skill registered, the review left notes", a));
    let mut a = conversation(true);
    a.screen = Screen::Sessions;
    a.sessions.sel = 2;
    // As the drawer builds it: the plan of the delete comes back, the dialog asks.
    let (a, _) = super::app::update(
        a,
        super::app::Input::Bg(super::app::Bg::SessionPlan(Ok((
            "/s/2_b.jsonl".into(),
            "What weighs the most on the disk?".into(),
            "the conversation and its working copy".into(),
        )))),
    );
    assert_eq!(
        a.confirm.as_ref().map(|c| c.action),
        Some(Action::DeleteSession)
    );
    out.push(("delete a session", a));
    let mut a = conversation(true);
    a.chat.items.truncate(1);
    a.chat.items.push(Item::Tool {
        id: "4".into(),
        name: "bash".into(),
        summary: "sudo rm -r /var/lib/docker/volumes/old".into(),
        reason: "Delete old volumes".into(),
        state: ToolState::SetAside,
        ms: Some(30),
    });
    a.chat.items.push(Item::Tool {
        id: "5".into(),
        name: "bash".into(),
        summary: "ps -eo pid,comm,rss --sort=-rss | head".into(),
        reason: "See which processes use the most memory".into(),
        state: ToolState::Ok,
        ms: Some(1_200),
    });
    a.chat.items.push(Item::Assistant {
        text: TABLE.into(),
        live: false,
    });
    a.chat.items.push(Item::Note(
        "approved a1b2: sudo rm -r /var/lib/docker/volumes/old (exit 0)".into(),
    ));
    out.push(("table and a set-aside action", a));
    let mut a = pi(true);
    a.chat = Chat::opening(1, 0);
    a.chat.opening = false;
    a.chat.title = "What weighs the most on the disk?".into();
    a.chat.failed = Some("could not open the session: Pi exited while starting (code 1)".into());
    out.push(("error on open", a));
    // `clio` reopened the last conversation: its file, with more before it and a compaction in it.
    let mut a = pi(true);
    a.chat = Chat::opening(1, 0);
    a.chat.title = "Which root folders weigh most on this server?".into();
    let m = super::tests_continuous::message;
    a.chat.add_page(
        &[
            m("user", "Is nginx still answering on 443?"),
            m(
                "assistant",
                "Yes: **nginx** answers on 443 with the new certificate.",
            ),
            serde_json::json!({"type": "compaction", "summary": "s", "firstKeptEntryId": "x"}),
            m("user", "Which root folders weigh most on this server?"),
            m(
                "assistant",
                "Almost all of it is in `/var/lib`: **docker** 56G, containerd 39G.",
            ),
        ],
        Some(40),
    );
    out.push(("reopened conversation, earlier messages and a summary", a));
    // The turn ended over the cap: Clio compacts by itself.
    let over = || {
        let mut a = conversation(true);
        a.chat.running = true;
        super::app::update(a, super::tests_continuous::settled(Some(246_000))).0
    };
    out.push(("automatic compaction over the cap", over()));
    let (a, _) = super::app::update(
        over(),
        super::app::Input::Bg(super::app::Bg::Compacted(
            Ok("context compacted: from 246k to about 212k tokens".into()),
            Some(212_000),
            None,
        )),
    );
    out.push(("automatic compaction, still over the cap", a));
    let mut a = pi(false);
    a.home.header.as_mut().unwrap().key = KeyState::Missing;
    a.home.doctor = Some((30, 2, 2));
    a.debug = true;
    out.push(("something is wrong: key, doctor, debug", a));
    out.extend(command_states());
    out
}

/// The slash commands: the menu, a command's choices, the boxes.
fn command_states() -> Vec<(&'static str, App)> {
    use super::commands::{self, Choice, Custom, Menu, Modal, Run, Target};
    let menu = |input: &str| {
        let mut a = conversation(true);
        a.chat.set_input(input.into());
        a.chat.menu = Some(Menu::default());
        a.chat.customs = vec![Custom {
            name: "review".into(),
            what: "Review a file carefully".into(),
            body: "Review $ARGUMENTS".into(),
        }];
        a
    };
    let mut out = vec![("command menu", menu("/")), ("filtered menu", menu("/co"))];
    let choice = |value: &str, label: &str, current: bool, blocked: Option<&str>| Choice {
        value: value.into(),
        label: label.into(),
        blocked: blocked.map(str::to_owned),
        current,
    };
    let mut a = menu("/effort");
    let levels = ["off", "minimal", "low", "medium", "high"]
        .iter()
        .map(|l| choice(l, l, *l == "low", None))
        .collect();
    a.chat.menu.as_mut().unwrap().pick = Some((Run::Effort, Some(levels)));
    a.chat.menu.as_mut().unwrap().sel = 2;
    out.push(("pick the effort", a));
    let mut a = menu("/connector");
    a.chat.menu.as_mut().unwrap().pick = Some((
        Run::Connector,
        Some(vec![
            choice("opencode", "opencode · beam-free", false, None),
            choice(
                "claude",
                "claude · claude-sonnet-5-5",
                false,
                Some("no key: clio connect claude"),
            ),
            choice(
                "anthropic-api",
                "anthropic-api · claude-sonnet-5-5",
                true,
                None,
            ),
        ]),
    ));
    out.push(("pick the connector", a));
    let mut a = conversation(true);
    commands::start(&mut a, Target::Builtin(Run::Usage), "");
    out.push(("/usage", a));
    let mut a = conversation(true);
    commands::start(&mut a, Target::Builtin(Run::Context), "");
    out.push(("/context with the cap", a));
    let mut a = conversation(true);
    let mut m = Modal::text(
        "Pending",
        vec![
            "a1b2  «delete»  sudo rm -r /var/lib/docker/volumes/old  · since 2026-11-18 10:02 UTC · expires 2026-11-19 10:02 UTC".into(),
            "c3d4  «control»  systemctl restart nginx  · since 2026-11-18 10:05 UTC · expires 2026-11-19 10:05 UTC".into(),
        ],
    );
    m.picks = vec![
        (
            "a1b2".into(),
            "«delete» sudo rm -r /var/lib/docker/volumes/old".into(),
        ),
        ("c3d4".into(), "«control» systemctl restart nginx".into()),
    ];
    m.foot = Some("and 1 from other sessions: clio pending in a terminal".into());
    a.modal = Some(m);
    out.push(("/pending", a));
    let mut a = conversation(true);
    let mut m = Modal::text(
        "skills",
        vec![
            "«disk-report» is off: from the next session".into(),
            String::new(),
            "disk-report [off (turned off by the person)] · v2 passes 20261118: Reports which folders take the disk. Use when the person asks what fills the disk.".into(),
            "skill-creator [on] · v1 shipped with Clio 20261118: How to write or fix one of Clio's skills with skill_write. Use when the person asks for a skill.".into(),
            "pdf-to-text [on] · v3 passes, approved 20261117 · 2 failed in a row: Converts PDF files to plain text. Use when the person wants the text of a PDF.".into(),
        ],
    );
    m.foot = Some("/skills off|on|undo|remove <name>".into());
    a.modal = Some(m);
    out.push(("/skills", a));
    services_states(&mut out);
    out
}

/// Services (S-69): the Approval box (short; long and scrolled; with hosts, secrets and write
/// paths), the note once registered, `/services`, and the notices at chat open.
fn services_states(out: &mut Vec<(&'static str, App)>) {
    let notes =
        "verdict: passes\nran: bash check-run.sh . 'bash check.sh' (exit 0)\nfindings: none";
    let m = service_manifest();
    let mut a = conversation(true);
    approve_service(
        &mut a,
        service_ask(&m, &[("check.sh", SERVICE_SCRIPT)], "passes", notes, false),
        0,
    );
    out.push(("service waits for approval, short", a));
    let long: String = (1..=40)
        .map(|i| format!("echo \"step {i}: look at the folder of level {i}\"\n"))
        .collect();
    let mut a = conversation(true);
    approve_service(
        &mut a,
        service_ask(
            &m,
            &[("check.sh", SERVICE_SCRIPT), ("steps.sh", &long)],
            "could_not_check",
            "verdict: could_not_check\nran: the check needs a disk over 90% full\nfindings: none",
            false,
        ),
        14,
    );
    out.push(("service waits for approval, long and scrolled", a));
    let mut m = service_manifest();
    m.name = "uptime-report".into();
    m.description =
        "Every day at 07:30, posts yesterday's uptime of the sites to the status page.".into();
    m.schedule = clio_toolhost::services::Schedule {
        every: "1d".into(),
        at: Some("07:30".into()),
    };
    m.command = "bash report.sh".into();
    m.network = vec!["api.statuspage.io".into(), "example.com".into()];
    m.secrets = vec![clio_toolhost::services::SecretUse {
        name: "statuspage-key".into(),
        env: "STATUSPAGE_KEY".into(),
        host: "api.statuspage.io".into(),
    }];
    m.writes = vec!["/home/ivan/reports/uptime".into()];
    m.limits = clio_toolhost::services::Limits {
        cpu_percent: 25,
        memory_mb: 256,
        seconds: 120,
    };
    let mut a = conversation(true);
    approve_service(
        &mut a,
        service_ask(
            &m,
            &[(
                "report.sh",
                "#!/bin/bash\ncurl -sf https://example.com/ > /dev/null && echo up\n",
            )],
            "passes",
            "verdict: passes\nran: bash check-run.sh . 'bash report.sh' (exit 0)\nfindings:\n1. it does not say what happens when the site is down\n   evidence: report.sh line 2",
            true,
        ),
        0,
    );
    out.push((
        "service waits for approval, with hosts, secrets and write paths",
        a,
    ));
    let (a, _) = super::app::update(
        conversation(true),
        super::app::Input::Bg(super::app::Bg::ChatAnswered(Ok(
            "approved s1a2b3c4: the service «disk-check» is registered (version 1): it runs every 1h".into(),
        ))),
    );
    out.push(("service registered", a));
    // `/services` from a real folder: one fine, one paused, one waiting for the vault.
    let t = tempfile::tempdir().unwrap();
    let d = crate::cli::home::Dirs {
        config: t.path().join("config"),
        data: t.path().join("data"),
        runs: t.path().join("data/runs"),
        runtime: None,
        home: t.path().join("home"),
    };
    let root = crate::cli::services::root(&d);
    let now = crate::cli::now_ms();
    for (name, every, enabled, off, runs) in [
        (
            "disk-check",
            "1h",
            true,
            None,
            serde_json::json!([{"start_ms": now - 1_200_000, "ms": 310, "version": 1, "outcome": "ok", "exit": 0, "tail": "/ is 61% full", "by": "timer"}]),
        ),
        (
            "backup-notes",
            "1d",
            false,
            Some("paused: three failed runs in a row"),
            serde_json::json!([{"start_ms": now - 3_600_000, "ms": 2100, "version": 2, "outcome": "failed", "exit": 23, "tail": "rsync: connection unexpectedly closed", "by": "timer"}]),
        ),
        (
            "uptime-report",
            "15m",
            true,
            None,
            serde_json::json!([{"start_ms": now - 60_000, "ms": 0, "version": 1, "outcome": "waiting", "why": "waiting for the vault (it is locked)", "by": "timer"}]),
        ),
    ] {
        let mut m = service_manifest();
        m.name = name.into();
        m.schedule.every = every.into();
        let v = root.join("versions").join(name).join("1");
        std::fs::create_dir_all(&v).unwrap();
        std::fs::write(v.join("service.json"), clio_toolhost::services::compose(&m)).unwrap();
        std::fs::create_dir_all(root.join("records")).unwrap();
        std::fs::write(root.join("records").join(format!("{name}.json")), serde_json::json!({"name": name, "description": m.description, "enabled": enabled, "off": off, "current": 1, "versions": [{"n": 1, "sha256": "x", "at_ms": now - 86_400_000, "by": "person", "verdict": "passes, approved"}]}).to_string()).unwrap();
        std::fs::create_dir_all(root.join("runs")).unwrap();
        std::fs::write(
            root.join("runs").join(format!("{name}.json")),
            runs.to_string(),
        )
        .unwrap();
    }
    std::fs::write(root.join("notices.json"), serde_json::json!([{"at_ms": now - 3_600_000, "name": "backup-notes", "text": "paused: three failed runs in a row; clio services logs backup-notes shows why", "seen": true}]).to_string()).unwrap();
    let mut a = conversation(true);
    a.modal = Some(crate::cli::services::chat_command(&d, "").unwrap());
    out.push((
        "/services: one fine, one paused, one waiting for the vault",
        a,
    ));
    let mut a = pi(true);
    for n in [
        "service «backup-notes»: paused: three failed runs in a row; clio services logs backup-notes shows why",
        "service «disk-check»: needs your attention: / is 93% full",
    ] {
        a.chat.items.push(Item::Note(n.into()));
    }
    out.push(("services' notices when a chat opens", a));
}

/// A service's approval box on screen, after the quiet second, scrolled `scroll` rows.
fn approve_service(a: &mut App, b: super::ask::BlockAsk, scroll: usize) {
    approve_block(a, b);
    let c = a.confirm.as_mut().unwrap();
    c.id = "s1a2b3c4".into();
    c.workspace =
        "«service · a service always waits for your approval» register the service «disk-check»"
            .into();
    c.block.as_mut().unwrap().scroll = scroll;
}

/// The screen as it looks: a wide character takes two columns, so the cell it covers is left out.
pub(super) fn screen_text(a: &App, w: u16, h: u16) -> String {
    let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
    t.draw(|f| super::app::draw(f, a)).unwrap();
    let buf = t.backend().buffer();
    let mut out = String::new();
    for y in 0..h {
        let mut skip = 0;
        for x in 0..w {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let s = buf[(x, y)].symbol();
            skip = super::text::width(s).saturating_sub(1);
            out.push_str(s);
        }
        out.push('\n');
    }
    out
}

#[test]
#[ignore = "prints the chat's states; not a check"]
fn tui_gallery() {
    for (name, a) in states() {
        for (w, h) in [(100, 30), (64, 18)] {
            println!("==== {name} ({w}x{h})");
            println!("{}", screen_text(&a, w, h));
        }
    }
    let mut a = super::tests::with_runs();
    for (name, s) in [("home", Screen::Home), ("chat help", Screen::Help)] {
        a.screen = s;
        if s == Screen::Help {
            a = pi(true);
            a.open_help();
        }
        println!("==== {name} (100x30)");
        println!("{}", screen_text(&a, 100, 30));
    }
    println!("==== chat help, whole (100x90)");
    println!("{}", screen_text(&a, 100, 90));
}

// Every state of the chat, at the sizes of the gallery and the minimum: the exact shape of the
// terminal, nothing raw from outside, and in ASCII when the terminal has no Unicode.
#[test]
fn every_chat_state_fits_is_clean_and_has_an_ascii_form() {
    for (name, mut a) in states() {
        for (w, h) in [(100, 30), (64, 18), (60, 16), (200, 50)] {
            // Measured in columns: a wide character (日, ❤️) is one symbol over two cells.
            let t = screen_text(&a, w, h);
            assert_eq!(t.lines().count(), usize::from(h), "{name}\n{t}");
            assert!(
                t.lines().all(|l| super::text::width(l) == usize::from(w)),
                "{name} {w}x{h}\n{t}"
            );
            super::tests::assert_clean(&t);
            assert!(
                t.lines().nth(1).unwrap().chars().all(|c| c == '─'),
                "{name}\n{t}"
            );
        }
        a.theme = super::theme::Theme {
            color: false,
            unicode: false,
            rgb: false,
        };
        let t = super::tests::screen_text(&a, 100, 30);
        let odd: Vec<char> = t
            .chars()
            .filter(|c| {
                !c.is_ascii()
                    && !"…·«»".contains(*c)
                    && !ANSWER.contains(*c)
                    && !TABLE.contains(*c)
                    && !rows().iter().any(|r| r.title.contains(*c))
            })
            .collect();
        assert!(odd.is_empty(), "{name}: {odd:?}\n{t}");
    }
}

// The chat uses the whole width: one column of margin on each side, and with the drawer open it
// starts right after the drawer.
#[test]
fn the_chat_spans_the_width_minus_the_margins() {
    let corners = |t: &str| -> (usize, usize) {
        let l = t.lines().rev().find(|l| l.contains('╭')).unwrap();
        let left = l.chars().position(|c| c == '╭').unwrap();
        let right = l.chars().position(|c| c == '╮').unwrap();
        (left, right)
    };
    let mut a = conversation(true);
    a.chat.items.push(Item::User("word ".repeat(60)));
    let t = screen_text(&a, 160, 40);
    // The person's message wraps at the full width (the old column stopped at 96), not at it.
    let widest = t
        .lines()
        .filter(|l| l.contains("word"))
        .map(|l| super::text::width(l.trim_end()))
        .max()
        .unwrap();
    assert!((140..=158).contains(&widest), "{widest}\n{t}");
    assert_eq!(corners(&t), (1, 158), "{t}");

    a.screen = Screen::Sessions;
    let t = screen_text(&a, 160, 40);
    let d = usize::from(super::sessions::DRAWER_W);
    assert_eq!(corners(&t), (d + 1, 158), "{t}");
}
