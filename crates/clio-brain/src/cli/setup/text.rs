//! Every word the installer shows or prints, in Spanish and in English, in one table: the macro
//! takes both for each entry, so neither can be missing; the test checks that both are written
//! and carry the same `{}`. The rest of Clio stays in English.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Es,
    En,
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Self::Es => "es",
            Self::En => "en",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        match s {
            "es" => Some(Self::Es),
            "en" => Some(Self::En),
            _ => None,
        }
    }

    /// Spanish when the locale says so, English otherwise.
    pub fn from_env() -> Self {
        Self::from_locale(|k| std::env::var(k).ok())
    }

    /// Spanish when the locale that decides messages (`LC_ALL`, else `LC_MESSAGES`, else `LANG`)
    /// is Spanish.
    pub fn from_locale(var: impl Fn(&str) -> Option<String>) -> Self {
        let es = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|k| var(k).filter(|v| !v.is_empty()))
            .is_some_and(|v| v.starts_with("es"));
        if es { Self::Es } else { Self::En }
    }

    /// The language an earlier run saved (`ui.language`), else the locale's.
    pub fn saved_or_env(dirs: &crate::cli::home::Dirs) -> Self {
        Self::saved(dirs).unwrap_or_else(Self::from_env)
    }

    /// The language an earlier run saved in the config.
    pub fn saved(dirs: &crate::cli::home::Dirs) -> Option<Self> {
        crate::cli::home::Config::load(dirs)
            .ok()
            .and_then(|c| c.ui.language)
            .and_then(|l| Self::from_code(&l))
    }
}

macro_rules! table {
    ($($k:ident: $es:literal, $en:literal;)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum M { $($k),* }

        impl M {
            #[cfg(test)]
            pub const ALL: &[M] = &[$(M::$k),*];

            pub fn get(self, l: Lang) -> &'static str {
                match (self, l) {
                    $((M::$k, Lang::Es) => $es, (M::$k, Lang::En) => $en,)*
                }
            }
        }
    };
}

