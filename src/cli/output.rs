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

/// Output format selected via `--output {table|json|csv|tsv|yaml}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
#[clap(rename_all = "lower")]
pub enum Format {
    /// Pretty box-drawing table (default on a terminal).
    #[default]
    Table,
    /// JSON array, pretty-printed.
    Json,
    /// CSV with a header row.
    Csv,
    /// Tab-separated values (default when stdout is piped) — the
    /// machine/LLM-friendly plain form: no borders, no truncation.
    Tsv,
    /// YAML sequence of mappings.
    Yaml,
}

/// Shared formatting flags. Flatten into list-style subcommands via
/// `#[clap(flatten)]`.
#[derive(Debug, Clone, Args, Default)]
pub struct OutputOpts {
    /// Output format [default: table on a terminal, tsv when piped]
    #[arg(long = "output", value_enum)]
    pub format: Option<Format>,
    /// Shorthand for `--output csv`.
    #[arg(long = "csv", default_value_t = false, conflicts_with = "format")]
    pub csv: bool,
    /// Comma-separated subset of columns to show (e.g. `id,title`).
    #[arg(long = "columns")]
    pub columns: Option<String>,
    /// Show every column of the records, not just the defaults.
    #[arg(short = 'x', long = "extended", default_value_t = false)]
    pub extended: bool,
    /// Column to sort by (string sort; prepend `-` for descending).
    #[arg(long = "sort")]
    pub sort: Option<String>,
    /// `key=value` filter — keep rows where `row[key]` contains `value`.
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
///
/// With no explicit `--output`, the format follows the destination: a
/// pretty table on a terminal, plain TSV when stdout is piped — so
/// scripts and LLM agents get parseable rows without remembering a flag.
pub fn print_table<T: Serialize>(
    rows: &[T],
    default_columns: &[&str],
    opts: &OutputOpts,
) -> Result<()> {
    use std::io::IsTerminal;
    let mut opts = opts.clone();
    if opts.format.is_none() {
        opts.format = Some(if opts.csv {
            Format::Csv
        } else if std::io::stdout().is_terminal() {
            Format::Table
        } else {
            Format::Tsv
        });
    }
    print_table_to(&mut std::io::stdout().lock(), rows, default_columns, &opts)
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

    // Apply filter — partial string matching, like oclif's `--filter`.
    if let Some(filt) = opts.filter.as_deref()
        && let Some((k, v)) = filt.split_once('=')
    {
        array.retain(|row| match row.get(k) {
            Some(field) => json_field_to_string(field).contains(v),
            None => false,
        });
    }

    // Apply sort (string-cmp; a `-` prefix sorts descending, like oclif).
    if let Some(key) = opts.sort.as_deref() {
        let (key, descending) = match key.strip_prefix('-') {
            Some(k) => (k, true),
            None => (key, false),
        };
        array.sort_by(|a, b| {
            let sa = a.get(key).map(json_field_to_string).unwrap_or_default();
            let sb = b.get(key).map(json_field_to_string).unwrap_or_default();
            if descending { sb.cmp(&sa) } else { sa.cmp(&sb) }
        });
    }

    // Compute the column list. `--columns` names are matched against the
    // row keys case-insensitively, ignoring spaces, so the header-style
    // names the original CLI shows (`--columns=ID,Title`) work too.
    // `--extended` appends every remaining row key after the defaults.
    let row_keys: Vec<String> = array
        .first()
        .and_then(|row| row.as_object())
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default();
    let columns: Vec<String> = match opts.columns.as_deref() {
        Some(s) => s
            .split(',')
            .map(|c| {
                let c = c.trim();
                row_keys
                    .iter()
                    .find(|k| normalize_column(k) == normalize_column(c))
                    .cloned()
                    .unwrap_or_else(|| c.to_string())
            })
            .collect(),
        None => {
            let mut cols: Vec<String> = default_columns.iter().map(|s| s.to_string()).collect();
            if opts.extended {
                for k in &row_keys {
                    if !cols.contains(k) {
                        cols.push(k.clone());
                    }
                }
            }
            cols
        }
    };

    let format = opts
        .format
        .unwrap_or(if opts.csv { Format::Csv } else { Format::Table });
    match format {
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
        Format::Tsv => {
            if !opts.no_header {
                writeln!(out, "{}", columns.join("\t")).map_err(Error::Io)?;
            }
            for row in &array {
                let line = columns
                    .iter()
                    .map(|c| {
                        tsv_sanitize(&json_field_to_string(
                            row.get(c).unwrap_or(&serde_json::Value::Null),
                        ))
                    })
                    .collect::<Vec<_>>()
                    .join("\t");
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

/// Case- and space-insensitive form for `--columns` name matching
/// (`User Path` / `userpath` / `userPath` all hit the `userPath` key).
fn normalize_column(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_lowercase())
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

/// TSV has no quoting convention — flatten embedded tabs/newlines to a
/// space so one row is always one line.
fn tsv_sanitize(s: &str) -> String {
    if s.contains(['\t', '\n', '\r']) {
        s.replace(['\t', '\n', '\r'], " ")
    } else {
        s.to_string()
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
        o.format = Some(Format::Csv);
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
        o.format = Some(Format::Csv);
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
        o.format = Some(Format::Json);
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
        o.format = Some(Format::Yaml);
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
        o.format = Some(Format::Csv);
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
    fn tsv_renders_plain_rows_and_sanitizes_tabs() {
        #[derive(Serialize)]
        struct R {
            id: String,
            title: String,
        }
        let rs = vec![R {
            id: "n1".into(),
            title: "has\ttab and\nnewline".into(),
        }];
        let mut buf = Vec::new();
        let mut o = opts();
        o.format = Some(Format::Tsv);
        print_table_to(&mut buf, &rs, &["id", "title"], &o).expect("ok");
        let text = String::from_utf8(buf).expect("utf8");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "id\ttitle");
        // Embedded tab/newline flattened: still exactly one row line.
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], "n1\thas tab and newline");
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
        o.format = Some(Format::Csv);
        print_table_to(&mut buf, &rs, &["title"], &o).expect("ok");
        let text = String::from_utf8(buf).expect("utf8");
        assert!(text.contains("\"hello, world\""), "got: {text}");
    }

    #[test]
    fn csv_shorthand_flag_selects_csv_format() {
        let mut buf = Vec::new();
        let mut o = opts();
        o.csv = true; // no explicit --output
        print_table_to(&mut buf, &rows(), &["id", "title"], &o).expect("ok");
        let text = String::from_utf8(buf).expect("utf8");
        assert!(text.starts_with("id,title"), "got: {text}");
    }

    #[test]
    fn extended_appends_remaining_columns_after_defaults() {
        let mut buf = Vec::new();
        let mut o = opts();
        o.format = Some(Format::Csv);
        o.extended = true;
        print_table_to(&mut buf, &rows(), &["id"], &o).expect("ok");
        let text = String::from_utf8(buf).expect("utf8");
        // Default column first, then the rest of the record's keys.
        assert!(text.starts_with("id,"), "got: {text}");
        assert!(text.contains("title"));
        assert!(text.contains("teamPath"));
    }

    #[test]
    fn sort_dash_prefix_sorts_descending() {
        let mut buf = Vec::new();
        let mut o = opts();
        o.format = Some(Format::Csv);
        o.sort = Some("-title".into());
        print_table_to(&mut buf, &rows(), &["id", "title"], &o).expect("ok");
        let text = String::from_utf8(buf).expect("utf8");
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[1].starts_with("n2,Second"), "got: {text}");
        assert!(lines[2].starts_with("n1,First"));
    }

    #[test]
    fn columns_match_header_style_names_case_insensitively() {
        let mut buf = Vec::new();
        let mut o = opts();
        o.format = Some(Format::Csv);
        // oclif-style header names: spaces and case shouldn't matter.
        o.columns = Some("ID,Team Path".into());
        print_table_to(&mut buf, &rows(), &["id", "title"], &o).expect("ok");
        let text = String::from_utf8(buf).expect("utf8");
        assert!(text.starts_with("id,teamPath"), "got: {text}");
        assert!(text.contains("n2,demo"));
    }

    #[test]
    fn filter_uses_partial_match() {
        let mut buf = Vec::new();
        let mut o = opts();
        o.format = Some(Format::Csv);
        o.filter = Some("title=Sec".into()); // partial, like oclif
        print_table_to(&mut buf, &rows(), &["id", "title"], &o).expect("ok");
        let text = String::from_utf8(buf).expect("utf8");
        assert!(text.contains("n2,Second"));
        assert!(!text.contains("n1,First"));
    }
}
