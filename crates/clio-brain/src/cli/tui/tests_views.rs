//! Tests of the status bar, the context and usage views, the debug view and the layouts.

#![allow(clippy::unwrap_used)]

use clio_brain::agent::{
    CallUsage, ContextItem, ContextReport, DecisionDebug, Event, Exchange, Msg,
};
use clio_core::Level as TaskLevel;
use clio_gateway::Outcome;
use crossterm::event::KeyCode;

use super::app::{App, Bg, Cmd, Input, Screen};
use super::live::{Live, LiveView};
use super::tests::{
    HOSTILE, ID1, LIMITS, SIZES, assert_clean, assert_fits, ch, ctrl, detail, feed, key,
    open_detail, report, screen_text, started, with_runs,
};
use crate::cli::runs::Status;
use crate::cli::usage::RunUsage;

fn call(n: u32, i: u32, o: u32, cost: u64, model_ms: u64, at_ms: u64) -> CallUsage {
    CallUsage {
        call: n,
        input_tokens: i,
        output_tokens: o,
        cost_micro: cost,
        model_ms,
        at_ms,
        retries: 0,
    }
}

pub(super) fn context(call: u32) -> ContextReport {
    ContextReport {
        call,
        prompt_version: "clio-agent@3".into(),
        system: 1_200,
        objective: 120,
        tools: 410,
        conversation: 3_100,
        turns: 7,
        external: vec![
            ContextItem {
                name: "dates.py".into(),
                tokens: 2_000,
            },
            ContextItem {
                name: "big.json".into(),
                tokens: 32_850,
            },
        ],
        tool_outputs: 0,
        memory: 0,
        memory_items: 0,
    }
}

/// Two calls: 12,4k in, 3,1k out, free; the context at 31 % of a 128k window.
pub(super) fn usage_with_context() -> RunUsage {
    let mut u = RunUsage::new(128_000, true, LIMITS);
    u.calls = vec![
        call(1, 10_000, 2_000, 0, 1_200, 1_500),
        call(2, 2_400, 1_100, 0, 2_100, 4_000),
    ];
    u.context = Some(context(2));
    u
}

fn exchange(call: u32, response: &str) -> Exchange {
    Exchange {
        call,
        messages: 3,
        request_chars: 4_200,
        request: vec![Msg {
            role: "user".into(),
            content: "Result of read_file:\nhola".into(),
        }],
        response: response.into(),
    }
}

fn decision(reason: &str) -> DecisionDebug {
    DecisionDebug {
        n: 1,
        tool: "write_file".into(),
        args: "{\"path\":\".git/hooks/x\"}".into(),
        outcome: Outcome::Deny,
        rule: "S-16".into(),
        reason: reason.into(),
        tool_ms: 3,
    }
}

/// A live run in progress: step 8, 21 s, two calls, a tainted session, one denied write.
pub(super) fn live_app() -> App {
    let mut a = with_runs();
    a.screen = Screen::Live;
    a.live = Live::starting(0, "arregla parse_date");
    let mut events: Vec<Bg> = vec![started()];
    let ev = |e: Event| Bg::Agent(e);
    events.extend([
        ev(Event::Started {
            level: TaskLevel::Simple,
        }),
        ev(Event::Context(ContextReport {
            call: 1,
            external: vec![],
            conversation: 0,
            turns: 0,
            ..context(1)
        })),
        ev(Event::Usage(call(1, 10_000, 2_000, 0, 1_200, 1_500))),
        ev(Event::Exchange(exchange(
            1,
            "{\"tool\":\"read_file\",\"args\":{\"path\":\"dates.py\"}}",
        ))),
        ev(Event::Step { n: 1 }),
        ev(Event::Decision(DecisionDebug {
            outcome: Outcome::Allow,
            tool: "read_file".into(),
            reason: "reversible in the sandbox".into(),
            ..decision("")
        })),
        ev(Event::Tool {
            name: "read_file".into(),
            target: "dates.py".into(),
            outcome: Outcome::Allow,
            rule: "step-8-reversible-in-sandbox".into(),
            ran: true,
            result_chars: 1234,
            tainted: true,
        }),
        ev(Event::Context(context(2))),
        ev(Event::Usage(call(2, 2_400, 1_100, 0, 2_100, 4_000))),
        ev(Event::Exchange(exchange(2, "{\"tool\":\"write_file\"}"))),
        ev(Event::Step { n: 8 }),
        ev(Event::Decision(decision("the path is protected (S-16)"))),
        ev(Event::Tool {
            name: "write_file".into(),
            target: ".git/hooks/pre-commit".into(),
            outcome: Outcome::Deny,
            rule: "S-16".into(),
            ran: false,
            result_chars: 60,
            tainted: true,
        }),
    ]);
    let (mut a, _) = feed(a, events.into_iter().map(Input::Bg).collect());
    a.now_ms = 21_000;
    a
}

