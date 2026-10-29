use crate::model::{MemoryRow, State};

/// `YYYY-MM` for a Unix time in UTC (civil-from-days arithmetic; no date dependency).
pub(crate) fn year_month(secs: i64) -> String {
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}")
}

/// One line per fact, no braces or quotes: `[m12] Vive en Medellín · desde 2026-03`.
/// "desde" is left out when the system learned the fact as it became true (it adds nothing);
/// "sabido" only appears for historical queries.
pub(crate) fn compact_line(m: &MemoryRow, historical: bool) -> String {
    let mut out = format!("[m{}] ", m.id.0);
    if m.state == State::Quarantine {
        out.push_str("⚠ cuarentena: ");
    }
    out.push_str(&m.content);
    if m.valid_from != m.known_since {
        out.push_str(&format!(" · desde {}", year_month(m.valid_from)));
    }
    if let Some(to) = m.valid_to {
        out.push_str(&format!(" · hasta {}", year_month(to)));
    }
    if historical {
        out.push_str(&format!(" · sabido {}", year_month(m.known_since)));
    }
    out
}

/// Full row as JSON, for programs rather than models.
pub(crate) fn json_item(m: &MemoryRow) -> String {
    serde_json::to_string(m).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ActorId, MemoryId, MemoryType, Origin, ScopeId, State};
    use crate::token::{ApproxTokens, TokenCounter};

    fn row(id: i64, content: &str) -> MemoryRow {
        MemoryRow {
            id: MemoryId(id),
            scope: ScopeId(1),
            kind: MemoryType::Semantic,
            content: content.into(),
            content_hash: "h".into(),
            topic_key: Some("residencia".into()),
            origin: Origin::User,
            actor: ActorId(1),
            session: None,
            episode: None,
            source: None,
            state: State::Active,
            valid_from: 1_773_532_800, // 2026-03-15
            valid_to: None,
            known_since: 1_773_532_800,
            replaced_by: None,
            supersedes: None,
        }
    }

    #[test]
    fn year_month_known_dates() {
        assert_eq!(year_month(0), "1970-01");
        assert_eq!(year_month(1_773_532_800), "2026-03");
        assert_eq!(year_month(951_868_799), "2000-02");
        assert_eq!(year_month(-1), "1969-12");
        let _ = (year_month(i64::MAX), year_month(i64::MIN));
    }

    #[test]
    fn compact_line_variants() {
        let mut m = row(12, "Vive en Medellín");
        assert_eq!(compact_line(&m, false), "[m12] Vive en Medellín");
        m.valid_from = 1_740_787_200; // 2025-03-01, earlier than known_since
        assert_eq!(
            compact_line(&m, false),
            "[m12] Vive en Medellín · desde 2025-03"
        );
        m.valid_to = Some(1_773_532_800);
        assert_eq!(
            compact_line(&m, false),
            "[m12] Vive en Medellín · desde 2025-03 · hasta 2026-03"
        );
        assert_eq!(
            compact_line(&m, true),
            "[m12] Vive en Medellín · desde 2025-03 · hasta 2026-03 · sabido 2026-03"
        );
        m.state = State::Quarantine;
        assert!(compact_line(&m, false).starts_with("[m12] ⚠ cuarentena: Vive en Medellín"));
    }

    #[test]
    fn t20_compact_is_cheaper_than_json() {
        let rows: Vec<MemoryRow> = (1..=10)
            .map(|i| row(i, &format!("hecho número {i} sobre el usuario")))
            .collect();
        let tokens = |f: fn(&MemoryRow) -> String| -> usize {
            rows.iter().map(|r| ApproxTokens.count(&f(r))).sum()
        };
        let compact = tokens(|r| compact_line(r, false));
        let json = tokens(json_item);
        assert!(
            compact < json,
            "compact {compact} must be below json {json}"
        );
    }
}
