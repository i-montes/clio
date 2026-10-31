use crate::{Error, Result, Sandbox};
use clio_core::Effect;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub type Run = fn(&Sandbox, &Value) -> Result<Value>;

/// A tool and its declared effect. `effect: None` exists so a definition coming from outside can
/// be refused (S-02); the registry never accepts it.
pub struct Tool {
    pub name: String,
    pub description: String,
    pub keywords: Vec<String>,
    pub effect: Option<Effect>,
    /// Recommended: shown to the model by default. The rest is found with `Registry::search`.
    pub visible: bool,
    pub run: Run,
}

pub struct Found<'a> {
    pub tool: &'a Tool,
    /// False when the model asked for a tool that was not on its list (D8). The gateway still
    /// reviews the action: this flag only tells it.
    pub recommended: bool,
}

#[derive(Default)]
pub struct Registry {
    tools: BTreeMap<String, Tool>,
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The basic tools, registered.
    pub fn basic() -> Result<Self> {
        let mut r = Self::new();
        for t in basic_tools() {
            r.register(t)?;
        }
        Ok(r)
    }

    pub fn register(&mut self, tool: Tool) -> Result<()> {
        // S-02: no declared effect, no tool. S-56: only read and sandbox-reversible in this phase.
        let effect = tool
            .effect
            .ok_or_else(|| Error::NoEffect(tool.name.clone()))?;
        if effect > Effect::LocalReversible {
            return Err(Error::PhaseLimit(tool.name, effect));
        }
        if self.tools.contains_key(&tool.name) {
            return Err(Error::Duplicate(tool.name));
        }
        self.tools.insert(tool.name.clone(), tool);
        Ok(())
    }

    pub fn visible(&self) -> Vec<&Tool> {
        self.tools.values().filter(|t| t.visible).collect()
    }

    /// Keyword retrieval over name, keywords and description, best first.
    pub fn search(&self, query: &str) -> Vec<&Tool> {
        let q = words(query);
        let mut hits: Vec<(usize, &Tool)> = self
            .tools
            .values()
            .filter_map(|t| {
                let n = words(&t.name);
                let k = words(&t.keywords.join(" "));
                let d = words(&t.description);
                let s: usize = q
                    .iter()
                    .map(|w| {
                        3 * usize::from(n.contains(w))
                            + 2 * usize::from(k.contains(w))
                            + usize::from(d.contains(w))
                    })
                    .sum();
                (s > 0).then_some((s, t))
            })
            .collect();
        hits.sort_by_key(|(s, t)| (std::cmp::Reverse(*s), t.name.clone()));
        hits.into_iter().map(|(_, t)| t).collect()
    }

    /// Unknown names are denied (None); a known but not recommended tool comes back flagged.
    pub fn get(&self, name: &str) -> Option<Found<'_>> {
        self.tools.get(name).map(|tool| Found {
            recommended: tool.visible,
            tool,
        })
    }
}

fn arg<'a>(a: &'a Value, k: &str) -> Result<&'a str> {
    a.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::BadArgs(format!("missing string `{k}`")))
}

pub fn basic_tools() -> Vec<Tool> {
    let t = |name: &str, d: &str, kw: &[&str], effect, visible, run: Run| Tool {
        name: name.into(),
        description: d.into(),
        keywords: kw.iter().map(|s| (*s).to_string()).collect(),
        effect: Some(effect),
        visible,
        run,
    };
    vec![
        t(
            "read_file",
            "Read a text file inside the sandbox",
            &["open", "cat"],
            Effect::Read,
            true,
            |s, a| {
                let b = s.read(arg(a, "path")?)?;
                Ok(json!({ "content": String::from_utf8_lossy(&b) }))
            },
        ),
        t(
            "list_dir",
            "List the entries of a folder inside the sandbox",
            &["ls", "folder", "directory"],
            Effect::Read,
            true,
            |s, a| Ok(json!({ "entries": s.list(arg(a, "path")?)? })),
        ),
        t(
            "write_file",
            "Write a file inside the sandbox",
            &["save", "create", "edit"],
            Effect::LocalReversible,
            true,
            |s, a| {
                s.write(arg(a, "path")?, arg(a, "content")?.as_bytes())?;
                Ok(json!({ "written": true }))
            },
        ),
        t(
            "make_dir",
            "Create a folder inside the sandbox",
            &["mkdir", "directory", "create"],
            Effect::LocalReversible,
            false,
            |s, a| {
                s.mkdir(arg(a, "path")?)?;
                Ok(json!({ "created": true }))
            },
        ),
    ]
}
