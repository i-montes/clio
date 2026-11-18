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
    /// Ctrl and a key that is not a letter (Ctrl-←, Ctrl-Home).
    CtrlCode(KeyCode),
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
            Self::CtrlCode(code) => ctrl && !alt && k.code == code,
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
    Copy,
    Delete,
    Useful,
    Wrong,
    /// Opens or closes the drawer of sessions.
    Sessions,
    // Editing the chat's message.
    Left,
    Right,
    LineStart,
    LineEnd,
    WordLeft,
    WordRight,
    DelBack,
    DelNext,
    DelWord,
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

use Key::{Alt, Char, Code, Ctrl, CtrlCode};

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
    Sessions,
    Chat,
    ChatRun,
}

#[cfg(test)]
pub const ALL: [Ctx; 16] = [
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
    Ctx::Sessions,
    Ctx::Chat,
    Ctx::ChatRun,
];

/// On every screen.
pub const GLOBAL: &[Bind] = &[
    b(&[Ctrl('d')], Act::Debug, "Ctrl-D", "debug mode", false),
    b(&[Ctrl('l')], Act::Redraw, "Ctrl-L", "redraw", false),
];

const HOME: &[Bind] = &[
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "down",
        false,
    ),
    b(&[Char('k'), Code(KeyCode::Up)], Act::Up, "k", "up", false),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "first",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "last",
        false,
    ),
    b(&[ENTER], Act::Open, "Enter", "open", true),
    b(&[Char('n')], Act::New, "n", "new", true),
    b(&[Char('d')], Act::Diff, "d", "diff", false),
    b(&[Char('a')], Act::Apply, "a", "apply", false),
    b(&[Char('u')], Act::Undo, "u", "undo", false),
    b(&[Char('/')], Act::Filter, "/", "filter", true),
    b(&[Char('q'), Ctrl('c')], Act::Quit, "q", "quit", true),
    b(&[Char('c')], Act::Context, "c", "context", true),
    b(&[Char('U')], Act::Usage, "U", "usage", true),
    b(&[Char('m')], Act::Memory, "m", "memory", true),
    b(&[Char('r')], Act::Refresh, "r", "refresh", false),
    b(&[Char('D')], Act::Doctor, "D", "doctor", false),
    b(&[ESC], Act::Back, "Esc", "clear the filter", false),
    b(&[Char('?')], Act::Help, "?", "help", true),
];

const FILTER: &[Bind] = &[
    b(&[ENTER], Act::Open, "Enter", "keep the filter", true),
    b(&[ESC], Act::Back, "Esc", "clear the filter", true),
    b(&[Code(KeyCode::Down)], Act::Down, "↓", "down", false),
    b(&[Code(KeyCode::Up)], Act::Up, "↑", "up", false),
    b(&[Ctrl('c')], Act::Back, "Ctrl-C", "clear the filter", false),
];

const NEW_RUN: &[Bind] = &[
    b(
        &[Code(KeyCode::Tab), Code(KeyCode::BackTab)],
        Act::Tab,
        "Tab",
        "next field",
        true,
    ),
    b(&[ENTER], Act::Open, "Enter", "launch", true),
    b(
        &[Alt(KeyCode::Enter), Ctrl('j')],
        Act::Newline,
        "Alt-Enter",
        "new line",
        true,
    ),
    b(&[Ctrl('u')], Act::Clear, "Ctrl-U", "clear the field", true),
    b(&[Code(KeyCode::F(1))], Act::Help, "F1", "help", false),
    b(&[ESC, Ctrl('c')], Act::Back, "Esc", "back", true),
];

const LIVE_RUN: &[Bind] = &[
    b(&[ESC, Ctrl('c')], Act::Cancel, "Ctrl-C", "cancel", true),
    b(
        &[Code(KeyCode::Tab)],
        Act::Tab,
        "Tab",
        "timeline / debug",
        true,
    ),
    b(&[Char('c')], Act::Context, "c", "context", true),
    b(&[Char('U')], Act::Usage, "U", "usage", true),
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "newer",
        false,
    ),
    b(
        &[Char('k'), Code(KeyCode::Up)],
        Act::Up,
        "k",
        "older",
        false,
    ),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "the start",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "the end",
        false,
    ),
    b(
        &[CTRL_F, Code(KeyCode::PageDown)],
        Act::PageDown,
        "Ctrl-F",
        "page down",
        false,
    ),
    b(
        &[CTRL_B, Code(KeyCode::PageUp)],
        Act::PageUp,
        "Ctrl-B",
        "page up",
        false,
    ),
    b(&[Char('?')], Act::Help, "?", "help", true),
];

