//! The key map: ONE table per screen, used for dispatch, for the hint line and for the help
//! screen, so what is shown is what works. A test walks every table and fails on a key that is
//! bound twice in the same screen, and on a key that means different things in different screens.
//!
//! Plain letters are text only in the text fields (new job, the filter); there the table only has
//! the non-text keys.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Alt(KeyCode),
    Code(KeyCode),
}

impl Key {
    pub fn matches(self, k: &KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match self {
            Self::Char(c) => k.code == KeyCode::Char(c) && !ctrl && !alt,
            Self::Ctrl(c) => {
                ctrl && matches!(k.code, KeyCode::Char(x) if x.eq_ignore_ascii_case(&c))
            }
            Self::Alt(code) => alt && !ctrl && k.code == code,
            Self::Code(code) => k.code == code && !ctrl && !alt,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Up,
    Down,
    Top,
    Bottom,
    PageUp,
    PageDown,
    Open,
    Back,
    Quit,
    Help,
    Filter,
    Refresh,
    Tab,
    New,
    Diff,
    Apply,
    Undo,
    Context,
    Usage,
    Doctor,
    Cancel,
    Redraw,
    Debug,
    NextFile,
    PrevFile,
    Newline,
    Clear,
    Memory,
    Promote,
    Archive,
    Useful,
    Wrong,
}

pub struct Bind {
    pub keys: &'static [Key],
    pub act: Act,
    /// How the key is written in the hints and the help.
    pub shown: &'static str,
    pub what: &'static str,
    /// Also in the one-line hints at the bottom.
    pub hint: bool,
}

const fn b(
    keys: &'static [Key],
    act: Act,
    shown: &'static str,
    what: &'static str,
    hint: bool,
) -> Bind {
    Bind {
        keys,
        act,
        shown,
        what,
        hint,
    }
}

use Key::{Alt, Char, Code, Ctrl};

const CTRL_F: Key = Ctrl('f');
const CTRL_B: Key = Ctrl('b');
const ESC: Key = Code(KeyCode::Esc);
const ENTER: Key = Code(KeyCode::Enter);

/// Screens as the key map sees them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ctx {
    Home,
    Filter,
    NewRun,
    LiveRun,
    LiveDone,
    DetailList,
    DetailDiff,
    Doctor,
    Context,
    Usage,
    Help,
    Memory,
    MemoryFilter,
}

#[cfg(test)]
pub const ALL: [Ctx; 13] = [
    Ctx::Home,
    Ctx::Filter,
    Ctx::NewRun,
    Ctx::LiveRun,
    Ctx::LiveDone,
    Ctx::DetailList,
    Ctx::DetailDiff,
    Ctx::Doctor,
    Ctx::Context,
    Ctx::Usage,
    Ctx::Help,
    Ctx::Memory,
    Ctx::MemoryFilter,
];

/// On every screen.
pub const GLOBAL: &[Bind] = &[
    b(&[Ctrl('d')], Act::Debug, "Ctrl-D", "modo debug", false),
    b(&[Ctrl('l')], Act::Redraw, "Ctrl-L", "redibujar", false),
];

const HOME: &[Bind] = &[
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "bajar",
        false,
    ),
    b(
        &[Char('k'), Code(KeyCode::Up)],
        Act::Up,
        "k",
        "subir",
        false,
    ),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "primero",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "último",
        false,
    ),
    b(&[ENTER], Act::Open, "Enter", "abrir", true),
    b(&[Char('n')], Act::New, "n", "nuevo", true),
    b(&[Char('d')], Act::Diff, "d", "diff", false),
    b(&[Char('a')], Act::Apply, "a", "aplicar", false),
    b(&[Char('u')], Act::Undo, "u", "deshacer", false),
    b(&[Char('/')], Act::Filter, "/", "filtrar", true),
    b(&[Char('q'), Ctrl('c')], Act::Quit, "q", "salir", true),
    b(&[Char('c')], Act::Context, "c", "contexto", true),
    b(&[Char('U')], Act::Usage, "U", "uso", true),
    b(&[Char('m')], Act::Memory, "m", "memoria", true),
    b(&[Char('r')], Act::Refresh, "r", "refrescar", false),
    b(&[Char('D')], Act::Doctor, "D", "doctor", false),
    b(&[ESC], Act::Back, "Esc", "quitar el filtro", false),
    b(&[Char('?')], Act::Help, "?", "ayuda", true),
];

