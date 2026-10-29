use rust_stemmers::{Algorithm, Stemmer};
use unicode_normalization::UnicodeNormalization;

fn algorithm(lang: &str) -> Option<Algorithm> {
    match lang {
        "es" => Some(Algorithm::Spanish),
        "en" => Some(Algorithm::English),
        "fr" => Some(Algorithm::French),
        "de" => Some(Algorithm::German),
        "pt" => Some(Algorithm::Portuguese),
        "it" => Some(Algorithm::Italian),
        _ => None,
    }
}

const STOP_ES: [&str; 62] = [
    "a", "al", "algo", "como", "cómo", "con", "cual", "cuál", "cuando", "cuándo", "de", "del",
    "donde", "dónde", "e", "el", "en", "es", "esa", "ese", "eso", "esta", "este", "esto", "la",
    "las", "le", "les", "lo", "los", "me", "mi", "mis", "muy", "no", "o", "para", "pero", "por",
    "que", "qué", "quien", "quién", "se", "si", "sí", "sin", "sobre", "son", "su", "sus", "te",
    "tu", "tus", "u", "un", "una", "uno", "unos", "unas", "y", "ya",
];
const STOP_EN: [&str; 28] = [
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "how", "in", "is", "it", "of",
    "on", "or", "that", "the", "this", "to", "was", "were", "what", "where", "which", "who",
    "with",
];

fn is_stop(lang: &str, word: &str) -> bool {
    match lang {
        "es" => STOP_ES.contains(&word),
        "en" => STOP_EN.contains(&word),
        _ => false,
    }
}

/// Like `tokens` but without the function words of languages we know ("de", "en la", "the"):
/// what is left is what a query is actually about.
pub(crate) fn content_tokens(lang: &str, text: &str) -> Vec<String> {
    tokens_where(lang, text, true)
}

/// Lowercased tokens of `text`, each reduced to its stem for `lang`.
pub(crate) fn tokens(lang: &str, text: &str) -> Vec<String> {
    tokens_where(lang, text, false)
}

fn tokens_where(lang: &str, text: &str, skip_stop: bool) -> Vec<String> {
    let stemmer = algorithm(lang).map(Stemmer::create);
    let text: String = text.nfc().collect();
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .filter(|lower| !(skip_stop && is_stop(lang, lower)))
        .map(|lower| {
            stemmer
                .as_ref()
                .map_or_else(|| lower.clone(), |s| s.stem(&lower).into_owned())
        })
        .collect()
}

/// Lowercased tokens reduced to their stem for `lang` (unknown languages are only lowercased).
/// Used both to fill the FTS index and to build queries, so both sides agree.
pub fn stem_text(lang: &str, text: &str) -> String {
    tokens(lang, text).join(" ")
}

/// A safe FTS5 query: every token is quoted (tokens are purely alphanumeric, so nothing in the
/// user's text can be read as FTS syntax) and joined with OR; `None` when nothing is left to search.
pub fn fts_query(lang: &str, q: &str) -> Option<String> {
    let t = tokens(lang, q);
    if t.is_empty() {
        return None;
    }
    Some(
        t.iter()
            .map(|x| format!("\"{x}\""))
            .collect::<Vec<_>>()
            .join(" OR "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spanish_stems_inflections() {
        let s = stem_text("es", "corriendo corren");
        let t: Vec<&str> = s.split(' ').collect();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0], t[1]);
    }

    #[test]
    fn unknown_language_only_lowercases_and_splits() {
        assert_eq!(stem_text("xx", "Hola, Mundo"), "hola mundo");
    }

    #[test]
    fn query_quotes_and_ors() {
        let q = fts_query("es", "vive en Medellín").expect("has tokens");
        let parts: Vec<&str> = q.split(" OR ").collect();
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.starts_with('"') && p.ends_with('"')));
    }

    #[test]
    fn hostile_query_is_safe() {
        let q = fts_query("es", r#"a" OR NEAR( * -x"#).expect("has tokens");
        for part in q.split(" OR ") {
            let inner = part.trim_matches('"');
            assert!(
                inner.chars().all(char::is_alphanumeric),
                "unsafe token: {part}"
            );
        }
    }

    #[test]
    fn empty_query_is_none() {
        assert_eq!(fts_query("es", "  "), None);
        assert_eq!(fts_query("es", "***"), None);
    }
}
