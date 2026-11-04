//! The questions about tools, generated from the catalog the guard reports at the start of a
//! session: one yes/no per active tool, built from the tool's own `use_when`. Adding a tool to the
//! guard adds its question here; there is no second list to keep (spec: «Catálogo de herramientas»).
//! The answers are SUGGESTIONS for the model, never a restriction.

use serde_json::Value;

use crate::wire::{Answers, Questions};

/// Probability from which a tool is suggested.
pub const THRESHOLD: f64 = 0.5;
const MAX_USE_WHEN: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolHint {
    pub name: String,
    /// What the message needs for this tool to be the right one, as an infinitive phrase:
    /// «leer archivos del equipo».
    pub use_when: String,
}

/// The catalog as the guard writes it (`[{"name": "read", "use_when": "..."}, ...]`). An empty
/// list, a name or a phrase that is empty, a repeated name or an overlong phrase is refused: a
/// tool Jev cannot be asked about would silently never be suggested.
pub fn parse_catalog(v: &Value) -> Result<Vec<ToolHint>, String> {
    let list = v
        .as_array()
        .filter(|l| !l.is_empty())
        .ok_or("empty catalog")?;
    let mut out: Vec<ToolHint> = Vec::new();
    for t in list {
        let name = t["name"].as_str().unwrap_or("").to_owned();
        let use_when = t["use_when"].as_str().unwrap_or("").trim().to_owned();
        if name.is_empty() || use_when.is_empty() || use_when.chars().count() > MAX_USE_WHEN {
            return Err(format!("the tool {name:?} has no usable use_when"));
        }
        if out.iter().any(|h| h.name == name) {
            return Err(format!("the tool {name:?} is repeated"));
        }
        out.push(ToolHint { name, use_when });
    }
    Ok(out)
}

/// One `noul` per tool, named after the tool. The state of the call is
/// `{"mensaje_de_la_persona": ...}`.
pub fn tool_questions(catalog: &[ToolHint]) -> Result<Questions, String> {
    catalog.iter().try_fold(Questions::new(), |qs, t| {
        qs.noul(
            &t.name,
            &format!(
                "Para atender `mensaje_de_la_persona` hace falta {}.",
                t.use_when.trim_end_matches('.')
            ),
        )
    })
}

/// The tools whose probability reaches `threshold`, in catalog order.
pub fn suggested<'a>(answers: &Answers, catalog: &'a [ToolHint], threshold: f64) -> Vec<&'a str> {
    catalog
        .iter()
        .filter(|t| answers.noul(&t.name).is_some_and(|p| p >= threshold))
        .map(|t| t.name.as_str())
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cat() -> Vec<ToolHint> {
        parse_catalog(&json!([
            {"name": "read", "use_when": "leer archivos del equipo"},
            {"name": "bash", "use_when": "ejecutar comandos en una terminal."},
            {"name": "memory_search", "use_when": "consultar lo que se recuerda de la persona"},
        ]))
        .unwrap()
    }

    #[test]
    fn every_tool_in_the_catalog_gets_its_own_question() {
        let qs = tool_questions(&cat()).unwrap();
        assert_eq!(qs.len(), 3);
        let j = qs.to_json();
        assert_eq!(
            j["read"]["instructions"],
            "Para atender `mensaje_de_la_persona` hace falta leer archivos del equipo."
        );
        assert_eq!(
            j["bash"]["instructions"],
            "Para atender `mensaje_de_la_persona` hace falta ejecutar comandos en una terminal."
        );
        assert_eq!(j["memory_search"]["type"], "noul");
    }

    #[test]
    fn a_tool_added_to_the_catalog_is_asked_about_without_touching_anything_else() {
        let mut c = cat();
        c.push(ToolHint {
            name: "grep".into(),
            use_when: "buscar texto dentro de archivos".into(),
        });
        let j = tool_questions(&c).unwrap().to_json();
        assert!(j.get("grep").is_some());
    }

    #[test]
    fn a_catalog_with_a_tool_that_cannot_be_asked_about_is_refused() {
        for bad in [
            json!([]),
            json!({}),
            json!([{"name": "read"}]),
            json!([{"name": "read", "use_when": "  "}]),
            json!([{"name": "", "use_when": "x"}]),
            json!([{"name": "read", "use_when": "x"}, {"name": "read", "use_when": "y"}]),
            json!([{"name": "read", "use_when": "x".repeat(301)}]),
        ] {
            assert!(parse_catalog(&bad).is_err(), "{bad}");
        }
        // A name Jev cannot take as a question name is refused when the questions are built.
        let odd = parse_catalog(&json!([{"name": "mi-tool", "use_when": "x"}])).unwrap();
        assert!(tool_questions(&odd).is_err());
    }

    #[test]
    fn suggestions_are_the_tools_at_or_above_the_threshold_in_catalog_order() {
        let c = cat();
        let qs = tool_questions(&c).unwrap();
        let reply = json!({"model":"jev-1","answers":{
            "read":{"type":"noul","noul":0.9},
            "bash":{"type":"noul","noul":0.49},
            "memory_search":{"type":"noul","noul":0.5}}});
        let a = Answers::parse(&qs, &reply).unwrap();
        assert_eq!(suggested(&a, &c, THRESHOLD), ["read", "memory_search"]);
        assert!(suggested(&a, &c, 0.95).is_empty());
    }
}
