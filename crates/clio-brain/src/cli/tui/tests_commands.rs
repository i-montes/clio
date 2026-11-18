//! The chat's slash commands: the menu, what each command asks the runtime to do, the person's
//! own commands, and that nothing but the person's typing is ever read as one.

use std::sync::Mutex;

use clio_brain::pi::PiError;
use clio_brain::pi::rpc::{Cancel, Event, RunSummary};
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use serde_json::{Value, json};

use super::app::{Action, App, Bg, Cmd, Confirm, Input, Screen};
use super::chat::{Chat, Item};
use super::commands::{
    self, COMMANDS, CUSTOM_MAX, Choice, Custom, Run, expand, level_name, level_of, load_custom,
};
use super::tests::{app, ch, feed, key, screen_text};
use crate::cli::chat::{Refresh, Session};
use crate::cli::home::{Config, Dirs};

/// The Pi chat with a session open (`sid`), idle, ready to type.
fn open() -> App {
    let mut a = app();
    a.use_pi();
    a.chat = Chat::opening(1, 0);
    a.chat.opened("sid".into(), "/w".into(), &[], None, 0);
    a
}

fn typed(text: &str) -> Vec<Input> {
    text.chars().map(ch).collect()
}

/// Types `text` in the composer and presses Enter.
fn run(a: App, text: &str) -> (App, Vec<Cmd>) {
    let mut inputs = typed(text);
    inputs.push(key(KeyCode::Enter));
    let (a, cmds) = feed(a, inputs);
    (
        a,
        cmds.into_iter()
            .filter(|c| *c != Cmd::LoadCommands)
            .collect(),
    )
}

#[test]
fn slash_opens_the_menu_that_filters_moves_completes_and_closes_leaving_the_text() {
    let (a, cmds) = feed(open(), vec![ch('/')]);
    assert_eq!(cmds, vec![Cmd::LoadCommands]);
    assert!(a.chat.menu.is_some());
    let s = screen_text(&a, 100, 30);
    assert!(
        s.contains("/modelo [nombre]") && s.contains("cambia el modelo"),
        "{s}"
    );
    // At most eight rows: the rest scrolls.
    assert!(!s.contains("/salir"), "{s}");

    // The filter: the Spanish name, the English alias, the description.
    let names = |a: &App, f: &str| -> Vec<String> {
        commands::matches(a, f)
            .into_iter()
            .map(|e| match e {
                commands::Entry::Builtin(i) => COMMANDS[i].name.to_owned(),
                commands::Entry::Custom(i) => a.chat.customs[i].name.clone(),
            })
            .collect()
    };
    assert_eq!(names(&a, "es")[0], "esfuerzo");
    assert_eq!(names(&a, "undo"), vec!["deshacer"]);
    assert!(names(&a, "tokens").contains(&"uso".to_owned()));

    let (a, _) = feed(a, typed("es"));
    let (a, _) = feed(
        a,
        vec![key(KeyCode::Down), key(KeyCode::Up), key(KeyCode::Tab)],
    );
    assert_eq!(a.chat.input, "/esfuerzo ");
    assert!(a.chat.menu.is_some());
    let (a, cmds) = feed(a, vec![key(KeyCode::Esc)]);
    assert!(cmds.is_empty() && a.chat.menu.is_none());
    assert_eq!(a.chat.input, "/esfuerzo ");
    // Deleting the slash is not a command any more.
    let (a, _) = feed(open(), vec![ch('/'), key(KeyCode::Backspace)]);
    assert!(a.chat.menu.is_none());
    // Only a slash typed first opens it.
    let (a, _) = feed(open(), typed("a/"));
    assert!(a.chat.menu.is_none());
}

#[test]
fn the_selected_row_runs_on_enter_and_a_click_runs_its_row() {
    let (a, cmds) = run(open(), "/doc");
    assert_eq!(cmds, vec![Cmd::RunDoctor]);
    assert_eq!(a.screen, Screen::Doctor);

    let (a, _) = feed(open(), typed("/me"));
    let s = screen_text(&a, 100, 30);
    let y = s.lines().position(|l| l.contains("/memoria")).unwrap();
    let (a, cmds) = super::app::update(
        a,
        Input::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 30,
            row: u16::try_from(y).unwrap(),
            modifiers: crossterm::event::KeyModifiers::NONE,
        }),
    );
    assert_eq!(cmds, vec![Cmd::LoadMemory]);
    assert_eq!(a.screen, Screen::Memory);
}

