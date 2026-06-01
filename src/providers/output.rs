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
            window.resets_at.to_rfc3339()
        ),
        None => format!("{label}=n/a reset=n/a"),
    }
}

fn format_percent(utilization: f64) -> String {
    if utilization.fract().abs() < f64::EPSILON {
        format!("{utilization:.0}")
    } else {
        format!("{utilization:.1}")
    }
}
