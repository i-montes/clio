//! Host mode: which actions are set aside for the person (four classes: `borrar`, `gastar`,
//! `enviar`, `control`) and what counts as foreign text.
//!
//! A command is cut into simple commands (at `;`, `&&`, `||`, `|`, new lines, `$(…)`, backticks,
//! outside quotes, and past shell keywords, `sudo`, `env`, `nohup`, `time`, `xargs`, `exec`,
//! `command`, `timeout`, `nice`, `ionice`, `stdbuf`, `sh -c`, `bash -c`, `su -c` and
//! `docker exec`) and each one is classified. This is a list, not a wall: whoever wants to get around it can (a script, a
//! variable, an alias). Against that stand the action log and the prompt's rule, not this module.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};

use Class::{Borrar, Control, Enviar, Gastar};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    Borrar,
    Gastar,
    Enviar,
    Control,
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Borrar => "borrar",
            Gastar => "gastar",
            Enviar => "enviar",
            Control => "control",
        })
    }
}

/// A name that looks like a backup. `{B}` in a factory pattern stands for it.
const BACKUP: &str = r"(?i:backup|respaldo|snapshot|\.bak\b|\.dump\b|\.sql\b)";

/// Matched against each simple command, normalized: no quotes, no wrappers, the program by its
/// bare name, words one space apart. The rest of the table is in `by_code`.
const FACTORY: &[(Class, &str)] = &[
    (Borrar, r"^(rm|truncate) .*{B}"),
    (Borrar, r"^find .*(-delete|-exec(dir)? (rm|shred))\b"),
    (Borrar, r"^(shred|wipefs|mkfs|dropdb)\b"),
    (Borrar, r"^dd .*\bof=/dev/"),
    (
        Borrar,
        r"(?i)^(psql|mysql|mariadb|sqlite3)\b.*\b(drop|truncate|delete from)\b",
    ),
    (Borrar, r"^docker (volume (rm|prune)|system prune)\b"),
    (Borrar, r"^docker(-| )compose down\b.* (-v|--volumes)\b"),
    (Borrar, r"(?i)^redis-cli\b.*\bflush(all|db)\b"),
    (
        Borrar,
        r"^(kubectl delete|terraform destroy|zfs destroy|apt(-get)? purge|gh repo delete)\b",
    ),
    (Borrar, r"^apt(-get)? (remove|autoremove)\b.* --purge\b"),
    (Borrar, r"^userdel\b.* (-[a-zA-Z]*r[a-zA-Z]*|--remove)\b"),
    (
        Gastar,
        r"^(aws|gcloud|oci|doctl|hcloud)\b.* (create|launch|run-instances)\b",
    ),
    (Gastar, r"^(terraform apply|pulumi up|fly(ctl)? scale)\b"),
    (Enviar, r"^(mail|mailx|sendmail|msmtp|swaks)\b"),
    (
        Enviar,
        r"^gh (pr (merge|comment|review)|issue (create|comment)|release create)\b",
    ),
    (
        Enviar,
        r"^((npm|yarn|pnpm|cargo) publish|twine upload|(docker|gem) push)\b",
    ),
    // `approve`/`reject`/`apply`/`undo` are the person's answers: the model must not give them.
    (
        Control,
        r"^clio (init|vault|connect|memory|approve|reject|apply|undo)\b",
    ),
    (Control, r"^clio-memory (purge|import)\b"),
    (
        Control,
        r"^systemctl( -\S+)* (start|stop|restart|reload|enable|disable|mask|unmask|kill|edit|revert|set-property)\b.*\bclio",
    ),
    (
        Control,
        r"^systemctl( -\S+)* (reboot|poweroff|halt|kexec)\b",
    ),
    (Control, r"^(visudo|reboot|shutdown|poweroff|halt)\b"),
    (Control, r"/etc/sudoers"),
    // Who may use the machine.
    (
        Control,
        r"^(useradd|adduser|usermod|passwd|chpasswd|gpasswd)\b",
    ),
    (Control, r"authorized_keys"),
    // The person's service manager (and the system's) reached over D-Bus runs things outside
    // the launcher, unmasked: `systemd-run --user clio approve …`.
    (Control, r"^(systemd-run|gdbus|dbus-send|busctl)\b"),
];

/// Absolute folders match by prefix; a bare name matches a folder of that name anywhere.
const FOREIGN: [&str; 6] = [
    "/var/log",
    "/var/mail",
    "/var/spool",
    "uploads",
    "Downloads",
    "Descargas",
];

