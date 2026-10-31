//! `VaultClient`: talks to the vault server over one of its Unix sockets. It opens a connection
//! per call, so a restarted server needs no reconnect logic. Any failure to get an answer is an
//! error, never a value, so with the server down nothing resolves (fail closed).

use crate::proto::{Request, Response};
use crate::{SecretValue, Token, VaultError};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct VaultClient {
    path: PathBuf,
}

fn io(e: impl ToString) -> VaultError {
    VaultError::Io(e.to_string())
}

/// The server sends `VaultError`'s text; turn it back into the variant.
fn parse_error(s: &str) -> VaultError {
    match s {
        "Locked" => VaultError::Locked,
        "BadPassphrase" => VaultError::BadPassphrase,
        "NotFound" => VaultError::NotFound,
        "Revoked" => VaultError::Revoked,
        "Expired" => VaultError::Expired,
        "BadToken" => VaultError::BadToken,
        "InvalidName" => VaultError::InvalidName,
        "InvalidValue" => VaultError::InvalidValue,
        "Crypto" => VaultError::Crypto,
        "Format" => VaultError::Format,
        other => match other.strip_prefix("denied: ") {
            Some(why) => VaultError::Denied(why.into()),
            None => VaultError::Io(other.into()),
        },
    }
}

impl VaultClient {
    /// `socket` is the gateway's socket for the gateway's calls, the person's for the CLI.
    pub fn new(socket: &Path) -> Self {
        Self {
            path: socket.to_path_buf(),
        }
    }

    pub fn call(&self, req: &Request) -> Result<Response, VaultError> {
        let mut stream = UnixStream::connect(&self.path).map_err(io)?;
        let line = serde_json::to_string(req).map_err(io)?;
        writeln!(stream, "{line}")
            .and_then(|()| stream.flush())
            .map_err(io)?;
        let mut out = String::new();
        match BufReader::new(stream).read_line(&mut out) {
            Ok(0) => Err(io("the vault closed the connection")),
            Ok(_) => serde_json::from_str(&out).map_err(io),
            Err(e) => Err(io(e)),
        }
    }

    fn data(&self, req: &Request) -> Result<serde_json::Value, VaultError> {
        let r = self.call(req)?;
        if r.ok {
            Ok(r.data.unwrap_or_default())
        } else {
            Err(parse_error(&r.error.unwrap_or_default()))
        }
    }

    fn text(&self, req: &Request, field: &str) -> Result<String, VaultError> {
        self.data(req)?[field]
            .as_str()
            .map(String::from)
            .ok_or_else(|| io("answer without the expected field"))
    }

    /// Same contract as `Vault::resolve`.
    pub fn resolve(
        &self,
        name: &str,
        tool: &str,
        domain: &str,
        trace_id: &str,
    ) -> Result<SecretValue, VaultError> {
        let req = Request::Resolve {
            name: name.into(),
            tool: tool.into(),
            domain: domain.into(),
            trace_id: trace_id.into(),
        };
        self.text(&req, "value").map(SecretValue)
    }

    /// Same contract as `Vault::check`: may `tool` use `name` towards `domain`? No value moves.
    pub fn check(
        &self,
        name: &str,
        tool: &str,
        domain: Option<&str>,
        trace_id: &str,
    ) -> Result<(), VaultError> {
        self.data(&Request::Check {
            name: name.into(),
            tool: tool.into(),
            domain: domain.map(Into::into),
            trace_id: trace_id.into(),
        })
        .map(|_| ())
    }

    pub fn issue_token(
        &self,
        name: &str,
        tool: &str,
        node: &str,
        ttl_ms: i64,
    ) -> Result<Token, VaultError> {
        let d = self.data(&Request::IssueToken {
            name: name.into(),
            tool: tool.into(),
            node: node.into(),
            ttl_ms,
        })?;
        match (
            d["token"].as_str(),
            d["node"].as_str(),
            d["expires_at_ms"].as_i64(),
        ) {
            (Some(id), Some(node), Some(expires_at_ms)) => Ok(Token {
                id: id.into(),
                node: node.into(),
                expires_at_ms,
            }),
            _ => Err(io("answer without the expected fields")),
        }
    }

    /// The vault's output filter over `text`. If the vault cannot answer this is an error: the
    /// caller must drop the output, not pass it on unscrubbed.
    pub fn scrub(&self, text: &str) -> Result<String, VaultError> {
        self.text(&Request::Scrub { text: text.into() }, "text")
    }
}
