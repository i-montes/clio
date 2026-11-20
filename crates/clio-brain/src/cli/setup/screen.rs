//! The installer's screens: one decision each, drawn with the chat's theme, readable at 80x24.

use std::io::Stdout;
use std::time::{Duration, Instant};

use clio_vault::SecretValue;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::fetch::Pins;
use super::steps::Action;
use super::text::{Lang, M};
use super::{Answer, Ask, Found, Opts, St, Sys, Ui};
use crate::cli::Res;
use crate::cli::home::Dirs;
use crate::cli::tui::text::{fit, width};
use crate::cli::tui::theme::Theme;
use crate::cli::tui::{self, Guard};

/// What one screen shows.
pub enum Page {
    Language {
        sel: usize,
    },
    Choice {
        title: String,
        lines: Vec<String>,
        options: Vec<String>,
        sel: usize,
    },
    Info {
        title: String,
        lines: Vec<String>,
    },
    /// A checklist: `checked` one per option, `sel` the one under the cursor.
    Checks {
        title: String,
        lines: Vec<String>,
        options: Vec<String>,
        checked: Vec<bool>,
        sel: usize,
    },
    Plan {
        title: String,
        items: Vec<String>,
    },
    Doing,
    Secret {
        title: String,
        lines: Vec<String>,
        labels: Vec<String>,
        values: Vec<SecretValue>,
        focus: usize,
        error: Option<String>,
    },
    Text {
        title: String,
        lines: Vec<String>,
        label: String,
        value: String,
        error: Option<String>,
    },
    Done {
        title: String,
        ok: bool,
        lines: Vec<String>,
        chat: bool,
    },
}

pub struct View {
    /// `None` until the person chose one.
    pub lang: Option<Lang>,
    pub page: Page,
    pub title: String,
    pub rows: Vec<(String, St)>,
    pub now: u64,
}

impl View {
    pub fn new(lang: Option<Lang>) -> Self {
        Self {
            lang,
            page: Page::Language { sel: 0 },
            title: String::new(),
            rows: Vec::new(),
            now: 0,
        }
    }

    fn t(&self, m: M) -> &'static str {
        m.get(self.lang.unwrap_or(Lang::En))
    }
}

/// The longest a secret field takes (a token is about 110 characters).
const SECRET_CAP: usize = 4096;

fn wrap(text: &str, w: usize) -> Vec<String> {
    crate::cli::ui::wrap(text, w)
}

pub(super) fn mb(n: u64) -> String {
    format!("{:.0}", n as f64 / 1_000_000.0)
}

/// A step's row: its mark, its label in a column, and its note (a progress bar while it
/// downloads; a failure in up to two lines and what to do).
fn row(label: &str, st: &St, t: Theme, w: usize, now: u64) -> Vec<Line<'static>> {
    let col = 30.min(w / 2);
    let note_w = w.saturating_sub(5 + col);
    let (mark, style) = match st {
        St::Waiting => (if t.unicode { "○" } else { "o" }, t.faint()),
        St::Working(_) => (t.spinner(now), t.accent()),
        St::Done(_) => (t.check(), t.ok()),
        St::Warn(_) => (t.bang(), t.warn()),
        St::Skip(_) => (t.dot(), t.dim()),
        St::Failed(..) => (t.cross(), t.fail()),
    };
    let label = fit(label, col - 1, t.ellipsis());
    let pad = " ".repeat(col - width(&label));
    let head = |note: Vec<Span<'static>>| {
        let mut v = vec![
            Span::raw("  "),
            Span::styled(mark.to_owned(), style),
            Span::raw(" "),
            Span::styled(
                format!("{label}{pad}"),
                if matches!(st, St::Waiting) {
                    t.dim()
                } else {
                    t.plain()
                },
            ),
        ];
        v.extend(note);
        Line::from(v)
    };
    let under =
        |text: String, s| Line::from(vec![Span::raw(" ".repeat(4 + col)), Span::styled(text, s)]);
    match st {
        St::Waiting | St::Working(None) => vec![head(vec![])],
        St::Working(Some((got, total))) => {
            let text = if *total > 0 {
                let pct = got.saturating_mul(100) / total.max(&1);
                format!("{} {}/{} MB", t.bar(pct, 12), mb(*got), mb(*total))
            } else {
                format!("{} MB", mb(*got))
            };
            vec![head(vec![Span::styled(fit(&text, note_w, ""), t.dim())])]
        }
        St::Done(n) | St::Skip(n) => {
            vec![head(vec![Span::styled(
                fit(n, note_w, t.ellipsis()),
                t.dim(),
            )])]
        }
        St::Warn(n) => {
            let mut parts = wrap(n, note_w).into_iter().take(2);
            let first = parts.next().unwrap_or_default();
            let mut v = vec![head(vec![Span::styled(
                fit(&first, note_w, t.ellipsis()),
                t.warn(),
            )])];
            v.extend(parts.map(|p| under(fit(&p, note_w, t.ellipsis()), t.warn())));
            v
        }
        St::Failed(why, fix) => {
            let mut parts = wrap(why, note_w).into_iter().take(2);
            let first = parts.next().unwrap_or_default();
            let mut v = vec![head(vec![Span::styled(
                fit(&first, note_w, t.ellipsis()),
                t.fail(),
            )])];
            v.extend(parts.map(|p| under(fit(&p, note_w, t.ellipsis()), t.fail())));
            let fix = format!("{} {fix}", t.arrow());
            v.extend(
                wrap(&fix, note_w)
                    .into_iter()
                    .take(2)
                    .map(|p| under(fit(&p, note_w, t.ellipsis()), t.dim())),
            );
            v
        }
    }
}