/// Wrappers that run the command after them: their options that take a value, and how many plain
/// words (`timeout`'s duration) come before the command.
const WRAPPERS: [(&str, &[&str], usize); 11] = [
    ("sudo", &["-u", "-g", "--user", "--group"], 0),
    ("env", &["-u", "-C", "--unset", "--chdir"], 0),
    ("nohup", &[], 0),
    ("time", &[], 0),
    ("xargs", &["-n", "-I", "-L", "-P"], 0),
    ("exec", &["-a"], 0),
    ("command", &[], 0),
    ("timeout", &["-s", "-k", "--signal", "--kill-after"], 1),
    ("nice", &["-n", "--adjustment"], 0),
    (
        "ionice",
        &["-c", "-n", "-p", "-P", "-u", "--class", "--classdata"],
        0,
    ),
    ("stdbuf", &["-i", "-o", "-e"], 0),
];

/// Shell words that come before a command: `then rm -rf x`, `{ rm -rf x; }`.
const KEYWORDS: [&str; 9] = [
    "if", "then", "do", "else", "elif", "while", "until", "{", "!",
];

/// The options of `docker compose` that come before its subcommand and take a value.
const COMPOSE: &[&str] = &[
    "-f",
    "--file",
    "-p",
    "--project-name",
    "--env-file",
    "--profile",
    "--project-directory",
];

/// The options of `docker exec` that take a value.
const EXEC: &[&str] = &[
    "-e",
    "--env",
    "--env-file",
    "-u",
    "--user",
    "-w",
    "--workdir",
    "--detach-keys",
];

/// Programs that reach outside the launcher by themselves: a daemon's socket (`docker`, which is
/// root and has the network) or another machine.
const REACH_OUT: [&str; 6] = ["docker", "docker-compose", "podman", "ssh", "scp", "rsync"];

/// Programs whose global options can come before the subcommand, with the ones that take a value.
const GLOBALS: [(&str, &[&str]); 7] = [
    (
        "git",
        &[
            "-C",
            "-c",
            "--git-dir",
            "--work-tree",
            "--namespace",
            "--config-env",
        ],
    ),
    (
        "docker",
        &[
            "-c",
            "--context",
            "-H",
            "--host",
            "--config",
            "-l",
            "--log-level",
        ],
    ),
    (
        "kubectl",
        &[
            "-n",
            "--namespace",
            "--context",
            "--cluster",
            "--user",
            "-s",
            "--server",
            "--kubeconfig",
            "--token",
            "--as",
            "--as-group",
        ],
    ),
    ("gh", &["-R", "--repo"]),
    ("terraform", &[]),
    ("apt", &["-o", "-c", "-t"]),
    ("apt-get", &["-o", "-c", "-t"]),
];

