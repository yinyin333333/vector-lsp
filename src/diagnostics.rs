use std::collections::{HashMap, HashSet};

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

use crate::document::{DocumentData, utf16_len};
use crate::schema::{FieldTypeName, Schema};
use crate::workspace::SymbolIndex;

const PLUGIN_LOOKUP_TARGETS: &[(&str, &str)] = &[("setitems", "index"), ("uniqueitems", "index")];

/// Validate a single document against the schema and symbol index.
///
/// Three classes of diagnostic are produced:
///   ERROR   — cross-reference target not found in the workspace symbol index
///   WARNING — value cannot be parsed as the column's declared int/float type
///   INFO    — column header is not declared in the schema and not in ignoreFields
pub fn validate_document(
    file_stem: &str,
    doc: &DocumentData,
    schema: Option<&Schema>,
    symbols: &SymbolIndex,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    let target_columns = reference_target_columns(file_stem, schema);
    let mut seen_targets: HashMap<usize, HashMap<String, (String, u32, u32)>> = HashMap::new();
    for (idx, header) in doc.headers.iter().enumerate() {
        if target_columns.contains(&header.to_lowercase()) {
            seen_targets.insert(idx, HashMap::new());
        }
    }

    // Row-level diagnostics.
    for row in &doc.rows {
        if row
            .cells
            .first()
            .map(|cell| cell.value.trim_start().starts_with('*'))
            .unwrap_or(false)
        {
            continue;
        }
        for (col_idx, cell) in row.cells.iter().enumerate() {
            if cell.value.is_empty() {
                continue;
            }
            let col_name = match doc.headers.get(col_idx) {
                Some(h) if !h.is_empty() => h.as_str(),
                _ => continue,
            };

            if !cell.value.trim().is_empty()
                && let Some(seen) = seen_targets.get_mut(&col_idx)
            {
                let key = cell.value.to_lowercase();
                if let Some((_first_value, first_line, first_col)) = seen.get(&key) {
                    let cell_end = cell.col_start + utf16_len(&cell.value);
                    diags.push(Diagnostic {
                        range: Range {
                            start: Position { line: row.line, character: cell.col_start },
                            end: Position { line: row.line, character: cell_end },
                        },
                        severity: Some(DiagnosticSeverity::WARNING),
                        source: Some("vector-lsp".into()),
                        message: format!(
                            "Duplicate value '{}' in unique key column '{}' (first seen at line {}, column {})",
                            cell.value,
                            col_name,
                            first_line + 1,
                            first_col + 1
                        ),
                        ..Default::default()
                    });
                } else {
                    seen.insert(key, (cell.value.clone(), row.line, cell.col_start));
                }
            }

            let field_type = schema
                .and_then(|s| s.find_field(file_stem, col_name))
                .and_then(|f| f.field_type.as_ref());

            let Some(ft) = field_type else { continue };

            let cell_end = cell.col_start + utf16_len(&cell.value);
            let cell_range = Range {
                start: Position {
                    line: row.line,
                    character: cell.col_start,
                },
                end: Position {
                    line: row.line,
                    character: cell_end,
                },
            };

            match ft.type_name {
                FieldTypeName::Reference => {
                    if cell.value.trim().is_empty() {
                        continue;
                    }
                    if let (Some(ref_file), Some(ref_col)) =
                        (ft.file.as_deref(), ft.field.as_deref())
                    {
                        if !target_available(schema, symbols, ref_file, ref_col) {
                            continue;
                        }
                        let found = target_exists(schema, symbols, ref_file, ref_col, &cell.value);
                        if !found {
                            let trimmed = cell.value.trim();
                            let trim_note = if trimmed != cell.value
                                && !trimmed.is_empty()
                                && target_exists(schema, symbols, ref_file, ref_col, trimmed)
                            {
                                format!(
                                    "; trimmed value '{}' exists, but references preserve leading/trailing whitespace",
                                    trimmed
                                )
                            } else {
                                String::new()
                            };
                            diags.push(Diagnostic {
                                range: cell_range,
                                severity: Some(DiagnosticSeverity::ERROR),
                                source: Some("vector-lsp".into()),
                                message: format!(
                                    "Reference value '{}' not found in {}.{}{}",
                                    cell.value, ref_file, ref_col, trim_note
                                ),
                                ..Default::default()
                            });
                        }
                    }
                }
                FieldTypeName::Int => {
                    if cell.value.parse::<i64>().is_err() {
                        diags.push(Diagnostic {
                            range: cell_range,
                            severity: Some(DiagnosticSeverity::WARNING),
                            source: Some("vector-lsp".into()),
                            message: format!(
                                "'{}' is not a valid integer for column '{col_name}'",
                                cell.value
                            ),
                            ..Default::default()
                        });
                    }
                }
                FieldTypeName::Float => {
                    if cell.value.parse::<f64>().is_err() {
                        diags.push(Diagnostic {
                            range: cell_range,
                            severity: Some(DiagnosticSeverity::WARNING),
                            source: Some("vector-lsp".into()),
                            message: format!(
                                "'{}' is not a valid number for column '{col_name}'",
                                cell.value
                            ),
                            ..Default::default()
                        });
                    }
                }
                FieldTypeName::Boolean => {
                    if cell.value.trim().is_empty() {
                        continue;
                    }
                    if cell.value != "0" && cell.value != "1" {
                        diags.push(Diagnostic {
                            range: cell_range,
                            severity: Some(DiagnosticSeverity::WARNING),
                            source: Some("vector-lsp".into()),
                            message: format!(
                                "'{}' is not a valid boolean for column '{col_name}' (expected empty, 0, or 1)",
                                cell.value
                            ),
                            ..Default::default()
                        });
                    }
                }
                _ => {}
            }
        }
    }

    diags
}