fn options(opts: &[String], sel: usize, t: Theme, w: usize) -> Vec<Line<'static>> {
    opts.iter()
        .enumerate()
        .map(|(i, o)| {
            let o = fit(o, w.saturating_sub(6), t.ellipsis());
            if i == sel {
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(format!("{} {o}", t.sel()), t.accent()),
                ])
            } else {
                Line::from(format!("    {o}"))
            }
        })
        .collect()
}

/// Every line of the screen but the last (the keys).
pub fn lines(v: &View, t: Theme, w: usize) -> Vec<Line<'static>> {
    let inner = w.saturating_sub(4);
    let mut out = vec![
        Line::from(vec![
            Span::styled(
                format!(" {} ", if t.unicode { "◆" } else { "*" }),
                t.accent(),
            ),
            Span::styled("Clio", t.bold()),
            Span::styled(
                format!(
                    "  {} · {}",
                    v.lang
                        .map_or("installer · instalador", |l| M::Installer.get(l)),
                    env!("CARGO_PKG_VERSION")
                ),
                t.dim(),
            ),
        ]),
        Line::styled(t.rule().repeat(w), t.faint()),
        Line::default(),
    ];
    let title = |s: &str, style| {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(fit(s, inner, t.ellipsis()), style),
        ])
    };
    let text = |out: &mut Vec<Line<'static>>, lines: &[String], style| {
        for l in lines {
            for p in wrap(l, inner) {
                out.push(Line::from(vec![Span::raw("  "), Span::styled(p, style)]));
            }
        }
    };
    match &v.page {
        Page::Language { sel } => {
            out.push(title("Idioma · Language", t.bold()));
            out.push(Line::default());
            out.extend(options(&["Español".into(), "English".into()], *sel, t, w));
        }
        Page::Choice {
            title: tt,
            lines,
            options: o,
            sel,
        } => {
            out.push(title(tt, t.bold()));
            text(&mut out, lines, t.dim());
            out.push(Line::default());
            out.extend(options(o, *sel, t, w));
        }
        Page::Info { title: tt, lines } => {
            out.push(title(tt, t.bold()));
            out.push(Line::default());
            text(&mut out, lines, t.plain());
        }
        Page::Checks {
            title: tt,
            lines,
            options: o,
            checked,
            sel,
        } => {
            out.push(title(tt, t.bold()));
            text(&mut out, lines, t.dim());
            out.push(Line::default());
            let marked: Vec<String> = o
                .iter()
                .zip(checked)
                .map(|(o, c)| format!("[{}] {o}", if *c { "x" } else { " " }))
                .collect();
            out.extend(options(&marked, *sel, t, w));
        }
        Page::Plan { title: tt, items } => {
            out.push(title(tt, t.bold()));
            out.push(Line::default());
            for i in items {
                out.push(Line::from(vec![
                    Span::styled(format!("  {} ", t.dot()), t.dim()),
                    Span::raw(fit(i, inner, t.ellipsis())),
                ]));
            }
        }
        Page::Doing => {
            out.push(title(&v.title, t.bold()));
            out.push(Line::default());
            for (label, st) in &v.rows {
                out.extend(row(label, st, t, w, v.now));
            }
        }
        Page::Secret {
            title: tt,
            lines,
            labels,
            values,
            focus,
            error,
        } => {
            out.push(title(tt, t.bold()));
            text(&mut out, lines, t.dim());
            out.push(Line::default());
            let lw = labels.iter().map(|l| width(l)).max().unwrap_or(0) + 2;
            for (i, l) in labels.iter().enumerate() {
                let n = values.get(i).map_or(0, SecretValue::chars);
                let dots =
                    (if t.unicode { "•" } else { "*" }).repeat(n.min(inner.saturating_sub(lw + 1)));
                let cur = if i == *focus { t.cursor() } else { "" };
                out.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{l:<lw$}"),
                        if i == *focus { t.accent() } else { t.dim() },
                    ),
                    Span::raw(dots),
                    Span::styled(cur.to_owned(), t.accent()),
                ]));
            }
            if let Some(e) = error {
                out.push(Line::default());
                text(&mut out, std::slice::from_ref(e), t.fail());
            }
        }
        Page::Text {
            title: tt,
            lines,
            label,
            value,
            error,
        } => {
            out.push(title(tt, t.bold()));
            text(&mut out, lines, t.dim());
            out.push(Line::default());
            let room = inner.saturating_sub(width(label) + 3);
            // The end of a long value stays in view, where the cursor is.
            let shown: String = {
                let chars: Vec<char> = value.chars().collect();
                let skip = chars.len().saturating_sub(room);
                chars[skip..].iter().collect()
            };
            out.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{label}  "), t.accent()),
                Span::raw(crate::cli::tui::sanitize::line(&shown)),
                Span::styled(t.cursor().to_owned(), t.accent()),
            ]));
            if let Some(e) = error {
                out.push(Line::default());
                text(&mut out, std::slice::from_ref(e), t.fail());
            }
        }
        Page::Done {
            title: tt,
            ok,
            lines,
            ..
        } => {
            out.push(title(tt, if *ok { t.ok() } else { t.fail() }));
            out.push(Line::default());
            text(&mut out, lines, t.plain());
            // Only what failed: what is left is the one line above.
            if v.rows.iter().any(|(_, st)| matches!(st, St::Failed(..))) {
                out.push(Line::default());
                for (label, st) in &v.rows {
                    if matches!(st, St::Failed(..)) {
                        out.extend(row(label, st, t, w, v.now));
                    }
                }
            }
        }
    }
    out
}