#[derive(Debug, Clone)]
pub struct HostRules {
    rules: Vec<(Class, Regex)>,
    foreign: Vec<PathBuf>,
    control: Option<Regex>,
    backup: Regex,
    /// A `>` onto a file that looks like a backup.
    overwrite: Regex,
    questions_per_day: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostFile {
    questions_per_day: Option<u32>,
    #[serde(default)]
    foreign: Vec<PathBuf>,
    #[serde(default)]
    ask: Vec<Ask>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ask {
    class: Class,
    pattern: String,
}

impl HostRules {
    pub fn factory() -> Self {
        let re = |p: &str| Regex::new(&p.replace("{B}", BACKUP)).expect("a factory pattern");
        Self {
            rules: FACTORY.iter().map(|(c, p)| (*c, re(p))).collect(),
            foreign: FOREIGN.iter().map(PathBuf::from).collect(),
            control: None,
            backup: re(BACKUP),
            overwrite: re(r"(^|[^>])>\s*[^\s>&]*{B}"),
            questions_per_day: 3,
        }
    }

    /// Clio's own paths: a command that names one, as `/abs`, or under `home` as `~/rel`,
    /// `$HOME/rel` or `rel`, is `control`.
    pub fn with_control(mut self, home: &Path, control: &[PathBuf]) -> Self {
        let mut spellings = Vec::new();
        for p in control {
            spellings.push(regex::escape(&p.to_string_lossy()));
            if let Ok(rel) = p.strip_prefix(home).map(Path::to_string_lossy)
                && !rel.is_empty()
            {
                let rel = regex::escape(&rel);
                spellings.push(format!(r"(~/|\$HOME/|\$\{{HOME\}}/|\./)?{rel}"));
            }
        }
        self.control = (!spellings.is_empty()).then(|| {
            let any = spellings.join("|");
            Regex::new(&format!(r"(^|[^\w.~/$-])({any})($|[^\w.-])")).expect("escaped paths")
        });
        self
    }

    /// The person's `host.json`: it can only add (patterns are matched like the factory ones,
    /// against each normalized simple command).
    pub fn with_file(mut self, json: &str) -> Result<Self, String> {
        let file: HostFile = serde_json::from_str(json).map_err(|e| format!("host.json: {e}"))?;
        for f in file.foreign {
            let mut c = f.components();
            let name = matches!((c.next(), c.next()), (Some(Component::Normal(_)), None));
            if !f.is_absolute() && !name {
                return Err(format!(
                    "host.json: la carpeta ajena «{}» tiene que ser una ruta absoluta o un nombre de carpeta",
                    f.display()
                ));
            }
            self.foreign.push(f);
        }
        for a in file.ask {
            let re = Regex::new(&a.pattern)
                .map_err(|e| format!("host.json: patrón inválido «{}»: {e}", a.pattern))?;
            self.rules.push((a.class, re));
        }
        if let Some(n) = file.questions_per_day {
            self.questions_per_day = n;
        }
        Ok(self)
    }

    pub fn questions_per_day(&self) -> u32 {
        self.questions_per_day
    }

    pub fn classify_command(&self, command: &str) -> Option<Class> {
        if self.control.as_ref().is_some_and(|r| r.is_match(command)) {
            return Some(Control);
        }
        simple_commands(command).iter().find_map(|w| {
            let line = w.join(" ");
            let w: Vec<&str> = w.iter().map(String::as_str).collect();
            self.rules
                .iter()
                .find(|(_, r)| r.is_match(&line))
                .map(|(c, _)| *c)
                .or_else(|| self.by_code(&w, &line))
        })
    }

    pub fn classify_write(&self, path: &Path) -> Option<Class> {
        let p = path.to_string_lossy();
        if p.starts_with("/etc/sudoers") || p.contains("authorized_keys") {
            Some(Control)
        } else if path.exists()
            && path
                .file_name()
                .is_some_and(|n| self.backup.is_match(&n.to_string_lossy()))
        {
            Some(Borrar)
        } else {
            None
        }
    }

    /// `path` is taken as given: the caller resolves `..` and links first.
    pub fn is_foreign(&self, path: &Path) -> bool {
        self.foreign.iter().any(|f| {
            if f.is_absolute() {
                path.starts_with(f)
            } else {
                path.components().any(|c| c.as_os_str() == f.as_os_str())
            }
        })
    }

    pub fn reads_foreign(&self, command: &str) -> bool {
        simple_commands(command).iter().any(|w| {
            let w: Vec<&str> = w.iter().map(String::as_str).collect();
            matches!(
                w.as_slice(),
                ["journalctl", ..]
                    | ["docker" | "docker-compose" | "kubectl", "logs", ..]
                    | ["docker", "compose", "logs", ..]
            ) || w.iter().any(|a| {
                let a = a.rsplit('=').next().unwrap_or(a);
                self.is_foreign(Path::new(a.trim_start_matches('<')))
            })
        })
    }

    /// A command that reaches past the launcher's network cut by itself (Unix sockets are not cut):
    /// it marks the session as `web`, as `network: true` does.
    pub fn reaches_out(&self, command: &str) -> bool {
        simple_commands(command)
            .iter()
            .any(|w| REACH_OUT.contains(&w[0].as_str()))
    }

    /// The rows a pattern says badly: they look at the arguments one by one.
    fn by_code(&self, w: &[&str], line: &str) -> Option<Class> {
        match w {
            ["rm", args @ ..] if rm_recursive(args) => Some(Borrar),
            // Moving a backup away is losing it; making one (`mv x x.bak`) is not.
            ["mv", args @ ..]
                if args
                    .iter()
                    .filter(|a| !a.starts_with('-'))
                    .rev()
                    .skip(1)
                    .any(|a| self.backup.is_match(a)) =>
            {
                Some(Borrar)
            }
            ["git", "push", args @ ..] if push_shared(args) => Some(Enviar),
            ["curl", args @ ..] if sends_out(args, true) => Some(Enviar),
            ["wget", args @ ..] if sends_out(args, false) => Some(Enviar),
            _ if self.overwrite.is_match(line) && !makes_backup(w, line) => Some(Borrar),
            _ => None,
        }
    }
}

/// Simple commands as words: quotes dropped (a quoted text is one word), wrappers and shell
/// keywords peeled, the program by its bare name, and its global options dropped (`git -C d push`
/// is `git push`). A shell's or `su`'s `-c` script, and what `docker exec` runs in a container,
/// are looked into as commands of their own.
fn simple_commands(command: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for words in split(command) {
        normalize(&words, &mut out);
    }
    out
}

/// A shell line cut into simple commands at `;`, `&`, `|`, new lines, backticks and parentheses
/// outside quotes. What a double-quoted text runs (`$(…)`, backticks) is a command of its own too.
fn split(line: &str) -> Vec<Vec<String>> {
    let (mut cmds, mut words, mut word) = (Vec::new(), Vec::new(), None::<String>);
    let mut nested = Vec::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => word
                .get_or_insert_default()
                .extend(chars.by_ref().take_while(|&c| c != '\'')),
            '"' => {
                let mut q = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => q.extend(chars.next()),
                        c => q.push(c),
                    }
                }
                if q.contains("$(") || q.contains('`') {
                    nested.push(q.clone());
                }
                word.get_or_insert_default().push_str(&q);
            }
            '\\' => match chars.next() {
                Some('\n') | None => {}
                Some(n) => word.get_or_insert_default().push(n),
            },
            ';' | '&' | '|' | '\n' | '`' | '(' | ')' => {
                words.extend(word.take());
                cmds.push(std::mem::take(&mut words));
            }
            c if c.is_whitespace() => words.extend(word.take()),
            c => word.get_or_insert_default().push(c),
        }
    }
    words.extend(word.take());
    cmds.push(words);
    for q in nested {
        cmds.extend(split(&q));
    }
    cmds
}

