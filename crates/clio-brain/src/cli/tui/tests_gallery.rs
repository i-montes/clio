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

pub(super) const ANSWER: &str = "## Lo que pesa\n\nCasi todo está en `/var/lib`:\n\n- **docker**: 56G, imágenes viejas y *volúmenes*\n- containerd: 39G\n  - snapshots: 30G\n\n1. Revisa con `docker system df`.\n2. Lee [la guía](https://docs.docker.com/config/pruning/).\n\n> Ojo: no borré nada.\n\n```sh\nsudo du -xh --max-depth=1 /var/lib 2>/dev/null | sort -rh | head -n 20\n```\n\nAñoro 日本語 y ❤\u{fe0f} sin romper la línea.";

pub(super) const TABLE: &str = "Los que más memoria usan:\n\n| PID | Proceso | Memoria (RSS) | Qué es |\n|---|---|---|---|\n| 4071 | java | ~678 MB | el servidor de Minecraft, con su mundo cargado en memoria |\n| 812 | dockerd | 96 MB | Docker |\n| 1 | systemd | 12 MB | 日本語 ❤\u{fe0f} |\n\nAparté el borrado de los volúmenes: espera tu visto bueno.";

/// The Pi chat with a model and nothing wrong, as `clio` opens.
pub(super) fn pi(host: bool) -> App {
    let mut a = App::new("/home/ivan/proyecto", THEME);
    a.use_pi();
    a.host = host;
    a.home.header = Some(Header {
        model: "claude-sonnet-5-5".into(),
        thinking: "low".into(),
        key: KeyState::Present,
        connector: "anthropic".into(),
        vault: "activa".into(),
        config_error: None,
        limits: RunLimits::default(),
        window: 200_000,
        window_known: true,
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
            "¿Qué carpetas de root pesan más en este servidor?",
            "/s/1_sid.jsonl",
            3,
            4,
        ),
        row(
            "¿Qué es lo que más pesa en el disco?",
            "/s/2_b.jsonl",
            26,
            3,
        ),
        Row {
            unavailable: Some("no es de Clio".into()),
            ..row("una sesión de Pi a mano", "/s/3_c.jsonl", 60 * 5, 9)
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
        "¿Qué carpetas de root pesan más en este servidor (por ejemplo bajo /var/lib)? Solo mira, no borres ni cambies nada.".into(),
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
        "Medir carpetas más pesadas",
        "sudo du -xh --max-depth=1 / 2>/dev/null | sort -rh | head -n 20",
        ToolState::Failed,
        Some(120_000),
    ));
    c.items.push(tool(
        "2",
        "Medir por partes",
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
        "pedido juzgado como nivel 1 (solo mirar)".into(),
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
        "«borrar» rm -r /var/lib/docker/volumes/viejo (borra 3 carpetas)".into(),
    );
    c.session = "sid".into();
    a.confirm = Some(c);
    a.now_ms = 300_000;
    a.last_key_ms = Some(if quiet { 0 } else { 299_800 });
}

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
        ("borrador vacío, en este servidor", pi(true)),
        ("borrador vacío, en una copia", pi(false)),
        ("conversación", conversation(true)),
    ];
    let mut a = conversation(true);
    a.chat.start_turn(188_000);
    a.chat.items.push(Item::User("y además mira /home".into()));
    a.chat.items.push(Item::Assistant {
        text: "Miro **/home** ahora; con `du -sh`".into(),
        live: true,
    });
    a.chat
        .set_input("mejor solo /home/ivan, y dime cuánto pesa cada carpeta de primer nivel".into());
    out.push(("turno en marcha, escribiendo", a));
    let mut a = conversation(true);
    a.screen = Screen::Sessions;
    a.sessions.sel = 2;
    out.push(("sesiones abiertas", a));
    let mut a = conversation(true);
    approve(&mut a, false);
    out.push(("pregunta apartada, dentro del segundo", a));
    let mut a = conversation(true);
    approve(&mut a, true);
    a.confirm.as_mut().unwrap().armed = true;
    out.push(("pregunta apartada, después del segundo, con s", a));
    for (name, quiet, typed) in [
        ("Clio pregunta, dentro del segundo", false, None),
        ("Clio pregunta, segunda opción elegida", true, None),
        (
            "Clio pregunta, Other… escrito",
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
    out.push(("Clio pregunta, una descripción larga", a));
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
    out.push(("lista de pasos", a));
    let mut a = conversation(true);
    a.host = true;
    a.chat.jobs = 2;
    a.chat.items.push(super::chat::Item::Note(
        "2 background commands are still running; all stop when the session ends".into(),
    ));
    out.push(("comandos en segundo plano", a));
    for (name, typed, suspect) in [
        ("Clio pide un secreto, vacío", 0, false),
        ("Clio pide un secreto, con caracteres", 23, true),
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
    let mut a = conversation(true);
    a.screen = Screen::Sessions;
    a.sessions.sel = 2;
    a.confirm = Some(Confirm::new(
        Action::DeleteSession,
        "/s/2_b.jsonl".into(),
        0,
        "«¿Qué es lo que más pesa en el disco?»: la conversación y su copia de trabajo".into(),
    ));
    out.push(("borrar una sesión", a));
    let mut a = conversation(true);
    a.chat.items.truncate(1);
    a.chat.items.push(Item::Tool {
        id: "4".into(),
        name: "bash".into(),
        summary: "sudo rm -r /var/lib/docker/volumes/viejo".into(),
        reason: "Borrar volúmenes viejos".into(),
        state: ToolState::SetAside,
        ms: Some(30),
    });
    a.chat.items.push(Item::Tool {
        id: "5".into(),
        name: "bash".into(),
        summary: "ps -eo pid,comm,rss --sort=-rss | head".into(),
        reason: "Ver qué procesos usan más memoria".into(),
        state: ToolState::Ok,
        ms: Some(1_200),
    });
    a.chat.items.push(Item::Assistant {
        text: TABLE.into(),
        live: false,
    });
    a.chat.items.push(Item::Note(
        "aprobado a1b2: sudo rm -r /var/lib/docker/volumes/viejo (salida 0)".into(),
    ));
    out.push(("tabla y una acción apartada", a));
    let mut a = pi(true);
    a.chat = Chat::opening(1, 0);
    a.chat.opening = false;
    a.chat.title = "¿Qué es lo que más pesa en el disco?".into();
    a.chat.failed = Some("no se pudo abrir la sesión: Pi terminó al arrancar (código 1)".into());
    out.push(("error al abrir", a));
    let mut a = pi(false);
    a.home.header.as_mut().unwrap().key = KeyState::Missing;
    a.home.doctor = Some((30, 2, 2));
    a.debug = true;
    out.push(("algo anda mal: llave, doctor, debug", a));
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
            name: "revisa".into(),
            what: "Revisa un archivo con cuidado".into(),
            body: "Revisa $ARGUMENTS".into(),
        }];
        a
    };
    let mut out = vec![
        ("menú de comandos", menu("/")),
        ("menú filtrado", menu("/co")),
    ];
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
    out.push(("elegir el esfuerzo", a));
    let mut a = menu("/connector");
    a.chat.menu.as_mut().unwrap().pick = Some((
        Run::Connector,
        Some(vec![
            choice("opencode", "opencode · beam-free", false, None),
            choice(
                "claude",
                "claude · claude-sonnet-5-5",
                false,
                Some("sin llave: clio connect claude"),
            ),
            choice(
                "anthropic-api",
                "anthropic-api · claude-sonnet-5-5",
                true,
                None,
            ),
        ]),
    ));
    out.push(("elegir el conector", a));
    let mut a = conversation(true);
    commands::start(&mut a, Target::Builtin(Run::Usage), "");
    out.push(("/usage", a));
    let mut a = conversation(true);
    let mut m = Modal::text(
        "Pendientes",
        vec![
            "a1b2  «borrar»  sudo rm -r /var/lib/docker/volumes/viejo  · desde 2026-11-18 10:02 UTC · vence 2026-11-19 10:02 UTC".into(),
            "c3d4  «servicio»  systemctl restart nginx  · desde 2026-11-18 10:05 UTC · vence 2026-11-19 10:05 UTC".into(),
        ],
    );
    m.picks = vec![
        (
            "a1b2".into(),
            "«borrar» sudo rm -r /var/lib/docker/volumes/viejo".into(),
        ),
        ("c3d4".into(), "«servicio» systemctl restart nginx".into()),
    ];
    m.foot = Some("y 1 de otras sesiones: clio pending en una terminal".into());
    a.modal = Some(m);
    out.push(("/pending", a));
    out
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
    for (name, s) in [("inicio", Screen::Home), ("ayuda del chat", Screen::Help)] {
        a.screen = s;
        if s == Screen::Help {
            a = pi(true);
            a.open_help();
        }
        println!("==== {name} (100x30)");
        println!("{}", screen_text(&a, 100, 30));
    }
    println!("==== ayuda del chat, entera (100x90)");
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
                    && !"áéíóúñÁÉÍÓÚÑü¿¡…·«»".contains(*c)
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
    a.chat.items.push(Item::User("palabra ".repeat(60)));
    let t = screen_text(&a, 160, 40);
    // The person's message wraps at the full width (the old column stopped at 96), not at it.
    let widest = t
        .lines()
        .filter(|l| l.contains("palabra"))
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
