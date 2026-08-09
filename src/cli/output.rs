//! Terminal output helpers.
//!
//! Data goes to stdout so it can be piped; status and progress messages
//! belong on stderr via `eprintln!` at the call site.

/// Pretty-print a raw API response for `--json` mode.
pub fn json(value: &serde_json::Value) {
    match serde_json::to_string_pretty(value) {
        Ok(rendered) => println!("{rendered}"),
        Err(_) => println!("{value}"),
    }
}

/// Print a left-aligned table with column widths fitted to content.
pub fn table(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(widths.len()) {
            widths[i] = widths[i].max(cell.len());
        }
    }

    let render = |cells: Vec<&str>| {
        let mut line = String::new();
        let last = widths.len().saturating_sub(1);
        for (i, cell) in cells.iter().enumerate().take(widths.len()) {
            if i == last {
                line.push_str(cell);
            } else {
                line.push_str(&format!("{:<width$}  ", cell, width = widths[i]));
            }
        }
        line.trim_end().to_string()
    };

    println!("{}", render(headers.to_vec()));
    for row in rows {
        println!("{}", render(row.iter().map(String::as_str).collect()));
    }
}

/// Render a serializable enum as its wire-format label (e.g. `system`,
/// not `System`), matching what the API and web UI display.
pub fn enum_label<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(label)) => label,
        Ok(other) => other.to_string(),
        Err(_) => String::new(),
    }
}

/// Shorten an RFC 3339 timestamp to `YYYY-MM-DD HH:MM` for display.
/// SQLite text timestamps ("YYYY-MM-DD HH:MM:SS") don't parse as RFC 3339
/// and are trimmed to minute precision instead; anything else passes through.
pub fn short_timestamp(value: &str) -> String {
    if let Ok(timestamp) = chrono::DateTime::parse_from_rfc3339(value) {
        return timestamp.format("%Y-%m-%d %H:%M").to_string();
    }
    let bytes = value.as_bytes();
    if bytes.len() >= 16 && bytes[4] == b'-' && bytes[7] == b'-' && bytes[10] == b' ' {
        return value[..16].to_string();
    }
    value.to_string()
}

/// Truncate to a character budget with an ellipsis for table cells.
pub fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() > max_chars {
        format!(
            "{}\u{2026}",
            value.chars().take(max_chars).collect::<String>()
        )
    } else {
        value.to_string()
    }
}

/// Render a byte count in human-readable units.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