fn normalize(words: &[String], out: &mut Vec<Vec<String>>) {
    let mut w = unwrap(words).to_vec();
    let Some(p) = w.first_mut() else { return };
    if !p.contains(['<', '>']) {
        *p = p.rsplit('/').next().unwrap_or(p).to_owned();
    }
    if let Some(s) = script(&w) {
        out.extend(simple_commands(s));
        return;
    }
    if let Some((_, valued)) = GLOBALS.iter().find(|(n, _)| w[0] == *n) {
        drop_options(&mut w, 1, valued);
    }
    match w.get(..2).unwrap_or(&[]) {
        [d, c] if d == "docker" && c == "compose" => drop_options(&mut w, 2, COMPOSE),
        [d, _] if d == "docker-compose" => drop_options(&mut w, 1, COMPOSE),
        _ => {}
    }
    // `docker exec [options] CONTAINER COMMAND…`: the command is classified too.
    if w.len() > 2 && matches!(w[0].as_str(), "docker" | "podman") && w[1] == "exec" {
        let mut i = 2;
        while let Some(o) = w.get(i).filter(|o| o.starts_with('-')) {
            i += if takes_value(o, EXEC) { 2 } else { 1 };
        }
        if let Some(inner) = w.get(i + 1..).filter(|r| !r.is_empty()) {
            normalize(inner, out);
        }
    }
    out.push(w);
}

/// The script a shell (`bash -c '…'`) or `su` (`su - u -c '…'`) runs.
fn script(w: &[String]) -> Option<&str> {
    match w {
        [sh, c, s, ..]
            if matches!(sh.as_str(), "sh" | "bash")
                && c.starts_with('-')
                && !c.starts_with("--")
                && c.ends_with('c') =>
        {
            Some(s)
        }
        [su, args @ ..] if su == "su" => {
            args.iter().enumerate().find_map(|(i, a)| match a.as_str() {
                "-c" | "--command" => args.get(i + 1).map(String::as_str),
                a => a.strip_prefix("--command="),
            })
        }
        _ => None,
    }
}

/// Drops the options from `w[from]` on, and the values of those in `valued`.
fn drop_options(w: &mut Vec<String>, from: usize, valued: &[&str]) {
    let mut i = from;
    while let Some(o) = w.get(i).filter(|o| o.starts_with('-')) {
        i += if valued.contains(&o.as_str()) { 2 } else { 1 };
    }
    w.drain(from..i.min(w.len()));
}

/// `opt` takes the next word: it is in `valued`, or it is a cluster of short options (`-iu`)
/// whose last one is.
fn takes_value(opt: &str, valued: &[&str]) -> bool {
    valued.contains(&opt)
        || (!opt.starts_with("--")
            && opt.len() > 2
            && opt
                .chars()
                .last()
                .is_some_and(|c| valued.contains(&format!("-{c}").as_str())))
}

fn unwrap(mut w: &[String]) -> &[String] {
    loop {
        let Some((first, rest)) = w.split_first() else {
            return w;
        };
        if KEYWORDS.contains(&first.as_str()) || first.contains('=') {
            w = rest;
            continue;
        }
        let Some((_, valued, plain)) = WRAPPERS.iter().find(|(n, ..)| first == n) else {
            return w;
        };
        w = rest;
        while let Some((opt, more)) = w.split_first() {
            if takes_value(opt, valued) {
                w = more.get(1..).unwrap_or(&[]);
            } else if opt.starts_with('-') || opt.contains('=') {
                w = more;
            } else {
                break;
            }
        }
        w = w.get(*plain..).unwrap_or(&[]);
    }
}

