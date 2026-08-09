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