/// The keys the screen takes, on its last line.
pub fn keys(v: &View) -> &'static str {
    match &v.page {
        Page::Language { .. } => "↑↓ · Enter · Esc",
        Page::Choice { .. } => v.t(M::KeysChoose),
        Page::Info { .. } => v.t(M::KeysGo),
        Page::Checks { .. } => v.t(M::KeysChecks),
        Page::Plan { .. } => v.t(M::PlanKeys),
        Page::Doing => v.t(M::KeysWait),
        Page::Secret { .. } | Page::Text { .. } => v.t(M::KeysField),
        Page::Done { chat: true, .. } => v.t(M::KeysDone),
        Page::Done { .. } => v.t(M::KeysExit),
    }
}

pub fn draw(f: &mut ratatui::Frame, v: &View, t: Theme) {
    let a = f.area();
    let w = usize::from(a.width);
    let body = Rect {
        height: a.height.saturating_sub(1),
        ..a
    };
    let mut ls = lines(v, t, w);
    // A locale that is not UTF-8 gets plain ASCII, not broken characters.
    if !t.unicode {
        for l in &mut ls {
            for sp in &mut l.spans {
                sp.content = super::text::ascii(&sp.content).into();
            }
        }
    }
    f.render_widget(Paragraph::new(ls), body);
    let k = keys(v);
    let k = if t.unicode {
        k.to_owned()
    } else {
        super::text::ascii(k)
    };
    f.render_widget(
        Paragraph::new(Line::styled(
            format!("  {}", fit(&k, w.saturating_sub(2), "")),
            t.dim(),
        )),
        Rect {
            y: a.y + a.height.saturating_sub(1),
            height: 1.min(a.height),
            ..a
        },
    );
}

// ---- the terminal ------------------------------------------------------------------------------

pub struct Tui {
    term: Option<Terminal<CrosstermBackend<Stdout>>>,
    guard: Option<Guard>,
    pub view: View,
    theme: Theme,
    t0: Instant,
}

impl Tui {
    fn enter(lang: Option<Lang>) -> Res<Self> {
        let (guard, term) = Guard::enter(false)?;
        Ok(Self {
            term: Some(term),
            guard: Some(guard),
            view: View::new(lang),
            theme: Theme::from_env(),
            t0: Instant::now(),
        })
    }