#[test]
fn an_unknown_command_is_never_sent_and_a_leading_space_makes_a_message() {
    let (a, cmds) = run(open(), "/xyz algo");
    assert!(cmds.is_empty(), "{cmds:?}");
    assert_eq!(a.flash.as_deref(), Some("no existe el comando /xyz"));
    assert_eq!(a.chat.input, "/xyz algo");
    // With the menu closed too.
    let (a, _) = feed(a, vec![key(KeyCode::Esc), key(KeyCode::Enter)]);
    assert_eq!(a.flash.as_deref(), Some("no existe el comando /xyz"));
    assert!(a.chat.items.is_empty());

    let (_, cmds) = run(open(), " /xyz algo");
    assert_eq!(
        cmds,
        vec![Cmd::ChatSend {
            text: "/xyz algo".into()
        }]
    );
}

#[test]
fn each_command_asks_for_its_own_work() {
    let cases: Vec<(&str, Vec<Cmd>)> = vec![
        ("/modelo", vec![Cmd::Choices(Run::Model)]),
        ("/modelo kimi-k3", vec![Cmd::SetModel("kimi-k3".into())]),
        ("/model kimi-k3", vec![Cmd::SetModel("kimi-k3".into())]),
        ("/conector", vec![Cmd::Choices(Run::Connector)]),
        (
            "/connector claude",
            vec![Cmd::SetConnector("claude".into())],
        ),
        ("/esfuerzo", vec![Cmd::Choices(Run::Effort)]),
        ("/esfuerzo alto", vec![Cmd::SetEffort("high".into())]),
        ("/effort max", vec![Cmd::SetEffort("max".into())]),
        ("/esfuerzo muy alto", vec![Cmd::SetEffort("xhigh".into())]),
        ("/nueva", vec![Cmd::ChatClose]),
        ("/clear", vec![Cmd::ChatClose]),
        ("/sesiones", vec![]),
        (
            "/nombre Arreglo del login",
            vec![Cmd::SessionName("Arreglo del login".into())],
        ),
        (
            "/compactar solo el código",
            vec![Cmd::Compact("solo el código".into())],
        ),
        ("/compact", vec![Cmd::Compact(String::new())]),
        ("/contexto", vec![]),
        ("/uso", vec![]),
        ("/cost", vec![]),
        (
            "/pendientes",
            vec![Cmd::LoadPending {
                session: "sid".into(),
            }],
        ),
        ("/acciones", vec![Cmd::LoadActions { run: "sid".into() }]),
        ("/deshacer", vec![]),
        ("/memoria", vec![Cmd::LoadMemory]),
        ("/doctor", vec![Cmd::RunDoctor]),
        ("/ayuda", vec![]),
        ("/salir", vec![]),
        ("/exit", vec![]),
    ];
    for (typed, want) in cases {
        let (a, cmds) = run(open(), typed);
        assert_eq!(cmds, want, "{typed}");
        let word = typed.split(' ').next().unwrap();
        match word {
            "/sesiones" => assert_eq!(a.screen, Screen::Sessions),
            "/contexto" => assert_eq!(a.modal.as_ref().unwrap().title, "Contexto"),
            "/uso" | "/cost" => assert_eq!(a.modal.as_ref().unwrap().title, "Uso"),
            "/ayuda" => assert_eq!(a.screen, Screen::Help),
            "/salir" | "/exit" => assert!(a.quit),
            "/compactar" | "/compact" => assert!(a.chat.compacting),
            "/nueva" | "/clear" => assert!(a.chat.is_draft()),
            "/deshacer" => {
                assert_eq!(a.confirm.as_ref().unwrap().action, Action::UndoSession);
                let (_, cmds) = feed(a, vec![ch('y')]);
                assert_eq!(cmds, vec![Cmd::UndoSession { id: "sid".into() }]);
                continue;
            }
            "/modelo" | "/conector" | "/esfuerzo" if typed == word => {
                assert!(a.chat.menu.as_ref().unwrap().pick.is_some(), "{typed}");
                assert_eq!(a.chat.input, word, "{typed}: the text stays while choosing");
                continue;
            }
            _ => {}
        }
        // A command that ran leaves the composer empty.
        assert!(a.chat.input.is_empty(), "{typed}: {:?}", a.chat.input);
    }
    // Wrong arguments say why and leave the text.
    for typed in ["/esfuerzo enorme", "/nombre"] {
        let (a, cmds) = run(open(), typed);
        assert!(cmds.is_empty(), "{typed}");
        assert!(a.flash.is_some(), "{typed}");
        assert_eq!(a.chat.input, typed);
    }
}