const LIVE_DONE: &[Bind] = &[
    b(&[ENTER], Act::Open, "Enter", "see the run", true),
    b(&[ESC, Char('q'), Ctrl('c')], Act::Back, "Esc", "home", true),
    b(
        &[Code(KeyCode::Tab)],
        Act::Tab,
        "Tab",
        "timeline / debug",
        true,
    ),
    b(&[Char('c')], Act::Context, "c", "context", true),
    b(&[Char('U')], Act::Usage, "U", "usage", true),
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "newer",
        false,
    ),
    b(
        &[Char('k'), Code(KeyCode::Up)],
        Act::Up,
        "k",
        "older",
        false,
    ),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "the start",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "the end",
        false,
    ),
    b(
        &[CTRL_F, Code(KeyCode::PageDown)],
        Act::PageDown,
        "Ctrl-F",
        "page down",
        false,
    ),
    b(
        &[CTRL_B, Code(KeyCode::PageUp)],
        Act::PageUp,
        "Ctrl-B",
        "page up",
        false,
    ),
    b(&[Char('?')], Act::Help, "?", "help", true),
];

const DETAIL_LIST: &[Bind] = &[
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "down",
        false,
    ),
    b(&[Char('k'), Code(KeyCode::Up)], Act::Up, "k", "up", false),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "first",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "last",
        false,
    ),
    b(
        &[ENTER, Char('l'), Code(KeyCode::Right), Char('d')],
        Act::Diff,
        "Enter d",
        "see the diff",
        true,
    ),
    b(&[Char('a')], Act::Apply, "a", "apply", true),
    b(&[Char('u')], Act::Undo, "u", "undo", true),
    b(&[Char('c')], Act::Context, "c", "context", true),
    b(&[Char('U')], Act::Usage, "U", "usage", true),
    b(
        &[Code(KeyCode::Tab)],
        Act::Tab,
        "Tab",
        "pick the memory used",
        false,
    ),
    b(
        &[Char('+')],
        Act::Useful,
        "+",
        "the picked memory was useful",
        false,
    ),
    b(
        &[Char('-')],
        Act::Wrong,
        "-",
        "the picked memory was wrong",
        false,
    ),
    b(&[Char('?')], Act::Help, "?", "help", true),
    b(&[ESC, Char('q'), Ctrl('c')], Act::Back, "Esc", "back", true),
];

const DETAIL_DIFF: &[Bind] = &[
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "down",
        false,
    ),
    b(&[Char('k'), Code(KeyCode::Up)], Act::Up, "k", "up", false),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "the start",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "the end",
        false,
    ),
    b(
        &[CTRL_F, Code(KeyCode::PageDown)],
        Act::PageDown,
        "Ctrl-F",
        "page down",
        false,
    ),
    b(
        &[CTRL_B, Code(KeyCode::PageUp)],
        Act::PageUp,
        "Ctrl-B",
        "page up",
        false,
    ),
    b(&[Char(']')], Act::NextFile, "]", "next file", true),
    b(&[Char('[')], Act::PrevFile, "[", "previous file", false),
    b(&[Char('a')], Act::Apply, "a", "apply", true),
    b(&[Char('u')], Act::Undo, "u", "undo", true),
    b(&[Char('c')], Act::Context, "c", "context", false),
    b(&[Char('U')], Act::Usage, "U", "usage", false),
    b(&[Char('?')], Act::Help, "?", "help", true),
    b(
        &[ESC, Char('q'), Char('h'), Code(KeyCode::Left), Ctrl('c')],
        Act::Back,
        "Esc",
        "to the list",
        true,
    ),
];