fn last_line(t: &str) -> &str {
    t.lines().last().unwrap()
}

// ------------------------------------------------------------ the status bar

#[test]
fn the_status_bar_has_the_four_data_on_every_screen_with_a_run() {
    let detail_app = open_detail(with_runs(), detail(ID1, Status::Pending), false);
    let (context_app, _) = feed(live_app(), vec![ch('c')]);
    let (usage_app, _) = feed(live_app(), vec![ch('U')]);
    let (help_app, _) = feed(live_app(), vec![ch('?')]);
    for (name, a, steps, secs, tainted) in [
        ("inicio", with_runs(), "7/40 pasos", "12 s/10 min", false),
        ("detalle", detail_app, "7/40 pasos", "12 s/10 min", false),
        ("en vivo", live_app(), "8/40 pasos", "21 s/10 min", true),
        ("contexto", context_app, "8/40 pasos", "21 s/10 min", true),
        ("uso", usage_app, "8/40 pasos", "21 s/10 min", true),
        ("ayuda", help_app, "8/40 pasos", "21 s/10 min", true),
    ] {
        let t = screen_text(&a, 80, 24);
        let bar = last_line(&t);
        for want in ["↑12,4k", "↓3,1k", "$0", steps, secs, "31 %"] {
            assert!(bar.contains(want), "{name}: {want:?} not in {bar:?}");
        }
        assert!(
            bar.contains("contexto") || bar.contains("ctx"),
            "{name}: {bar:?}"
        );
        assert_eq!(bar.contains("externo"), tainted, "{name}: {bar:?}");
        assert_eq!(bar.chars().count(), 80);
    }
}

#[test]
fn the_status_bar_abbreviates_without_cutting_at_the_minimum_size() {
    let t = screen_text(&live_app(), 60, 16);
    assert_fits(&t, 60, 16);
    let bar = last_line(&t);
    for want in ["↑12,4k", "↓3,1k", "$0", "8/40", "21s", "31%", "ext"] {
        assert!(bar.contains(want), "{want:?} not in {bar:?}");
    }
    // At 80 with the session tainted it still shows everything, shortened.
    let t = screen_text(&live_app(), 80, 24);
    let bar = last_line(&t).trim_end();
    assert!(
        bar.contains("externo") && bar.contains("31 %") && bar.contains("pasos"),
        "{bar:?}"
    );
}

#[test]
fn without_a_run_the_status_bar_says_so_and_new_job_and_doctor_leave_it_empty() {
    let t = screen_text(&super::tests::app(), 80, 24);
    assert!(last_line(&t).contains("sin uso todavía"), "{t}");
    let (a, _) = feed(with_runs(), vec![ch('n')]);
    assert!(last_line(&screen_text(&a, 80, 24)).trim().is_empty());
}

#[test]
fn the_context_bar_changes_color_at_70_and_90_percent() {
    use ratatui::style::Color;
    let color_of_bar = |tokens: u64| {
        let mut a = live_app();
        let mut c = context(2);
        c.external = vec![ContextItem {
            name: "x".into(),
            tokens,
        }];
        c.system = 0;
        c.objective = 0;
        c.tools = 0;
        c.conversation = 0;
        a.live.usage.context = Some(c);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        term.draw(|f| super::app::draw(f, &a)).unwrap();
        let buf = term.backend().buffer();
        let line: String = (0..120).map(|x| buf[(x, 29)].symbol()).collect();
        let x = line.find('▕').unwrap();
        buf[(line[..x].chars().count() as u16 + 1, 29)].fg
    };
    assert_eq!(color_of_bar(10_000), Color::Reset, "quiet below 70 %");
    assert_eq!(color_of_bar(95_000), Color::Yellow, "74 %");
    assert_eq!(color_of_bar(120_000), Color::Red, "93 %");
}

// ------------------------------------------------------------ the context view

#[test]
fn c_opens_the_context_in_layers_and_esc_goes_back() {
    let (a, cmds) = feed(live_app(), vec![ch('c')]);
    assert!(cmds.is_empty());
    assert_eq!(a.screen, Screen::Context);
    let t = screen_text(&a, 80, 24);
    assert_fits(&t, 80, 24);
    for want in [
        "contexto ",
        "31 %",
        "39,7k de 128k tokens (aprox.)",
        "prompt de sistema",
        "clio-agent@3",
        "1,2k",
        "0,9 %",
        "objetivo / contrato",
        "tu pedido",
        "herramientas",
        "descripciones",
        "conversación",
        "7 turnos",
        "3,1k",
        "externo",
        "2 lecturas",
        "dates.py",
        "big.json",
        "32,9k",
        "salidas de herramientas",
        "cuatro caracteres por token",
    ] {
        assert!(t.contains(want), "{want:?}\n{t}");
    }
    // Each file read is marked as external.
    assert_eq!(
        t.matches("externo").count() - 1 /* status bar */ - 1 /* layer */ - 1 /* note */ + 1,
        2,
        "{t}"
    );
    let (a, _) = feed(a, vec![key(KeyCode::Esc)]);
    assert_eq!(a.screen, Screen::Live);
}