// Review: a word that is not a command ran the highlighted one with the typed arguments
// (`/ hola⏎` saved the model «hola»). With arguments only a whole name or alias runs.
#[test]
fn a_word_with_arguments_runs_only_a_whole_command_name() {
    let draft: fn() -> App = || {
        let mut a = app();
        a.use_pi();
        a
    };
    for make in [open, draft] {
        for typed in ["/ hola", "/m sonnet"] {
            let (a, cmds) = run(make(), typed);
            assert!(cmds.is_empty(), "{typed}: {cmds:?}");
            assert_eq!(a.chat.input, typed);
            let word = typed.split(' ').next().unwrap();
            assert_eq!(
                a.flash.as_deref(),
                Some(format!("no existe el comando {word}").as_str())
            );
        }
    }
    // A prefix still completes with Tab.
    let (a, _) = feed(open(), typed("/mod"));
    let (a, _) = feed(a, vec![key(KeyCode::Tab)]);
    assert!(a.chat.input.starts_with("/mod") && a.chat.input.ends_with(' '));
}

// Review: before the first message `/modelo <name>` saved any plausible name. Only one of the
// catalog's for the active connector (or the one configured) is taken; nothing else is saved.
#[test]
fn a_model_not_on_the_list_is_refused_with_the_list_and_not_saved() {
    let (_t, d) = dirs_with_config();
    let before = std::fs::read_to_string(d.config_file()).unwrap();
    let e = super::set_model(&d, None, "hola").unwrap_err();
    assert!(e.contains("«hola»") && e.contains("tiene:"), "{e}");
    assert_eq!(std::fs::read_to_string(d.config_file()).unwrap(), before);
    let current = Config::load(&d).unwrap().model.model;
    assert!(e.contains(&current), "{e}");
    assert!(super::set_model(&d, None, &current).is_ok());
}

#[test]
fn a_choice_comes_back_into_the_menu_marked_and_enter_applies_it() {
    let (a, _) = run(open(), "/modelo");
    let items = vec![
        Choice {
            value: "beam-free".into(),
            label: "beam-free".into(),
            blocked: None,
            current: false,
        },
        Choice {
            value: "kimi-k3".into(),
            label: "kimi-k3".into(),
            blocked: None,
            current: true,
        },
    ];
    let (a, _) = feed(a, vec![Input::Bg(Bg::Choices(Run::Model, Ok(items)))]);
    let s = screen_text(&a, 100, 30);
    assert!(s.contains("● kimi-k3") || s.contains("kimi-k3"), "{s}");
    let (a, cmds) = feed(a, vec![key(KeyCode::Enter)]);
    assert_eq!(cmds, vec![Cmd::SetModel("kimi-k3".into())]);
    assert!(a.chat.menu.is_none() && a.chat.input.is_empty());

    // A connector without a key is shown, dim, and cannot be chosen.
    let (a, _) = run(open(), "/conector");
    let items = vec![Choice {
        value: "claude".into(),
        label: "claude · claude-sonnet-5-5".into(),
        blocked: Some("sin llave: clio connect claude".into()),
        current: false,
    }];
    let (a, _) = feed(a, vec![Input::Bg(Bg::Choices(Run::Connector, Ok(items)))]);
    assert!(screen_text(&a, 100, 30).contains("sin llave: clio connect claude"));
    let (a, cmds) = feed(a, vec![key(KeyCode::Enter)]);
    assert!(cmds.is_empty());
    assert_eq!(a.flash.as_deref(), Some("sin llave: clio connect claude"));
}

