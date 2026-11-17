//! The model connectors: the only two places Clio sends a prompt, each with everything that
//! differs between them in one row. Which one is active is `model.provider` in `config.json`.

use std::collections::BTreeMap;

use crate::pi::launch::Provider;

pub struct Connector {
    /// What is written in `clio connect`.
    pub name: &'static str,
    /// The provider id for Pi, the catalog and the traces.
    pub provider: &'static str,
    pub base_url: &'static str,
    /// Pi's API kind.
    pub pi_api: &'static str,
    pub pi_headers: &'static [(&'static str, &'static str)],
    /// The variable that carries the key inside the jail.
    pub key_env: &'static str,
    /// The adapter for single calls (`clio run`, the sleep).
    pub adapter: &'static str,
    /// The vault secret that holds the key, and the domain it may be sent to (`model:<domain>`).
    pub secret: &'static str,
    pub domain: &'static str,
    pub default_model: &'static str,
}

pub const CONNECTORS: [Connector; 2] = [
    Connector {
        name: "opencode",
        provider: "opencode-go",
        base_url: "https://opencode.ai/inference/go/openai/v1",
        pi_api: "openai-completions",
        pi_headers: &[
            ("User-Agent", "clio/0.1"),
            ("x-opencode-session", "${CLIO_SESSION_ID}"),
        ],
        key_env: "OPENCODE_API_KEY",
        adapter: "openai_compat.py",
        secret: "opencode",
        domain: "opencode.ai",
        default_model: "beam-free",
    },
    Connector {
        name: "claude",
        provider: "claude",
        base_url: "https://api.anthropic.com",
        pi_api: "anthropic-messages",
        pi_headers: &[],
        key_env: "CLAUDE_CODE_OAUTH_TOKEN",
        adapter: "anthropic_oauth.py",
        secret: "claude",
        domain: "api.anthropic.com",
        default_model: "claude-sonnet-5-5",
    },
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
    pub fn pi_provider(&self, model: &str) -> Provider {
        Provider {
            id: self.provider.into(),
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
            api_key_env: self.key_env.into(),
            dollars_per_mtok: (0.0, 0.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_two_connectors_and_nothing_else() {
        assert_eq!(by_name("claude").unwrap().provider, "claude");
        assert_eq!(by_provider("opencode-go").unwrap().name, "opencode");
        assert!(by_name("gpt").is_none());
        assert!(by_provider("opencode").is_none());
    }

    #[test]
    fn claude_goes_to_anthropic_with_the_oauth_variable() {
        let p = by_name("claude").unwrap().pi_provider("claude-sonnet-5-5");
        assert_eq!(p.base_url, "https://api.anthropic.com");
        assert_eq!(p.api, "anthropic-messages");
        assert_eq!(p.api_key_env, "CLAUDE_CODE_OAUTH_TOKEN");
        assert!(p.headers.is_empty());
        let t = p.models_json().to_string();
        assert!(t.contains("${CLAUDE_CODE_OAUTH_TOKEN}") && !t.contains("sk-ant"));
    }

    #[test]
    fn opencode_is_what_it_was() {
        let got = by_name("opencode").unwrap().pi_provider("beam-free");
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