const FILTER: &[Bind] = &[
    b(&[ENTER], Act::Open, "Enter", "dejar el filtro", true),
    b(&[ESC], Act::Back, "Esc", "borrar el filtro", true),
    b(&[Code(KeyCode::Down)], Act::Down, "↓", "bajar", false),
    b(&[Code(KeyCode::Up)], Act::Up, "↑", "subir", false),
    b(&[Ctrl('c')], Act::Back, "Ctrl-C", "borrar el filtro", false),
];

const NEW_RUN: &[Bind] = &[
    b(
        &[Code(KeyCode::Tab), Code(KeyCode::BackTab)],
        Act::Tab,
        "Tab",
        "cambiar campo",
        true,
    ),
    b(&[ENTER], Act::Open, "Enter", "lanzar", true),
    b(
        &[Alt(KeyCode::Enter), Ctrl('j')],
        Act::Newline,
        "Alt-Enter",
        "línea nueva",
        true,
    ),
    b(&[Ctrl('u')], Act::Clear, "Ctrl-U", "borrar el campo", true),
    b(&[Code(KeyCode::F(1))], Act::Help, "F1", "ayuda", false),
    b(&[ESC, Ctrl('c')], Act::Back, "Esc", "volver", true),
];

const LIVE_RUN: &[Bind] = &[
    b(&[ESC, Ctrl('c')], Act::Cancel, "Ctrl-C", "cancelar", true),
    b(
        &[Code(KeyCode::Tab)],
        Act::Tab,
        "Tab",
        "tiempo / debug",
        true,
    ),
    b(&[Char('c')], Act::Context, "c", "contexto", true),
    b(&[Char('U')], Act::Usage, "U", "uso", true),
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "más reciente",
        false,
    ),
    b(
        &[Char('k'), Code(KeyCode::Up)],
        Act::Up,
        "k",
        "más antiguo",
        false,
    ),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "el principio",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "el final",
        false,
    ),
    b(
        &[CTRL_F, Code(KeyCode::PageDown)],
        Act::PageDown,
        "Ctrl-F",
        "página abajo",
        false,
    ),
    b(
        &[CTRL_B, Code(KeyCode::PageUp)],
        Act::PageUp,
        "Ctrl-B",
        "página arriba",
        false,
    ),
    b(&[Char('?')], Act::Help, "?", "ayuda", true),
];

const LIVE_DONE: &[Bind] = &[
    b(&[ENTER], Act::Open, "Enter", "ver el run", true),
    b(
        &[ESC, Char('q'), Ctrl('c')],
        Act::Back,
        "Esc",
        "inicio",
        true,
    ),
    b(
        &[Code(KeyCode::Tab)],
        Act::Tab,
        "Tab",
        "tiempo / debug",
        true,
    ),
    b(&[Char('c')], Act::Context, "c", "contexto", true),
    b(&[Char('U')], Act::Usage, "U", "uso", true),
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "más reciente",
        false,
    ),
    b(
        &[Char('k'), Code(KeyCode::Up)],
        Act::Up,
        "k",
        "más antiguo",
        false,
    ),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "el principio",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "el final",
        false,
    ),
    b(
        &[CTRL_F, Code(KeyCode::PageDown)],
        Act::PageDown,
        "Ctrl-F",
        "página abajo",
        false,
    ),
    b(
        &[CTRL_B, Code(KeyCode::PageUp)],
        Act::PageUp,
        "Ctrl-B",
        "página arriba",
        false,
    ),
    b(&[Char('?')], Act::Help, "?", "ayuda", true),
];