    fn draw(&mut self) {
        self.view.now = u64::try_from(self.t0.elapsed().as_millis()).unwrap_or(0);
        let (v, t) = (&self.view, self.theme);
        if let Some(term) = self.term.as_mut() {
            let _ = term.draw(|f| draw(f, v, t));
        }
    }

    fn event(wait: Duration) -> Option<Event> {
        event::poll(wait).ok()?.then(event::read)?.ok()
    }

    /// Shows `page` until Enter (it comes back as the person left it) or Esc (`None`). Keys
    /// pressed before it showed (during a download, `sudo`'s output) are dropped: they were not
    /// meant for it.
    fn show(&mut self, page: Page) -> Option<Page> {
        while event::poll(Duration::ZERO).unwrap_or(false) {
            let _ = event::read();
        }
        self.view.page = page;
        let lang = self.view.lang.unwrap_or(Lang::En);
        loop {
            self.draw();
            let Some(ev) = Self::event(Duration::from_millis(100)) else {
                continue;
            };
            let typed: Vec<char> = match ev {
                Event::Paste(s) => s.chars().filter(|c| !c.is_control()).collect(),
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                    match k.code {
                        KeyCode::Esc => return self.leave(false),
                        KeyCode::Char('c') if ctrl => return self.leave(false),
                        KeyCode::Up | KeyCode::Down => {
                            let down = k.code == KeyCode::Down;
                            match &mut self.view.page {
                                Page::Language { sel } => *sel = usize::from(down),
                                Page::Choice { options, sel, .. }
                                | Page::Checks { options, sel, .. } => {
                                    *sel = if down {
                                        (*sel + 1).min(options.len().saturating_sub(1))
                                    } else {
                                        sel.saturating_sub(1)
                                    };
                                }
                                _ => {}
                            }
                            continue;
                        }
                        KeyCode::Backspace => {
                            match &mut self.view.page {
                                Page::Secret { values, focus, .. } => {
                                    if let Some(v) = values.get_mut(*focus) {
                                        v.pop();
                                    }
                                }
                                Page::Text { value, .. } => {
                                    value.pop();
                                }
                                _ => {}
                            }
                            continue;
                        }
                        KeyCode::Enter => {
                            if self.enter_pressed(lang) {
                                return self.leave(true);
                            }
                            continue;
                        }
                        KeyCode::Char(c) if !ctrl => vec![c],
                        _ => continue,
                    }
                }
                _ => continue,
            };
            match &mut self.view.page {
                Page::Secret { values, focus, .. } => {
                    if let Some(v) = values.get_mut(*focus) {
                        for c in typed {
                            v.push(c);
                        }
                    }
                }
                Page::Checks { checked, sel, .. } if typed == [' '] => {
                    if let Some(c) = checked.get_mut(*sel) {
                        *c = !*c;
                    }
                }
                Page::Text { value, .. } => value.extend(typed),
                _ => {}
            }
        }
    }

    /// Enter on the page: whether it is answered (a field moves to the next one first, and a
    /// passphrase typed twice must match).
    fn enter_pressed(&mut self, lang: Lang) -> bool {
        let Page::Secret {
            values,
            focus,
            error,
            ..
        } = &mut self.view.page
        else {
            return true;
        };
        if *focus + 1 < values.len() {
            *focus += 1;
            return false;
        }
        let bad = if values.first().is_none_or(SecretValue::is_empty) {
            Some(M::ErrEmpty)
        } else if values.len() == 2 && values[0].expose() != values[1].expose() {
            Some(M::ErrMismatch)
        } else {
            None
        };
        if let Some(m) = bad {
            *error = Some(m.get(lang).to_owned());
            for v in values.iter_mut() {
                *v = SecretValue::with_capacity(SECRET_CAP);
            }
            *focus = 0;
            return false;
        }
        true
    }

    fn leave(&mut self, answered: bool) -> Option<Page> {
        let p = std::mem::replace(&mut self.view.page, Page::Doing);
        answered.then_some(p)
    }

    fn close(&mut self) {
        self.term = None;
        self.guard = None;
    }
}

impl Ui for Tui {
    fn set(&mut self, i: usize, st: St) {
        if let Some(r) = self.view.rows.get_mut(i) {
            r.1 = st;
        }
        self.draw();
    }

