//! `clio-vault`: the vault as its own process, and the person's command line for it.
//!
//! ```text
//! clio-vault init  --vault FILE                     (passphrase: one line on stdin)
//! clio-vault serve --vault FILE --gateway-sock P --person-sock P
//! clio-vault [--sock P] unlock [--eager NAME]...    (passphrase: one line on stdin)
//! clio-vault [--sock P] put NAME --dest TOOL:DOMAIN... [--kind api_key|oauth_token|password|ssh_key]
//!                                [--rotate-days N]  (value: one line on stdin)
//! clio-vault [--sock P] rotate NAME                 (new value: one line on stdin)
//! clio-vault [--sock P] scan DIR                    (vault values found in files; exit 1 if any)
//! clio-vault [--sock P] revoke NAME | list | audit | alerts | due | lock | status
//! ```
//!
//! `--sock` is the PERSON socket (default: `$CLIO_VAULT_PERSON_SOCK`). The server starts locked and
//! never reads the terminal; `unlock` sends the passphrase over the person socket.

#[cfg(unix)]
fn main() {
    if let Err(e) = unix::run(std::env::args().skip(1).collect()) {
        eprintln!("clio-vault: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("clio-vault needs Unix sockets");
    std::process::exit(2);
}

#[cfg(unix)]
mod unix {
    use clio_core::SystemClock;
    use clio_vault::proto::Request;
    use clio_vault::{Dest, Kind, Scope, Vault, VaultClient};
    use std::io::{IsTerminal, Write};
    use std::path::PathBuf;
    use std::sync::Arc;

    const DAY_MS: i64 = 86_400_000;

    /// One line from stdin. On a terminal this echoes: shortcut, no raw-mode dependency; pipe the
    /// value in (or use a terminal with echo off) until a prompt crate is worth it.
    fn read_line(prompt: &str) -> Result<String, String> {
        let stdin = std::io::stdin();
        if stdin.is_terminal() {
            eprint!("{prompt}: ");
            let _ = std::io::stderr().flush();
        }
        let mut s = String::new();
        stdin.read_line(&mut s).map_err(|e| e.to_string())?;
        Ok(s.trim_end_matches(['\n', '\r']).to_string())
    }

    /// Removes `--flag value` pairs from `args` and returns the values.
    fn take_all(args: &mut Vec<String>, flag: &str) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        while let Some(i) = args.iter().position(|a| a == flag) {
            if i + 1 >= args.len() {
                return Err(format!("{flag} needs a value"));
            }
            out.push(args.remove(i + 1));
            args.remove(i);
        }
        Ok(out)
    }

    fn take_one(args: &mut Vec<String>, flag: &str) -> Result<Option<String>, String> {
        Ok(take_all(args, flag)?.pop())
    }

    fn need(v: Option<String>, what: &str) -> Result<String, String> {
        v.ok_or_else(|| format!("missing {what}"))
    }

    pub fn run(mut args: Vec<String>) -> Result<(), String> {
        let vault_file = take_one(&mut args, "--vault")?;
        let gw_sock = take_one(&mut args, "--gateway-sock")?;
        let sock =
            take_one(&mut args, "--sock")?.or_else(|| std::env::var("CLIO_VAULT_PERSON_SOCK").ok());
        let person_sock = take_one(&mut args, "--person-sock")?.or_else(|| sock.clone());
        if args.is_empty() {
            return Err("usage: clio-vault init|serve|unlock|put|rotate|revoke|list|audit|alerts|due|lock|status".into());
        }
        let cmd = args.remove(0);
        match cmd.as_str() {
            "init" => {
                let path = PathBuf::from(need(vault_file, "--vault")?);
                let pass = read_line("new passphrase")?;
                Vault::init(Some(&path), &pass, Arc::new(SystemClock))
                    .map_err(|e| e.to_string())?;
                println!("vault created at {}", path.display());
                Ok(())
            }
            "serve" => {
                let path = PathBuf::from(need(vault_file, "--vault")?);
                let vault = Vault::open(&path, Arc::new(SystemClock)).map_err(|e| e.to_string())?;
                let gw = PathBuf::from(need(gw_sock, "--gateway-sock")?);
                let person = PathBuf::from(need(person_sock, "--person-sock")?);
                clio_vault::server::serve(Arc::new(vault), &gw, &person).map_err(|e| e.to_string())
            }
            other => {
                let client = VaultClient::new(&PathBuf::from(need(person_sock, "--sock")?));
                let req = request(other, args)?;
                let resp = client.call(&req).map_err(|e| e.to_string())?;
                if let Some(d) = &resp.data {
                    println!("{d}");
                }
                let found = cmd_found_hits(other, &resp);
                if resp.ok && !found {
                    Ok(())
                } else if found {
                    Err("vault values found (file, secret, form above)".into())
                } else {
                    Err(resp.error.unwrap_or_else(|| "failed".into()))
                }
            }
        }
    }

    /// `scan` reports success only when it found nothing.
    fn cmd_found_hits(cmd: &str, resp: &clio_vault::proto::Response) -> bool {
        cmd == "scan"
            && resp
                .data
                .as_ref()
                .and_then(|d| d.as_array())
                .is_some_and(|a| !a.is_empty())
    }

    fn request(cmd: &str, mut args: Vec<String>) -> Result<Request, String> {
        let name =
            |args: &mut Vec<String>| need((!args.is_empty()).then(|| args.remove(0)), "NAME");
        Ok(match cmd {
            "status" => Request::Status,
            "lock" => Request::Lock,
            "list" => Request::List,
            "audit" => Request::Audit,
            "alerts" => Request::Alerts,
            "due" => Request::Due,
            "scan" => Request::ScanDir {
                path: name(&mut args)?,
            },
            "revoke" => Request::Revoke {
                name: name(&mut args)?,
            },
            "unlock" => Request::Unlock {
                eager: take_all(&mut args, "--eager")?,
                passphrase: read_line("passphrase")?,
            },
            "rotate" => Request::Rotate {
                name: name(&mut args)?,
                value: read_line("new value")?,
            },
            "put" => {
                let destinations = take_all(&mut args, "--dest")?
                    .iter()
                    .map(|d| {
                        d.split_once(':')
                            .map(|(t, dom)| Dest::new(t, dom))
                            .ok_or_else(|| format!("--dest wants TOOL:DOMAIN, got {d}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let kind = match take_one(&mut args, "--kind")?.as_deref() {
                    None | Some("api_key") => Kind::ApiKey,
                    Some("oauth_token") => Kind::OAuthToken,
                    Some("password") => Kind::Password,
                    Some("ssh_key") => Kind::SshKey,
                    Some(k) => return Err(format!("unknown kind {k}")),
                };
                let rotate_every_ms = take_one(&mut args, "--rotate-days")?
                    .map(|d| {
                        d.parse::<i64>()
                            .map(|d| d * DAY_MS)
                            .map_err(|e| e.to_string())
                    })
                    .transpose()?;
                Request::Put {
                    name: name(&mut args)?,
                    value: read_line("value")?,
                    kind,
                    destinations,
                    scope: Scope::Personal,
                    rotate_every_ms,
                    expires_at_ms: None,
                }
            }
            other => return Err(format!("unknown command {other}")),
        })
    }
}