/// `rm -r` of anything but build output or `/tmp` (with no target: fed by `xargs`).
fn rm_recursive(args: &[&str]) -> bool {
    let recursive = args.iter().any(|a| {
        *a == "--recursive"
            || (a.starts_with('-') && !a.starts_with("--") && a.contains(['r', 'R']))
    });
    let mut targets = args.iter().filter(|a| !a.starts_with('-')).peekable();
    let disposable = |t: &str| {
        !t.contains("..")
            && (t.starts_with("/tmp/")
                || matches!(
                    Path::new(t).file_name().and_then(|n| n.to_str()),
                    Some(
                        "node_modules"
                            | "target"
                            | "dist"
                            | "build"
                            | ".venv"
                            | "venv"
                            | "__pycache__"
                            | ".cache"
                            | ".next"
                            | "coverage"
                    )
                ))
    };
    recursive && (targets.peek().is_none() || targets.any(|t| !disposable(t)))
}

/// What writes the file makes a backup (a dump, an archive, a copy), or it is text typed right
/// there (a heredoc): writing it is not losing one.
fn makes_backup(w: &[&str], line: &str) -> bool {
    const MAKERS: [&str; 11] = [
        "pg_dump",
        "pg_dumpall",
        "mysqldump",
        "mariadb-dump",
        "tar",
        "zip",
        "gzip",
        "cp",
        "rsync",
        "restic",
        "borg",
    ];
    MAKERS.contains(&w[0]) || (w[0] == "sqlite3" && w.contains(&".dump")) || line.contains("<<")
}

/// Pushing a work branch of one's own is not sending; a bare push, a forced one, tags, deletes,
/// and `main`/`master`/`HEAD` are.
fn push_shared(args: &[&str]) -> bool {
    const PLAIN: [&str; 6] = ["-u", "--set-upstream", "-q", "--quiet", "-v", "--verbose"];
    let flagged = args
        .iter()
        .any(|a| a.starts_with('-') && !PLAIN.contains(a));
    let refs: Vec<&str> = args
        .iter()
        .filter(|a| !a.starts_with('-'))
        .skip(1)
        .copied()
        .collect();
    flagged
        || refs.is_empty()
        || refs.iter().any(|r| {
            let dst = r
                .rsplit(':')
                .next()
                .unwrap_or(r)
                .trim_start_matches("refs/heads/");
            r.starts_with(['+', ':']) || matches!(dst, "main" | "master" | "HEAD")
        })
}

/// curl or wget that sends data or a non-GET method to anywhere but this machine. `curl`: a
/// cluster of short options (`-sd`, `-sSX POST`) counts too.
fn sends_out(args: &[&str], curl: bool) -> bool {
    const DATA: [&str; 9] = [
        "-d",
        "-F",
        "-T",
        "--data",
        "--form",
        "--upload-file",
        "--json",
        "--post-data",
        "--post-file",
    ];
    let sending = args.iter().enumerate().any(|(i, a)| {
        let short = if curl { cluster(a) } else { None };
        let method = ["-X", "--request", "--method"]
            .iter()
            .find_map(|f| a.strip_prefix(f))
            .map(|m| m.trim_start_matches('='))
            .or(short.filter(|(o, _)| *o == 'X').map(|(_, m)| m));
        DATA.iter().any(|d| a.starts_with(d))
            || short.is_some_and(|(o, _)| matches!(o, 'd' | 'F' | 'T'))
            || method.is_some_and(|m| {
                let m = if m.is_empty() {
                    args.get(i + 1).copied().unwrap_or("")
                } else {
                    m
                };
                !m.eq_ignore_ascii_case("GET") && !m.eq_ignore_ascii_case("HEAD")
            })
    });
    let urls: Vec<&&str> = args.iter().filter(|a| a.contains("://")).collect();
    sending && (urls.is_empty() || !urls.iter().all(|u| local(u)))
}

/// In a cluster of curl's short options (`-sSX`), the first one that takes a value and what is
/// left of the cluster after it (its value, or empty when the value is the next word).
fn cluster(a: &str) -> Option<(char, &str)> {
    const VALUED: &str = "AbcCdDeEFHKmoPQrtTuUwxXyYz";
    let opts = a
        .strip_prefix('-')
        .filter(|o| o.len() > 1 && !o.starts_with('-'))?;
    opts.char_indices()
        .find(|(_, c)| VALUED.contains(*c))
        .map(|(i, c)| (c, &opts[i + c.len_utf8()..]))
}