table! {
    Installer: "instalador", "installer";
    KeysChoose: "↑↓ elegir · Enter aceptar · Esc salir", "↑↓ choose · Enter accept · Esc quit";
    KeysGo: "Enter seguir · Esc salir", "Enter continue · Esc quit";
    KeysField: "Enter aceptar · Esc omitir", "Enter accept · Esc skip";
    KeysChecks: "↑↓ elegir · Espacio marcar · Enter aceptar · Esc salir", "↑↓ choose · Space tick · Enter accept · Esc quit";
    KeysDone: "Enter abre el chat · Esc salir", "Enter opens the chat · Esc quit";
    KeysExit: "Enter salir", "Enter quit";
    KeysWait: "trabajando…", "working…";

    FoundNothing: "Clio no está instalada en esta máquina.", "Clio is not installed on this machine.";
    FoundNothingLine: "Instalaré lo que necesita, todo en tu carpeta personal.", "I will install what it needs, all in your home folder.";
    FoundSystem: "Sistema: {}", "System: {}";
    FoundJail: "bubblewrap (la jaula): {}", "bubblewrap (the jail): {}";
    FoundInstalled: "Clio {} ya está instalada.", "Clio {} is already installed.";
    FoundData: "Datos: {}", "Data: {}";
    FoundVault: "Baúl: {}", "Vault: {}";
    FoundTimer: "Temporizador de servicios: {}", "Services timer: {}";
    FoundAsk: "¿Qué hago?", "What shall I do?";
    Yes: "sí", "yes";
    No: "no", "no";
    Present: "está", "present";
    Missing: "falta", "missing";
    Update: "Actualizar (conserva todo)", "Update (keeps everything)";
    Repair: "Reparar", "Repair";
    RemoveKeep: "Quitar Clio pero conservar mis datos", "Remove Clio but keep my data";
    RemoveAll: "Quitar todo", "Remove everything";

    PlanTitle: "Esto es lo que haré:", "This is what I will do:";
    PlanKeys: "Enter empezar · Esc salir", "Enter start · Esc quit";
    DoingTitle: "Instalando", "Installing";

    StFolders: "Carpetas y configuración", "Folders and configuration";
    StSystem: "Paquetes del sistema", "System packages";
    StNode: "Node 22 privado", "Private Node 22";
    StPi: "Pi, el motor del agente", "Pi, the agent's harness";
    StMemory: "clio-memory y su modelo", "clio-memory and its model";
    StVault: "El baúl", "The vault";
    StModel: "El modelo", "The model";
    StJudge: "El juez", "The judge";
    StTimer: "Temporizador de servicios", "Services timer";
    StSkills: "Skills incluidas", "Shipped skills";
    StMcp: "Servidores MCP", "MCP servers";
    StDoctor: "Revisión final", "Final check";
    StBinaries: "clio y clio-vault", "clio and clio-vault";
    StStopVault: "Detener el baúl", "Stop the vault";
    StRemoveTimer: "Quitar el temporizador", "Remove the timer";
    StRemoveNodePi: "Quitar Node y Pi privados", "Remove the private Node and Pi";
    StRemoveBinaries: "Quitar clio, clio-vault y clio-memory", "Remove clio, clio-vault and clio-memory";
    StRemoveModel: "Quitar clio-memory y su modelo", "Remove clio-memory and its model";
    StRemoveData: "Borrar configuración y datos", "Delete configuration and data";

    NoteCreated: "creado", "created";
    NoteThere: "ya estaba", "already there";
    NoteInstalled: "{} instalado", "{} installed";
    NoteUpToDate: "al día", "up to date";
    NoteFromTo: "{} → {}", "{} → {}";
    NoteSkipped: "omitido", "skipped";
    NoteLater: "más tarde", "later";
    NoteDone: "hecho", "done";
    NoteRemoved: "quitado", "removed";
    NoteRemovedNoSystemd: "quitado (systemd no respondió)", "removed (systemd did not answer)";
    NoteNoRelease: "no pude buscar una versión nueva", "could not look for a newer version";
    NoteAtEnd: "al terminar", "when the install finishes";
    NoteOn: "activado", "on";
    NoteRestarted: "reiniciado", "restarted";
    NoteOff: "desactivado", "off";
    NoteDoctorOk: "todo bien", "all good";
    NoteNewVault: "creado y abierto", "created and open";
    NoteOpened: "abierto", "open";
    NoteMac: "macOS: todavía sin jaula; los servicios no funcionan", "macOS: no jail yet; services do not run";
    NoteMacTimer: "macOS: no hay temporizador todavía", "macOS: no timer yet";
    NoteNoSudo: "no tienes sudo: pide a un administrador que instale bubblewrap", "no sudo: ask an administrator to install bubblewrap";
    NoteNoSudoLinger: "no tienes sudo: el temporizador se detiene al cerrar la sesión", "no sudo: the timer stops when you log out";
    NoteNoPkg: "no encontré un gestor de paquetes: instala bubblewrap a mano", "no package manager found: install bubblewrap by hand";
    NoteSysSkipped: "omitido: sin la jaula no funcionan los servicios ni las pruebas", "skipped: without the jail services and tests do not run";
    NoteNoJail: "la jaula no arranca aquí", "the jail does not start here";
    NoteSysFailed: "no se pudo: {}", "it did not work: {}";
    NoteNoTarget: "no hay una versión para este sistema", "there is no build for this system";
    NoteNewer: "{} (más nuevo, se deja)", "{} (newer, left as it is)";
    NoteNoVault: "sin baúl", "no vault";
    NoteTemis: "Temis activado", "Temis on";
    NoteModelLater: "búsqueda por significado: se completará más tarde", "search by meaning: finishes later";
    NoteTimerLater: "más tarde: no hay sesión de systemd para tu usuario", "later: there is no systemd session for your user";
    NoteNone: "ninguno", "none";
    NoteMcpLater: "{}: termina más tarde (clio mcp add {})", "{}: finishes later (clio mcp add {})";
    NoteMcpToken: "{}: apagado hasta tener su token (clio mcp add {})", "{}: off until it has its token (clio mcp add {})";
    NoteMcpNewer: "{} tiene una versión nueva en el catálogo, {}: clio mcp add {}", "{} has a newer version in the catalog, {}: clio mcp add {}";
    NoteMcpBrowser: "{}: el catálogo cambió su Chromium ({} → {}); no arranca hasta clio mcp add {}", "{}: the catalog changed its Chromium ({} → {}); it does not start until clio mcp add {}";
    NoteMcpNoLibs: "Playwright queda fuera: a este sistema le faltan bibliotecas que no sé instalar aquí", "Playwright is left out: this system lacks libraries I cannot install here";
    NoteMcpLibsFailed: "Playwright queda fuera: no se pudieron instalar las bibliotecas de Chromium (clio mcp add playwright después)", "Playwright is left out: Chromium's libraries could not be installed (clio mcp add playwright later)";
    NoteNoSudoLibs: "no tienes sudo: faltan las bibliotecas de Chromium (Playwright)", "no sudo: Chromium's libraries (Playwright) are missing";
    NoteDoctorIssues: "hay avisos: ejecuta «clio doctor»", "there are warnings: run `clio doctor`";

    FailNet: "No se pudo descargar {}.", "Could not download {}.";
    FixNet: "Revisa la conexión y ejecuta «clio setup» otra vez.", "Check the connection and run `clio setup` again.";
    FailSum: "{} no coincide con su suma de verificación: no se instaló nada.", "{} does not match its checksum: nothing was installed.";
    FixSum: "Vuelve a intentarlo; si se repite, avisa en github.com/i-montes/clio.", "Try again; if it happens again, report it at github.com/i-montes/clio.";
    FixOther: "Corrige eso y ejecuta «clio setup» otra vez.", "Fix that and run `clio setup` again.";
    FailRuntime: "No hay una carpeta privada para los sockets de Clio.", "There is no private folder for Clio's sockets.";
    FixRuntime: "Inicia sesión de forma normal (por ssh o en el escritorio) y ejecuta «clio setup».", "Log in through a normal session (ssh or desktop) and run `clio setup`.";
    FailPass: "La contraseña no abre el baúl.", "The passphrase does not open the vault.";
    FixPass: "Ejecuta «clio vault start» cuando la recuerdes.", "Run `clio vault start` when you remember it.";

    VaultNewTitle: "Crea la contraseña de tu baúl", "Create your vault's passphrase";
    VaultNewHint: "El baúl guarda tus claves en esta máquina. Si olvidas la contraseña, nadie puede recuperarla: anótala en un lugar seguro.", "The vault keeps your keys on this machine. If you forget the passphrase, nobody can recover it: write it down somewhere safe.";
    VaultOpenTitle: "La contraseña de tu baúl", "Your vault's passphrase";
    VaultOpenHint: "Para abrir el baúl que ya tienes.", "To open the vault you already have.";
    FieldPass: "Contraseña", "Passphrase";
    FieldAgain: "Otra vez", "Again";
    ErrMismatch: "No coinciden: escríbela otra vez.", "They do not match: type it again.";
    ErrEmpty: "Está vacía.", "It is empty.";
    ErrWrongPass: "No abre el baúl: ¿la escribiste bien?", "It does not open the vault: did you type it right?";

    ModelTitle: "¿Qué modelo usará Clio?", "Which model will Clio use?";
    ModelHint: "Su clave va directamente al baúl.", "Its key goes straight into the vault.";
    ModelClaude: "Suscripción de Claude (Pro o Max)", "Claude subscription (Pro or Max)";
    ModelApi: "Clave de la API de Anthropic", "Anthropic API key";
    ModelOpencode: "OpenCode", "OpenCode";
    ModelLater: "Más tarde", "Later";
    KeyTitle: "Pega tu clave de {}", "Paste your {} key";
    ClaudeHowTitle: "¿Cómo conectas tu suscripción?", "How do you connect your subscription?";
    ClaudeSignIn: "Iniciar sesión cuando termine la instalación", "Sign in when the install finishes";
    ClaudePaste: "Pegar un token", "Paste a token";
    ClaudeBefore: "Inicia sesión en el navegador. Cuando Claude muestre el token, cópialo y pulsa Ctrl-C; luego pégalo.", "Sign in in the browser. When Claude shows the token, copy it and press Ctrl-C, then paste it.";
    ClaudeLeave: "Para salir sin iniciar sesión: Ctrl-C y luego Enter.", "To leave without signing in: Ctrl-C, then Enter.";
    ClaudeCopy: "Pega el token y pulsa Enter (una línea vacía sale sin conectar).", "Paste the token and press Enter (an empty line leaves without connecting).";
    KeyClaudeHint: "Empieza por sk-ant-oat. Sale de «claude setup-token», en cualquier equipo con Claude Code. Esc: más tarde.", "It starts with sk-ant-oat. It comes from `claude setup-token`, on any machine with Claude Code. Esc: later.";
    KeyApiHint: "Empieza por sk-ant-api. Está en console.anthropic.com → API keys.", "It starts with sk-ant-api. It is in console.anthropic.com → API keys.";
    KeyOpencodeHint: "Está en opencode.ai, en tu cuenta → API keys.", "It is in opencode.ai, your account → API keys.";
    FieldKey: "Clave", "Key";
    ErrPrefix: "No empieza por {}: escríbela otra vez.", "It does not start with {}: type it again.";

    JudgeTitle: "El juez lee el texto de fuera antes que el modelo.", "The judge reads outside text before the model does.";
    JudgeHint: "Es opcional: Clio funciona sin él.", "It is optional: Clio works without it.";
    JudgeSkip: "Omitir por ahora", "Skip for now";
    JudgeTemis: "Tengo mi propio Temis", "I have my own Temis";
    TemisUrlTitle: "La dirección de tu Temis", "Your Temis's address";
    TemisUrlHint: "Clio solo habla con {}.", "Clio only talks to {}.";
    FieldUrl: "Dirección", "Address";
    ErrUrl: "Esa dirección no la acepta el juez de Clio.", "Clio's judge does not accept that address.";
    TemisKeyTitle: "La clave de tu Temis", "Your Temis's key";
    TemisKeyHint: "Va directamente al baúl.", "It goes straight into the vault.";

    SudoTitle: "Hace falta tu contraseña del sistema, una vez.", "Your system password is needed, once.";
    SudoBwrap: "Instalar bubblewrap ({}): la jaula donde se ejecutan los comandos.", "Install bubblewrap ({}): the jail the commands run in.";
    SudoLinger: "Mantener los servicios activos aunque cierres la sesión.", "Keep the services running after you log out.";
    SudoBrowser: "Instalar lo que Chromium necesita para Playwright.", "Install what Chromium needs for Playwright.";
    SudoProfile: "Si la jaula aún no arranca: un perfil de AppArmor que deja a bubblewrap crearla.", "If the jail still does not start: an AppArmor profile that lets bubblewrap make it.";
    SudoRuns: "Ejecutaré:", "I will run:";
    SudoHow: "sudo la pide en la terminal; Clio nunca la ve.", "sudo asks for it in the terminal; Clio never sees it.";
    SudoGo: "Seguir", "Continue";
    SudoSkip: "Omitir", "Skip";

    McpTitle: "¿Qué servidores MCP quieres?", "Which MCP servers do you want?";
    McpHint: "Cada uno da a Clio más herramientas y se ejecuta en su jaula. Ninguno está marcado: elige los que quieras, o ninguno.", "Each gives Clio more tools and runs in its jail. None is ticked: choose the ones you want, or none.";
    McpTokenTitle: "Pega tu token de {}", "Paste your {} token";
    McpTokenHint: "Va directamente al baúl. Esc: más tarde (el servidor queda apagado).", "It goes straight into the vault. Esc: later (the server stays off).";
    FieldToken: "Token", "Token";

    ChatOpenTitle: "Hay un chat de Clio abierto.", "A Clio chat is open.";
    ChatOpenHint: "Ciérralo y pulsa Enter.", "Close it and press Enter.";
    ConfirmTitle: "¿Quitar todo?", "Remove everything?";
    ConfirmLost: "Se pierden el baúl con todas sus claves, la memoria, las conversaciones, la configuración, las skills, los servicios, los servidores MCP, y clio-memory y su modelo.", "You lose the vault with every key in it, the memory, the conversations, the configuration, the skills, the services, the MCP servers, and clio-memory and its model.";
    ConfirmNo: "No, conservarlo", "No, keep it";
    ConfirmYes: "Sí, quitarlo todo", "Yes, remove everything";

    DoneTitle: "Clio está lista.", "Clio is ready.";
    DoneLeft: "Falta una cosa: {}", "One thing left: {}";
    LeftModel: "conectar un modelo (clio connect claude, anthropic-api u opencode)", "connect a model (clio connect claude, anthropic-api or opencode)";
    LeftModelAgain: "conectar el modelo: ejecuta «clio setup» otra vez y elige el modelo", "connect the model: run `clio setup` again and choose the model";
    LeftJailBwrap: "pide a un administrador que instale bubblewrap: sin él Clio no arranca", "ask an administrator to install bubblewrap: Clio does not start without it";
    LeftJailUserns: "pide a un administrador que permita a bubblewrap crear espacios de nombres de usuario (un perfil de AppArmor para /usr/bin/bwrap)", "ask an administrator to let bubblewrap create user namespaces (an AppArmor profile for /usr/bin/bwrap)";
    LeftJailOther: "pide a un administrador que permita a tu usuario usar bubblewrap (prueba: bwrap --ro-bind / / true)", "ask an administrator to let your user run bubblewrap (test: bwrap --ro-bind / / true)";
    DoneBlocked: "Clio todavía no puede arrancar aquí.", "Clio cannot start here yet.";
    DonePath: "«clio» se encuentra desde la próxima terminal.", "`clio` is found from the next terminal on.";
    LeftVault: "abrir el baúl (clio vault start) y conectar un modelo", "open the vault (clio vault start) and connect a model";
    DoneFailed: "Algo falló.", "Something failed.";
    DoneFailedLine: "Arregla lo que dice el paso y ejecuta «clio setup» otra vez.", "Fix what the step says and run `clio setup` again.";
    RemovedTitle: "Clio se quitó.", "Clio was removed.";
    RemovedKept: "Tus datos, la configuración y el modelo siguen en {}.", "Your data, configuration and model stay in {}.";

    NoTerminal: "No hay una terminal: ejecuta «clio setup» en una.", "There is no terminal: run `clio setup` in one.";
    UpdateNone: "Clio ya está al día ({}).", "Clio is already up to date ({}).";
    UpdateAvailable: "Hay una versión nueva: {} → {}. Ejecuta «clio update».", "A newer version exists: {} → {}. Run `clio update`.";
    UpdateChatOpen: "Hay un chat de Clio abierto: ciérralo y ejecuta «clio update» otra vez.", "A Clio chat is open: close it and run `clio update` again.";
}