// Review: a click on the «cargando…» row applied the first choice that came after the draw,
// one the person never saw. A click counts only on the rows that were drawn.
#[test]
fn a_click_on_the_loading_row_never_picks_a_choice_that_came_later() {
    let (a, _) = run(open(), "/model");
    let s = screen_text(&a, 100, 30);
    let y = u16::try_from(s.lines().position(|l| l.contains("cargando")).unwrap()).unwrap();
    let click = || {
        Input::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 30,
            row: y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        })
    };
    let items = vec![Choice {
        value: "kimi-k3".into(),
        label: "kimi-k3".into(),
        blocked: None,
        current: false,
    }];
    let (a, cmds) = feed(
        a,
        vec![Input::Bg(Bg::Choices(Run::Model, Ok(items))), click()],
    );
    assert!(cmds.is_empty(), "{cmds:?}");
    // Once drawn, the same click picks it.
    let s = screen_text(&a, 100, 30);
    assert!(
        s.lines().nth(usize::from(y)).unwrap().contains("kimi-k3"),
        "{s}"
    );
    let (_, cmds) = feed(a, vec![click()]);
    assert_eq!(cmds, vec![Cmd::SetModel("kimi-k3".into())]);
}

// Review: `/acciones` showed raw UTC stamps. It shows the local `HH:MM:SS`, the tool and the
// outcome in columns, and the command in the code color with its wrapped rows under it.
#[test]
fn the_actions_box_shows_local_time_columns_and_the_command_in_code_color() {
    let log = vec![
        json!({"at_ms": 1_795_016_000_000_i64 + 13_000, "tool": "exec", "outcome": "run",
            "what": format!("sudo du -sh /var/{}", "muy-largo/".repeat(12)), "exit_code": 0}),
        json!({"at_ms": 1_795_016_060_000_i64, "tool": "fs_write", "outcome": "aside",
            "what": "/etc/hosts"}),
    ];
    // 1 795 016 000 is 2026-11-18 15:33:20 UTC; five hours behind it.
    let m = super::actions_box(&log, -5 * 3600);
    assert!(
        m.lines[0].starts_with("10:33:33  exec      run    sudo du"),
        "{:?}",
        m.lines
    );
    assert!(
        m.lines[1].starts_with("10:34:20  fs_write  aside  /etc/hosts"),
        "{:?}",
        m.lines
    );
    let mut a = open();
    a.theme = super::theme::Theme {
        color: true,
        unicode: true,
        rgb: false,
    };
    a.modal = Some(m);
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    term.draw(|f| super::app::draw(f, &a)).unwrap();
    let s = screen_text(&a, 100, 30);
    assert!(!s.contains("UTC"), "{s}");
    let lines: Vec<&str> = s.lines().collect();
    let y = lines.iter().position(|l| l.contains("sudo du")).unwrap();
    // The wrapped row starts where the command does, and the command is in the code color.
    let col = |l: &str, w: &str| l[..l.find(w).unwrap()].chars().count();
    let first = col(lines[y], "sudo");
    assert_eq!(col(lines[y + 1], "/var/muy-largo"), first, "{s}");
    let cell = &term.backend().buffer()[(u16::try_from(first).unwrap(), u16::try_from(y).unwrap())];
    assert_eq!(cell.style().fg, a.theme.code().fg, "{s}");
}

#[test]
fn a_command_that_changes_the_session_waits_and_one_that_reads_does_not() {
    for busy in ["running", "opening", "answering", "compacting"] {
        for typed in [
            "/modelo x",
            "/conector claude",
            "/esfuerzo alto",
            "/compactar",
            "/nueva",
            "/nombre t",
            "/deshacer",
            "/salir",
        ] {
            let mut a = open();
            match busy {
                "running" => a.chat.running = true,
                "opening" => a.chat.opening = true,
                "answering" => a.answering = 1,
                _ => a.chat.compacting = true,
            }
            let (a, cmds) = run(a, typed);
            assert!(cmds.is_empty(), "{busy} {typed}: {cmds:?}");
            assert!(
                a.flash.is_some() && !a.quit && a.confirm.is_none(),
                "{busy} {typed}"
            );
            assert_eq!(a.chat.input, typed, "{busy} {typed}: the text stays");
        }
    }
    let mut a = open();
    a.chat.running = true;
    let (a, _) = run(a, "/uso");
    assert_eq!(a.modal.as_ref().unwrap().title, "Uso");
    // While compacting, a message waits too, and is not lost.
    let mut a = open();
    a.chat.compacting = true;
    let (a, cmds) = run(a, "hola");
    assert!(cmds.is_empty());
    assert_eq!(a.chat.input, "hola");
}