#[test]
fn the_layers_add_up_and_the_percentage_matches_the_window() {
    let c = context(2);
    let layers =
        c.system + c.objective + c.tools + c.conversation + c.external_tokens() + c.tool_outputs;
    assert_eq!(c.total(), layers);
    assert_eq!(c.total(), 39_680);
    let a = live_app();
    let (snap, _) = a.run_data().unwrap();
    assert_eq!(snap.ctx, Some(39_680));
    assert_eq!(snap.window, 128_000);
    assert_eq!(snap.ctx_percent(), Some(31));
    // The shares printed for the layers are of the window and add up to the total's, within rounding.
    let t = screen_text(&feed(a, vec![ch('c')]).0, 120, 30);
    assert!(t.contains("31,0 %") || t.contains("0,9 %"), "{t}");
}

#[test]
fn the_context_warns_discreetly_past_70_and_past_90_percent() {
    let with = |tokens: u64| {
        let mut a = live_app();
        let mut c = context(2);
        c.external = vec![ContextItem {
            name: "big.txt".into(),
            tokens,
        }];
        a.live.usage.context = Some(c);
        screen_text(&feed(a, vec![ch('c')]).0, 80, 24)
    };
    let quiet = with(1_000);
    assert!(
        !quiet.contains("70 %") && !quiet.contains("90 %"),
        "{quiet}"
    );
    assert!(with(90_000).contains("pasa del 70 % de la ventana"));
    let full = with(120_000);
    assert!(full.contains("casi llena (más del 90 %)"), "{full}");
}

#[test]
fn an_unknown_window_says_the_default_was_used() {
    let mut a = live_app();
    a.live.usage.window_known = false;
    let t = screen_text(&feed(a, vec![ch('c')]).0, 80, 24);
    assert!(
        t.contains("por defecto") && t.contains("no está en el catálogo"),
        "{t}"
    );
}

#[test]
fn a_run_without_saved_context_says_so() {
    let mut d = detail(ID1, Status::Pending);
    d.usage = RunUsage::default();
    let a = open_detail(with_runs(), d, false);
    let (a, _) = feed(a, vec![ch('c')]);
    let t = screen_text(&a, 80, 24);
    assert!(t.contains("no guardó las capas del contexto"), "{t}");
    assert!(
        last_line(&t).contains("contexto ─"),
        "the bar shows a dash, not a made-up number: {}",
        last_line(&t)
    );
    let (a, _) = feed(a, vec![ch('U')]);
    let t = screen_text(&a, 80, 24);
    assert!(
        t.contains("no guardó el uso por paso") && t.contains("↑12,4k"),
        "{t}"
    );
}

#[test]
fn c_and_u_open_from_home_detail_and_live_and_esc_returns_to_each() {
    for (name, start, back) in [
        ("home", with_runs(), Screen::Home),
        (
            "detalle",
            open_detail(with_runs(), detail(ID1, Status::Pending), false),
            Screen::Detail,
        ),
        (
            "detalle con diff",
            open_detail(with_runs(), detail(ID1, Status::Pending), true),
            Screen::Detail,
        ),
        ("en vivo", live_app(), Screen::Live),
    ] {
        let (a, _) = feed(start, vec![ch('c')]);
        assert_eq!(a.screen, Screen::Context, "{name}");
        let (a, _) = feed(a, vec![ch('U')]);
        assert_eq!(
            a.screen,
            Screen::Usage,
            "{name}: U inside the context goes to the usage"
        );
        let (a, _) = feed(a, vec![ch('c')]);
        assert_eq!(a.screen, Screen::Context, "{name}");
        let (a, _) = feed(a, vec![ch('c')]);
        assert_eq!(a.screen, back, "{name}: the same key closes it");
        let (a, _) = feed(a, vec![ch('U'), key(KeyCode::Esc)]);
        assert_eq!(a.screen, back, "{name}");
        let (a, _) = feed(a, vec![ch('c'), ch('?'), key(KeyCode::Esc)]);
        assert_eq!(
            a.screen,
            Screen::Context,
            "{name}: help returns to the view"
        );
        let (a, _) = feed(a, vec![ch('q')]);
        assert_eq!(a.screen, back, "{name}");
    }
}

#[test]
fn with_no_run_the_views_do_not_open() {
    for k in ['c', 'U'] {
        let (a, cmds) = feed(super::tests::app(), vec![ch(k)]);
        assert!(cmds.is_empty());
        assert_eq!(a.screen, Screen::Home);
        assert_eq!(a.flash.as_deref(), Some("todavía no hay runs"));
    }
}