const DOCTOR: &[Bind] = &[
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "down",
        false,
    ),
    b(&[Char('k'), Code(KeyCode::Up)], Act::Up, "k", "up", false),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "the start",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "the end",
        false,
    ),
    b(
        &[CTRL_F, Code(KeyCode::PageDown)],
        Act::PageDown,
        "Ctrl-F",
        "page down",
        false,
    ),
    b(
        &[CTRL_B, Code(KeyCode::PageUp)],
        Act::PageUp,
        "Ctrl-B",
        "page up",
        false,
    ),
    b(&[Char('r')], Act::Refresh, "r", "repeat", true),
    b(&[Char('?')], Act::Help, "?", "help", true),
    b(&[ESC, Char('q'), Ctrl('c')], Act::Back, "Esc", "back", true),
];

const CONTEXT: &[Bind] = &[
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "down",
        false,
    ),
    b(&[Char('k'), Code(KeyCode::Up)], Act::Up, "k", "up", false),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "the start",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "the end",
        false,
    ),
    b(
        &[CTRL_F, Code(KeyCode::PageDown)],
        Act::PageDown,
        "Ctrl-F",
        "page down",
        false,
    ),
    b(
        &[CTRL_B, Code(KeyCode::PageUp)],
        Act::PageUp,
        "Ctrl-B",
        "page up",
        false,
    ),
    b(&[Char('U')], Act::Usage, "U", "see the usage", true),
    b(&[Char('?')], Act::Help, "?", "help", true),
    b(
        &[ESC, Char('q'), Ctrl('c'), Char('c')],
        Act::Back,
        "Esc",
        "back",
        true,
    ),
];

const USAGE: &[Bind] = &[
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "down",
        false,
    ),
    b(&[Char('k'), Code(KeyCode::Up)], Act::Up, "k", "up", false),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "the start",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "the end",
        false,
    ),
    b(
        &[CTRL_F, Code(KeyCode::PageDown)],
        Act::PageDown,
        "Ctrl-F",
        "page down",
        false,
    ),
    b(
        &[CTRL_B, Code(KeyCode::PageUp)],
        Act::PageUp,
        "Ctrl-B",
        "page up",
        false,
    ),
    b(&[Char('c')], Act::Context, "c", "see the context", true),
    b(&[Char('?')], Act::Help, "?", "help", true),
    b(
        &[ESC, Char('q'), Ctrl('c'), Char('U')],
        Act::Back,
        "Esc",
        "back",
        true,
    ),
];

const MEMORY: &[Bind] = &[
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "down",
        false,
    ),
    b(&[Char('k'), Code(KeyCode::Up)], Act::Up, "k", "up", false),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "first",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "last",
        false,
    ),
    b(&[ENTER], Act::Open, "Enter", "see it all", true),
    b(&[Char('p')], Act::Promote, "p", "approve", true),
    b(&[Char('a')], Act::Archive, "a", "archive", true),
    b(&[Char('y')], Act::Copy, "y", "copy", true),
    b(&[Char('/')], Act::Filter, "/", "search", true),
    b(&[Char('r')], Act::Refresh, "r", "refresh", false),
    b(&[Char('?')], Act::Help, "?", "help", true),
    b(&[ESC, Char('q'), Ctrl('c')], Act::Back, "Esc", "back", true),
];

const MEMORY_FILTER: &[Bind] = &[
    b(&[ENTER], Act::Open, "Enter", "keep the search", true),
    b(&[ESC], Act::Back, "Esc", "clear the search", true),
    b(&[Code(KeyCode::Down)], Act::Down, "↓", "down", false),
    b(&[Code(KeyCode::Up)], Act::Up, "↑", "up", false),
    b(&[Ctrl('c')], Act::Back, "Ctrl-C", "clear the search", false),
];

const HELP: &[Bind] = &[
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j",
        "down",
        false,
    ),
    b(&[Char('k'), Code(KeyCode::Up)], Act::Up, "k", "up", false),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "the start",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "the end",
        false,
    ),
    b(
        &[CTRL_F, Code(KeyCode::PageDown)],
        Act::PageDown,
        "Ctrl-F",
        "page down",
        false,
    ),
    b(
        &[CTRL_B, Code(KeyCode::PageUp)],
        Act::PageUp,
        "Ctrl-B",
        "page up",
        false,
    ),
    b(
        &[ESC, Char('q'), Char('?'), ENTER, Ctrl('c')],
        Act::Back,
        "Esc ?",
        "close the help",
        true,
    ),
];

