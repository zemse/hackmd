//! Tabular / JSON / YAML / CSV output for list-style CLI commands.
//!
//! Mirrors the `ux.table` semantics of `@oclif/core` used by
//! `_ref/hackmd-cli/src/commands/*.ts`. Reuses a single flattened
//! [`OutputOpts`] across all listing commands.

use std::io::Write;

use clap::{Args, ValueEnum};
use comfy_table::{ContentArrangement, Table, presets};
use serde::Serialize;

use crate::error::{Error, Result};

/// Output format selected via `--output {table|json|csv|yaml}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
#[clap(rename_all = "lower")]
pub enum Format {
    /// Pretty box-drawing table (default).
    #[default]
    Table,
    /// JSON array, pretty-printed.
    Json,
    /// CSV with a header row.
    Csv,
    /// YAML sequence of mappings.
    Yaml,
}

/// Shared formatting flags. Flatten into list-style subcommands via
/// `#[clap(flatten)]`.
#[derive(Debug, Clone, Args, Default)]
pub struct OutputOpts {
    /// Output format. Defaults to `table`.
    #[arg(long = "output", value_enum, default_value_t = Format::Table)]
    pub format: Format,
    /// Comma-separated subset of columns to show (e.g. `id,title`).
    #[arg(long = "columns")]
    pub columns: Option<String>,
    /// Column to sort by (best-effort string sort).
    #[arg(long = "sort")]
    pub sort: Option<String>,
    /// `key=value` filter — keep rows where `row[key] == value`.
    #[arg(long = "filter")]
    pub filter: Option<String>,
    /// Omit the table header row.
    #[arg(long = "no-header", default_value_t = false)]
    pub no_header: bool,
    /// Don't truncate long cell values.
    #[arg(long = "no-truncate", default_value_t = false)]
    pub no_truncate: bool,
}

/// Render a list of records via the selected [`OutputOpts::format`].
///
/// `default_columns` is the ordered set of camelCase JSON keys to show
/// when the caller doesn't pass `--columns`.
pub fn print_table<T: Serialize>(
    rows: &[T],
    default_columns: &[&str],
    opts: &OutputOpts,
) -> Result<()> {
    print_table_to(&mut std::io::stdout().lock(), rows, default_columns, opts)
}