/// The locale says UTF-8 (the first of `LC_ALL`, `LC_CTYPE`, `LANG` that is set, as the chat
/// decides).
pub fn utf8_locale() -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .is_some_and(|v| {
            let v = v.to_ascii_lowercase();
            v.contains("utf-8") || v.contains("utf8")
        })
}

/// `s` in ASCII, for a terminal whose locale is not UTF-8: symbols become their plain
/// look-alikes and letters lose their accents, instead of arriving as broken bytes.
pub fn ascii(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.replace("↑↓", "up/down").chars() {
        match c {
            c if c.is_ascii() => out.push(c),
            '·' | '─' | '–' | '—' => out.push('-'),
            '→' => out.push_str("->"),
            '«' | '»' | '“' | '”' => out.push('"'),
            '…' => out.push_str("..."),
            '•' | '◆' => out.push('*'),
            '▸' => out.push('>'),
            '¿' | '¡' => {}
            'á' | 'à' => out.push('a'),
            'é' | 'è' => out.push('e'),
            'í' => out.push('i'),
            'ó' => out.push('o'),
            'ú' | 'ü' => out.push('u'),
            'ñ' => out.push('n'),
            'Á' => out.push('A'),
            'É' => out.push('E'),
            'Í' => out.push('I'),
            'Ó' => out.push('O'),
            'Ú' => out.push('U'),
            'Ñ' => out.push('N'),
            _ => out.push('?'),
        }
    }
    out
}