fn target_exists(
    schema: Option<&Schema>,
    symbols: &SymbolIndex,
    ref_file: &str,
    ref_col: &str,
    value: &str,
) -> bool {
    symbols.lookup(ref_file, ref_col, value).is_some()
        || schema
            .and_then(|s| s.enum_values_for_target(ref_file, ref_col))
            .map(|vals| vals.iter().any(|v| v.eq_ignore_ascii_case(value)))
            .unwrap_or(false)
}

fn target_available(
    schema: Option<&Schema>,
    symbols: &SymbolIndex,
    ref_file: &str,
    ref_col: &str,
) -> bool {
    schema
        .and_then(|s| s.enum_values_for_target(ref_file, ref_col))
        .is_some()
        || (symbols.has_file(ref_file) && symbols.has_column(ref_file, ref_col))
}

fn reference_target_columns(file_stem: &str, schema: Option<&Schema>) -> HashSet<String> {
    let stem = file_stem.to_lowercase();
    let mut targets: HashSet<String> = schema
        .map(|s| {
            s.reference_targets()
                .into_iter()
                .filter_map(|(file, col)| if file == stem { Some(col) } else { None })
                .collect()
        })
        .unwrap_or_default();
    for (file, col) in PLUGIN_LOOKUP_TARGETS {
        if *file == stem {
            targets.insert(col.to_lowercase());
        }
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{FieldType, SchemaField, SchemaFile};

    #[test]
    fn diagnostic_ranges_use_utf16_code_units() {
        let document = DocumentData::parse("lead\tcount\n🙂\tbad", '\t');
        let mut schema = Schema::default();
        schema.files.insert(
            "items".to_string(),
            SchemaFile {
                fields: vec![SchemaField {
                    name: "count".to_string(),
                    description: None,
                    field_type: Some(FieldType {
                        type_name: FieldTypeName::Int,
                        data_length: 0,
                        mem_size: 0,
                        file: None,
                        field: None,
                    }),
                    alt_names: vec![],
                    append_field: None,
                    table: None,
                }],
                ..Default::default()
            },
        );

        let diagnostics = validate_document("items", &document, Some(&schema), &SymbolIndex::new());

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].range.start, Position::new(1, 3));
        assert_eq!(diagnostics[0].range.end, Position::new(1, 6));
    }

    #[test]
    fn current_unknown_and_ignored_header_behavior_is_fingerprinted_without_new_diagnostics() {
        let document = DocumentData::parse("known\tignored\tunknown\nvalue\tx\ty", '\t');
        let mut schema = Schema::default();
        schema.files.insert(
            "items".to_string(),
            SchemaFile {
                fields: vec![SchemaField {
                    name: "known".to_string(),
                    description: None,
                    field_type: Some(FieldType {
                        type_name: FieldTypeName::Text,
                        data_length: 0,
                        mem_size: 0,
                        file: None,
                        field: None,
                    }),
                    alt_names: vec![],
                    append_field: None,
                    table: None,
                }],
                ignore_fields: vec!["ignored".to_string()],
                ..Default::default()
            },
        );

        assert!(
            validate_document("items", &document, Some(&schema), &SymbolIndex::new()).is_empty(),
            "current product emits no header-only diagnostics; changing that requires an explicit contract decision"
        );
    }
}
