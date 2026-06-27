use std::collections::{HashMap, HashSet};

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

use crate::document::DocumentData;
use crate::schema::{FieldTypeName, Schema};
use crate::workspace::SymbolIndex;

const PLUGIN_LOOKUP_TARGETS: &[(&str, &str)] = &[("setitems", "index"), ("uniqueitems", "index")];

#[derive(Clone, Copy, Debug)]
pub struct ValidationOptions {
    pub check_references: bool,
}

impl ValidationOptions {
    pub const FULL: Self = Self {
        check_references: true,
    };

    pub const LOCAL_ONLY: Self = Self {
        check_references: false,
    };
}

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
    validate_document_with_options(
        file_stem,
        doc,
        schema,
        Some(symbols),
        ValidationOptions::FULL,
    )
}

/// Validate only diagnostics that do not require the full workspace symbol index.
pub fn validate_document_local(
    file_stem: &str,
    doc: &DocumentData,
    schema: Option<&Schema>,
) -> Vec<Diagnostic> {
    validate_document_with_options(file_stem, doc, schema, None, ValidationOptions::LOCAL_ONLY)
}

fn validate_document_with_options(
    file_stem: &str,
    doc: &DocumentData,
    schema: Option<&Schema>,
    symbols: Option<&SymbolIndex>,
    options: ValidationOptions,
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
                    let cell_end = cell.col_start + cell.value.chars().count() as u32;
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

            let cell_end = cell.col_start + cell.value.chars().count() as u32;
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
                    if !options.check_references {
                        continue;
                    }
                    let Some(symbols) = symbols else {
                        continue;
                    };
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
    use std::collections::HashMap;

    use tower_lsp::lsp_types::Url;

    use super::*;
    use crate::schema::{FieldType, SchemaField, SchemaFile};

    fn schema_field(
        name: &str,
        type_name: FieldTypeName,
        file: Option<&str>,
        field: Option<&str>,
    ) -> SchemaField {
        SchemaField {
            name: name.to_string(),
            description: None,
            field_type: Some(FieldType {
                type_name,
                data_length: 0,
                mem_size: 0,
                file: file.map(str::to_string),
                field: field.map(str::to_string),
            }),
            alt_names: vec![],
            append_field: None,
            table: None,
        }
    }

    fn test_schema() -> Schema {
        let mut files = HashMap::new();
        files.insert(
            "source".to_string(),
            SchemaFile {
                fields: vec![
                    schema_field("id", FieldTypeName::Text, None, None),
                    schema_field(
                        "ref",
                        FieldTypeName::Reference,
                        Some("target"),
                        Some("code"),
                    ),
                    schema_field("qty", FieldTypeName::Int, None, None),
                    schema_field("enabled", FieldTypeName::Boolean, None, None),
                ],
                ..Default::default()
            },
        );
        files.insert(
            "target".to_string(),
            SchemaFile {
                fields: vec![schema_field("code", FieldTypeName::Text, None, None)],
                ..Default::default()
            },
        );
        Schema { files }
    }

    fn has_message(diags: &[Diagnostic], needle: &str) -> bool {
        diags.iter().any(|diag| diag.message.contains(needle))
    }

    #[test]
    fn validate_document_local_does_not_emit_missing_reference_errors() {
        let schema = test_schema();
        let doc = DocumentData::parse("id\tref\tqty\tenabled\nrow1\tmissing\t1\t1\n", '\t');

        let diags = validate_document_local("source", &doc, Some(&schema));

        assert!(
            !has_message(&diags, "Reference value 'missing' not found in target.code"),
            "local diagnostics should not report missing references: {diags:?}"
        );
    }

    #[test]
    fn validate_document_still_emits_missing_reference_errors_in_full_mode() {
        let schema = test_schema();
        let target_doc = DocumentData::parse("code\npresent\n", '\t');
        let mut symbols = SymbolIndex::new();
        let ref_targets = schema.reference_targets();
        symbols.index_document(
            &Url::parse("file:///target.txt").unwrap(),
            "target",
            &target_doc,
            &ref_targets,
        );
        let source_doc = DocumentData::parse("id\tref\tqty\tenabled\nrow1\tmissing\t1\t1\n", '\t');

        let diags = validate_document("source", &source_doc, Some(&schema), &symbols);

        assert!(
            has_message(&diags, "Reference value 'missing' not found in target.code"),
            "full diagnostics should report missing references: {diags:?}"
        );
    }

    #[test]
    fn validate_document_local_keeps_type_parsing_warnings() {
        let schema = test_schema();
        let doc = DocumentData::parse("id\tref\tqty\tenabled\nrow1\tmissing\tbad\t2\n", '\t');

        let diags = validate_document_local("source", &doc, Some(&schema));

        assert!(
            has_message(&diags, "'bad' is not a valid integer for column 'qty'"),
            "local diagnostics should keep integer warnings: {diags:?}"
        );
        assert!(
            has_message(
                &diags,
                "'2' is not a valid boolean for column 'enabled' (expected empty, 0, or 1)"
            ),
            "local diagnostics should keep boolean warnings: {diags:?}"
        );
    }
}