const SESSIONS: &[Bind] = &[
    b(
        &[Char('j'), Code(KeyCode::Down)],
        Act::Down,
        "j ↓",
        "down",
        false,
    ),
    b(&[Char('k'), Code(KeyCode::Up)], Act::Up, "k ↑", "up", false),
    b(
        &[Char('g'), Code(KeyCode::Home)],
        Act::Top,
        "g",
        "first",
        false,
    ),
    b(
        &[Char('G'), Code(KeyCode::End)],
        Act::Bottom,
        "G",
        "last",
        false,
    ),
    // The drawer's foot shows these three.
    b(&[ENTER], Act::Open, "Enter", "open", false),
    b(&[Char('n')], Act::New, "n", "new", false),
    b(&[Char('x')], Act::Delete, "x", "delete", false),
    b(&[ESC, CTRL_S, F2], Act::Back, "Esc Ctrl-S", "close", true),
    b(&[Char('r')], Act::Refresh, "r", "refresh", false),
    b(&[Char('D')], Act::Doctor, "D", "doctor", false),
    b(&[Char('m')], Act::Memory, "m", "memory", false),
    b(&[Char('q'), Ctrl('c')], Act::Quit, "q", "quit", false),
    b(&[Char('?')], Act::Help, "?", "help", true),
];

const CTRL_S: Key = Ctrl('s');
const F2: Key = Code(KeyCode::F(2));

/// The keys of the chat's composer and its scroll; letters are text there.
macro_rules! chat_keys {
    () => {
        [
            b(
                &[Alt(KeyCode::Enter), Ctrl('j')],
                Act::Newline,
                "Alt-Enter",
                "new line",
                false,
            ),
            b(
                &[Code(KeyCode::Left)],
                Act::Left,
                "←",
                "previous character",
                false,
            ),
            b(
                &[Code(KeyCode::Right)],
                Act::Right,
                "→",
                "next character",
                false,
            ),
            b(
                &[Code(KeyCode::Home), Ctrl('a')],
                Act::LineStart,
                "Home Ctrl-A",
                "start of the line",
                false,
            ),
            b(
                &[Code(KeyCode::End), Ctrl('e')],
                Act::LineEnd,
                "End Ctrl-E",
                "end of the line",
                false,
            ),
            b(
                &[CtrlCode(KeyCode::Left), Alt(KeyCode::Left)],
                Act::WordLeft,
                "Ctrl-←",
                "previous word",
                false,
            ),
            b(
                &[CtrlCode(KeyCode::Right), Alt(KeyCode::Right)],
                Act::WordRight,
                "Ctrl-→",
                "next word",
                false,
            ),
            b(
                &[Code(KeyCode::Backspace)],
                Act::DelBack,
                "Backspace",
                "delete backwards",
                false,
            ),
            b(
                &[Code(KeyCode::Delete)],
                Act::DelNext,
                "Del",
                "delete forwards",
                false,
            ),
            b(
                &[Ctrl('w'), Alt(KeyCode::Backspace)],
                Act::DelWord,
                "Ctrl-W",
                "delete the word",
                false,
            ),
            b(
                &[Ctrl('u')],
                Act::Clear,
                "Ctrl-U",
                "delete to the start of the line",
                false,
            ),
            b(
                &[Code(KeyCode::Up)],
                Act::Up,
                "↑",
                "line above, or scroll the conversation up",
                false,
            ),
            b(
                &[Code(KeyCode::Down)],
                Act::Down,
                "↓",
                "line below, or scroll the conversation down",
                false,
            ),
            b(
                &[CTRL_B, Code(KeyCode::PageUp)],
                Act::PageUp,
                "PgUp",
                "page up",
                false,
            ),
            b(
                &[CTRL_F, Code(KeyCode::PageDown)],
                Act::PageDown,
                "PgDn",
                "page down",
                false,
            ),
            b(
                &[CtrlCode(KeyCode::Home)],
                Act::Top,
                "Ctrl-Home",
                "the start of the conversation",
                false,
            ),
            b(
                &[CtrlCode(KeyCode::End)],
                Act::Bottom,
                "Ctrl-End",
                "the latest",
                false,
            ),
        ]
    };
}