#[test]
fn a_command_never_replaces_or_answers_an_open_question() {
    let mut a = open();
    a.confirm = Some(Confirm::new(
        Action::Approve,
        "a1".into(),
        0,
        "«borrar» rm x".into(),
    ));
    let mut inputs = vec![Input::Tick(5_000)];
    inputs.extend(typed("/salir"));
    inputs.push(key(KeyCode::Enter));
    let (a, cmds) = feed(a, inputs);
    assert!(cmds.is_empty(), "{cmds:?}");
    assert!(!a.quit && a.chat.menu.is_none());
    assert_eq!(a.confirm.as_ref().unwrap().id, "a1");
}

#[test]
fn what_the_model_writes_is_never_a_command() {
    let a = open();
    let (a, cmds) = feed(
        a,
        vec![
            Input::Bg(Bg::ChatEvent(Event::MessageUpdate {
                usage: None,
                kind: "text_delta".into(),
                delta: Some("/salir\n/deshacer".into()),
            })),
            Input::Bg(Bg::ChatSettled {
                result: Err("x".into()),
                refresh: None,
            }),
        ],
    );
    assert_eq!(cmds, vec![Cmd::LoadSessions]);
    assert!(!a.quit && a.confirm.is_none() && a.chat.menu.is_none());
    // Nor what a reopened session's history holds.
    let mut a = app();
    a.use_pi();
    a.chat = Chat::opening(1, 0);
    let history = vec![json!({"role": "assistant", "content": "/salir"})];
    a.chat
        .opened("sid".into(), String::new(), &history, None, 0);
    let (a, cmds) = feed(a, vec![key(KeyCode::Enter)]);
    assert!(cmds.is_empty() && !a.quit);
}

#[test]
fn copiar_puts_the_last_answer_on_the_clipboard() {
    let mut a = open();
    let (b, cmds) = run(open(), "/copiar");
    assert!(cmds.is_empty());
    assert_eq!(
        b.flash.as_deref(),
        Some("todavía no hay una respuesta que copiar")
    );
    a.chat.items.push(Item::Assistant {
        text: "primera".into(),
        live: false,
    });
    a.chat.items.push(Item::User("otra".into()));
    a.chat.items.push(Item::Assistant {
        text: "**la última**".into(),
        live: false,
    });
    let (a, cmds) = run(a, "/copy");
    assert_eq!(cmds, vec![Cmd::Clipboard("**la última**".into())]);
    assert_eq!(a.flash.as_deref(), Some("copiado"));
}

// Review: `/copiar` gave the clipboard the model's raw bytes. It copies what was shown: no
// escape (a bracketed-paste end), no C1 control, no bidi or zero-width character.
#[test]
fn copiar_copies_the_answer_as_shown_not_its_raw_bytes() {
    let mut a = open();
    a.chat.items.push(Item::Assistant {
        text: "ok\u{1b}[201~ rm -rf ~\u{9b}2J a\u{202e}b\u{200b}c".into(),
        live: false,
    });
    let (_, cmds) = run(a, "/copy");
    let [Cmd::Clipboard(text)] = cmds.as_slice() else {
        panic!("{cmds:?}");
    };
    assert!(
        !text.contains(['\u{1b}', '\u{9b}', '\u{202e}', '\u{200b}']),
        "{text:?}"
    );
    assert!(
        text.starts_with("ok rm -rf ~") && !text.contains("[201~"),
        "{text:?}"
    );
}