#[test]
fn the_views_scroll_and_never_past_their_end() {
    let mut a = live_app();
    // Many files read: the layer list is long.
    let mut c = context(2);
    c.external = (0..40)
        .map(|i| ContextItem {
            name: format!("src/file_{i}.rs"),
            tokens: 100,
        })
        .collect();
    a.live.usage.context = Some(c);
    let (a, _) = feed(a, vec![ch('c')]);
    let top = screen_text(&a, 60, 16);
    assert!(top.contains("file_0.rs"), "{top}");
    let (a, _) = feed(a, vec![ch('G')]);
    let bottom = screen_text(&a, 60, 16);
    assert!(
        bottom.contains("file_39.rs") && !bottom.contains("file_0.rs"),
        "{bottom}"
    );
    let max = a.view_max.get();
    assert!(max > 0 && a.view_scroll == max);
    let (a, _) = feed(a, vec![ch('j'), ch('j'), key(KeyCode::PageDown)]);
    assert_eq!(a.view_scroll, max, "no scrolling past the end");
    let (a, _) = feed(a, vec![ch('k')]);
    assert_eq!(a.view_scroll, max - 1, "one press goes back one line");
    let (a, _) = feed(a, vec![ctrl('b')]);
    assert_eq!(a.view_scroll, (max - 1).saturating_sub(8));
    let (a, _) = feed(a, vec![ch('g')]);
    assert_eq!(a.view_scroll, 0);
    assert_fits(&screen_text(&a, 60, 16), 60, 16);
}

#[test]
fn hostile_file_names_in_the_context_never_reach_the_screen_raw() {
    let mut a = live_app();
    let mut c = context(3);
    c.external = HOSTILE
        .iter()
        .map(|n| ContextItem {
            name: (*n).into(),
            tokens: 50,
        })
        .collect();
    c.prompt_version = "clio-agent\u{1b}]0;pwned-title\u{7}@3".into();
    a.live.on_event(&Event::Context(c));
    let (a, _) = feed(a, vec![ch('c')]);
    for (w, h) in SIZES {
        let t = screen_text(&a, w, h);
        assert_clean(&t);
        assert!(
            !t.contains("pwned-title") && !t.contains("http://evil.example"),
            "{t}"
        );
    }
    let t = screen_text(&a, 120, 40);
    assert!(
        t.contains("evilname.txt") && t.contains("report\u{FFFD}txt.exe"),
        "{t}"
    );
    // The live side column and the debug view show the same names, clean.
    let (a, _) = feed(a, vec![key(KeyCode::Esc), ctrl('d'), key(KeyCode::Tab)]);
    for (w, h) in SIZES {
        let t = screen_text(&a, w, h);
        assert_clean(&t);
        assert!(!t.contains("pwned-title"), "{t}");
    }
}

// ------------------------------------------------------------ the usage view

#[test]
fn u_shows_a_compact_table_per_step_with_running_totals_and_the_limits() {
    let (a, _) = feed(live_app(), vec![ch('U')]);
    assert_eq!(a.screen, Screen::Usage);
    let t = screen_text(&a, 80, 24);
    assert_fits(&t, 80, 24);
    for want in [
        "uso ↑12,4k ↓3,1k tokens · $0",
        "paso",
        "entrada",
        "salida",
        "costo",
        "modelo",
        "acum.tok",
        "acum.costo",
    ] {
        assert!(t.contains(want), "{want:?}\n{t}");
    }
    let row1 = t
        .lines()
        .find(|l| l.trim_start().starts_with("1 "))
        .unwrap();
    for want in ["10k", "2k", "$0", "1,2 s", "12k"] {
        assert!(row1.contains(want), "{want:?} in {row1:?}");
    }
    let row2 = t
        .lines()
        .find(|l| l.trim_start().starts_with("2 "))
        .unwrap();
    for want in ["2,4k", "1,1k", "2,1 s", "15,5k"] {
        assert!(row2.contains(want), "{want:?} in {row2:?}");
    }
    for want in [
        "pasos",
        "8 de 40",
        "tiempo",
        "21 s de 10 min",
        "costo",
        "$0 de $0,10",
    ] {
        assert!(t.contains(want), "{want:?}\n{t}");
    }
    // At the minimum size the same table fits.
    let t = screen_text(&a, 60, 16);
    assert_fits(&t, 60, 16);
    assert!(
        t.contains("acum.$") && t.contains("acum.tok") && t.contains("15,5k"),
        "{t}"
    );
}

#[test]
fn usage_costs_are_exact_and_accumulate_as_integers() {
    let mut a = live_app();
    a.live.usage.calls = vec![
        call(1, 1_000, 100, 1_500, 100, 100),
        call(2, 1_000, 100, 2_500, 100, 200),
        call(3, 1_000, 100, 1, 100, 300),
    ];
    let (a, _) = feed(a, vec![ch('U')]);
    let t = screen_text(&a, 120, 30);
    for want in [
        "$0,0015",
        "$0,0025",
        "$0,000001",
        "$0,004",
        "$0,004001",
        "uso ↑3k ↓300 tokens · $0,004001",
    ] {
        assert!(t.contains(want), "{want:?}\n{t}");
    }
    // The limit line uses the same exact money.
    assert!(t.contains("$0,004001 de $0,10"), "{t}");
}