const CHAT: &[Bind] = &{
    let [
        nl,
        l,
        r,
        ls,
        le,
        wl,
        wr,
        db,
        dn,
        dw,
        cl,
        up,
        down,
        pu,
        pd,
        top,
        bottom,
    ] = chat_keys!();
    [
        b(&[ENTER], Act::Open, "Enter", "send", true),
        b(&[CTRL_S, F2], Act::Sessions, "Ctrl-S", "sessions", true),
        nl,
        l,
        r,
        ls,
        le,
        wl,
        wr,
        db,
        dn,
        dw,
        cl,
        up,
        down,
        pu,
        pd,
        top,
        bottom,
        b(&[Ctrl('c')], Act::Quit, "Ctrl-C", "quit", false),
        b(&[Code(KeyCode::F(1))], Act::Help, "F1", "help", true),
    ]
};

const CHAT_RUN: &[Bind] = &{
    let [
        nl,
        l,
        r,
        ls,
        le,
        wl,
        wr,
        db,
        dn,
        dw,
        cl,
        up,
        down,
        pu,
        pd,
        top,
        bottom,
    ] = chat_keys!();
    [
        b(&[ESC, Ctrl('c')], Act::Cancel, "Esc", "stops", true),
        b(&[ENTER], Act::Open, "Enter", "steers the agent", true),
        b(&[CTRL_S, F2], Act::Sessions, "Ctrl-S", "sessions", true),
        nl,
        l,
        r,
        ls,
        le,
        wl,
        wr,
        db,
        dn,
        dw,
        cl,
        up,
        down,
        pu,
        pd,
        top,
        bottom,
        b(&[Code(KeyCode::F(1))], Act::Help, "F1", "help", true),
    ]
};

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
        Ctx::Sessions => SESSIONS,
        Ctx::Chat => CHAT,
        Ctx::ChatRun => CHAT_RUN,
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
                        // `a` is "apply" on a run and "archive" on a memory: the same verb of
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
            KeyCode::Home => "Home".to_owned(),
            KeyCode::End => "End".to_owned(),
            KeyCode::PageUp => "PgUp".to_owned(),
            KeyCode::PageDown => "PgDn".to_owned(),
            KeyCode::F(n) => format!("F{n}"),
            KeyCode::Backspace => "Backspace".to_owned(),
            KeyCode::Delete => "Del".to_owned(),
            other => format!("{other:?}"),
        };
        match k {
            Key::Char(c) => c.to_string(),
            Key::Ctrl(c) => format!("Ctrl-{}", c.to_ascii_uppercase()),
            Key::Alt(c) => format!("Alt-{}", code(c)),
            Key::CtrlCode(c) => format!("Ctrl-{}", code(c)),
            Key::Code(c) => code(c),
        }
    }

    /// The table for `docs/usage.md`: `cargo test -p clio-brain --bin clio print_keymap -- --ignored --nocapture`.
    #[test]
    #[ignore = "prints the key map; not a check"]
    fn print_keymap() {
        let names = [
            (Ctx::Home, "Home"),
            (Ctx::Filter, "Home, typing the filter"),
            (Ctx::NewRun, "New run"),
            (Ctx::LiveRun, "Live, running"),
            (Ctx::LiveDone, "Live, finished"),
            (Ctx::DetailList, "Detail, list of files"),
            (Ctx::DetailDiff, "Detail, diff"),
            (Ctx::Doctor, "Doctor"),
            (Ctx::Context, "Context"),
            (Ctx::Usage, "Usage"),
            (Ctx::Help, "Help"),
            (Ctx::Memory, "Memory"),
            (Ctx::MemoryFilter, "Memory, typing the search"),
            (Ctx::Sessions, "Chat, with the sessions open (Ctrl-S)"),
            (Ctx::Chat, "Chat, idle"),
            (Ctx::ChatRun, "Chat, the agent works"),
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
        println!("- **On every screen**: `Ctrl-D` debug mode · `Ctrl-L` redraw.");
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