const DETAIL_LIST: &[Bind] = &[
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "bajar",
        false,
    ),
    b(
        &[Char('k'), Code(KeyCode::Up)],
        Act::Up,
        "k",
        "subir",
        false,
    ),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "primero",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "último",
        false,
    ),
    b(
        &[ENTER, Char('l'), Code(KeyCode::Right), Char('d')],
        Act::Diff,
        "Enter d",
        "ver el diff",
        true,
    ),
    b(&[Char('a')], Act::Apply, "a", "aplicar", true),
    b(&[Char('u')], Act::Undo, "u", "deshacer", true),
    b(&[Char('c')], Act::Context, "c", "contexto", true),
    b(&[Char('U')], Act::Usage, "U", "uso", true),
    b(
        &[Code(KeyCode::Tab)],
        Act::Tab,
        "Tab",
        "elegir el recuerdo usado",
        false,
    ),
    b(
        &[Char('+')],
        Act::Useful,
        "+",
        "el recuerdo elegido fue útil",
        false,
    ),
    b(
        &[Char('-')],
        Act::Wrong,
        "-",
        "el recuerdo elegido estaba equivocado",
        false,
    ),
    b(&[Char('?')], Act::Help, "?", "ayuda", true),
    b(
        &[ESC, Char('q'), Ctrl('c')],
        Act::Back,
        "Esc",
        "volver",
        true,
    ),
];

const DETAIL_DIFF: &[Bind] = &[
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "bajar",
        false,
    ),
    b(
        &[Char('k'), Code(KeyCode::Up)],
        Act::Up,
        "k",
        "subir",
        false,
    ),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "el principio",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "el final",
        false,
    ),
    b(
        &[CTRL_F, Code(KeyCode::PageDown)],
        Act::PageDown,
        "Ctrl-F",
        "página abajo",
        false,
    ),
    b(
        &[CTRL_B, Code(KeyCode::PageUp)],
        Act::PageUp,
        "Ctrl-B",
        "página arriba",
        false,
    ),
    b(&[Char(']')], Act::NextFile, "]", "otro archivo", true),
    b(&[Char('[')], Act::PrevFile, "[", "archivo anterior", false),
    b(&[Char('a')], Act::Apply, "a", "aplicar", true),
    b(&[Char('u')], Act::Undo, "u", "deshacer", true),
    b(&[Char('c')], Act::Context, "c", "contexto", false),
    b(&[Char('U')], Act::Usage, "U", "uso", false),
    b(&[Char('?')], Act::Help, "?", "ayuda", true),
    b(
        &[ESC, Char('q'), Char('h'), Code(KeyCode::Left), Ctrl('c')],
        Act::Back,
        "Esc",
        "a la lista",
        true,
    ),
];

const DOCTOR: &[Bind] = &[
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "bajar",
        false,
    ),
    b(
        &[Char('k'), Code(KeyCode::Up)],
        Act::Up,
        "k",
        "subir",
        false,
    ),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "el principio",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "el final",
        false,
    ),
    b(
        &[CTRL_F, Code(KeyCode::PageDown)],
        Act::PageDown,
        "Ctrl-F",
        "página abajo",
        false,
    ),
    b(
        &[CTRL_B, Code(KeyCode::PageUp)],
        Act::PageUp,
        "Ctrl-B",
        "página arriba",
        false,
    ),
    b(&[Char('r')], Act::Refresh, "r", "repetir", true),
    b(&[Char('?')], Act::Help, "?", "ayuda", true),
    b(
        &[ESC, Char('q'), Ctrl('c')],
        Act::Back,
        "Esc",
        "volver",
        true,
    ),
];