    fn ask(&mut self, q: Ask) -> Option<Answer> {
        let page = match q {
            Ask::Choice {
                title,
                lines,
                options,
                default,
            } => Page::Choice {
                title,
                lines,
                options,
                sel: default,
            },
            Ask::Secret {
                title,
                lines,
                labels,
                error,
            } => Page::Secret {
                values: labels
                    .iter()
                    .map(|_| SecretValue::with_capacity(SECRET_CAP))
                    .collect(),
                title,
                lines,
                labels,
                focus: 0,
                error,
            },
            Ask::Text {
                title,
                lines,
                label,
                value,
                error,
            } => Page::Text {
                title,
                lines,
                label,
                value,
                error,
            },
            Ask::Checks {
                title,
                lines,
                options,
            } => Page::Checks {
                checked: vec![false; options.len()],
                title,
                lines,
                options,
                sel: 0,
            },
        };
        let answer = match self.show(page)? {
            Page::Choice { sel, .. } => Answer::Choice(sel),
            Page::Secret { mut values, .. } => Answer::Secret(values.swap_remove(0)),
            Page::Text { value, .. } => Answer::Text(value.trim().to_owned()),
            Page::Checks { checked, .. } => Answer::Checks(checked),
            _ => return None,
        };
        self.draw();
        Some(answer)
    }

    fn outside(&mut self, f: &mut dyn FnMut()) {
        self.close();
        f();
        if let Ok((g, term)) = Guard::enter(false) {
            self.guard = Some(g);
            self.term = Some(term);
        }
        self.draw();
    }

    fn busy(&mut self, f: &mut (dyn FnMut() + Send)) {
        std::thread::scope(|s| {
            let h = s.spawn(f);
            while !h.is_finished() {
                self.draw();
                // Keys typed meanwhile are dropped: nothing waits for them.
                let _ = Self::event(Duration::from_millis(100));
            }
        });
    }
}

// ---- the wizard --------------------------------------------------------------------------------

/// The screens, in order; `resume`: the new binary an Update started goes straight on.
pub fn wizard(
    dirs: &Dirs,
    lang: Option<Lang>,
    sys: Sys,
    pins: Pins,
    mut opts: Opts,
    resume: bool,
) -> Res<i32> {
    tui::install_panic_hook();
    let mut ui = Tui::enter(lang)?;
    let lang = match lang {
        Some(l) => l,
        None => {
            let sel = usize::from(Lang::saved_or_env(dirs) == Lang::En);
            match ui.show(Page::Language { sel }) {
                Some(Page::Language { sel: 0 }) => Lang::Es,
                Some(_) => Lang::En,
                None => return Ok(0),
            }
        }
    };
    ui.view.lang = Some(lang);
    let t = |m: M| m.get(lang).to_owned();
    let action = if resume {
        Action::Update
    } else {
        match ui.show(found_page(dirs, &sys, lang)) {
            None => return Ok(0),
            Some(Page::Choice { sel, .. }) => [
                Action::Update,
                Action::Repair,
                Action::RemoveKeep,
                Action::RemoveAll,
            ][sel.min(3)],
            Some(_) => Action::Install,
        }
    };
    if action == Action::RemoveAll {
        let sure = ui.show(Page::Choice {
            title: t(M::ConfirmTitle),
            lines: vec![t(M::ConfirmLost)],
            options: vec![t(M::ConfirmNo), t(M::ConfirmYes)],
            sel: 0,
        });
        if !matches!(sure, Some(Page::Choice { sel: 1, .. })) {
            return Ok(0);
        }
    }
    // Update and both removals wait for every chat to close (its run folder's lock tells).
    while action != Action::Install && crate::cli::pending::any_open(dirs) {
        let p = Page::Info {
            title: t(M::ChatOpenTitle),
            lines: vec![t(M::ChatOpenHint)],
        };
        if ui.show(p).is_none() {
            return Ok(0);
        }
    }
    let labels: Vec<String> = action.shown().iter().map(|s| t(s.label())).collect();
    if !resume {
        let p = Page::Plan {
            title: t(M::PlanTitle),
            items: labels.clone(),
        };
        if ui.show(p).is_none() {
            return Ok(0);
        }
    }
    ui.view.title = t(M::DoingTitle);
    ui.view.rows = labels.into_iter().map(|l| (l, St::Waiting)).collect();
    ui.view.page = Page::Doing;
    opts.update = action == Action::Update;
    // Every question first; after the last answer nothing stops until the last screen.
    let (failed, replaced, left, blocked) = {
        let mut r = super::run_with(dirs, lang, sys, pins, opts.clone(), &mut ui);
        let steps = action.steps();
        r.ask_all(&steps);
        r.all(&steps);
        r.sign_in(&steps);
        (r.failed, r.replaced, r.left, r.blocked)
    };
    if replaced {
        ui.close();
        return super::reexec(
            &opts.exe,
            &["setup", "--lang", lang.code(), "--resume-update"],
        );
    }
    // Not when the jail cannot start: the next run offers Repair, which tries it again.
    if !failed && blocked.is_none() && action != Action::RemoveAll {
        super::mark_version(dirs);
    }
    let removed = matches!(action, Action::RemoveKeep | Action::RemoveAll);
    let on_path =
        std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d == opts.bin));
    let end = End {
        failed,
        blocked,
        left,
        new_path: !on_path && !removed,
    };
    let page = done_page(dirs, lang, action, &end);
    let chat = !failed && !removed && blocked.is_none();
    let go = ui.show(page).is_some();
    ui.close();
    if failed || blocked.is_some() {
        return Ok(1);
    }
    if go && chat {
        // The person types nothing else: this process becomes the chat.
        let h = crate::cli::chat::resolve(None, dirs)?;
        return tui::run(dirs.clone(), false, h, None, false);
    }
    Ok(0)
}

