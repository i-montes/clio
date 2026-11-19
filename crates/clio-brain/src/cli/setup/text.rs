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
        let es = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
            .is_some_and(|v| v.starts_with("es"));
        if es { Self::Es } else { Self::En }
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
    KeysDone: "Enter abre el chat · Esc salir", "Enter opens the chat · Esc quit";
    KeysExit: "Enter salir", "Enter quit";
    KeysWait: "trabajando…", "working…";

    FoundNothing: "Clio no está instalada en esta máquina.", "Clio is not installed on this machine.";
    FoundNothingLine: "Instalaré lo que necesita, todo en tu carpeta personal.", "I will install what it needs, all in your home folder.";
    FoundSystem: "Sistema: {}", "System: {}";
    FoundJail: "bubblewrap (la jaula): {}", "bubblewrap (the jail): {}";
    FoundInstalled: "Clio {} ya está instalada.", "Clio {} is already installed.";
    FoundData: "Datos: {}", "Data: {}";
    FoundVault: "Bóveda: {}", "Vault: {}";
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
    StPi: "Pi, el arnés del agente", "Pi, the agent's harness";
    StMemory: "clio-memory y su modelo", "clio-memory and its model";
    StVault: "La bóveda", "The vault";
    StModel: "El modelo", "The model";
    StJudge: "El juez", "The judge";
    StTimer: "Temporizador de servicios", "Services timer";
    StSkills: "Habilidades incluidas", "Shipped skills";
    StDoctor: "Revisión final", "Final check";
    StBinaries: "clio y clio-vault", "clio and clio-vault";
    StStopVault: "Detener la bóveda", "Stop the vault";
    StRemoveTimer: "Quitar el temporizador", "Remove the timer";
    StRemoveNodePi: "Quitar Node y Pi privados", "Remove the private Node and Pi";
    StRemoveBinaries: "Quitar clio y clio-vault", "Remove clio and clio-vault";
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
    NoteOn: "encendido", "on";
    NoteRestarted: "reiniciado", "restarted";
    NoteOff: "apagado", "off";
    NoteKept: "{} se queda", "{} stays";
    NoteDoctorOk: "todo bien", "all good";
    NoteNewVault: "creada y abierta", "created and open";
    NoteOpened: "abierta", "open";
    NoteMac: "macOS: sin jaula todavía; los servicios no corren", "macOS: no jail yet; services do not run";
    NoteMacTimer: "macOS: no hay temporizador todavía", "macOS: no timer yet";
    NoteNoSudo: "sin sudo: pide a un administrador {}", "no sudo: ask an admin for {}";
    NoteNoPkg: "no encontré un gestor de paquetes: instala bubblewrap a mano", "no package manager found: install bubblewrap by hand";
    NoteSysSkipped: "omitido: sin la jaula no corren los servicios ni run_tests", "skipped: without the jail services and run_tests do not run";
    NoteSysFailed: "no se pudo: {}", "it did not work: {}";
    NoteNoTarget: "no hay una versión para este sistema", "there is no build for this system";
    NoteNewer: "{} (más nuevo, se deja)", "{} (newer, left as it is)";
    NoteNoVault: "sin bóveda", "no vault";
    NoteTemis: "Temis encendido", "Temis on";

    FailNet: "No se pudo descargar {}.", "Could not download {}.";
    FixNet: "Revisa la conexión y corre «clio setup» otra vez.", "Check the connection and run `clio setup` again.";
    FailSum: "{} no coincide con su suma: no se instaló nada.", "{} does not match its checksum: nothing was installed.";
    FixSum: "Vuelve a intentarlo; si se repite, avisa en github.com/i-montes/clio.", "Try again; if it happens again, report it at github.com/i-montes/clio.";
    FixOther: "Corrige eso y corre «clio setup» otra vez.", "Fix that and run `clio setup` again.";
    FailRuntime: "XDG_RUNTIME_DIR no está definido.", "XDG_RUNTIME_DIR is not set.";
    FixRuntime: "Entra con una sesión normal (ssh o escritorio) y corre «clio setup».", "Log in through a normal session (ssh or desktop) and run `clio setup`.";
    FailPass: "La contraseña no abre la bóveda.", "The passphrase does not open the vault.";
    FixPass: "Corre «clio vault start» cuando la recuerdes.", "Run `clio vault start` when you remember it.";

    VaultNewTitle: "Crea la contraseña de tu bóveda", "Create your vault's passphrase";
    VaultNewHint: "Guarda tus claves en esta máquina. Nadie puede recuperarla: anótala en un lugar seguro.", "It keeps your keys on this machine. Nobody can recover it: write it down somewhere safe.";
    VaultOpenTitle: "La contraseña de tu bóveda", "Your vault's passphrase";
    VaultOpenHint: "Para abrir la bóveda que ya tienes.", "To open the vault you already have.";
    FieldPass: "Contraseña", "Passphrase";
    FieldAgain: "Otra vez", "Again";
    ErrMismatch: "No coinciden: escríbela otra vez.", "They do not match: type it again.";
    ErrEmpty: "Está vacía.", "It is empty.";
    ErrWrongPass: "No abre la bóveda: ¿la escribiste bien?", "It does not open the vault: did you type it right?";

    ModelTitle: "¿Qué modelo usará Clio?", "Which model will Clio use?";
    ModelHint: "Su clave va directo a la bóveda.", "Its key goes straight into the vault.";
    ModelClaude: "Suscripción de Claude (Pro o Max)", "Claude subscription (Pro or Max)";
    ModelApi: "Clave de la API de Anthropic", "Anthropic API key";
    ModelOpencode: "OpenCode", "OpenCode";
    ModelLater: "Más tarde", "Later";
    ClaudeTitle: "Un token de tu suscripción de Claude", "A token of your Claude subscription";
    ClaudeHint: "Claude Code lo da con su propio inicio de sesión.", "Claude Code gives it through its own login.";
    ClaudeGet: "Conseguirlo ahora (se abre el inicio de sesión de Claude Code)", "Get it now (Claude Code's login opens)";
    ClaudeHave: "Ya tengo uno", "I already have one";
    ClaudeCopy: "Copia el token que aparece arriba y pulsa Enter: la pantalla se borrará.", "Copy the token shown above and press Enter: the screen will be cleared.";
    KeyTitle: "Pega tu clave de {}", "Paste your {} key";
    KeyClaudeHint: "Empieza por sk-ant-oat. Sale de «claude setup-token».", "It starts with sk-ant-oat. It comes from `claude setup-token`.";
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
    TemisKeyHint: "Va directo a la bóveda.", "It goes straight into the vault.";

    SudoTitle: "Hace falta tu contraseña del sistema, una vez.", "Your system password is needed, once.";
    SudoBwrap: "Instalar bubblewrap ({}): la jaula de los comandos.", "Install bubblewrap ({}): the jail of the commands.";
    SudoLinger: "Encender lingering: el temporizador sigue sin sesión abierta.", "Turn on lingering: the timer runs without a session open.";
    SudoHow: "sudo la pide en la terminal; Clio nunca la ve.", "sudo asks for it in the terminal; Clio never sees it.";
    SudoGo: "Seguir", "Continue";
    SudoSkip: "Omitir", "Skip";

    ChatOpenTitle: "Hay un chat de Clio abierto.", "A Clio chat is open.";
    ChatOpenHint: "Ciérralo y pulsa Enter.", "Close it and press Enter.";
    ConfirmTitle: "¿Quitar todo?", "Remove everything?";
    ConfirmLost: "Se pierden la bóveda con todas sus claves, la memoria, las conversaciones, la configuración, las habilidades, los servicios y los servidores MCP.", "You lose the vault with every key in it, the memory, the conversations, the configuration, the skills, the services and the MCP servers.";
    ConfirmNo: "No, conservarlo", "No, keep it";
    ConfirmYes: "Sí, quitarlo todo", "Yes, remove everything";

    DoneTitle: "Clio está lista.", "Clio is ready.";
    DoneLeft: "Falta una cosa: {}", "One thing left: {}";
    LeftModel: "conectar un modelo (clio connect claude, anthropic-api u opencode)", "connect a model (clio connect claude, anthropic-api or opencode)";
    LeftVault: "abrir la bóveda (clio vault start) y conectar un modelo", "open the vault (clio vault start) and connect a model";
    DoneFailed: "Algo falló.", "Something failed.";
    DoneFailedLine: "Arregla lo que dice el paso y corre «clio setup» otra vez.", "Fix what the step says and run `clio setup` again.";
    RemovedTitle: "Clio se quitó.", "Clio was removed.";
    RemovedKept: "Tus datos siguen en {}.", "Your data stays in {}.";
    RemovedMemory: "clio-memory se queda: otros agentes pueden usarlo.", "clio-memory stays: other agents may use it.";

    NoTerminal: "No hay una terminal: corre «clio setup» en una.", "There is no terminal: run `clio setup` in one.";
    UpdateNone: "Clio ya está al día ({}).", "Clio is already up to date ({}).";
    UpdateAvailable: "Hay una versión nueva: {} → {}. Corre «clio update».", "A newer version exists: {} → {}. Run `clio update`.";
    UpdateChatOpen: "Hay un chat de Clio abierto: ciérralo y corre «clio update» otra vez.", "A Clio chat is open: close it and run `clio update` again.";
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
}