#[test]
fn a_free_model_shows_a_plain_zero() {
    let (a, _) = feed(live_app(), vec![ch('U')]);
    let t = screen_text(&a, 80, 24);
    assert!(!t.contains("$0,00") && !t.contains("$0.0"), "{t}");
}

// ------------------------------------------------------------ the debug view

#[test]
fn tab_opens_the_debug_view_only_while_debug_is_on() {
    let (a, cmds) = feed(live_app(), vec![key(KeyCode::Tab)]);
    assert!(cmds.is_empty() && a.live.view == LiveView::Timeline);
    assert!(a.flash.as_deref().unwrap().contains("Ctrl-D"));
    let (a, cmds) = feed(a, vec![ctrl('d')]);
    assert_eq!(cmds, vec![Cmd::Debug(true)]);
    let (a, _) = feed(a, vec![key(KeyCode::Tab)]);
    assert_eq!(a.live.view, LiveView::Debug);
    let t = screen_text(&a, 160, 60);
    for want in [
        "debug",
        "llamada 1",
        "llamada 2",
        "contexto",
        "tok aprox.",
        "sistema 1,2k (clio-agent@3)",
        "modelo 1,2 s",
        "entrada 10k",
        "salida 2k",
        "petición",
        "3 mensajes en total (4200 car.)",
        "[user] Result of read_file:",
        "respuesta",
        "write_file",
        "pasarela #1 write_file",
        "negado · regla S-16 · 3 ms",
        "razón: the path is protected (S-16)",
        "externo: big.json 32,9k",
    ] {
        assert!(t.contains(want), "{want:?}\n{t}");
    }
    assert!(
        !t.contains("línea de tiempo"),
        "the timeline is the other view"
    );
    // Tab again, or turning debug off, goes back to the timeline.
    let (b, _) = feed(a.clone_view(), vec![key(KeyCode::Tab)]);
    assert!(screen_text(&b, 100, 30).contains("línea de tiempo"));
    let (b, _) = feed(a, vec![ctrl('d')]);
    let t = screen_text(&b, 100, 30);
    assert!(
        t.contains("línea de tiempo") && !t.contains("llamada 1"),
        "{t}"
    );
}

impl App {
    /// Debug view of a copy of the live state.
    fn clone_view(&self) -> App {
        let mut b = live_app();
        b.debug = self.debug;
        b.live.view = self.live.view;
        b
    }
}

#[test]
fn the_debug_view_cuts_long_text_and_says_how_much_is_missing() {
    let mut a = live_app();
    a.live
        .on_event(&Event::Exchange(exchange(3, &"x".repeat(5_000))));
    let (a, _) = feed(a, vec![ctrl('d'), key(KeyCode::Tab)]);
    let t = screen_text(&a, 200, 60);
    assert!(t.contains("respuesta: 5000 car."), "{t}");
    assert!(t.contains("(faltan 3500 caracteres)"), "{t}");
    assert!(!t.contains(&"x".repeat(1_600)), "cut at 1500");
    // The notice survives a narrow terminal too: long lines continue on the next one.
    let t = screen_text(&a, 80, 40);
    assert!(t.contains("faltan 3500"), "{t}");
}

#[test]
fn the_debug_view_shows_protocol_errors_and_retries() {
    let mut a = live_app();
    a.live.on_event(&Event::Context(context(3)));
    a.live.on_event(&Event::Usage(CallUsage {
        retries: 2,
        ..call(3, 100, 10, 0, 65_000, 70_000)
    }));
    a.live.on_event(&Event::ProtocolError {
        reason: "no valid JSON object found".into(),
    });
    let (a, _) = feed(a, vec![ctrl('d'), key(KeyCode::Tab)]);
    let t = screen_text(&a, 120, 30);
    assert!(t.contains("2 reintentos") && t.contains("65,0 s"), "{t}");
    assert!(
        t.contains("error de protocolo: no valid JSON object found"),
        "{t}"
    );
}

#[test]
fn the_debug_view_scrolls_from_the_newest() {
    let mut a = live_app();
    for n in 3..60 {
        a.live.on_event(&Event::Context(context(n)));
        a.live.on_event(&Event::Usage(call(n, 1, 1, 0, 1, 1)));
    }
    let (a, _) = feed(a, vec![ctrl('d'), key(KeyCode::Tab)]);
    let newest = screen_text(&a, 100, 24);
    assert!(
        newest.contains("llamada 59") && !newest.contains("llamada 1\n"),
        "{newest}"
    );
    let (a, _) = feed(a, vec![ch('g')]);
    let oldest = screen_text(&a, 100, 24);
    assert!(
        oldest.contains("llamada 1") && !oldest.contains("llamada 59"),
        "{oldest}"
    );
    let (a, _) = feed(a, vec![ch('G')]);
    assert!(screen_text(&a, 100, 24).contains("llamada 59"));
    let (a, _) = feed(a, vec![ch('k'), ch('k'), ch('j')]);
    assert_eq!(a.live.scroll, 1);
}