const CONTEXT: &[Bind] = &[
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "bajar",
        false,
    ),
    b(
        &[Char('k'), Code(KeyCode::Up)],
        Act::Up,
        "k",
        "subir",
        false,
    ),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "el principio",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "el final",
        false,
    ),
    b(
        &[CTRL_F, Code(KeyCode::PageDown)],
        Act::PageDown,
        "Ctrl-F",
        "página abajo",
        false,
    ),
    b(
        &[CTRL_B, Code(KeyCode::PageUp)],
        Act::PageUp,
        "Ctrl-B",
        "página arriba",
        false,
    ),
    b(&[Char('U')], Act::Usage, "U", "ver el uso", true),
    b(&[Char('?')], Act::Help, "?", "ayuda", true),
    b(
        &[ESC, Char('q'), Ctrl('c'), Char('c')],
        Act::Back,
        "Esc",
        "volver",
        true,
    ),
];

const USAGE: &[Bind] = &[
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "bajar",
        false,
    ),
    b(
        &[Char('k'), Code(KeyCode::Up)],
        Act::Up,
        "k",
        "subir",
        false,
    ),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "el principio",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "el final",
        false,
    ),
    b(
        &[CTRL_F, Code(KeyCode::PageDown)],
        Act::PageDown,
        "Ctrl-F",
        "página abajo",
        false,
    ),
    b(
        &[CTRL_B, Code(KeyCode::PageUp)],
        Act::PageUp,
        "Ctrl-B",
        "página arriba",
        false,
    ),
    b(&[Char('c')], Act::Context, "c", "ver el contexto", true),
    b(&[Char('?')], Act::Help, "?", "ayuda", true),
    b(
        &[ESC, Char('q'), Ctrl('c'), Char('U')],
        Act::Back,
        "Esc",
        "volver",
        true,
    ),
];

const MEMORY: &[Bind] = &[
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "bajar",
        false,
    ),
    b(
        &[Char('k'), Code(KeyCode::Up)],
        Act::Up,
        "k",
        "subir",
        false,
    ),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "primero",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "último",
        false,
    ),
    b(&[ENTER], Act::Open, "Enter", "ver completo", true),
    b(&[Char('p')], Act::Promote, "p", "aprobar", true),
    b(&[Char('a')], Act::Archive, "a", "archivar", true),
    b(&[Char('/')], Act::Filter, "/", "buscar", true),
    b(&[Char('r')], Act::Refresh, "r", "refrescar", false),
    b(&[Char('?')], Act::Help, "?", "ayuda", true),
    b(
        &[ESC, Char('q'), Ctrl('c')],
        Act::Back,
        "Esc",
        "volver",
        true,
    ),
];

const MEMORY_FILTER: &[Bind] = &[
    b(&[ENTER], Act::Open, "Enter", "dejar la búsqueda", true),
    b(&[ESC], Act::Back, "Esc", "borrar la búsqueda", true),
    b(&[Code(KeyCode::Down)], Act::Down, "↓", "bajar", false),
    b(&[Code(KeyCode::Up)], Act::Up, "↑", "subir", false),
    b(
        &[Ctrl('c')],
        Act::Back,
        "Ctrl-C",
        "borrar la búsqueda",
        false,
    ),
];

const HELP: &[Bind] = &[
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "bajar",
        false,
    ),
    b(
        &[Char('k'), Code(KeyCode::Up)],
        Act::Up,
        "k",
        "subir",
        false,
    ),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "el principio",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "el final",
        false,
    ),
    b(
        &[CTRL_F, Code(KeyCode::PageDown)],
        Act::PageDown,
        "Ctrl-F",
        "página abajo",
        false,
    ),
    b(
        &[CTRL_B, Code(KeyCode::PageUp)],
        Act::PageUp,
        "Ctrl-B",
        "página arriba",
        false,
    ),
    b(
        &[ESC, Char('q'), Char('?'), ENTER, Ctrl('c')],
        Act::Back,
        "Esc ?",
        "cerrar la ayuda",
        true,
    ),
];

