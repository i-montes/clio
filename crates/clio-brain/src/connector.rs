//! The model connectors: the only places Clio sends a prompt, each with everything that
//! differs between them in one row. Which one is active is `model.provider` in `config.json`.

use std::collections::BTreeMap;

use crate::pi::launch::Provider;

pub struct Connector {
    /// What is written in `clio connect`.
    pub name: &'static str,
    /// The provider id for `config.json`, the catalog and the traces.
    pub provider: &'static str,
    /// The provider id inside Pi (`--provider`).
    pub pi_provider: &'static str,
    pub base_url: &'static str,
    /// Pi's API kind.
    pub pi_api: &'static str,
    pub pi_headers: &'static [(&'static str, &'static str)],
    /// The variable that overrides the vault's key (`clio`'s own environment).
    pub key_env: &'static str,
    /// The variable that carries the key to Pi inside the jail.
    pub pi_key_env: &'static str,
    /// What a key must start with to be stored.
    pub key_prefix: &'static str,
    /// A Pi extension that registers `pi_provider`, relative to the Pi install folder.
    pub pi_extension: Option<&'static str>,
    /// The adapter for single calls (`clio run`, the sleep).
    pub adapter: &'static str,
    /// The vault secret that holds the key, and the domain it may be sent to (`model:<domain>`).
    pub secret: &'static str,
    pub domain: &'static str,
    pub default_model: &'static str,
}

/// The name the model's key is struck out under, everywhere (`[secret:model-api-key redacted]`):
/// the session's tool host and an approval from the terminal must clean a command alike, or the
/// approval sees a changed command and refuses it.
pub const MODEL_KEY_NAME: &str = "model-api-key";

pub const CONNECTORS: [Connector; 3] = [
    Connector {
        name: "opencode",
        provider: "opencode-go",
        pi_provider: "opencode-go",
        base_url: "https://opencode.ai/inference/go/openai/v1",
        pi_api: "openai-completions",
        pi_headers: &[
            ("User-Agent", "clio/0.1"),
            ("x-opencode-session", "${CLIO_SESSION_ID}"),
        ],
        key_env: "OPENCODE_API_KEY",
        pi_key_env: "OPENCODE_API_KEY",
        key_prefix: "",
        pi_extension: None,
        adapter: "openai_compat.py",
        secret: "opencode",
        domain: "opencode.ai",
        default_model: "beam-free",
    },
    Connector {
        name: "claude",
        provider: "claude",
        // The subscription goes through the official Claude Code (Agent SDK), never straight
        // to the API.
        pi_provider: "claude-bridge",
        base_url: "https://api.anthropic.com",
        pi_api: "anthropic-messages",
        pi_headers: &[],
        key_env: "CLAUDE_CODE_OAUTH_TOKEN",
        // Pi resolves this one as an OAuth credential; the bridge hands it to Claude Code.
        pi_key_env: "ANTHROPIC_OAUTH_TOKEN",
        key_prefix: "sk-ant-oat",
        pi_extension: Some("node_modules/pi-claude-agent-sdk/src/index.ts"),
        // Single calls run the official Claude Code too (`claude -p`), never the API directly.
        adapter: "claude_code.py",
        secret: "claude",
        domain: "api.anthropic.com",
        default_model: "claude-sonnet-5-5",
    },
    Connector {
        name: "anthropic-api",
        provider: "anthropic",
        pi_provider: "anthropic",
        base_url: "https://api.anthropic.com",
        pi_api: "anthropic-messages",
        pi_headers: &[],
        key_env: "ANTHROPIC_API_KEY",
        pi_key_env: "ANTHROPIC_API_KEY",
        key_prefix: "sk-ant-api",
        pi_extension: None,
        adapter: "anthropic_api.py",
        secret: "anthropic-api",
        domain: "api.anthropic.com",
        default_model: "claude-sonnet-5-5",
    },
];

/// What Claude Code always gets, inside the jail and out: no traffic beyond the model's.
pub const CLAUDE_CODE_ENV: [(&str, &str); 3] = [
    ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
    ("DISABLE_AUTOUPDATER", "1"),
    ("DISABLE_TELEMETRY", "1"),
];

pub fn by_name(name: &str) -> Option<&'static Connector> {
    CONNECTORS.iter().find(|c| c.name == name)
}

pub fn by_provider(provider: &str) -> Option<&'static Connector> {
    CONNECTORS.iter().find(|c| c.provider == provider)
}

/// Whether `url` points at this machine: the only address `model.base_url` may name, so a key
/// never goes anywhere else.
pub fn is_loopback(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // Split the host from an optional `:port`; `user@host` and `host.evil.test` fall out as
    // hosts that are not loopback.
    let (host, port) = match authority.strip_prefix('[').and_then(|v| v.split_once(']')) {
        Some((h, tail)) => (format!("[{h}]"), tail.strip_prefix(':').unwrap_or(tail)),
        None => {
            let (h, p) = authority.split_once(':').unwrap_or((authority, ""));
            (h.to_owned(), p)
        }
    };
    matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]")
        && port.bytes().all(|b| b.is_ascii_digit())
}