#[test]
fn hostile_model_answers_and_gateway_text_in_debug_never_reach_the_screen_raw() {
    let mut a = live_app();
    let hostile = "ok\u{1b}[2J\u{1b}]0;pwned-title\u{7}\r\u{202E}evil\u{1b}]8;;http://evil.example\u{7}x\n\u{1b}[31mred\nline3";
    a.live.on_event(&Event::Exchange(Exchange {
        call: 3,
        messages: 9,
        request_chars: 99,
        request: vec![Msg {
            role: "us\u{1b}[2Jer".into(),
            content: hostile.into(),
        }],
        response: hostile.into(),
    }));
    a.live.on_event(&Event::Decision(DecisionDebug {
        tool: "wri\u{1b}[2Jte".into(),
        args: hostile.into(),
        reason: hostile.into(),
        rule: "S\u{1b}]0;pwned-title\u{7}-16".into(),
        ..decision("")
    }));
    a.live.on_event(&Event::ProtocolError {
        reason: hostile.into(),
    });
    let (a, _) = feed(a, vec![ctrl('d'), key(KeyCode::Tab)]);
    for (w, h) in SIZES {
        let t = screen_text(&a, w, h);
        assert_clean(&t);
        assert!(
            !t.contains("pwned-title") && !t.contains("http://evil.example"),
            "{t}"
        );
    }
    let t = screen_text(&a, 200, 50);
    assert!(
        t.contains("ok\u{FFFD}") || t.contains("evil"),
        "the visible part is still there\n{t}"
    );
    assert!(t.contains("line3"), "{t}");
    // The timeline too.
    let (a, _) = feed(a, vec![key(KeyCode::Tab)]);
    assert_clean(&screen_text(&a, 120, 30));
}

// ------------------------------------------------------------ layouts

#[test]
fn the_live_screen_has_two_columns_from_100_and_one_below() {
    let a = live_app();
    for (w, h) in [(100, 30), (120, 30), (200, 50)] {
        let t = screen_text(&a, w, h);
        assert_fits(&t, w, h);
        assert!(t.contains("─ uso") && t.contains("─ contexto"), "{w}: {t}");
        assert!(
            t.contains("sistema") && t.contains("externo") && t.contains("pasos"),
            "{w}: {t}"
        );
        assert!(
            t.contains("read_file") && t.contains("línea de tiempo"),
            "{w}: {t}"
        );
        // The timeline is on the left of the same row as the first layer of the context.
        let row = t.lines().find(|l| l.contains("sistema")).unwrap();
        assert!(row.find('│').is_some(), "{row}");
    }
    for (w, h) in [(60, 16), (80, 24), (99, 30)] {
        let t = screen_text(&a, w, h);
        assert_fits(&t, w, h);
        assert!(!t.contains("─ uso"), "{w}: one column\n{t}");
        assert!(t.contains("read_file") || w < 80, "{w}: {t}");
    }
}

#[test]
fn every_screen_fits_every_size_without_overflow() {
    let (new_run, _) = feed(with_runs(), vec![ch('n')]);
    let mut doctor = with_runs();
    doctor.screen = Screen::Doctor;
    doctor.doctor.set(report());
    let mut finished = live_app();
    finished
        .live
        .finish(Ok(super::tests::summary(ID1, "Final", &["a.py"])));
    let (debug_view, _) = feed(live_app(), vec![ctrl('d'), key(KeyCode::Tab)]);
    let screens: Vec<(&str, App)> = vec![
        ("inicio", with_runs()),
        ("inicio vacío", super::tests::app()),
        ("nuevo", new_run),
        ("en vivo", live_app()),
        ("en vivo terminado", finished),
        ("debug", debug_view),
        (
            "detalle",
            open_detail(with_runs(), detail(ID1, Status::Pending), false),
        ),
        (
            "diff",
            open_detail(with_runs(), detail(ID1, Status::Pending), true),
        ),
        ("contexto", feed(live_app(), vec![ch('c')]).0),
        ("uso", feed(live_app(), vec![ch('U')]).0),
        ("ayuda", feed(live_app(), vec![ch('?')]).0),
        ("doctor", doctor),
    ];
    for (name, a) in &screens {
        for (w, h) in SIZES {
            let t = screen_text(a, w, h);
            assert_fits(&t, w, h);
            assert_clean(&t);
            assert!(!t.contains("Terminal muy pequeña"), "{name} {w}x{h}");
            // The hint line and the status bar are always the last two lines.
            let n = t.lines().count();
            assert!(
                t.lines()
                    .nth(n - 2)
                    .is_some_and(|l| !l.trim().is_empty() || *name == "nuevo"),
                "{name} {w}x{h}\n{t}"
            );
        }
    }
}

