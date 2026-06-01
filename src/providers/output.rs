use super::model::{UsageRecord, UsageWindow};

pub fn render_human(records: &[UsageRecord]) -> String {
    records
        .iter()
        .map(render_record)
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_record(record: &UsageRecord) -> String {
    let mut parts = vec![
        record.provider.as_str().to_string(),
        record.status.as_str().to_string(),
        format!("plan={}", record.plan.as_deref().unwrap_or("n/a")),
        window_part("5h", record.five_hour.as_ref()),
        window_part("7d", record.seven_day.as_ref()),
    ];

    if let Some(source) = &record.auth_source {
        parts.push(format!("source={source}"));
    }

    if let Some(error) = &record.error {
        parts.push(format!("error={error}"));
    }

    parts.join("  ")
}

fn window_part(label: &str, window: Option<&UsageWindow>) -> String {
    match window {
        Some(window) => format!(
            "{label}={}% reset={}",
            format_percent(window.utilization),
            format_reset(window.resets_at.as_ref())
        ),
        None => format!("{label}=n/a reset=n/a"),
    }
}

fn format_reset(resets_at: Option<&chrono::DateTime<chrono::Utc>>) -> String {
    resets_at
        .map(chrono::DateTime::to_rfc3339)
        .unwrap_or_else(|| "n/a".to_string())
}

fn format_percent(utilization: f64) -> String {
    if utilization.fract().abs() < f64::EPSILON {
        format!("{utilization:.0}")
    } else {
        format!("{utilization:.1}")
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::providers::model::{ProviderKind, ProviderStatus};

    #[test]
    fn renders_missing_reset_as_na() {
        let rendered = render_human(&[UsageRecord {
            provider: ProviderKind::ClaudeCode,
            status: ProviderStatus::Ok,
            auth_source: None,
            plan: Some("pro".to_string()),
            five_hour: Some(UsageWindow {
                utilization: 0.0,
                resets_at: None,
            }),
            seven_day: Some(UsageWindow {
                utilization: 12.0,
                resets_at: Some(Utc.timestamp_opt(1_780_298_127, 0).single().unwrap()),
            }),
            generated_at: Utc.timestamp_opt(1_780_280_000, 0).single().unwrap(),
            error: None,
        }]);

        assert!(rendered.contains("5h=0% reset=n/a"));
        assert!(rendered.contains("7d=12% reset="));
    }
}