/// A line the installer prints: as it is, or in ASCII when the locale is not UTF-8.
pub fn say(s: &str) -> String {
    if utf8_locale() {
        s.to_owned()
    } else {
        ascii(s)
    }
}

impl M {
    /// The entry with each `{}` replaced by the next of `args`.
    pub fn fill(self, l: Lang, args: &[&str]) -> String {
        let mut out = String::new();
        let mut it = args.iter();
        let mut parts = self.get(l).split("{}").peekable();
        while let Some(p) = parts.next() {
            out.push_str(p);
            if parts.peek().is_some() {
                out.push_str(it.next().copied().unwrap_or(""));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Both languages are complete: every entry written in each, with the same placeholders.
    #[test]
    fn every_entry_is_in_both_languages_with_the_same_placeholders() {
        for m in M::ALL {
            let (es, en) = (m.get(Lang::Es), m.get(Lang::En));
            assert!(!es.trim().is_empty() && !en.trim().is_empty(), "{m:?}");
            assert_eq!(es.matches("{}").count(), en.matches("{}").count(), "{m:?}");
        }
        assert_eq!(M::NoteFromTo.fill(Lang::Es, &["1", "2"]), "1 → 2");
        assert_eq!(
            M::FoundInstalled.fill(Lang::En, &["0.2.0"]),
            "Clio 0.2.0 is already installed."
        );
        assert_eq!(Lang::from_code("es"), Some(Lang::Es));
        assert_eq!(Lang::from_code("fr"), None);
    }

    // Without a UTF-8 locale every entry still comes out readable, in plain ASCII.
    #[test]
    fn every_entry_has_an_ascii_form() {
        for m in M::ALL {
            for l in [Lang::Es, Lang::En] {
                let (s, a) = (m.get(l), ascii(m.get(l)));
                // Nothing turned into `?`: each character has its look-alike.
                assert!(
                    a.is_ascii() && a.matches('?').count() == s.matches('?').count(),
                    "{m:?}: {a}"
                );
            }
        }
        assert_eq!(
            ascii("¿Qué hago? «clio» · Esc → más…"),
            "Que hago? \"clio\" - Esc -> mas..."
        );
    }
}