fn local(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = auth.rsplit('@').next().unwrap_or("");
    let h = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => host.split(':').next().unwrap_or(""),
    };
    matches!(h, "localhost" | "::1" | "0.0.0.0")
        || (h.starts_with("127.") && h.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> HostRules {
        HostRules::factory().with_control(
            Path::new("/home/u"),
            &[
                PathBuf::from("/home/u/.config/clio"),
                PathBuf::from("/home/u/.local/share/clio"),
            ],
        )
    }

    /// The plan's table (Task 3), row by row.
    const TABLE: &[(Option<Class>, &[&str])] = &[
        (
            Some(Borrar),
            &["rm -rf ~/proyecto", "rm -r datos", "sudo rm -fR /srv/x"],
        ),
        (
            None,
            &[
                "rm -rf node_modules",
                "rm -rf target dist",
                "rm -rf /tmp/x",
                "rm archivo.txt",
            ],
        ),
        (
            Some(Borrar),
            &[
                "rm backup.sql",
                "mv /var/backups/a /tmp",
                "truncate -s 0 dump.sql.gz",
                "> respaldo.tar",
            ],
        ),
        (
            Some(Borrar),
            &[
                "find . -name '*.log' -delete",
                "find /srv -exec rm {} +",
                "shred x",
            ],
        ),
        (
            Some(Borrar),
            &[
                "mkfs.ext4 /dev/sdb",
                "dd if=x of=/dev/sda",
                "wipefs -a /dev/sdb",
            ],
        ),
        (
            Some(Borrar),
            &[
                "psql -c 'DROP TABLE t'",
                "mysql -e \"truncate table t\"",
                "sqlite3 d 'delete from t'",
                "dropdb x",
            ],
        ),
        (
            Some(Borrar),
            &[
                "docker volume rm v",
                "docker system prune -af",
                "docker compose down -v",
            ],
        ),
        (
            Some(Borrar),
            &[
                "redis-cli flushall",
                "kubectl delete ns x",
                "terraform destroy",
                "zfs destroy p/x",
                "userdel -r u",
                "apt-get purge x",
            ],
        ),
        (
            None,
            &[
                "docker compose down",
                "docker rm c",
                "git reset --hard",
                "git clean -fd",
            ],
        ),
        (
            Some(Gastar),
            &[
                "aws ec2 run-instances --image-id ami-1",
                "gcloud compute instances create x",
                "oci compute instance launch",
                "doctl compute droplet create",
                "hcloud server create",
                "terraform apply",
                "pulumi up",
                "fly scale count 3",
            ],
        ),
        (
            None,
            &[
                "aws s3 ls",
                "gcloud compute instances list",
                "terraform plan",
            ],
        ),
        (
            Some(Enviar),
            &[
                "mail -s x a@b.c",
                "sendmail a@b.c",
                "msmtp a@b.c",
                "swaks --to a@b.c",
            ],
        ),
        (
            Some(Enviar),
            &[
                "git push",
                "git push origin main",
                "git push origin HEAD",
                "git push -f origin rama",
                "git push --tags",
                "git push origin :rama",
            ],
        ),
        (
            None,
            &["git push origin feat/x", "git push -u origin fix-1"],
        ),
        // Global options before the subcommand do not hide it.
        (
            Some(Enviar),
            &[
                "git -C dir push origin main",
                "git -c k=v push",
                "git --no-pager -C a push --tags",
                "gh -R o/r pr merge 3",
            ],
        ),
        (
            Some(Borrar),
            &[
                "docker -H h volume rm v",
                "docker --context x system prune -af",
                "kubectl -n x delete pod p",
                "kubectl --context=c delete ns x",
                "gh --repo o/r repo delete x",
            ],
        ),
        (
            None,
            &[
                "git -C dir push origin feat/x",
                "git -C dir status",
                "docker -H h ps",
                "kubectl -n x get pods",
            ],
        ),
        (
            Some(Enviar),
            &[
                "gh pr merge 3",
                "gh pr comment 3 -b x",
                "gh issue create",
                "gh release create v1",
                "gh pr review 3 --approve",
            ],
        ),
        (None, &["gh pr create --fill", "gh pr list", "gh run view"]),
        (Some(Borrar), &["gh repo delete x"]),
        (
            Some(Enviar),
            &[
                "npm publish",
                "cargo publish",
                "twine upload dist/*",
                "docker push img",
                "gem push x.gem",
            ],
        ),
        (
            Some(Enviar),
            &[
                "curl -X POST https://api.x.com -d a=1",
                "curl -F f=@x https://x.com",
                "curl -T f https://x.com",
                "wget --post-data=a https://x.com",
            ],
        ),
        (
            None,
            &[
                "curl https://x.com",
                "curl -X POST http://localhost:3000/x -d a=1",
                "curl -d a=1 http://127.0.0.1:8080",
            ],
        ),
        (
            Some(Control),
            &[
                "clio vault stop",
                "clio connect claude",
                "clio init",
                "clio memory identity",
                "clio-memory purge",
                "clio-memory import x",
            ],
        ),
        (
            None,
            &["clio-memory m search x", "clio doctor", "clio pending"],
        ),
        (
            Some(Control),
            &[
                "cat ~/.config/clio/config.json",
                "ls ~/.local/share/clio",
                "head /home/u/.local/share/clio/m.db",
                "cp x \"$HOME/.config/clio/\"",
            ],
        ),
        (
            Some(Control),
            &[
                "systemctl --user stop clio-memory-sleep.timer",
                "sudo systemctl disable clio-telegram",
            ],
        ),
        (
            Some(Control),
            &[
                "visudo",
                "sudo tee /etc/sudoers.d/x",
                "reboot",
                "sudo shutdown -h now",
                "poweroff",
            ],
        ),
        (
            None,
            &[
                "systemctl status nginx",
                "sudo systemctl restart nginx",
                "sudo apt-get install -y jq",
            ],
        ),
        // Ordinary spellings: shell keywords, more wrappers, options before the subcommand.
        (
            Some(Borrar),
            &[
                "if [ -d x ]; then rm -rf x; fi",
                "for d in a b; do rm -rf \"$d\"; done",
                "{ rm -rf ~/p; }",
                "timeout 60 rm -rf ~/p",
                "nice -n 19 rm -rf ~/p",
                "docker compose -f prod.yml down -v",
                "docker-compose -p app down --volumes",
                "sudo -iu postgres psql -c 'DROP DATABASE app'",
                "sudo --user postgres dropdb app",
                "su - postgres -c 'dropdb app'",
                "docker exec db psql -U u -c 'DROP TABLE users'",
                "terraform -chdir=infra destroy",
                "sudo apt-get -y purge nginx",
                "apt-get remove --purge nginx",
                "userdel -rf bob",
            ],
        ),
        (Some(Gastar), &["terraform -chdir=infra apply"]),
        (
            Some(Enviar),
            &[
                "curl -sd a=1 https://x.com",
                "curl -sSX POST https://x.com",
                "gh issue comment 3 -b x",
            ],
        ),
        // Making a backup is not losing one; build output and caches are not work.
        (
            None,
            &[
                "pg_dump app > backup.sql",
                "mv nginx.conf nginx.conf.bak",
                "rm -rf build",
                "rm -rf .venv __pycache__",
                "git commit -m \"fix; rm -rf cache handling\"",
                "git commit -m 'chore: tidy (rm -r old dir)'",
                "cat <<EOF > schema.sql",
                "curl -sS https://x.com",
            ],
        ),
        (
            Some(Borrar),
            &[
                "cat x > backup.sql",
                "mv backup.sql /tmp/x",
                "echo \"$(rm -rf ~/x)\"",
            ],
        ),
        // Who may use the machine, and the service managers over D-Bus (they run unmasked).
        (
            Some(Control),
            &[
                "useradd -m eve",
                "sudo adduser eve",
                "usermod -aG sudo eve",
                "passwd eve",
                "chpasswd < x",
                "gpasswd -a eve docker",
                "echo k >> ~/.ssh/authorized_keys",
                "systemd-run --user clio approve 1",
                "gdbus call --session -d org.freedesktop.systemd1 -m x",
                "dbus-send --session --dest=org.freedesktop.systemd1 /x",
                "busctl --user call org.freedesktop.systemd1 /x",
            ],
        ),
    ];

    #[test]
    fn the_table() {
        let r = rules();
        for (class, commands) in TABLE {
            for c in *commands {
                assert_eq!(r.classify_command(c), *class, "{c}");
            }
        }
    }

    #[test]
    fn a_command_is_a_list_of_simple_commands() {
        let r = rules();
        for c in [
            "ls; rm -rf ~/x",
            "cd x && git push",
            "make || rm -rf ~/x",
            "echo $(rm -rf ~/x)",
            "echo `rm -rf ~/x`",
            "ls\nrm -rf ~/x",
            "bash -c \"rm -rf ~/x\"",
            "sh -c 'ls; rm -rf ~/x'",
            "env FOO=1 nohup time rm -rf ~/x",
            "FOO=1 /bin/rm -rf ~/x",
            "find . -type d | xargs -n 1 rm -rf",
            "sudo -u postgres psql -c 'drop database x'",
        ] {
            assert!(r.classify_command(c).is_some(), "{c}");
        }
        for c in [
            "ls ~/.config/clio-other",
            "cat proyecto/.config/clio/x",
            "echo ok >> backup.log",
            "curl -X GET https://x.com",
        ] {
            assert_eq!(r.classify_command(c), None, "{c}");
        }
    }

    #[test]
    fn writes() {
        let r = HostRules::factory();
        let d = tempfile::tempdir().unwrap();
        for name in [
            "backup.tar",
            "db.sql.gz",
            "x.bak",
            "y.dump",
            "snapshot-1",
            "respaldo",
        ] {
            let p = d.path().join(name);
            assert_eq!(r.classify_write(&p), None, "{name} does not exist yet");
            std::fs::write(&p, "x").unwrap();
            assert_eq!(r.classify_write(&p), Some(Borrar), "{name}");
        }
        let p = d.path().join("notes.txt");
        std::fs::write(&p, "x").unwrap();
        assert_eq!(r.classify_write(&p), None);
        assert_eq!(r.classify_write(Path::new("/etc/sudoers")), Some(Control));
        assert_eq!(
            r.classify_write(Path::new("/etc/sudoers.d/x")),
            Some(Control)
        );
        assert_eq!(
            r.classify_write(Path::new("/home/u/.ssh/authorized_keys")),
            Some(Control)
        );
    }

    #[test]
    fn foreign_text() {
        let r = HostRules::factory();
        for p in [
            "/var/log/nginx/a.log",
            "/var/mail/u",
            "/var/spool/x",
            "/srv/app/uploads/a.png",
            "/home/u/Downloads/f",
            "/home/u/Descargas",
        ] {
            assert!(r.is_foreign(Path::new(p)), "{p}");
        }
        for p in ["/var/logs/x", "/home/u/proyecto/src", "/var/lib/x"] {
            assert!(!r.is_foreign(Path::new(p)), "{p}");
        }
        for c in [
            "journalctl -u nginx",
            "docker logs c",
            "docker compose logs web",
            "kubectl logs p",
            "docker --context x logs c",
            "kubectl -n x logs p",
            "tail -f /var/log/syslog",
            "ls ~/Downloads",
            "sudo cat </var/mail/u",
            "grep -r x --include=*.rs /srv/uploads",
        ] {
            assert!(r.reads_foreign(c), "{c}");
        }
        for c in ["ls ~", "cat src/main.rs", "docker ps", "git log"] {
            assert!(!r.reads_foreign(c), "{c}");
        }
        assert!(r.reads_foreign("docker compose -f x.yml logs"));
    }

    #[test]
    fn what_reaches_out_by_itself() {
        let r = HostRules::factory();
        for c in [
            "docker ps",
            "sudo docker run -v /:/h alpine",
            "podman ps",
            "ssh host ls",
            "scp a host:b",
            "rsync -a a host:b",
            "cd x && docker-compose up -d",
        ] {
            assert!(r.reaches_out(c), "{c}");
        }
        for c in ["ls", "echo docker", "git push origin feat/x"] {
            assert!(!r.reaches_out(c), "{c}");
        }
    }

    #[test]
    fn the_persons_file_only_adds() {
        let r = HostRules::factory()
            .with_file(r#"{"questions_per_day": 5, "foreign": ["/srv/inbox", "buzon"], "ask": [{"class": "enviar", "pattern": "^rsync .*prod:"}]}"#)
            .unwrap();
        assert_eq!(r.questions_per_day(), 5);
        assert!(r.is_foreign(Path::new("/srv/inbox/a")) && r.is_foreign(Path::new("/a/buzon/b")));
        assert!(r.is_foreign(Path::new("/var/log/x")));
        assert_eq!(
            r.classify_command("sudo rsync -a x prod:/srv"),
            Some(Enviar)
        );
        assert_eq!(r.classify_command("rm -rf ~/x"), Some(Borrar));
        assert_eq!(
            HostRules::factory()
                .with_file("{}")
                .unwrap()
                .questions_per_day(),
            3
        );

        for (json, says) in [
            (r#"{"remove": ["/var/log"]}"#, "unknown field"),
            (r#"{"foreign": ["a/b"]}"#, "ruta absoluta"),
            (
                r#"{"ask": [{"class": "enviar", "pattern": "("}]}"#,
                "patrón inválido «(»",
            ),
            (
                r#"{"ask": [{"class": "otra", "pattern": "x"}]}"#,
                "unknown variant",
            ),
        ] {
            let e = HostRules::factory().with_file(json).unwrap_err();
            assert!(
                e.starts_with("host.json: ") && e.contains(says),
                "{json}: {e}"
            );
        }
    }

    #[test]
    fn class_names() {
        for (c, n) in [
            (Borrar, "borrar"),
            (Gastar, "gastar"),
            (Enviar, "enviar"),
            (Control, "control"),
        ] {
            assert_eq!(c.to_string(), n);
            assert_eq!(serde_json::to_string(&c).unwrap(), format!("\"{n}\""));
        }
    }
}