/// Same as [`print_table`] but writes to an arbitrary sink (handy for tests).
pub fn print_table_to<W: Write, T: Serialize>(
    out: &mut W,
    rows: &[T],
    default_columns: &[&str],
    opts: &OutputOpts,
) -> Result<()> {
    // Convert to JSON once — every output mode just walks the values.
    let value = serde_json::to_value(rows).map_err(Error::Json)?;
    let mut array = match value {
        serde_json::Value::Array(a) => a,
        other => vec![other],
    };

    // Apply filter (naive `key=value` equality on JSON values).
    if let Some(filt) = opts.filter.as_deref()
        && let Some((k, v)) = filt.split_once('=')
    {
        array.retain(|row| match row.get(k) {
            Some(field) => json_field_to_string(field) == v,
            None => false,
        });
    }

    // Apply sort (naive string-cmp).
    if let Some(key) = opts.sort.as_deref() {
        array.sort_by(|a, b| {
            let sa = a.get(key).map(json_field_to_string).unwrap_or_default();
            let sb = b.get(key).map(json_field_to_string).unwrap_or_default();
            sa.cmp(&sb)
        });
    }

    // Compute the column list.
    let columns: Vec<String> = match opts.columns.as_deref() {
        Some(s) => s.split(',').map(|c| c.trim().to_string()).collect(),
        None => default_columns.iter().map(|s| s.to_string()).collect(),
    };

    match opts.format {
        Format::Json => {
            let projected = project(&array, &columns);
            let text = serde_json::to_string_pretty(&projected).map_err(Error::Json)?;
            writeln!(out, "{text}").map_err(Error::Io)?;
        }
        Format::Yaml => {
            let projected = project(&array, &columns);
            let text = serde_yaml::to_string(&projected)
                .map_err(|e| Error::Config(format!("yaml: {e}")))?;
            write!(out, "{text}").map_err(Error::Io)?;
        }
        Format::Csv => {
            if !opts.no_header {
                writeln!(out, "{}", columns.join(",")).map_err(Error::Io)?;
            }
            for row in &array {
                let line = columns
                    .iter()
                    .map(|c| {
                        csv_escape(&json_field_to_string(
                            row.get(c).unwrap_or(&serde_json::Value::Null),
                        ))
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                writeln!(out, "{line}").map_err(Error::Io)?;
            }
        }
        Format::Table => {
            let mut table = Table::new();
            table
                .load_preset(presets::UTF8_FULL)
                .set_content_arrangement(if opts.no_truncate {
                    ContentArrangement::Disabled
                } else {
                    ContentArrangement::Dynamic
                });
            if !opts.no_header {
                table.set_header(&columns);
            }
            for row in &array {
                let cells: Vec<String> = columns
                    .iter()
                    .map(|c| json_field_to_string(row.get(c).unwrap_or(&serde_json::Value::Null)))
                    .collect();
                table.add_row(cells);
            }
            writeln!(out, "{table}").map_err(Error::Io)?;
        }
    }

    Ok(())
}

fn project(
    rows: &[serde_json::Value],
    columns: &[String],
) -> Vec<serde_json::Map<String, serde_json::Value>> {
    rows.iter()
        .map(|row| {
            let mut m = serde_json::Map::new();
            for c in columns {
                let v = row.get(c).cloned().unwrap_or(serde_json::Value::Null);
                m.insert(c.clone(), v);
            }
            m
        })
        .collect()
}

fn json_field_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Serialize)]
    struct Row {
        id: String,
        title: String,
        #[serde(rename = "teamPath")]
        team_path: Option<String>,
    }

    fn rows() -> Vec<Row> {
        vec![
            Row {
                id: "n1".into(),
                title: "First".into(),
                team_path: None,
            },
            Row {
                id: "n2".into(),
                title: "Second".into(),
                team_path: Some("demo".into()),
            },
        ]
    }

    fn opts() -> OutputOpts {
        OutputOpts::default()
    }

    #[test]
    fn csv_renders_default_columns() {
        let mut buf = Vec::new();
        let mut o = opts();
        o.format = Format::Csv;
        print_table_to(&mut buf, &rows(), &["id", "title", "teamPath"], &o).expect("ok");
        let text = String::from_utf8(buf).expect("utf8");
        assert!(text.contains("id,title,teamPath"));
        assert!(text.contains("n1,First,null"));
        assert!(text.contains("n2,Second,demo"));
    }

    #[test]
    fn csv_filter_keeps_matching_rows() {
        let mut buf = Vec::new();
        let mut o = opts();
        o.format = Format::Csv;
        o.filter = Some("id=n2".into());
        print_table_to(&mut buf, &rows(), &["id", "title", "teamPath"], &o).expect("ok");
        let text = String::from_utf8(buf).expect("utf8");
        assert!(text.contains("n2,Second"));
        assert!(!text.contains("n1,First"));
    }

    #[test]
    fn json_projects_subset_of_columns() {
        let mut buf = Vec::new();
        let mut o = opts();
        o.format = Format::Json;
        o.columns = Some("id".into());
        print_table_to(&mut buf, &rows(), &["id", "title"], &o).expect("ok");
        let text = String::from_utf8(buf).expect("utf8");
        let v: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(v, json!([{ "id": "n1" }, { "id": "n2" }]));
    }

    #[test]
    fn yaml_includes_team_path() {
        let mut buf = Vec::new();
        let mut o = opts();
        o.format = Format::Yaml;
        print_table_to(&mut buf, &rows(), &["id", "teamPath"], &o).expect("ok");
        let text = String::from_utf8(buf).expect("utf8");
        assert!(text.contains("teamPath"));
        assert!(text.contains("n1"));
        assert!(text.contains("demo"));
    }

    #[test]
    fn sort_orders_rows_by_column() {
        let mut buf = Vec::new();
        let mut o = opts();
        o.format = Format::Csv;
        o.sort = Some("title".into());
        print_table_to(&mut buf, &rows(), &["id", "title"], &o).expect("ok");
        let text = String::from_utf8(buf).expect("utf8");
        let lines: Vec<&str> = text.lines().collect();
        // header + 2 rows
        assert_eq!(lines.len(), 3);
        assert!(lines[1].starts_with("n1,First"));
        assert!(lines[2].starts_with("n2,Second"));
    }

    #[test]
    fn table_renders_with_header() {
        let mut buf = Vec::new();
        let o = opts();
        print_table_to(&mut buf, &rows(), &["id", "title"], &o).expect("ok");
        let text = String::from_utf8(buf).expect("utf8");
        assert!(text.contains("id"));
        assert!(text.contains("title"));
        assert!(text.contains("First"));
    }

    #[test]
    fn csv_escapes_embedded_comma() {
        #[derive(Serialize)]
        struct R {
            title: String,
        }
        let rs = vec![R {
            title: "hello, world".into(),
        }];
        let mut buf = Vec::new();
        let mut o = opts();
        o.format = Format::Csv;
        print_table_to(&mut buf, &rs, &["title"], &o).expect("ok");
        let text = String::from_utf8(buf).expect("utf8");
        assert!(text.contains("\"hello, world\""), "got: {text}");
    }
}
