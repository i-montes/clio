//! The wire format between the vault server and its clients: one JSON object per line, request
//! out, response back. No type here derives `Debug`, because requests carry values and
//! passphrases (S-18).
//!
//! Who may send what is decided by the socket the request arrived on, never by the request:
//! see [`Side`].

use crate::{Dest, Kind, Scope};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The two sockets. The gateway's can resolve and scrub; the person's can put, rotate, revoke,
/// list, audit and unlock. A request on the wrong socket is refused (S-22, task 266).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Gateway,
    Person,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Either socket.
    Status,
    // Gateway socket.
    Resolve {
        name: String,
        tool: String,
        domain: String,
        #[serde(default)]
        trace_id: String,
    },
    Check {
        name: String,
        tool: String,
        domain: Option<String>,
        #[serde(default)]
        trace_id: String,
    },
    IssueToken {
        name: String,
        tool: String,
        node: String,
        ttl_ms: i64,
    },
    Scrub {
        text: String,
    },
    // Person socket.
    Unlock {
        passphrase: String,
        /// Secrets to decrypt now (task 272); empty means all of them.
        #[serde(default)]
        eager: Vec<String>,
    },
    Lock,
    Put {
        name: String,
        value: String,
        kind: Kind,
        destinations: Vec<Dest>,
        scope: Scope,
        rotate_every_ms: Option<i64>,
        expires_at_ms: Option<i64>,
    },
    Rotate {
        name: String,
        value: String,
    },
    Revoke {
        name: String,
    },
    List,
    Audit,
    Alerts,
    /// Names due for rotation now.
    Due,
    /// Task 257: files under `path` (on the server's disk) that hold a vault value.
    ScanDir {
        path: String,
    },
}

impl Request {
    /// The side that may send it; `None` for the ones both accept.
    pub fn side(&self) -> Option<Side> {
        match self {
            Self::Status => None,
            Self::Resolve { .. }
            | Self::Check { .. }
            | Self::IssueToken { .. }
            | Self::Scrub { .. } => Some(Side::Gateway),
            _ => Some(Side::Person),
        }
    }

    /// For the server log: the operation and the secret name, never a value.
    pub fn label(&self) -> String {
        match self {
            Self::Status => "status".into(),
            Self::Resolve {
                name, tool, domain, ..
            } => format!("resolve {name} {tool} {domain}"),
            Self::Check { name, tool, .. } => format!("check {name} {tool}"),
            Self::IssueToken { name, tool, .. } => format!("issue_token {name} {tool}"),
            Self::Scrub { .. } => "scrub".into(),
            Self::Unlock { .. } => "unlock".into(),
            Self::Lock => "lock".into(),
            Self::Put { name, .. } => format!("put {name}"),
            Self::Rotate { name, .. } => format!("rotate {name}"),
            Self::Revoke { name } => format!("revoke {name}"),
            Self::List => "list".into(),
            Self::Audit => "audit".into(),
            Self::Alerts => "alerts".into(),
            Self::Due => "due".into(),
            Self::ScanDir { path } => format!("scan_dir {path}"),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl Response {
    pub fn ok(data: Value) -> Self {
        Self {
            ok: true,
            error: None,
            data: Some(data),
        }
    }

    pub fn err(e: impl ToString) -> Self {
        Self {
            ok: false,
            error: Some(e.to_string()),
            data: None,
        }
    }
}
