//! Host mode: which actions are set aside for the person (four classes: `borrar`, `gastar`,
//! `enviar`, `control`) and what counts as foreign text.
//!
//! A command is cut into simple commands (at `;`, `&&`, `||`, `|`, new lines, `$(…)`, backticks,
//! and past `sudo`, `env`, `nohup`, `time`, `xargs`, `sh -c`, `bash -c`) and each one is
//! classified. This is a list, not a wall: whoever wants to get around it can (a script, a
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
    (Borrar, r"^(rm|mv|truncate) .*{B}"),
    (Borrar, r"(^|[^>])>\s*[^\s>&]*{B}"),
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
    (Borrar, r"^userdel\b.* (-r|--remove)\b"),
    (
        Gastar,
        r"^(aws|gcloud|oci|doctl|hcloud)\b.* (create|launch|run-instances)\b",
    ),
    (Gastar, r"^(terraform apply|pulumi up|fly(ctl)? scale)\b"),
    (Enviar, r"^(mail|mailx|sendmail|msmtp|swaks)\b"),
    (
        Enviar,
        r"^gh (pr (merge|comment|review)|issue create|release create)\b",
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

/// Wrappers that run the command after them, with their options that take a value.
const WRAPPERS: [(&str, &[&str]); 5] = [
    ("sudo", &["-u", "-g"]),
    ("env", &[]),
    ("nohup", &[]),
    ("time", &[]),
    ("xargs", &["-n", "-I", "-L", "-P"]),
];

/// Programs whose global options can come before the subcommand, with the ones that take a value.
const GLOBALS: [(&str, &[&str]); 4] = [
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
];

#[derive(Debug, Clone)]
pub struct HostRules {
    rules: Vec<(Class, Regex)>,
    foreign: Vec<PathBuf>,
    control: Option<Regex>,
    backup: Regex,
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
        simple_commands(command).find_map(|w| {
            let line = w.join(" ");
            self.rules
                .iter()
                .find(|(_, r)| r.is_match(&line))
                .map(|(c, _)| *c)
                .or_else(|| by_code(&w))
        })
    }

    pub fn classify_write(&self, path: &Path) -> Option<Class> {
        if path.to_string_lossy().starts_with("/etc/sudoers") {
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
        simple_commands(command).any(|w| {
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
}

/// Simple commands as words: quotes dropped (`'DROP TABLE t'` is three words), wrappers peeled,
/// the program by its bare name, and its global options dropped (`git -C d push` is `git push`).
fn simple_commands(command: &str) -> impl Iterator<Item = Vec<&str>> {
    command
        .split([';', '&', '|', '\n', '`', '(', ')'])
        .map(|part| {
            let words: Vec<&str> = part
                .split(|c: char| c.is_whitespace() || c == '\'' || c == '"')
                .filter(|w| !w.is_empty())
                .collect();
            let mut w = unwrap(&words).to_vec();
            if let Some(p) = w.first_mut()
                && !p.contains(['<', '>'])
            {
                *p = p.rsplit('/').next().unwrap_or(p);
            }
            if let Some((_, valued)) = GLOBALS.iter().find(|(n, _)| w.first() == Some(n)) {
                let mut i = 1;
                while let Some(o) = w.get(i).filter(|o| o.starts_with('-')) {
                    i += if valued.contains(o) { 2 } else { 1 };
                }
                w.drain(1..i.min(w.len()));
            }
            w
        })
        .filter(|w| !w.is_empty())
}

fn unwrap<'s, 'a>(mut w: &'s [&'a str]) -> &'s [&'a str] {
    loop {
        match w {
            [sh, c, rest @ ..]
                if matches!(*sh, "sh" | "bash") && c.starts_with('-') && c.ends_with('c') =>
            {
                w = rest
            }
            [first, rest @ ..] if first.contains('=') => w = rest,
            [first, rest @ ..] => {
                let Some((_, valued)) = WRAPPERS.iter().find(|(n, _)| n == first) else {
                    return w;
                };
                w = rest;
                while let [opt, more @ ..] = w {
                    if valued.contains(opt) {
                        w = more.get(1..).unwrap_or(&[]);
                    } else if opt.starts_with('-') || opt.contains('=') {
                        w = more;
                    } else {
                        break;
                    }
                }
            }
            [] => return w,
        }
    }
}

/// The rows a pattern says badly: they look at the arguments one by one.
fn by_code(w: &[&str]) -> Option<Class> {
    match w {
        ["rm", args @ ..] => rm_recursive(args).then_some(Borrar),
        ["git", "push", args @ ..] => push_shared(args).then_some(Enviar),
        ["curl" | "wget", args @ ..] => sends_out(args).then_some(Enviar),
        _ => None,
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
                    Some("node_modules" | "target" | "dist")
                ))
    };
    recursive && (targets.peek().is_none() || targets.any(|t| !disposable(t)))
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

/// curl or wget that sends data or a non-GET method to anywhere but this machine.
fn sends_out(args: &[&str]) -> bool {
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
        let method = ["-X", "--request", "--method"]
            .iter()
            .find_map(|f| a.strip_prefix(f));
        DATA.iter().any(|d| a.starts_with(d))
            || method.is_some_and(|m| {
                let m = m.trim_start_matches('=');
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
