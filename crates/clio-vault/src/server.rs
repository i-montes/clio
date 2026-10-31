//! The vault server (tasks 233, 234, 266): two Unix sockets with mode 0600, one for the gateway
//! and one for the person, JSON lines on each. Never the network. The capability a request runs
//! with comes from the socket it arrived on, so a process that can only reach the person's socket
//! cannot call `resolve`.
//!
//! The log (stderr) names operations and secrets, never values or passphrases.

use crate::proto::{Request, Response, Side};
use crate::{GatewayCap, PersonCap, SecretSpec, Vault};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// A `scrub` request can carry a long tool output; anything bigger is refused.
const MAX_LINE: u64 = 16 * 1024 * 1024;

/// Runs one request with the capability of `side`. No I/O, so it can be tested directly.
pub fn handle(vault: &Vault, side: Side, req: Request) -> Response {
    if req.side().is_some_and(|s| s != side) {
        return Response::err(format!("not allowed on the {side:?} socket"));
    }
    let gw = GatewayCap::for_gateway();
    let person = PersonCap::for_person_socket();
    let done = |r: Result<(), crate::VaultError>| match r {
        Ok(()) => Response::ok(Value::Null),
        Err(e) => Response::err(e),
    };
    let as_json = |v: Result<Value, serde_json::Error>| match v {
        Ok(v) => Response::ok(v),
        Err(e) => Response::err(e),
    };
    match req {
        Request::Status => Response::ok(json!({
            "locked": vault.is_locked(),
            "secrets": vault.list(&person).len(),
        })),
        Request::Resolve {
            name,
            tool,
            domain,
            trace_id,
        } => match vault.resolve(&gw, &name, &tool, &domain, &trace_id) {
            Ok(v) => Response::ok(json!({ "value": v.expose() })),
            Err(e) => Response::err(e),
        },
        Request::Check {
            name,
            tool,
            domain,
            trace_id,
        } => done(vault.check(&gw, &name, &tool, domain.as_deref(), &trace_id)),
        Request::IssueToken {
            name,
            tool,
            node,
            ttl_ms,
        } => match vault.issue_token(&gw, &name, &tool, &node, ttl_ms) {
            Ok(t) => Response::ok(json!({
                "token": t.as_str(), "node": t.node, "expires_at_ms": t.expires_at_ms
            })),
            Err(e) => Response::err(e),
        },
        Request::Scrub { text } => Response::ok(json!({ "text": vault.scrubber().scrub(&text) })),
        Request::Unlock { passphrase, eager } => {
            let eager: Vec<&str> = eager.iter().map(String::as_str).collect();
            let r = if eager.is_empty() {
                vault.unlock(&passphrase)
            } else {
                vault.unlock_lazy(&passphrase, &eager)
            };
            done(r.map(|_| ()))
        }
        Request::Lock => {
            vault.lock();
            Response::ok(Value::Null)
        }
        Request::Put {
            name,
            value,
            kind,
            destinations,
            scope,
            rotate_every_ms,
            expires_at_ms,
        } => done(vault.put(
            &person,
            SecretSpec {
                name,
                value,
                kind,
                destinations,
                scope,
                rotate_every_ms,
                expires_at_ms,
            },
        )),
        Request::Rotate { name, value } => done(vault.rotate(&person, &name, &value)),
        Request::Revoke { name } => done(vault.revoke(&person, &name)),
        Request::List => as_json(serde_json::to_value(vault.list(&person))),
        Request::Audit => as_json(serde_json::to_value(vault.audit(&person))),
        Request::Alerts => as_json(serde_json::to_value(vault.alerts())),
        Request::ScanDir { path } => match scan_dir(vault, Path::new(&path)) {
            Ok(hits) => Response::ok(Value::Array(hits)),
            Err(e) => Response::err(format!("io: {e}")),
        },
        Request::Due => as_json(serde_json::to_value(vault.due_for_rotation(vault.now_ms()))),
    }
}

/// Every file under `dir` read as lossy text, with the vault values found in it: file, secret
/// name and form, never the value. Unreadable files are an error, not a pass.
fn scan_dir(vault: &Vault, dir: &Path) -> std::io::Result<Vec<Value>> {
    let scrubber = vault.scrubber();
    let mut hits = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(p) = todo.pop() {
        // Symlinks, sockets and the like are skipped: no loops, no blocking reads.
        let kind = std::fs::symlink_metadata(&p)?.file_type();
        if kind.is_dir() {
            for e in std::fs::read_dir(&p)? {
                todo.push(e?.path());
            }
        } else if kind.is_file() {
            let text = String::from_utf8_lossy(&std::fs::read(&p)?).into_owned();
            for (name, form) in scrubber.find(&text) {
                hits.push(json!({ "file": p.display().to_string(), "name": name, "form": form }));
            }
        }
    }
    Ok(hits)
}

fn bind(path: &Path) -> std::io::Result<UnixListener> {
    // Only ever replace a stale socket, never some other file.
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_socket()) {
        std::fs::remove_file(path)?;
    }
    let l = UnixListener::bind(path)?;
    // shortcut: between bind and chmod the mode follows the umask; keep the socket's parent
    // directory private (0700) to close that moment.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(l)
}

fn serve_conn(vault: &Vault, side: Side, stream: UnixStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        match reader.by_ref().take(MAX_LINE + 1).read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let too_big = line.len() as u64 > MAX_LINE;
        let resp = if too_big {
            Response::err("request too large")
        } else {
            match serde_json::from_str::<Request>(&line) {
                Ok(req) => {
                    let label = req.label();
                    let r = handle(vault, side, req);
                    eprintln!(
                        "clio-vault: {side:?} {label} -> {}",
                        r.error.as_deref().unwrap_or("ok")
                    );
                    r
                }
                Err(_) => Response::err("unreadable request"),
            }
        };
        let text = serde_json::to_string(&resp)
            .unwrap_or_else(|_| r#"{"ok":false,"error":"internal"}"#.into());
        if writeln!(writer, "{text}")
            .and_then(|()| writer.flush())
            .is_err()
            || too_big
        {
            return;
        }
    }
}

/// Binds both sockets (mode 0600) and serves them until the process dies.
pub fn serve(vault: Arc<Vault>, gateway_sock: &Path, person_sock: &Path) -> std::io::Result<()> {
    let listeners = [
        (Side::Gateway, bind(gateway_sock)?),
        (Side::Person, bind(person_sock)?),
    ];
    eprintln!("clio-vault: ready (locked: {})", vault.is_locked());
    let threads: Vec<_> = listeners
        .into_iter()
        .map(|(side, l)| {
            let vault = vault.clone();
            std::thread::spawn(move || {
                for stream in l.incoming().flatten() {
                    let vault = vault.clone();
                    std::thread::spawn(move || serve_conn(&vault, side, stream));
                }
            })
        })
        .collect();
    for t in threads {
        let _ = t.join();
    }
    Ok(())
}