/// What is on this machine: nothing (Enter goes on), or an installation and what to do with it.
pub fn found_page(dirs: &Dirs, sys: &Sys, lang: Lang) -> Page {
    let t = |m: M| m.get(lang).to_owned();
    let f = Found::detect(dirs, sys);
    let yn = |b: bool| t(if b { M::Yes } else { M::No });
    if !f.installed {
        return Page::Info {
            title: t(M::FoundNothing),
            lines: vec![
                t(M::FoundNothingLine),
                String::new(),
                M::FoundSystem.fill(
                    lang,
                    &[&format!(
                        "{} {}",
                        std::env::consts::OS,
                        std::env::consts::ARCH
                    )],
                ),
                M::FoundJail.fill(lang, &[&t(if f.bwrap { M::Present } else { M::Missing })]),
            ],
        };
    }
    Page::Choice {
        title: M::FoundInstalled
            .fill(lang, &[f.version.as_deref().unwrap_or("")])
            .replace("  ", " "),
        lines: vec![
            M::FoundData.fill(lang, &[&dirs.data.display().to_string()]),
            M::FoundVault.fill(lang, &[&yn(f.vault)]),
            M::FoundTimer.fill(lang, &[&yn(f.timer)]),
            String::new(),
            t(M::FoundAsk),
        ],
        options: vec![
            t(M::Update),
            t(M::Repair),
            t(M::RemoveKeep),
            t(M::RemoveAll),
        ],
        // No version noted: a run that did not finish (or a removal that kept the data, or a
        // build from source): Repair goes through every step.
        sel: usize::from(f.version.is_none()),
    }
}

/// How a run ended, for the last screen.
pub struct End {
    pub failed: bool,
    /// The jail cannot start: what to ask an administrator for.
    pub blocked: Option<M>,
    pub left: Option<M>,
    /// `clio`'s folder was not on the PATH: the next terminal finds it.
    pub new_path: bool,
}

/// The last screen: its title, the one thing left (if any) in one line, and where `clio` is
/// found from now on.
pub fn done_page(dirs: &Dirs, lang: Lang, action: Action, end: &End) -> Page {
    let t = |m: M| m.get(lang).to_owned();
    let left = |m: M| M::DoneLeft.fill(lang, &[m.get(lang)]);
    let (title, mut lines) = if end.failed {
        (t(M::DoneFailed), vec![t(M::DoneFailedLine)])
    } else if let Some(b) = end.blocked {
        (t(M::DoneBlocked), vec![left(b)])
    } else if action == Action::RemoveKeep {
        let kept = format!(
            "{} · {} · {}",
            dirs.config.display(),
            dirs.data.display(),
            dirs.home.join(".cache/clio-models").display()
        );
        (
            t(M::RemovedTitle),
            vec![M::RemovedKept.fill(lang, &[&kept])],
        )
    } else if action == Action::RemoveAll {
        (t(M::RemovedTitle), vec![])
    } else {
        (t(M::DoneTitle), end.left.map(left).into_iter().collect())
    };
    if end.new_path {
        lines.push(t(M::DonePath));
    }
    let ok = !end.failed && end.blocked.is_none();
    Page::Done {
        title,
        ok,
        lines,
        chat: ok && !matches!(action, Action::RemoveKeep | Action::RemoveAll),
    }
}
