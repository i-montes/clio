//! The questions Jev is asked and the answers it gives. Jev does not write: each question is a
//! yes/no (`noul`) or a pick among named options (`choice`), and the answer comes with
//! probabilities. Whatever comes back is checked against what was asked.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

const MAX_OPTIONS: usize = 255;
const MAX_NAME: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub enum Question {
    Noul {
        instructions: String,
    },
    Choice {
        instructions: String,
        /// Option name and what it means, in order.
        criteria: Vec<(String, String)>,
    },
}

/// The questions of one call, by name. A name is what the answer is read by.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Questions(BTreeMap<String, Question>);

fn name_ok(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= MAX_NAME
        && n.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

impl Questions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a yes/no question. A name that is not `[a-z0-9_]{1,64}` or is repeated is refused.
    pub fn noul(mut self, name: &str, instructions: &str) -> Result<Self, String> {
        self.check(name)?;
        self.0.insert(
            name.to_owned(),
            Question::Noul {
                instructions: instructions.to_owned(),
            },
        );
        Ok(self)
    }

    /// Adds a pick among `1..=255` options (names obey the rule of question names).
    pub fn choice(
        mut self,
        name: &str,
        instructions: &str,
        criteria: &[(&str, &str)],
    ) -> Result<Self, String> {
        self.check(name)?;
        if criteria.is_empty() || criteria.len() > MAX_OPTIONS {
            return Err(format!("{name}: between 1 and {MAX_OPTIONS} options"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for (k, _) in criteria {
            if !name_ok(k) || !seen.insert(*k) {
                return Err(format!("{name}: bad or repeated option {k:?}"));
            }
        }
        self.0.insert(
            name.to_owned(),
            Question::Choice {
                instructions: instructions.to_owned(),
                criteria: criteria
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                    .collect(),
            },
        );
        Ok(self)
    }

    fn check(&self, name: &str) -> Result<(), String> {
        if !name_ok(name) {
            return Err(format!("bad question name {name:?}"));
        }
        if self.0.contains_key(name) {
            return Err(format!("repeated question {name:?}"));
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn to_json(&self) -> Value {
        let mut m = Map::new();
        for (name, q) in &self.0 {
            m.insert(
                name.clone(),
                match q {
                    Question::Noul { instructions } => {
                        json!({"type": "noul", "instructions": instructions})
                    }
                    Question::Choice {
                        instructions,
                        criteria,
                    } => {
                        let c: Map<String, Value> = criteria
                            .iter()
                            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                            .collect();
                        json!({"type": "choice", "instructions": instructions, "criteria": c})
                    }
                },
            );
        }
        Value::Object(m)
    }

    fn get(&self, name: &str) -> Option<&Question> {
        self.0.get(name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// Probability that the answer is yes, `0.0..=1.0`.
    Noul(f64),
    Choice {
        choice: String,
        confidence: f64,
    },
}

/// What came back for every question asked, checked: nothing missing, nothing of another type,
/// every number in range and every choice one of the options.
#[derive(Debug, Clone, PartialEq)]
pub struct Answers {
    pub model: String,
    pub input_tokens: u64,
    by_name: BTreeMap<String, Answer>,
}

impl Answers {
    pub fn get(&self, name: &str) -> Option<&Answer> {
        self.by_name.get(name)
    }

    pub fn noul(&self, name: &str) -> Option<f64> {
        match self.by_name.get(name) {
            Some(Answer::Noul(p)) => Some(*p),
            _ => None,
        }
    }

    pub(crate) fn parse(asked: &Questions, v: &Value) -> Result<Self, String> {
        let model = v["model"]
            .as_str()
            .filter(|m| !m.is_empty() && m.len() <= 80)
            .ok_or("no model in the answer")?
            .to_owned();
        let answers = v["answers"].as_object().ok_or("no answers in the answer")?;
        let unit = |x: &Value| x.as_f64().filter(|p| (0.0..=1.0).contains(p));
        let mut by_name = BTreeMap::new();
        for (name, q) in &asked.0 {
            let a = answers
                .get(name)
                .ok_or_else(|| format!("{name}: missing"))?;
            let got = match q {
                Question::Noul { .. } => {
                    if a["type"] != "noul" {
                        return Err(format!("{name}: not a noul"));
                    }
                    Answer::Noul(unit(&a["noul"]).ok_or_else(|| format!("{name}: out of range"))?)
                }
                Question::Choice { criteria, .. } => {
                    let choice = a["choice"].as_str().unwrap_or("");
                    if a["type"] != "choice" || !criteria.iter().any(|(k, _)| k == choice) {
                        return Err(format!("{name}: not one of the options"));
                    }
                    Answer::Choice {
                        choice: choice.to_owned(),
                        confidence: unit(&a["confidence"])
                            .ok_or_else(|| format!("{name}: confidence out of range"))?,
                    }
                }
            };
            by_name.insert(name.clone(), got);
        }
        if let Some(extra) = answers.keys().find(|k| asked.get(k).is_none()) {
            return Err(format!("{extra}: answered but never asked"));
        }
        Ok(Self {
            model,
            input_tokens: v["usage"]["input_tokens"].as_u64().unwrap_or(0),
            by_name,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn qs() -> Questions {
        Questions::new()
            .noul("necesita_memoria", "¿Hace falta memoria?")
            .unwrap()
            .choice(
                "operacion",
                "¿Qué hacer?",
                &[("add", "nueva"), ("noop", "repetida")],
            )
            .unwrap()
    }

    fn reply() -> Value {
        json!({"model":"jev-1.14.0","answers":{
            "necesita_memoria":{"type":"noul","noul":0.9},
            "operacion":{"type":"choice","choice":"add","confidence":0.8,"probabilities":{"add":0.8,"noop":0.2}}},
            "usage":{"input_tokens":283,"output_tokens":20}})
    }

    #[test]
    fn questions_serialize_in_the_shape_of_the_api() {
        let j = qs().to_json();
        assert_eq!(
            j["necesita_memoria"],
            json!({"type":"noul","instructions":"¿Hace falta memoria?"})
        );
        assert_eq!(
            j["operacion"]["criteria"],
            json!({"add":"nueva","noop":"repetida"})
        );
        assert_eq!(j["operacion"]["type"], "choice");
    }

    #[test]
    fn names_and_options_are_checked() {
        for bad in ["", "Mayuscula", "con espacio", "ñ", &"a".repeat(65)] {
            assert!(Questions::new().noul(bad, "x").is_err(), "{bad:?}");
        }
        assert!(qs().noul("operacion", "x").is_err(), "repeated");
        assert!(Questions::new().choice("c", "x", &[]).is_err());
        assert!(
            Questions::new()
                .choice("c", "x", &[("a", ""), ("a", "")])
                .is_err()
        );
        assert!(Questions::new().choice("c", "x", &[("A b", "")]).is_err());
    }

    #[test]
    fn a_good_answer_is_read_by_name() {
        let a = Answers::parse(&qs(), &reply()).unwrap();
        assert_eq!(a.model, "jev-1.14.0");
        assert_eq!(a.input_tokens, 283);
        assert_eq!(a.noul("necesita_memoria"), Some(0.9));
        assert_eq!(
            a.get("operacion"),
            Some(&Answer::Choice {
                choice: "add".into(),
                confidence: 0.8
            })
        );
        assert_eq!(a.noul("operacion"), None);
    }

    #[test]
    fn an_answer_that_does_not_match_the_questions_is_refused() {
        type Mutate = Box<dyn Fn(&mut Value)>;
        let cases: Vec<(&str, Mutate)> = vec![
            (
                "missing",
                Box::new(|v| {
                    v["answers"].as_object_mut().unwrap().remove("operacion");
                }),
            ),
            (
                "unasked",
                Box::new(|v| {
                    v["answers"]["otra"] = json!({"type":"noul","noul":0.5});
                }),
            ),
            (
                "wrong type",
                Box::new(|v| {
                    v["answers"]["necesita_memoria"]["type"] = json!("choice");
                }),
            ),
            (
                "noul above 1",
                Box::new(|v| {
                    v["answers"]["necesita_memoria"]["noul"] = json!(1.5);
                }),
            ),
            (
                "noul below 0",
                Box::new(|v| {
                    v["answers"]["necesita_memoria"]["noul"] = json!(-0.1);
                }),
            ),
            (
                "noul a string",
                Box::new(|v| {
                    v["answers"]["necesita_memoria"]["noul"] = json!("0.9");
                }),
            ),
            (
                "choice not offered",
                Box::new(|v| {
                    v["answers"]["operacion"]["choice"] = json!("purge");
                }),
            ),
            (
                "confidence above 1",
                Box::new(|v| {
                    v["answers"]["operacion"]["confidence"] = json!(2);
                }),
            ),
            (
                "no model",
                Box::new(|v| {
                    v.as_object_mut().unwrap().remove("model");
                }),
            ),
            (
                "no answers",
                Box::new(|v| {
                    v.as_object_mut().unwrap().remove("answers");
                }),
            ),
        ];
        for (what, mutate) in cases {
            let mut v = reply();
            mutate(&mut v);
            assert!(Answers::parse(&qs(), &v).is_err(), "{what}");
        }
    }
}