// Whatever bytes the answer holds, the terminal receives one OSC 52 sequence whose payload is
// pure base64: no escape, bell or newline of the text can end it early or start another.
#[test]
fn osc52_carries_any_text_as_pure_base64_and_is_capped() {
    let nasty = "a\x1b]0;título\x07b\x1b\\c\n\r\u{9b}\u{202e}ñ\0\u{7f}fin";
    for text in [nasty, "", "x", &"é".repeat(40_000)] {
        let out = super::osc52(text);
        let payload = out
            .strip_prefix("\x1b]52;c;")
            .and_then(|s| s.strip_suffix('\x07'))
            .unwrap();
        assert!(payload.len().is_multiple_of(4));
        assert!(
            payload
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'='),
            "{payload:?}"
        );
        assert!(payload.len() <= (64 * 1024_usize).div_ceil(3) * 4);
    }
}

// Review: a FIFO named `*.md` in the commands folder hung the loader (and a link could read a
// file outside it). Only regular files are read; the rest is left out, at once.
#[test]
fn a_fifo_or_a_link_in_the_commands_folder_is_left_out_without_waiting() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("commands");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("bueno.md"), "hola").unwrap();
    std::fs::write(t.path().join("fuera.md"), "secreto").unwrap();
    std::os::unix::fs::symlink(t.path().join("fuera.md"), d.join("enlace.md")).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(d.join("tubo.md"))
        .status()
        .unwrap();
    assert!(made.success());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(load_custom(&d));
    });
    let (cmds, left) = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the loader waited on the FIFO");
    let names: Vec<&str> = cmds.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["bueno"]);
    assert_eq!(left, vec!["enlace.md", "tubo.md"]);
}

#[test]
fn the_persons_commands_are_read_named_capped_and_expanded() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("commands");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("revisa.md"),
        "# Revisa un archivo\nRevisa $ARGUMENTS con cuidado.\n",
    )
    .unwrap();
    std::fs::write(d.join("resumen.md"), "Resume lo hecho hoy.").unwrap();
    // `resume` is the English name of /sesiones: the built-in wins.
    std::fs::write(d.join("resume.md"), "x").unwrap();
    std::fs::write(d.join("Mal.md"), "x").unwrap();
    std::fs::write(d.join("con espacio.md"), "x").unwrap();
    std::fs::write(d.join("modelo.md"), "x").unwrap();
    std::fs::write(d.join("grande.md"), "x".repeat(CUSTOM_MAX as usize + 1)).unwrap();
    std::fs::write(d.join("justo.md"), "y".repeat(CUSTOM_MAX as usize)).unwrap();
    std::fs::write(d.join("binario.md"), [0xff, 0xfe, 0x00]).unwrap();
    std::fs::write(d.join("nota.txt"), "x").unwrap();
    std::fs::create_dir_all(d.join("carpeta.md")).unwrap();
    let (cmds, left) = load_custom(&d);
    let names: Vec<&str> = cmds.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["justo", "resumen", "revisa"]);
    assert_eq!(
        left,
        vec![
            "Mal.md",
            "binario.md",
            "carpeta.md",
            "con espacio.md",
            "grande.md",
            "modelo.md",
            "resume.md"
        ]
    );
    let revisa = cmds.iter().find(|c| c.name == "revisa").unwrap();
    assert_eq!(revisa.what, "Revisa un archivo");
    assert_eq!(revisa.body, "Revisa $ARGUMENTS con cuidado.");
    assert_eq!(cmds[1].what, "tu comando");
    assert!(load_custom(&t.path().join("no-existe")).0.is_empty());

    assert_eq!(expand("Revisa $ARGUMENTS ya", "a.rs"), "Revisa a.rs ya");
    assert_eq!(expand("Resume.", "corto"), "Resume.\ncorto");
    assert_eq!(expand("Resume.", ""), "Resume.");
}