#[test]
fn the_header_is_one_line_with_folder_model_screen_and_the_debug_mark() {
    let t = screen_text(&with_runs(), 80, 24);
    let first = t.lines().next().unwrap();
    assert!(
        first.starts_with("clio · proyecto · beam-free"),
        "{first:?}"
    );
    assert!(
        first.trim_end().ends_with("inicio") && !first.contains("debug"),
        "{first:?}"
    );
    assert!(
        t.lines().nth(1).unwrap().trim().is_empty(),
        "air under the header"
    );
    let (a, _) = feed(with_runs(), vec![ctrl('d')]);
    let first = screen_text(&a, 80, 24).lines().next().unwrap().to_owned();
    assert!(
        first.contains("debug") && first.trim_end().ends_with("inicio"),
        "{first:?}"
    );
    // The folder follows the run on screen.
    let mut d = detail(ID1, Status::Pending);
    d.info.workspace = "/srv/otro-proyecto".into();
    let a = open_detail(with_runs(), d, false);
    assert!(
        screen_text(&a, 80, 24)
            .lines()
            .next()
            .unwrap()
            .contains("otro-proyecto")
    );
}

#[test]
fn the_help_and_quit_hints_survive_the_narrowest_terminal() {
    let t = screen_text(&with_runs(), 60, 16);
    let hints = t.lines().nth(14).unwrap();
    assert!(
        hints.contains("? ayuda") && hints.contains("q salir"),
        "{hints:?}"
    );
    for (name, a) in [
        ("en vivo", live_app()),
        (
            "detalle",
            open_detail(with_runs(), detail(ID1, Status::Pending), false),
        ),
        (
            "diff",
            open_detail(with_runs(), detail(ID1, Status::Pending), true),
        ),
        ("contexto", feed(live_app(), vec![ch('c')]).0),
        ("doctor", feed(with_runs(), vec![ch('D')]).0),
    ] {
        let t = screen_text(&a, 60, 16);
        let hints = t.lines().nth(14).unwrap();
        assert!(hints.contains("? ayuda"), "{name}: {hints:?}");
    }
}

#[test]
fn the_hint_line_comes_from_the_key_map() {
    let t = screen_text(&live_app(), 80, 24);
    let hints = t.lines().nth(22).unwrap();
    for want in [
        "Ctrl-C",
        "cancelar",
        "Tab",
        "c contexto",
        "U uso",
        "? ayuda",
    ] {
        assert!(hints.contains(want), "{want:?} in {hints:?}");
    }
    let (a, _) = feed(live_app(), vec![ch('c')]);
    let t = screen_text(&a, 80, 24);
    assert!(t.lines().nth(22).unwrap().contains("U ver el uso"));
}

// ------------------------------------------------------------ pictures for the docs

/// Prints every screen as text (for the docs): `cargo test -p clio-brain --bin clio print_screens -- --ignored --nocapture`.
#[test]
#[ignore = "prints the screens; not a check"]
fn print_screens() {
    use super::tests::{app, summary};
    let show = |name: &str, a: &App| println!("===== {name} =====\n{}", screen_text(a, 80, 24));
    show("inicio", &with_runs());
    let (mut a, _) = feed(with_runs(), vec![ch('n')]);
    a.new_run.instruction = "arregla parse_date en dates.py".into();
    a.new_run.check = Some(Ok(super::new_run::Preview {
        workspace: "/home/ivan/proyecto".into(),
        files: 12,
        bytes: 3 * 1024 * 1024,
        secrets: vec![".env".into()],
        skipped: vec![".git".into(), "node_modules".into()],
        symlinks: vec![],
    }));
    show("nuevo", &a);
    show("en vivo", &live_app());
    let (a, _) = feed(live_app(), vec![ch('c')]);
    show("contexto", &a);
    let (a, _) = feed(live_app(), vec![ch('U')]);
    show("uso", &a);
    let (a, _) = feed(live_app(), vec![ctrl('d'), key(KeyCode::Tab)]);
    show("debug", &a);
    show(
        "detalle",
        &open_detail(with_runs(), detail(ID1, Status::Pending), false),
    );
    show(
        "detalle con diff",
        &open_detail(with_runs(), detail(ID1, Status::Pending), true),
    );
    let (a, _) = feed(
        open_detail(with_runs(), detail(ID1, Status::Pending), false),
        vec![
            ch('a'),
            Input::Bg(Bg::Planned(Ok(super::app::Planned {
                id: ID1.into(),
                files: 2,
                workspace: "/home/ivan/proyecto".into(),
            }))),
        ],
    );
    show("confirmar", &a);
    let mut a = with_runs();
    a.screen = Screen::Doctor;
    a.doctor.set(report());
    show("doctor", &a);
    let (a, _) = feed(with_runs(), vec![ch('?')]);
    show("ayuda (inicio)", &a);
    let (a, _) = feed(live_app(), vec![ch('?')]);
    show("ayuda (en vivo)", &a);
    let mut f = live_app();
    f.live.finish(Ok(summary(ID1, "Final", &["dates.py"])));
    show("en vivo terminado", &f);
    println!(
        "===== en vivo 120x30 =====\n{}",
        screen_text(&live_app(), 120, 30)
    );
    println!(
        "===== en vivo 60x16 =====\n{}",
        screen_text(&live_app(), 60, 16)
    );
    show("inicio sin runs", &app());
}