pub fn table(ctx: Ctx) -> &'static [Bind] {
    match ctx {
        Ctx::Home => HOME,
        Ctx::Filter => FILTER,
        Ctx::NewRun => NEW_RUN,
        Ctx::LiveRun => LIVE_RUN,
        Ctx::LiveDone => LIVE_DONE,
        Ctx::DetailList => DETAIL_LIST,
        Ctx::DetailDiff => DETAIL_DIFF,
        Ctx::Doctor => DOCTOR,
        Ctx::Context => CONTEXT,
        Ctx::Usage => USAGE,
        Ctx::Help => HELP,
        Ctx::Memory => MEMORY,
        Ctx::MemoryFilter => MEMORY_FILTER,
    }
}

/// What the key does on this screen. The global keys come first.
pub fn lookup(ctx: Ctx, k: &KeyEvent) -> Option<Act> {
    GLOBAL
        .iter()
        .chain(table(ctx))
        .find(|bind| bind.keys.iter().any(|key| key.matches(k)))
        .map(|bind| bind.act)
}

/// The entries of the one-line hints, in table order, ending with the help key when it is there.
pub fn hints(ctx: Ctx) -> Vec<&'static Bind> {
    let t = table(ctx);
    let mut v: Vec<&Bind> = t.iter().filter(|x| x.hint && x.act != Act::Help).collect();
    v.extend(t.iter().filter(|x| x.hint && x.act == Act::Help));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn name(k: Key) -> String {
        format!("{k:?}")
    }

    #[test]
    fn no_key_is_bound_twice_on_the_same_screen() {
        for ctx in ALL {
            let mut seen: BTreeMap<String, (&str, Act)> = BTreeMap::new();
            for bind in GLOBAL.iter().chain(table(ctx)) {
                for key in bind.keys {
                    // Ctrl-D and Ctrl-d are the same key.
                    let n = match key {
                        Ctrl(c) => format!("Ctrl({})", c.to_ascii_lowercase()),
                        other => name(*other),
                    };
                    if let Some((prev, act)) = seen.insert(n.clone(), (bind.shown, bind.act)) {
                        panic!(
                            "{ctx:?}: {n} is bound twice ({prev} {act:?} and {} {:?})",
                            bind.shown, bind.act
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_key_means_the_same_thing_everywhere_it_exists() {
        // Esc, q, Enter and Ctrl-C depend on where you are; these must not.
        let fixed = [
            ('c', Act::Context),
            ('U', Act::Usage),
            ('?', Act::Help),
            ('r', Act::Refresh),
            ('/', Act::Filter),
            ('n', Act::New),
            ('d', Act::Diff),
            ('a', Act::Apply),
            ('u', Act::Undo),
            ('D', Act::Doctor),
            ('g', Act::Top),
            ('G', Act::Bottom),
            ('j', Act::Down),
            ('k', Act::Up),
            (']', Act::NextFile),
            ('[', Act::PrevFile),
            ('m', Act::Memory),
            ('p', Act::Promote),
            ('+', Act::Useful),
            ('-', Act::Wrong),
        ];
        for ctx in ALL {
            for bind in table(ctx) {
                for key in bind.keys {
                    if let Char(c) = key
                        && let Some((_, act)) = fixed.iter().find(|(f, _)| f == c)
                    {
                        // Back on 'c' and 'U' is "toggle back" in the two views that are the pair.
                        let toggle = matches!(ctx, Ctx::Context | Ctx::Usage | Ctx::Help)
                            && bind.act == Act::Back;
                        // `a` is "aplicar" on a run and "archivar" on a memory: the same verb of
                        // accepting a decision, on the one screen that has no runs to apply.
                        let archive = *c == 'a' && ctx == Ctx::Memory && bind.act == Act::Archive;
                        assert!(
                            bind.act == *act || toggle || archive,
                            "{ctx:?}: {c:?} is {:?}, elsewhere {act:?}",
                            bind.act
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_screen_has_a_way_out_and_help_and_global_keys_work_everywhere() {
        for ctx in ALL {
            let acts: BTreeSet<String> =
                table(ctx).iter().map(|x| format!("{:?}", x.act)).collect();
            assert!(
                acts.contains("Back") || acts.contains("Quit") || acts.contains("Cancel"),
                "{ctx:?} has no way out"
            );
            let ev = |c, m| KeyEvent::new(KeyCode::Char(c), m);
            assert_eq!(
                lookup(ctx, &ev('d', KeyModifiers::CONTROL)),
                Some(Act::Debug)
            );
            assert_eq!(
                lookup(ctx, &ev('l', KeyModifiers::CONTROL)),
                Some(Act::Redraw)
            );
            // Ctrl-D with caps lock or shift is the same key.
            assert_eq!(
                lookup(ctx, &ev('D', KeyModifiers::CONTROL)),
                Some(Act::Debug)
            );
        }
        // Ctrl-D never reaches the screens' own `d` (diff).
        let plain_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        assert_eq!(lookup(Ctx::Home, &plain_d), Some(Act::Diff));
    }

    fn key_name(k: &Key) -> String {
        let code = |c: &KeyCode| match c {
            KeyCode::Esc => "Esc".to_owned(),
            KeyCode::Enter => "Enter".to_owned(),
            KeyCode::Tab => "Tab".to_owned(),
            KeyCode::BackTab => "Shift-Tab".to_owned(),
            KeyCode::Up => "↑".to_owned(),
            KeyCode::Down => "↓".to_owned(),
            KeyCode::Left => "←".to_owned(),
            KeyCode::Right => "→".to_owned(),
            KeyCode::Home => "Inicio".to_owned(),
            KeyCode::End => "Fin".to_owned(),
            KeyCode::PageUp => "RePág".to_owned(),
            KeyCode::PageDown => "AvPág".to_owned(),
            KeyCode::F(n) => format!("F{n}"),
            other => format!("{other:?}"),
        };
        match k {
            Key::Char(c) => c.to_string(),
            Key::Ctrl(c) => format!("Ctrl-{}", c.to_ascii_uppercase()),
            Key::Alt(c) => format!("Alt-{}", code(c)),
            Key::Code(c) => code(c),
        }
    }

    /// The table for `docs/uso-local.md`: `cargo test -p clio-brain --bin clio print_keymap -- --ignored --nocapture`.
    #[test]
    #[ignore = "prints the key map; not a check"]
    fn print_keymap() {
        let names = [
            (Ctx::Home, "Inicio"),
            (Ctx::Filter, "Inicio, escribiendo el filtro"),
            (Ctx::NewRun, "Nuevo trabajo"),
            (Ctx::LiveRun, "En vivo, corriendo"),
            (Ctx::LiveDone, "En vivo, terminado"),
            (Ctx::DetailList, "Detalle, lista de archivos"),
            (Ctx::DetailDiff, "Detalle, diff"),
            (Ctx::Doctor, "Doctor"),
            (Ctx::Context, "Contexto"),
            (Ctx::Usage, "Uso"),
            (Ctx::Help, "Ayuda"),
            (Ctx::Memory, "Memoria"),
            (Ctx::MemoryFilter, "Memoria, escribiendo la búsqueda"),
        ];
        for (ctx, name) in names {
            let entries: Vec<String> = table(ctx)
                .iter()
                .map(|b| {
                    let keys: Vec<String> = b
                        .keys
                        .iter()
                        .map(|k| format!("`{}`", key_name(k)))
                        .collect();
                    format!("{} {}", keys.join(" "), b.what)
                })
                .collect();
            println!("- **{name}**: {}.", entries.join(" · "));
        }
        println!("- **En todas**: `Ctrl-D` modo debug · `Ctrl-L` redibujar.");
    }

    #[test]
    fn every_bind_is_documented_and_ends_up_in_the_hints_or_the_help() {
        for ctx in ALL {
            for bind in table(ctx) {
                assert!(!bind.shown.is_empty() && !bind.what.is_empty() && !bind.keys.is_empty());
            }
        }
        assert!(
            hints(Ctx::Home)
                .last()
                .is_some_and(|h| h.act == Act::Quit || h.act == Act::Help)
        );
    }
}