#[test]
fn a_persons_command_is_sent_as_their_message_and_never_read_again() {
    let customs = vec![
        Custom {
            name: "revisa".into(),
            what: "Revisa un archivo".into(),
            body: "Revisa $ARGUMENTS con cuidado.".into(),
        },
        Custom {
            name: "raro".into(),
            what: "tu comando".into(),
            body: "/salir".into(),
        },
    ];
    let a = open();
    let (a, _) = feed(
        a,
        vec![
            ch('/'),
            Input::Bg(Bg::Commands(customs, vec!["Mal.md".into()])),
        ],
    );
    let s = screen_text(&a, 100, 30);
    assert!(s.contains("comandos sin usar: Mal.md"), "{s}");
    let (a, cmds) = run(a, "revisa src/a.rs");
    assert_eq!(
        cmds,
        vec![Cmd::ChatSend {
            text: "Revisa src/a.rs con cuidado.".into()
        }]
    );
    assert!(a.chat.input.is_empty());
    // Its text goes out as a message, even when it looks like a command.
    let mut a = a;
    a.chat.running = false;
    let (a, cmds) = run(a, "/raro");
    assert_eq!(
        cmds,
        vec![Cmd::ChatSend {
            text: "/salir".into()
        }]
    );
    assert!(!a.quit);
}

#[test]
fn the_levels_have_spanish_names_both_ways() {
    assert_eq!(level_name("low"), Some("bajo"));
    assert_eq!(level_name("xhigh"), Some("muy alto"));
    assert_eq!(level_name("nope"), None);
    assert_eq!(level_of("Medio"), Some("medium"));
    assert_eq!(level_of("muyalto"), Some("xhigh"));
    assert_eq!(level_of("max"), Some("max"));
    assert_eq!(level_of("enorme"), None);
    for l in clio_brain::pi::launch::THINKING {
        assert_eq!(level_of(level_name(l).unwrap()), Some(l));
    }
}

#[test]
fn the_help_lists_every_command() {
    let mut a = open();
    a.open_help();
    let s = screen_text(&a, 120, 120);
    assert!(s.contains("los comandos"), "{s}");
    for c in COMMANDS {
        assert!(s.contains(&format!("/{}", c.name)), "/{}\n{s}", c.name);
    }
    assert!(s.contains("OSC 52"));
}

#[test]
fn the_top_bar_says_the_effort() {
    let mut a = open();
    let mut h = super::tests::with_runs().home.header.unwrap();
    h.model = "claude-sonnet-5-5".into();
    h.thinking = "low".into();
    a.home.header = Some(h);
    let s = screen_text(&a, 100, 30);
    assert!(
        s.lines()
            .next()
            .unwrap()
            .contains("claude-sonnet-5-5 · bajo"),
        "{s}"
    );
}

// ---------------------------------------------------------------- the runtime side

/// A session that answers Pi's commands from a table and records what it was sent.
struct FakePi {
    sent: Mutex<Vec<(String, Value)>>,
}

impl Session for FakePi {
    fn run_prompt(
        &self,
        _: &str,
        _: &Cancel,
        _: &mut dyn FnMut(&Event),
    ) -> Result<RunSummary, PiError> {
        Err(PiError::Protocol("no".into()))
    }
    fn steer(&self, _: &str) -> Result<(), PiError> {
        Ok(())
    }
    fn refresh(&self) -> Result<Refresh, PiError> {
        Err(PiError::Protocol("no".into()))
    }
    fn command(&self, kind: &str, fields: Value) -> Result<Value, String> {
        self.sent.lock().unwrap().push((kind.into(), fields));
        Ok(match kind {
            "get_available_models" => json!({"models": [
                {"provider": "opencode-go", "id": "beam-free"},
                {"provider": "opencode-go", "id": "kimi-k3"},
                {"provider": "anthropic", "id": "claude-opus-5-5"},
            ]}),
            "get_available_thinking_levels" => json!({"levels": ["off", "low", "medium", "high"]}),
            _ => json!({}),
        })
    }
    fn shutdown(&self) {}
}

fn dirs_with_config() -> (tempfile::TempDir, Dirs) {
    let t = tempfile::tempdir().unwrap();
    let home = t.path();
    let d = Dirs {
        config: home.join("config"),
        runs: home.join("data/runs"),
        data: home.join("data"),
        runtime: None,
        home: home.to_path_buf(),
    };
    std::fs::create_dir_all(&d.config).unwrap();
    let mut v = serde_json::to_value(Config::default_with(vec![])).unwrap();
    v["extra"] = json!("se queda");
    std::fs::write(d.config_file(), v.to_string()).unwrap();
    (t, d)
}