// ------------------------------------------------------------ secrets never reach a view

fn b64(s: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in s.as_bytes().chunks(3) {
        let n = c
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            out.push(if i <= c.len() {
                T[((n >> (18 - 6 * i)) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

struct Collect(std::sync::Arc<std::sync::Mutex<Vec<Event>>>);

impl clio_brain::agent::Observer for Collect {
    fn event(&mut self, e: &Event) {
        self.0.lock().unwrap().push(e.clone());
    }
}

fn app_with(events: &[Event]) -> App {
    let mut a = with_runs();
    a.screen = Screen::Live;
    a.live = Live::starting(0, "mira");
    let mut inputs = vec![Input::Bg(started())];
    inputs.extend(events.iter().cloned().map(|e| Input::Bg(Bg::Agent(e))));
    let (mut a, _) = feed(a, inputs);
    a.now_ms = 5_000;
    a.debug = true;
    a
}

/// Every view of a run, at every size.
fn screens_for(events: Vec<Event>) -> Vec<String> {
    let mut texts = Vec::new();
    for keys in [
        vec![],
        vec![key(KeyCode::Tab)],
        vec![ch('c')],
        vec![ch('U')],
        vec![ch('?')],
    ] {
        let (b, _) = feed(app_with(&events), keys);
        for (w, h) in SIZES {
            texts.push(screen_text(&b, w, h));
        }
    }
    texts
}

#[test]
fn a_decoy_in_an_answer_a_file_name_a_tool_call_or_a_reason_reaches_no_view_no_log() {
    use crate::cli::debug::{DebugCtl, Tee};
    use clio_canaries::Canaries;
    use std::sync::{Arc, Mutex};
    let canaries = Canaries::from_seed(5);
    let decoy = canaries.value("github").unwrap().to_owned();
    let url: String = decoy.bytes().map(|b| format!("%{b:02X}")).collect();
    let forms = format!("{decoy} {} {url}", b64(&decoy));
    let raw = vec![
        Event::Started {
            level: TaskLevel::Simple,
        },
        Event::Context(ContextReport {
            external: vec![ContextItem {
                name: format!("claves/{decoy}.txt"),
                tokens: 40,
            }],
            ..context(1)
        }),
        Event::Usage(call(1, 100, 10, 0, 100, 100)),
        Event::Exchange(Exchange {
            call: 1,
            messages: 3,
            request_chars: 99,
            request: vec![Msg {
                role: "user".into(),
                content: format!("Result of read_file: {forms}"),
            }],
            response: format!("{{\"tool\":\"read_file\",\"note\":\"{forms}\"}}"),
        }),
        Event::Step { n: 1 },
        Event::Decision(DecisionDebug {
            args: format!("{{\"path\":\"{forms}\"}}"),
            reason: format!("path {forms} is protected"),
            ..decision("")
        }),
        Event::ProtocolError {
            reason: format!("bad {forms}"),
        },
        Event::Tool {
            name: "read_file".into(),
            target: forms.clone(),
            outcome: Outcome::Deny,
            rule: "S-16".into(),
            ran: false,
            result_chars: 1,
            tainted: true,
        },
        Event::Final,
    ];
    // Without the scrubber the decoy IS on the debug screen: the test can fail.
    let leaked = screens_for(raw.clone())
        .iter()
        .any(|t| !canaries.scan("screen", t).is_empty());
    assert!(
        leaked,
        "the control case must leak, or this test proves nothing"
    );
    // Through the Tee, with the decoy known to the scrubber (as the API key is): nothing leaks.
    let tmp = tempfile::tempdir().unwrap();
    let scrubber = clio_vault::Scrubber::default();
    scrubber.add("github", &decoy);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut tee = Tee::new(
        Box::new(Collect(seen.clone())),
        Arc::new(Mutex::new(RunUsage::default())),
        scrubber,
        DebugCtl::new(true, false),
        tmp.path().join("debug.log"),
    );
    for e in &raw {
        clio_brain::agent::Observer::event(&mut tee, e);
    }
    let clean = seen.lock().unwrap().clone();
    for (i, t) in screens_for(clean).iter().enumerate() {
        canaries.assert_clean(&format!("screen {i}"), t);
        assert_clean(t);
    }
    canaries.assert_clean(
        "debug.log",
        &std::fs::read_to_string(tmp.path().join("debug.log")).unwrap(),
    );
}