impl Connector {
    /// The Pi provider for `model`: a 128 000-token window and a price of 0 until the caller
    /// sets the real ones.
    pub fn pi_launch_provider(&self, model: &str) -> Provider {
        Provider {
            id: self.pi_provider.into(),
            base_url: self.base_url.into(),
            api: self.pi_api.into(),
            model: model.into(),
            context_window: 128_000,
            reasoning: true,
            headers: self
                .pi_headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<BTreeMap<_, _>>(),
            api_key_env: self.pi_key_env.into(),
            dollars_per_mtok: (0.0, 0.0),
            extension: self.pi_extension.map(Into::into),
            // Claude Code, run by the bridge, inherits Pi's environment.
            child_env: if self.pi_extension.is_some() {
                CLAUDE_CODE_ENV
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                    .collect()
            } else {
                vec![]
            },
            // A reasoning model thinks for thousands of tokens on a simple step at Pi's default
            // ("medium"): measured 46 s against 7 s at "low" for the same answer.
            thinking: "low".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn three_connectors() {
        let names: Vec<_> = CONNECTORS.iter().map(|c| c.name).collect();
        assert_eq!(names, ["opencode", "claude", "anthropic-api"]);
        assert_eq!(by_name("claude").unwrap().provider, "claude");
        assert_eq!(by_provider("opencode-go").unwrap().name, "opencode");
        assert!(by_name("gpt").is_none());
        assert!(by_provider("opencode").is_none());

        let c = by_name("claude").unwrap();
        assert_eq!(c.pi_provider, "claude-bridge");
        assert_eq!(c.pi_key_env, "ANTHROPIC_OAUTH_TOKEN");
        assert_eq!(c.key_env, "CLAUDE_CODE_OAUTH_TOKEN");
        assert_eq!(c.key_prefix, "sk-ant-oat");
        assert_eq!(
            c.pi_extension,
            Some("node_modules/pi-claude-agent-sdk/src/index.ts")
        );

        let a = by_name("anthropic-api").unwrap();
        assert_eq!(a.provider, "anthropic");
        assert_eq!(by_provider("anthropic").unwrap().name, "anthropic-api");
        assert_eq!(a.pi_provider, "anthropic");
        assert_eq!(
            (a.pi_key_env, a.key_env),
            ("ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY")
        );
        assert_eq!(a.key_prefix, "sk-ant-api");
        assert!(a.pi_extension.is_none());
        assert_eq!((a.secret, a.domain), ("anthropic-api", "api.anthropic.com"));
        assert_eq!(a.default_model, "claude-sonnet-5-5");
    }

    #[test]
    fn anthropic_api_goes_to_anthropic_with_the_api_key_variable() {
        let p = by_name("anthropic-api")
            .unwrap()
            .pi_launch_provider("claude-sonnet-5-5");
        assert_eq!(p.id, "anthropic");
        assert_eq!(p.base_url, "https://api.anthropic.com");
        assert_eq!(p.api, "anthropic-messages");
        assert_eq!(p.api_key_env, "ANTHROPIC_API_KEY");
        assert!(p.headers.is_empty() && p.extension.is_none() && p.child_env.is_empty());
        let t = p.models_json().to_string();
        assert!(t.contains("${ANTHROPIC_API_KEY}") && !t.contains("sk-ant"));
    }

    #[test]
    fn opencode_is_what_it_was() {
        let got = by_name("opencode").unwrap().pi_launch_provider("beam-free");
        let want = json!({"providers": {"opencode-go": {
            "baseUrl": "https://opencode.ai/inference/go/openai/v1",
            "api": "openai-completions",
            "apiKey": "${OPENCODE_API_KEY}",
            "headers": {"User-Agent": "clio/0.1", "x-opencode-session": "${CLIO_SESSION_ID}"},
            "models": [{"id": "beam-free", "name": "beam-free", "reasoning": true,
                "contextWindow": 128_000,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0, "cacheWrite": 0}}],
        }}});
        assert_eq!(got.models_json(), want);
    }

    #[test]
    fn only_loopback_is_loopback() {
        for ok in [
            "http://127.0.0.1:9/v1",
            "http://localhost:1",
            "http://[::1]:2",
            "http://localhost",
        ] {
            assert!(is_loopback(ok), "{ok}");
        }
        for bad in [
            "https://opencode.ai/x",
            "https://127.0.0.1:9",
            "http://127.0.0.1.evil.test",
            "http://user@127.0.0.1",
            "http://127.0.0.1:9@evil.test",
            "http://localhost.evil.test:1",
            "",
        ] {
            assert!(!is_loopback(bad), "{bad}");
        }
    }
}