#[test]
fn model_and_effort_go_to_pi_and_are_saved_as_the_default() {
    let (_t, d) = dirs_with_config();
    let pi = FakePi {
        sent: Mutex::new(vec![]),
    };
    // Pi's list, only the active connector's models, the configured one marked.
    let models = super::choices(&d, Some(&pi), Run::Model).unwrap();
    let ids: Vec<&str> = models.iter().map(|c| c.value.as_str()).collect();
    assert_eq!(ids, vec!["beam-free", "kimi-k3"]);
    assert!(models[0].current && !models[1].current);

    assert!(super::set_model(&d, Some(&pi), "claude-opus-5-5").is_err());
    assert_eq!(
        super::set_model(&d, Some(&pi), "kimi-k3").unwrap(),
        "modelo: kimi-k3"
    );
    assert!(pi.sent.lock().unwrap().contains(&(
        "set_model".into(),
        json!({"provider": "opencode-go", "modelId": "kimi-k3"})
    )));
    let cfg = Config::load(&d).unwrap();
    assert_eq!(cfg.model.model, "kimi-k3");
    assert_eq!(
        cfg.model.last.get("opencode-go").map(String::as_str),
        Some("kimi-k3")
    );

    let levels = super::choices(&d, Some(&pi), Run::Effort).unwrap();
    let names: Vec<&str> = levels.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(names, vec!["apagado", "bajo", "medio", "alto"]);
    assert!(levels[1].current);
    assert!(super::set_effort(&d, Some(&pi), "max").is_err());
    assert_eq!(
        super::set_effort(&d, Some(&pi), "high").unwrap(),
        "esfuerzo: alto"
    );
    assert!(
        pi.sent
            .lock()
            .unwrap()
            .contains(&("set_thinking_level".into(), json!({"level": "high"})))
    );
    // Before the first message: no Pi, every level, saved for the session to come.
    assert_eq!(
        super::set_effort(&d, None, "medium").unwrap(),
        "esfuerzo: medio"
    );
    let cfg = Config::load(&d).unwrap();
    assert_eq!(cfg.model.thinking, "medium");
    // Every other field of the file stays.
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(d.config_file()).unwrap()).unwrap();
    assert_eq!(v["extra"], "se queda");
}

#[test]
fn a_connector_needs_its_key_and_the_list_marks_the_active_one() {
    let (_t, d) = dirs_with_config();
    let list = super::choices(&d, None, Run::Connector).unwrap();
    let names: Vec<&str> = list.iter().map(|c| c.value.as_str()).collect();
    assert_eq!(names, vec!["opencode", "claude", "anthropic-api"]);
    assert!(list[0].current && !list[1].current);
    assert!(list[0].label.contains("beam-free"));
    assert!(
        super::set_connector(&d, "ninguno")
            .unwrap_err()
            .contains("no existe el conector")
    );
    for c in &list {
        // No vault here: a connector is usable only with its key in the environment.
        if let Some(why) = &c.blocked {
            assert!(
                why.contains("clio connect") || why.contains("clio pi install"),
                "{why}"
            );
            assert!(super::set_connector(&d, &c.value).is_err());
        }
    }
}

/// The table for `docs/uso-local.md`:
/// `cargo test -p clio-brain --bin clio print_commands -- --ignored --nocapture`.
#[test]
#[ignore = "prints the table of commands; not a check"]
fn print_commands() {
    println!("| Comando | También | Qué hace |");
    println!("|---|---|---|");
    for c in COMMANDS {
        let head = if c.args.is_empty() {
            format!("`/{}`", c.name)
        } else {
            format!("`/{} {}`", c.name, c.args.replace('|', "\\|"))
        };
        let alias: Vec<String> = c.aliases.iter().map(|a| format!("`/{a}`")).collect();
        let wait = if c.idle { " (espera al agente)" } else { "" };
        println!(
            "| {head} | {} | {}{wait} |",
            alias.join(" "),
            c.what.replace('|', "\\|")
        );
    }
}
