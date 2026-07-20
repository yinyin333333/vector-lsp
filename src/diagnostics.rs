use std::collections::{HashMap, HashSet};

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

use crate::document::{DocumentData, utf16_len};
use crate::i18n::{self, Locale};
use crate::schema::{FieldTypeName, ReferenceResolver, ReferenceUnknownPolicy, Schema};
use crate::workspace::SymbolIndex;

/// Validate a single document against the schema and symbol index.
///
/// Three classes of diagnostic are produced:
///   ERROR   — cross-reference target not found in the workspace symbol index
///   WARNING — value cannot be parsed as the column's declared int/float type
///   INFO    — column header is not declared in the schema and not in ignoreFields
#[cfg(test)]
pub fn validate_document(
    file_stem: &str,
    doc: &DocumentData,
    schema: Option<&Schema>,
    symbols: &SymbolIndex,
) -> Vec<Diagnostic> {
    legacy_schema_diagnostics(validate_document_for_locale(
        file_stem,
        doc,
        schema,
        symbols,
        None,
        Locale::EnUs,
    ))
}

pub fn validate_document_for_version(
    file_stem: &str,
    doc: &DocumentData,
    schema: Option<&Schema>,
    symbols: &SymbolIndex,
    game_version: Option<&str>,
) -> Vec<Diagnostic> {
    legacy_schema_diagnostics(validate_document_for_locale(
        file_stem,
        doc,
        schema,
        symbols,
        game_version,
        Locale::EnUs,
    ))
}

fn legacy_schema_diagnostics(mut diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
    for diagnostic in &mut diagnostics {
        if diagnostic
            .data
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .is_some_and(|data| {
                data.len() == 2
                    && data.contains_key("messageKey")
                    && data.contains_key("messageArgs")
            })
        {
            diagnostic.data = None;
        }
    }
    diagnostics
}

/// Locale-aware schema validation.  The semantic validation logic, ranges,
/// severity, codes and metadata do not depend on the selected language.
pub fn validate_document_for_locale(
    file_stem: &str,
    doc: &DocumentData,
    schema: Option<&Schema>,
    symbols: &SymbolIndex,
    game_version: Option<&str>,
    locale: Locale,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    let target_columns = unique_target_columns(file_stem, schema);
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
                    diags.push(i18n::localized_diagnostic(
                        locale,
                        "diag.duplicate_unique",
                        i18n::args([
                            ("value", serde_json::json!(cell.value)),
                            ("column", serde_json::json!(col_name)),
                            ("line", serde_json::json!(first_line + 1)),
                            ("firstColumn", serde_json::json!(first_col + 1)),
                        ]),
                        Diagnostic {
                            range: Range {
                                start: Position {
                                    line: row.line,
                                    character: cell.col_start,
                                },
                                end: Position {
                                    line: row.line,
                                    character: cell_end,
                                },
                            },
                            severity: Some(DiagnosticSeverity::WARNING),
                            source: Some("vector-lsp".into()),
                            ..Default::default()
                        },
                    ));
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
                    if !reference_cell_is_consumed(file_stem, doc, row, col_name) {
                        continue;
                    }
                    if let (Some(ref_file), Some(ref_col)) =
                        (ft.file.as_deref(), ft.field.as_deref())
                    {
                        if !target_available(schema, symbols, ref_file, ref_col) {
                            continue;
                        }
                        let found = target_exists(
                            schema,
                            symbols,
                            ref_file,
                            ref_col,
                            &cell.value,
                            ft.resolver,
                        );
                        if !found {
                            if ft.unknown_policy == ReferenceUnknownPolicy::Ignore {
                                continue;
                            }
                            let trimmed = cell.value.trim();
                            let (leading_whitespace, trailing_whitespace) =
                                edge_whitespace_counts(&cell.value);
                            let has_trimmed_match = ft.resolver == ReferenceResolver::AsciiCi
                                && trimmed != cell.value
                                && !trimmed.is_empty()
                                && target_exists(
                                    schema,
                                    symbols,
                                    ref_file,
                                    ref_col,
                                    trimmed,
                                    ft.resolver,
                                );
                            let monpet_consume_stat =
                                is_monpet_consumestat_reference(file_stem, col_name);
                            let properties_stat = is_properties_stat_reference(file_stem, col_name);
                            let properties_stat_func =
                                properties_stat_dispatch_func(file_stem, doc, row, col_name);
                            let skills_range =
                                is_3_2_skills_range_reference(file_stem, col_name, ft.resolver);
                            let data = if monpet_consume_stat {
                                Some(serde_json::json!({
                                    "rule": "reference",
                                    "kind": "unresolved-reference",
                                    "scope": "monpet-consumestat",
                                    "namespace": "ItemStatCost.Stat",
                                    "lookup": "whole-cell ASCII case-insensitive no-trim",
                                    "resolverResult": -1,
                                    "storedValue": 65535,
                                    "runtimeEffect": "Consume skips only this slot"
                                }))
                            } else if properties_stat_func == Some(17) {
                                Some(serde_json::json!({
                                    "rule": "reference",
                                    "kind": "unresolved-reference",
                                    "scope": "properties-stat",
                                    "namespace": "ItemStatCost.Stat",
                                    "lookup": "whole-cell ASCII case-insensitive no-trim",
                                    "resolverResult": -1,
                                    "storedValue": 65535,
                                    "runtimeEffect": "The active property slot applies no stat"
                                }))
                            } else if properties_stat {
                                Some(serde_json::json!({
                                    "rule": "reference",
                                    "kind": "unresolved-reference",
                                    "scope": "properties-stat",
                                    "namespace": "ItemStatCost.Stat",
                                    "lookup": "whole-cell ASCII case-insensitive no-trim",
                                    "resolverResult": -1,
                                    "storedValue": 65535
                                }))
                            } else if skills_range {
                                Some(serde_json::json!({
                                    "rule": "reference",
                                    "kind": "unresolved-reference",
                                    "scope": "skills.range",
                                    "lookup": "first four bytes, ASCII-space padded, case-sensitive",
                                    "knownCodes": ["none", "h2h", "rng", "both", "loc"]
                                }))
                            } else {
                                None
                            };
                            let (message_key, message_args) = if skills_range {
                                ("diag.reference.range", i18n::args([]))
                            } else if monpet_consume_stat {
                                (
                                    "diag.reference.monpet_consumestat",
                                    i18n::args([("value", serde_json::json!(cell.value))]),
                                )
                            } else if properties_stat {
                                if properties_stat_func == Some(17) {
                                    (
                                        "diag.reference.properties_stat_noeffect",
                                        i18n::args([("value", serde_json::json!(cell.value))]),
                                    )
                                } else {
                                    (
                                        "diag.reference.properties_stat",
                                        i18n::args([("value", serde_json::json!(cell.value))]),
                                    )
                                }
                            } else if ft.resolver == ReferenceResolver::Fixed4 {
                                let effective = crate::workspace::fixed4_display(&cell.value);
                                let shown_value = mark_whitespace(&cell.value);
                                let shown_effective = mark_whitespace(&effective);
                                (
                                    "diag.fixed4_unknown",
                                    i18n::args([
                                        ("value", serde_json::json!(shown_value)),
                                        ("effective", serde_json::json!(shown_effective)),
                                        (
                                            "hasSpaceMarker",
                                            serde_json::json!(
                                                cell.value.contains(' ') || effective.contains(' ')
                                            ),
                                        ),
                                        (
                                            "hasTabMarker",
                                            serde_json::json!(
                                                cell.value.contains('\t')
                                                    || effective.contains('\t')
                                            ),
                                        ),
                                    ]),
                                )
                            } else {
                                (
                                    "diag.reference.unresolved",
                                    i18n::args([
                                        ("value", serde_json::json!(cell.value)),
                                        ("file", serde_json::json!(ref_file)),
                                        ("column", serde_json::json!(ref_col)),
                                        (
                                            "trimmedValue",
                                            serde_json::json!(if has_trimmed_match {
                                                trimmed
                                            } else {
                                                ""
                                            }),
                                        ),
                                        (
                                            "leadingWhitespace",
                                            serde_json::json!(if has_trimmed_match {
                                                leading_whitespace
                                            } else {
                                                0
                                            }),
                                        ),
                                        (
                                            "trailingWhitespace",
                                            serde_json::json!(if has_trimmed_match {
                                                trailing_whitespace
                                            } else {
                                                0
                                            }),
                                        ),
                                    ]),
                                )
                            };
                            diags.push(i18n::localized_diagnostic(
                                locale,
                                message_key,
                                message_args,
                                Diagnostic {
                                    range: cell_range,
                                    severity: Some(match ft.unknown_policy {
                                        ReferenceUnknownPolicy::Error => DiagnosticSeverity::ERROR,
                                        ReferenceUnknownPolicy::Warning => {
                                            DiagnosticSeverity::WARNING
                                        }
                                        ReferenceUnknownPolicy::Ignore => unreachable!(),
                                    }),
                                    source: Some("vector-lsp".into()),
                                    data,
                                    ..Default::default()
                                },
                            ));
                        }
                    }
                }
                FieldTypeName::Int => {
                    if !integer_cell_is_consumed(file_stem, doc, row, col_name) {
                        continue;
                    }
                    if is_hit_summon_mode_cell(file_stem, doc, row, col_name, game_version) {
                        let result = hit_summon_mode_result(&cell.value);
                        if result.message.is_some() {
                            let (message_key, message_args) =
                                hit_summon_message_key_args(&cell.value, &result);
                            diags.push(i18n::localized_diagnostic(locale, message_key, message_args, Diagnostic {
                                range: cell_range,
                                severity: Some(DiagnosticSeverity::WARNING),
                                source: Some("vector-lsp".into()),
                                data: Some(serde_json::json!({
                                    "rule": "hit-summon-mode",
                                    "scope": "missiles.pSrvHitFunc6.sHitPar2",
                                    "parsedUint32": result.parsed,
                                    "effectiveMode": result.effective,
                                    "modeCode": HIT_SUMMON_MODE_CODES[result.effective as usize],
                                    "fallbackApplied": result.fallback_applied
                                })),
                                ..Default::default()
                            }));
                        }
                        continue;
                    }
                    if cell.value.parse::<i64>().is_err() {
                        let message_key = if file_stem.eq_ignore_ascii_case("missiles")
                            && col_name.eq_ignore_ascii_case("CltParam5")
                            && cell.value == "`"
                        {
                            "diag.integer.backtick"
                        } else {
                            "diag.integer.invalid"
                        };
                        diags.push(i18n::localized_diagnostic(
                            locale,
                            message_key,
                            i18n::args([
                                ("value", serde_json::json!(cell.value)),
                                ("column", serde_json::json!(col_name)),
                            ]),
                            Diagnostic {
                                range: cell_range,
                                severity: Some(DiagnosticSeverity::WARNING),
                                source: Some("vector-lsp".into()),
                                ..Default::default()
                            },
                        ));
                    }
                }
                FieldTypeName::Float => {
                    if cell.value.parse::<f64>().is_err() {
                        diags.push(i18n::localized_diagnostic(
                            locale,
                            "diag.float.invalid",
                            i18n::args([
                                ("value", serde_json::json!(cell.value)),
                                ("column", serde_json::json!(col_name)),
                            ]),
                            Diagnostic {
                                range: cell_range,
                                severity: Some(DiagnosticSeverity::WARNING),
                                source: Some("vector-lsp".into()),
                                ..Default::default()
                            },
                        ));
                    }
                }
                FieldTypeName::Boolean => {
                    if cell.value.trim().is_empty() {
                        continue;
                    }
                    if is_confirmed_type29_boolean(file_stem, col_name) {
                        if parse_type29_boolean(&cell.value).is_none() {
                            diags.push(i18n::localized_diagnostic(
                                locale,
                                "diag.boolean.type29_invalid",
                                i18n::args([
                                    ("value", serde_json::json!(cell.value)),
                                    ("column", serde_json::json!(col_name)),
                                ]),
                                Diagnostic {
                                    range: cell_range,
                                    severity: Some(DiagnosticSeverity::WARNING),
                                    source: Some("vector-lsp".into()),
                                    ..Default::default()
                                },
                            ));
                        }
                        continue;
                    }
                    if cell.value != "0" && cell.value != "1" {
                        diags.push(i18n::localized_diagnostic(
                            locale,
                            "diag.boolean.invalid",
                            i18n::args([
                                ("value", serde_json::json!(cell.value)),
                                ("column", serde_json::json!(col_name)),
                            ]),
                            Diagnostic {
                                range: cell_range,
                                severity: Some(DiagnosticSeverity::WARNING),
                                source: Some("vector-lsp".into()),
                                ..Default::default()
                            },
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    diags
}

pub(crate) const HIT_SUMMON_MODE_CODES: [&str; 16] = [
    "DT", "NU", "WL", "GH", "A1", "A2", "BL", "SC", "S1", "S2", "S3", "S4", "DD", "KB", "xx", "RN",
];

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct HitSummonModeResult {
    pub parsed: u32,
    pub effective: u32,
    pub fallback_applied: bool,
    pub message: Option<String>,
}

pub(crate) fn is_hit_summon_mode_cell(
    file_stem: &str,
    doc: &DocumentData,
    row: &crate::document::Row,
    column: &str,
    game_version: Option<&str>,
) -> bool {
    if game_version != Some("3.2")
        || !file_stem.eq_ignore_ascii_case("missiles")
        || !column.eq_ignore_ascii_case("sHitPar2")
    {
        return false;
    }
    doc.headers
        .iter()
        .position(|header| header.eq_ignore_ascii_case("pSrvHitFunc"))
        .and_then(|index| row.cells.get(index))
        .is_some_and(|cell| cell.value == "6")
}

pub(crate) fn parse_type2_uint32(value: &str) -> u32 {
    let bytes = value.as_bytes();
    let negative = bytes.first() == Some(&b'-');
    let parsed = bytes
        .iter()
        .skip(usize::from(negative))
        .fold(0u32, |parsed, byte| {
            parsed
                .wrapping_mul(10)
                .wrapping_add(u32::from(*byte))
                .wrapping_sub(u32::from(b'0'))
        });
    if negative {
        parsed.wrapping_neg()
    } else {
        parsed
    }
}

pub(crate) fn hit_summon_mode_result(value: &str) -> HitSummonModeResult {
    let parsed = parse_type2_uint32(value);
    let shown_value = value.replace(' ', "␠").replace('\t', "⇥");
    if value.is_empty() {
        return HitSummonModeResult {
            parsed,
            effective: 0,
            fallback_applied: false,
            message: None,
        };
    }

    let digits_only = value.bytes().all(|byte| byte.is_ascii_digit());
    let normalized = if digits_only {
        let stripped = value.trim_start_matches('0');
        if stripped.is_empty() { "0" } else { stripped }
    } else {
        ""
    };
    let canonical_mode =
        digits_only && (normalized.len() < 2 || (normalized.len() == 2 && normalized <= "15"));
    if canonical_mode {
        return HitSummonModeResult {
            parsed,
            effective: parsed,
            fallback_applied: false,
            message: None,
        };
    }

    let fallback_applied = parsed > 15;
    let effective = if fallback_applied { 1 } else { parsed };
    let message = if digits_only
        || value
            .strip_prefix('-')
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
    {
        if fallback_applied {
            format!(
                "'{shown_value}' is outside the HitSummon mode range 0 through 15. The game uses 1 (NU). Enter a value from 0 through 15."
            )
        } else {
            format!(
                "'{shown_value}' does not directly name a HitSummon mode from 0 through 15. The game reads it as {effective} ({}). Replace it with the mode number you actually want.",
                HIT_SUMMON_MODE_CODES[effective as usize]
            )
        }
    } else if value == "NU" {
        "'NU' is not a numeric mode ID here. The game replaces it with 1 (NU). Use 1 for neutral mode."
            .to_string()
    } else if fallback_applied {
        format!(
            "'{shown_value}' is not a numeric mode ID here. The game replaces it with 1 (NU). Enter a value from 0 through 15."
        )
    } else {
        format!(
            "'{shown_value}' is not a numeric mode ID here. The game reads it as {effective} ({}). Replace it with the mode number you actually want from 0 through 15.",
            HIT_SUMMON_MODE_CODES[effective as usize]
        )
    };

    HitSummonModeResult {
        parsed,
        effective,
        fallback_applied,
        message: Some(message),
    }
}

fn hit_summon_message_key_args(
    value: &str,
    result: &HitSummonModeResult,
) -> (&'static str, serde_json::Map<String, serde_json::Value>) {
    let shown_value = mark_whitespace(value);
    let numeric = value.bytes().all(|byte| byte.is_ascii_digit())
        || value.strip_prefix('-').is_some_and(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        });
    let key = if numeric && result.fallback_applied {
        "diag.hit.out_of_range"
    } else if numeric {
        "diag.hit.noncanonical"
    } else if value == "NU" {
        "diag.hit.nu_literal"
    } else if result.fallback_applied {
        "diag.hit.non_numeric_outside"
    } else {
        "diag.hit.non_numeric"
    };
    (
        key,
        i18n::args([
            ("value", serde_json::json!(shown_value)),
            ("effective", serde_json::json!(result.effective)),
            (
                "mode",
                serde_json::json!(HIT_SUMMON_MODE_CODES[result.effective as usize]),
            ),
        ]),
    )
}

/// Properties `val#` fields are dispatcher inputs rather than unconditional
/// integers. Binary revalidation established that a null/unknown function
/// stops the slot walk and that only functions 21 and 36 consume the matching
/// property `val` input. Keep every other schema-int column on the normal
/// strict-decimal policy path.
fn integer_cell_is_consumed(
    file_stem: &str,
    doc: &DocumentData,
    row: &crate::document::Row,
    column: &str,
) -> bool {
    if !file_stem.eq_ignore_ascii_case("properties") {
        return true;
    }

    let Some(slot) = property_slot(column, "val") else {
        return true;
    };

    property_dispatch_func(doc, row, slot).is_some_and(|func| func == 21 || func == 36)
}

/// Properties `stat#` references are loaded for every row, but the property
/// dispatcher only reaches a slot when that slot and every earlier slot has a
/// non-null handler. This gate models reachability only; it does not infer the
/// per-handler meaning of the stat argument.
pub(crate) fn reference_cell_is_consumed(
    file_stem: &str,
    doc: &DocumentData,
    row: &crate::document::Row,
    column: &str,
) -> bool {
    if !file_stem.eq_ignore_ascii_case("properties") {
        return true;
    }
    let Some(slot) = property_slot(column, "stat") else {
        return true;
    };
    property_dispatch_func(doc, row, slot).is_some()
}

pub(crate) fn properties_stat_dispatch_func(
    file_stem: &str,
    doc: &DocumentData,
    row: &crate::document::Row,
    column: &str,
) -> Option<u8> {
    if !is_properties_stat_reference(file_stem, column) {
        return None;
    }
    property_dispatch_func(doc, row, property_slot(column, "stat")?)
}

fn property_dispatch_func(doc: &DocumentData, row: &crate::document::Row, slot: u8) -> Option<u8> {
    let mut current = None;
    for prior_slot in 1..=slot {
        let func_header = format!("func{prior_slot}");
        let func_column = doc
            .headers
            .iter()
            .position(|header| header.eq_ignore_ascii_case(&func_header))?;
        let func = row.cells.get(func_column)?.value.parse::<u8>().ok()?;
        if !((1..=25).contains(&func) || func == 36) {
            return None;
        }
        current = Some(func);
    }
    current
}

fn property_slot(column: &str, prefix: &str) -> Option<u8> {
    let leading = column.get(..prefix.len())?;
    if !leading.eq_ignore_ascii_case(prefix) {
        return None;
    }
    let suffix = column.get(prefix.len()..)?;
    let slot = suffix.parse::<u8>().ok()?;
    (1..=7).contains(&slot).then_some(slot)
}

pub(crate) fn is_confirmed_type29_boolean(file_stem: &str, column: &str) -> bool {
    file_stem.eq_ignore_ascii_case("missiles")
        && (column.eq_ignore_ascii_case("Explosion") || column.eq_ignore_ascii_case("NoMultiShot"))
}

pub(crate) fn is_monpet_consumestat_reference(file_stem: &str, column: &str) -> bool {
    if !file_stem.eq_ignore_ascii_case("monpet") {
        return false;
    }
    let Some(suffix) = column.get("consumestat".len()..) else {
        return false;
    };
    column
        .get(.."consumestat".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("consumestat"))
        && matches!(suffix, "1" | "2" | "3" | "4" | "5")
}

pub(crate) fn is_properties_stat_reference(file_stem: &str, column: &str) -> bool {
    file_stem.eq_ignore_ascii_case("properties") && property_slot(column, "stat").is_some()
}

fn is_3_2_skills_range_reference(
    file_stem: &str,
    column: &str,
    resolver: ReferenceResolver,
) -> bool {
    file_stem.eq_ignore_ascii_case("skills")
        && column.eq_ignore_ascii_case("range")
        && resolver == ReferenceResolver::Fixed4
}

fn edge_whitespace_counts(value: &str) -> (usize, usize) {
    let leading = value.chars().take_while(|ch| ch.is_whitespace()).count();
    let trailing = value
        .chars()
        .rev()
        .take_while(|ch| ch.is_whitespace())
        .count();
    (leading, trailing)
}

fn mark_whitespace(value: &str) -> String {
    value.replace(' ', "␠").replace('\t', "⇥")
}

/// Parse the binary-revalidated type-29 meaning without imposing an
/// unverified host-language integer limit. The binary evidence establishes
/// signed decimal zero vs nonzero, but not an i64 storage boundary.
pub(crate) fn parse_type29_boolean(value: &str) -> Option<bool> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(digits.bytes().any(|byte| byte != b'0'))
}

fn target_exists(
    schema: Option<&Schema>,
    symbols: &SymbolIndex,
    ref_file: &str,
    ref_col: &str,
    value: &str,
    resolver: ReferenceResolver,
) -> bool {
    symbols.contains_resolved(ref_file, ref_col, value, resolver)
        || schema
            .and_then(|s| s.enum_values_for_target(ref_file, ref_col))
            .map(|vals| match resolver {
                ReferenceResolver::AsciiCi => vals
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(value)),
                ReferenceResolver::Fixed4 => vals.iter().any(|candidate| {
                    crate::workspace::fixed4_key(candidate) == crate::workspace::fixed4_key(value)
                }),
            })
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

fn unique_target_columns(file_stem: &str, schema: Option<&Schema>) -> HashSet<String> {
    let stem = file_stem.to_lowercase();
    schema
        .and_then(|schema| schema.get_file(&stem))
        .map(|file| {
            file.fields
                .iter()
                .filter(|field| field.unique)
                .flat_map(|field| {
                    std::iter::once(field.name.to_lowercase())
                        .chain(field.alt_names.iter().map(|name| name.to_lowercase()))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{FieldType, SchemaField, SchemaFile, find_loader};
    use crate::source_selection::SourceKind;

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
                        resolver: ReferenceResolver::default(),
                        unknown_policy: ReferenceUnknownPolicy::default(),
                    }),
                    alt_names: vec![],
                    append_field: None,
                    table: None,
                    unique: false,
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
    fn properties_val_diagnostics_follow_the_active_dispatch_slots() {
        let mut schema = Schema::default();
        let int_field = |name: &str| SchemaField {
            name: name.to_string(),
            description: None,
            field_type: Some(FieldType {
                type_name: FieldTypeName::Int,
                data_length: 0,
                mem_size: 0,
                file: None,
                field: None,
                resolver: ReferenceResolver::default(),
                unknown_policy: ReferenceUnknownPolicy::default(),
            }),
            alt_names: vec![],
            append_field: None,
            table: None,
            unique: false,
        };
        schema.files.insert(
            "properties".to_string(),
            SchemaFile {
                fields: vec![
                    int_field("func1"),
                    int_field("func2"),
                    int_field("func7"),
                    int_field("val1"),
                    int_field("val2"),
                    int_field("val7"),
                    int_field("uiRangeType"),
                ],
                ..Default::default()
            },
        );

        let document = DocumentData::parse(
            "func1\tfunc2\tfunc7\tval1\tval2\tval7\tuiRangeType\n17\t\t\tunused\tunused\t# Upgrades\tbad\n21\t21\t\tbad1\tbad2\tunused\t0\n21\t0\t36\tbad1\tunused\tunused\t0\n",
            '\t',
        );
        let diagnostics =
            validate_document("properties", &document, Some(&schema), &SymbolIndex::new());

        assert_eq!(diagnostics.len(), 4, "{diagnostics:#?}");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.range == Range::new(Position::new(1, 30), Position::new(1, 33))
                && diagnostic.message.contains("uiRangeType")
        }));
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.message.contains("for 'val1'"))
                .count(),
            2
        );
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.message.contains("for 'val2'"))
                .count(),
            1
        );
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| !diagnostic.message.contains("for 'val7'")),
            "a blank func slot, or an earlier null slot, makes val7 unreachable"
        );
    }

    #[test]
    fn properties_stat_references_follow_dispatch_reachability_and_report_active_misses() {
        let mut schema = Schema::default();
        let int_field = |name: &str| SchemaField {
            name: name.to_string(),
            description: None,
            field_type: Some(FieldType {
                type_name: FieldTypeName::Int,
                data_length: 0,
                mem_size: 0,
                file: None,
                field: None,
                resolver: ReferenceResolver::default(),
                unknown_policy: ReferenceUnknownPolicy::default(),
            }),
            alt_names: vec![],
            append_field: None,
            table: None,
            unique: false,
        };
        let stat_field = |name: &str| SchemaField {
            name: name.to_string(),
            description: None,
            field_type: Some(FieldType {
                type_name: FieldTypeName::Reference,
                data_length: 47,
                mem_size: 16,
                file: Some("itemstatcost".to_string()),
                field: Some("Stat".to_string()),
                resolver: ReferenceResolver::AsciiCi,
                unknown_policy: ReferenceUnknownPolicy::Warning,
            }),
            alt_names: vec![],
            append_field: None,
            table: None,
            unique: false,
        };
        schema.files.insert(
            "properties".to_string(),
            SchemaFile {
                fields: vec![
                    int_field("func1"),
                    stat_field("stat1"),
                    int_field("func2"),
                    stat_field("stat2"),
                ],
                ..Default::default()
            },
        );

        let targets = HashSet::from([("itemstatcost".to_string(), "stat".to_string())]);
        let mut symbols = SymbolIndex::new();
        symbols.index_effective_document(
            None,
            "itemstatcost",
            &DocumentData::parse("Stat\nvalid\n", '\t'),
            &targets,
            SourceKind::Bundled,
            Some("3.2"),
        );
        let source = DocumentData::parse(
            "func1\tstat1\tfunc2\tstat2\n\
17\tunknown1\t\t\n\
\tunknown_blank\t\t\n\
0\tunknown_zero\t\t\n\
17\tvalid\t0\tunknown2\n\
26\tignored1\t17\tunknown2\n\
17\tvalid\t26\tunknown2\n\
17\tvalid\t1\tunknown2\n\
17\tVaLiD\t\t\n",
            '\t',
        );

        let diagnostics = validate_document("properties", &source, Some(&schema), &symbols);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:#?}");
        assert_eq!(
            diagnostics[0].message,
            "Unknown stat name 'unknown1'. This property has no effect. Use the exact Stat name from itemstatcost.txt."
        );
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(
            diagnostics[0].range,
            Range::new(Position::new(1, 3), Position::new(1, 11))
        );
        assert_eq!(
            diagnostics[1].message,
            "Unknown stat name 'unknown2'. Use the exact Stat name from itemstatcost.txt."
        );
        assert_eq!(diagnostics[1].severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(
            diagnostics[1].range,
            Range::new(Position::new(7, 11), Position::new(7, 19))
        );
        assert_eq!(
            diagnostics[0].data.as_ref().unwrap()["scope"],
            "properties-stat"
        );
        assert!(reference_cell_is_consumed(
            "properties",
            &source,
            &source.rows[0],
            "stat1"
        ));
        assert!(!reference_cell_is_consumed(
            "properties",
            &source,
            &source.rows[1],
            "stat1"
        ));
        assert_eq!(
            properties_stat_dispatch_func("properties", &source, &source.rows[6], "stat2"),
            Some(1)
        );
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
                        resolver: ReferenceResolver::default(),
                        unknown_policy: ReferenceUnknownPolicy::default(),
                    }),
                    alt_names: vec![],
                    append_field: None,
                    table: None,
                    unique: false,
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

    #[test]
    fn stock_numeric_policy_messages_distinguish_storage_from_parser_rejection() {
        let document = DocumentData::parse("CltParam5\tExplosion\tNoMultiShot\n`\t2\t2\n", '\t');
        let mut schema = Schema::default();
        let field = |name: &str, type_name| SchemaField {
            name: name.to_string(),
            description: None,
            field_type: Some(FieldType {
                type_name,
                data_length: 0,
                mem_size: 0,
                file: None,
                field: None,
                resolver: ReferenceResolver::default(),
                unknown_policy: ReferenceUnknownPolicy::default(),
            }),
            alt_names: vec![],
            append_field: None,
            table: None,
            unique: false,
        };
        schema.files.insert(
            "missiles".to_string(),
            SchemaFile {
                fields: vec![
                    field("CltParam5", FieldTypeName::Int),
                    field("Explosion", FieldTypeName::Boolean),
                    field("NoMultiShot", FieldTypeName::Boolean),
                ],
                ..Default::default()
            },
        );

        let diagnostics =
            validate_document("missiles", &document, Some(&schema), &SymbolIndex::new());
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].message,
            "'`' is not written as a normal integer. The game converts it to 48. Replace it with the number you actually want."
        );
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity == Some(DiagnosticSeverity::WARNING))
        );
    }

    #[test]
    fn type29_boolean_accepts_zero_and_every_signed_decimal_nonzero() {
        for (value, expected) in [
            ("0", false),
            ("-0", false),
            ("000", false),
            ("1", true),
            ("2", true),
            ("3", true),
            ("999999", true),
            ("184467440737095516160000", true),
            ("-1", true),
            ("-987654321", true),
        ] {
            assert_eq!(parse_type29_boolean(value), Some(expected), "{value}");
        }
        for value in ["", "+1", "1.0", "1x", "true", " 1"] {
            assert_eq!(parse_type29_boolean(value), None, "{value}");
        }

        let document = DocumentData::parse(
            "Explosion\tNoMultiShot\n0\t1\n2\t3\n999999\t-1\n184467440737095516160000\t-0\ntrue\t+1\n",
            '\t',
        );
        let mut schema = Schema::default();
        let boolean_field = |name: &str| SchemaField {
            name: name.to_string(),
            description: None,
            field_type: Some(FieldType {
                type_name: FieldTypeName::Boolean,
                data_length: 0,
                mem_size: 0,
                file: None,
                field: None,
                resolver: ReferenceResolver::default(),
                unknown_policy: ReferenceUnknownPolicy::default(),
            }),
            alt_names: vec![],
            append_field: None,
            table: None,
            unique: false,
        };
        schema.files.insert(
            "missiles".to_string(),
            SchemaFile {
                fields: vec![boolean_field("Explosion"), boolean_field("NoMultiShot")],
                ..Default::default()
            },
        );
        let diagnostics =
            validate_document("missiles", &document, Some(&schema), &SymbolIndex::new());
        assert_eq!(diagnostics.len(), 2, "{diagnostics:#?}");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("'true'"))
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("'+1'"))
        );
    }

    #[test]
    fn hit_summon_mode_uses_type2_values_only_for_3_2_server_hit_parameter_two() {
        for (value, parsed) in [
            ("", 0),
            ("0", 0),
            ("1", 1),
            ("15", 15),
            ("16", 16),
            ("NU", 337),
            ("nu", 689),
            ("NU ", 3354),
            (" NU", 4_294_966_033),
            ("NUxx", 34_492),
            ("-1", u32::MAX),
            (":", 10),
        ] {
            assert_eq!(parse_type2_uint32(value), parsed, "{value:?}");
        }
        for value in ["", "0", "1", "15"] {
            assert_eq!(hit_summon_mode_result(value).message, None, "{value:?}");
        }
        for value in ["16", "-1", "NU", "nu", "NU ", " NU", "NUxx"] {
            let result = hit_summon_mode_result(value);
            assert_eq!(result.effective, 1, "{value:?}");
            assert!(result.message.is_some(), "{value:?}");
        }
        assert_eq!(
            hit_summon_mode_result("NU").message.as_deref(),
            Some(
                "'NU' is not a numeric mode ID here. The game replaces it with 1 (NU). Use 1 for neutral mode."
            )
        );
        let malformed_but_in_range = hit_summon_mode_result(":");
        assert_eq!(malformed_but_in_range.effective, 10);
        assert!(!malformed_but_in_range.fallback_applied);
        assert!(
            malformed_but_in_range
                .message
                .as_deref()
                .is_some_and(|message| message.contains("game reads it as 10 (S3)"))
        );
        assert!(
            hit_summon_mode_result("NU ")
                .message
                .as_deref()
                .is_some_and(|message| message.contains("'NU␠'"))
        );
        assert!(
            hit_summon_mode_result(" NU")
                .message
                .as_deref()
                .is_some_and(|message| message.contains("'␠NU'"))
        );

        let document = DocumentData::parse(
            "missile\tpSrvHitFunc\tsHitPar2\tcHitPar2\nblank\t6\t\tNU\ndt\t6\t0\tNU\nnu-id\t6\t1\tNU\nrn\t6\t15\tNU\nhigh\t6\t16\tNU\nnegative\t6\t-1\tNU\ntext\t6\tNU\tNU\nother\t5\tNU\tNU",
            '\t',
        );
        let mut schema = Schema::default();
        let int_field = |name: &str| SchemaField {
            name: name.to_string(),
            description: None,
            field_type: Some(FieldType {
                type_name: FieldTypeName::Int,
                data_length: 0,
                mem_size: 0,
                file: None,
                field: None,
                resolver: ReferenceResolver::default(),
                unknown_policy: ReferenceUnknownPolicy::default(),
            }),
            alt_names: vec![],
            append_field: None,
            table: None,
            unique: false,
        };
        schema.files.insert(
            "missiles".to_string(),
            SchemaFile {
                fields: vec![
                    int_field("pSrvHitFunc"),
                    int_field("sHitPar2"),
                    int_field("cHitPar2"),
                ],
                ..Default::default()
            },
        );

        let diagnostics = validate_document_for_version(
            "missiles",
            &document,
            Some(&schema),
            &SymbolIndex::new(),
            Some("3.2"),
        );
        let hit_summon = diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic
                    .data
                    .as_ref()
                    .and_then(|data| data.get("rule"))
                    .and_then(|value| value.as_str())
                    == Some("hit-summon-mode")
            })
            .collect::<Vec<_>>();
        assert_eq!(hit_summon.len(), 3, "{diagnostics:#?}");
        assert_eq!(
            hit_summon[2].message,
            "'NU' is not a numeric mode ID here. The game replaces it with 1 (NU). Use 1 for neutral mode."
        );
        assert_eq!(hit_summon[2].severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(hit_summon[2].range.start.line, 7);
        assert_eq!(
            hit_summon[2].range.end.character - hit_summon[2].range.start.character,
            2
        );
        assert!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.range.start.line == 8)
                .all(|diagnostic| !diagnostic.message.contains("HitSummon"))
        );
        assert!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.range.start.character > 0)
                .filter(|diagnostic| diagnostic.message.contains("cHitPar2"))
                .all(|diagnostic| !diagnostic.message.contains("mode"))
        );

        let diagnostics_3_1 = validate_document_for_version(
            "missiles",
            &document,
            Some(&schema),
            &SymbolIndex::new(),
            Some("3.1"),
        );
        assert!(
            diagnostics_3_1
                .iter()
                .all(|diagnostic| diagnostic.data.is_none())
        );
    }

    #[test]
    fn fixed4_reference_diagnostics_use_first_four_bytes_and_whole_cell_ranges() {
        let target = DocumentData::parse("Code\nstaf\nring\n", '\t');
        let targets = HashSet::from([("itemtypes".to_string(), "code".to_string())]);
        let mut symbols = SymbolIndex::new();
        symbols.index_effective_document(
            None,
            "itemtypes",
            &target,
            &targets,
            SourceKind::Bundled,
            Some("3.2"),
        );
        let mut schema = Schema::default();
        schema.files.insert(
            "magicprefix".to_string(),
            SchemaFile {
                fields: vec![SchemaField {
                    name: "itype1".to_string(),
                    description: None,
                    field_type: Some(FieldType {
                        type_name: FieldTypeName::Reference,
                        data_length: 4,
                        mem_size: 4,
                        file: Some("itemtypes".to_string()),
                        field: Some("Code".to_string()),
                        resolver: ReferenceResolver::Fixed4,
                        unknown_policy: ReferenceUnknownPolicy::Warning,
                    }),
                    alt_names: vec![],
                    append_field: None,
                    table: None,
                    unique: false,
                }],
                ..Default::default()
            },
        );
        let source = DocumentData::parse("itype1\nstaff\nring  \nStaff\n", '\t');

        let diagnostics = validate_document("magicprefix", &source, Some(&schema), &symbols);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].range,
            Range::new(Position::new(3, 0), Position::new(3, 5))
        );
        assert_eq!(
            diagnostics[0].message,
            "Unknown code 'Staff'. The game reads this code as 'Staf'. Check the four-character code and letter case."
        );
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::WARNING));
    }

    #[test]
    fn fixed4_reference_diagnostics_make_whitespace_visible() {
        let target = DocumentData::parse("Code\nstaf\n", '\t');
        let targets = HashSet::from([("itemtypes".to_string(), "code".to_string())]);
        let mut symbols = SymbolIndex::new();
        symbols.index_effective_document(
            None,
            "itemtypes",
            &target,
            &targets,
            SourceKind::Bundled,
            Some("3.2"),
        );
        let mut schema = Schema::default();
        schema.files.insert(
            "magicprefix".to_string(),
            SchemaFile {
                fields: vec![SchemaField {
                    name: "itype1".to_string(),
                    description: None,
                    field_type: Some(FieldType {
                        type_name: FieldTypeName::Reference,
                        data_length: 4,
                        mem_size: 4,
                        file: Some("itemtypes".to_string()),
                        field: Some("Code".to_string()),
                        resolver: ReferenceResolver::Fixed4,
                        unknown_policy: ReferenceUnknownPolicy::Warning,
                    }),
                    alt_names: vec![],
                    append_field: None,
                    table: None,
                    unique: false,
                }],
                ..Default::default()
            },
        );
        let source = DocumentData::parse("itype1\n Staff\n", '\t');

        let diagnostics = validate_document("magicprefix", &source, Some(&schema), &symbols);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].message,
            "Unknown code '␠Staff'. The game reads this code as '␠Sta'. Check the four-character code and letter case. ␠ = space."
        );
    }

    #[test]
    fn monpet_consumestat_miss_uses_plain_user_message_and_keeps_structured_evidence() {
        let target = DocumentData::parse(
            "Stat\nstrength\nitem_addsksrc\nitem_sksrc\nitem_allsksrc\n",
            '\t',
        );
        let targets = HashSet::from([("itemstatcost".to_string(), "stat".to_string())]);
        let mut symbols = SymbolIndex::new();
        symbols.index_effective_document(
            None,
            "itemstatcost",
            &target,
            &targets,
            SourceKind::Bundled,
            Some("3.1"),
        );

        let mut schema = Schema::default();
        schema.files.insert(
            "monpet".to_string(),
            SchemaFile {
                fields: vec![SchemaField {
                    name: "consumestat1".to_string(),
                    description: None,
                    field_type: Some(FieldType {
                        type_name: FieldTypeName::Reference,
                        data_length: 47,
                        mem_size: 16,
                        file: Some("itemstatcost".to_string()),
                        field: Some("Stat".to_string()),
                        resolver: ReferenceResolver::AsciiCi,
                        unknown_policy: ReferenceUnknownPolicy::Warning,
                    }),
                    alt_names: vec![],
                    append_field: None,
                    table: None,
                    unique: false,
                }],
                ..Default::default()
            },
        );
        let source = DocumentData::parse(
            "consumestat1\nStReNgTh\n strength\nstrength \nitem_addsksrc _tab\nitem_sksrc ongethit\nitem_sksrc onattack\nitem_allsksrc s\n",
            '\t',
        );

        let diagnostics = validate_document("monpet", &source, Some(&schema), &symbols);
        assert_eq!(diagnostics.len(), 6, "{diagnostics:#?}");
        assert!(diagnostics.iter().all(|diagnostic| {
            diagnostic.severity == Some(DiagnosticSeverity::WARNING)
                && diagnostic.message.contains("Unknown stat name")
                && diagnostic
                    .message
                    .contains("This Consume bonus is not applied")
                && diagnostic
                    .message
                    .contains("other Consume slots still work")
                && diagnostic.message.contains("Use the exact Stat name")
                && !diagnostic.message.contains("0xFFFF")
                && !diagnostic.message.contains("loader")
        }));
        let composite = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.message.contains("item_addsksrc _tab"))
            .expect("composite whole-key miss");
        assert_eq!(
            composite.range,
            Range::new(Position::new(4, 0), Position::new(4, 18))
        );
        assert_eq!(
            composite.message,
            "Unknown stat name 'item_addsksrc _tab'. This Consume bonus is not applied; other Consume slots still work. Use the exact Stat name from itemstatcost.txt."
        );
        let data = composite.data.as_ref().expect("consumer metadata");
        assert_eq!(data["scope"], "monpet-consumestat");
        assert_eq!(data["namespace"], "ItemStatCost.Stat");
        assert_eq!(data["lookup"], "whole-cell ASCII case-insensitive no-trim");
        assert_eq!(data["resolverResult"], -1);
        assert_eq!(data["storedValue"], 65535);
        assert_eq!(data["runtimeEffect"], "Consume skips only this slot");
        assert!(is_monpet_consumestat_reference("monpet", "ConsumeStat5"));
        assert!(!is_monpet_consumestat_reference("monpet", "consumestat6"));
        assert!(!is_monpet_consumestat_reference("items", "consumestat1"));
    }

    #[test]
    fn loaded_3_2_magicsuffix_etype_accepts_space_padded_fixed4_reference() {
        let contrib = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("contrib")
            .join("d2rdoc");
        let schema_dir = contrib.join("3.2").join("schema");
        let loader = find_loader("d2rdoc", "3.2".to_string(), Some(contrib)).unwrap();
        let schema = loader.load(Some(&schema_dir)).unwrap();
        let field = schema
            .find_field("magicsuffix", "etype1")
            .expect("loaded magicsuffix.etype1");
        assert_eq!(
            field.field_type.as_ref().map(|field| field.resolver),
            Some(ReferenceResolver::Fixed4)
        );

        let target = DocumentData::parse("Code\nring\n", '\t');
        let targets = HashSet::from([("itemtypes".to_string(), "code".to_string())]);
        let mut symbols = SymbolIndex::new();
        symbols.index_effective_document(
            None,
            "itemtypes",
            &target,
            &targets,
            SourceKind::Bundled,
            Some("3.2"),
        );
        let source = DocumentData::parse("Name\tetype1\nrow\tring  \n", '\t');

        assert!(
            validate_document("magicsuffix", &source, Some(&schema), &symbols).is_empty(),
            "ring plus trailing spaces must pack to the same four bytes as ring"
        );
    }

    #[test]
    fn loaded_3_2_skills_range_uses_scoped_space_padded_fixed4_codes() {
        let contrib = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("contrib")
            .join("d2rdoc");
        let schema_3_2_dir = contrib.join("3.2").join("schema");
        let schema_3_2 = find_loader("d2rdoc", "3.2".to_string(), Some(contrib.clone()))
            .unwrap()
            .load(Some(&schema_3_2_dir))
            .unwrap();
        let field = schema_3_2
            .find_field("skills", "range")
            .expect("loaded skills.range");
        assert_eq!(
            field.field_type.as_ref().map(|field| field.resolver),
            Some(ReferenceResolver::Fixed4)
        );

        let source = DocumentData::parse(
            "skill|range\nplain|rng\npadded|rng \nlong|rng suffix\nleading| rng\nupper|RNG\ntab|rng\t\nunknown|bad\n",
            '|',
        );
        let diagnostics =
            validate_document("skills", &source, Some(&schema_3_2), &SymbolIndex::new());
        assert_eq!(diagnostics.len(), 4, "{diagnostics:#?}");
        assert!(diagnostics.iter().all(|diagnostic| {
            diagnostic.message == "Unknown range code. Use one of: none, h2h, rng, both, loc."
                && diagnostic.severity == Some(DiagnosticSeverity::WARNING)
        }));

        let schema_3_1_dir = contrib.join("3.1").join("schema");
        let schema_3_1 = find_loader("d2rdoc", "3.1".to_string(), Some(contrib))
            .unwrap()
            .load(Some(&schema_3_1_dir))
            .unwrap();
        assert_eq!(
            schema_3_1
                .find_field("skills", "range")
                .and_then(|field| field.field_type.as_ref())
                .map(|field| field.resolver),
            Some(ReferenceResolver::AsciiCi),
            "the custom 3.2 descriptor must not be applied to 3.1"
        );
    }

    #[test]
    fn loaded_3_2_properties_schema_skips_unreachable_val7_but_not_other_ints() {
        let contrib = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("contrib")
            .join("d2rdoc");
        let schema_dir = contrib.join("3.2").join("schema");
        let loader = find_loader("d2rdoc", "3.2".to_string(), Some(contrib)).unwrap();
        let schema = loader.load(Some(&schema_dir)).unwrap();
        let source = DocumentData::parse(
            "func1\tfunc2\tfunc3\tfunc4\tfunc5\tfunc6\tfunc7\tval7\tuiRangeType\n17\t\t\t\t\t\t\t# Upgrades\tbad\n",
            '\t',
        );

        let diagnostics =
            validate_document("properties", &source, Some(&schema), &SymbolIndex::new());
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert!(diagnostics[0].message.contains("for 'uiRangeType'"));
        assert!(!diagnostics[0].message.contains("# Upgrades"));
    }

    #[test]
    fn duplicate_diagnostics_only_apply_to_explicit_unique_fields() {
        let field = |name: &str, unique| SchemaField {
            name: name.to_string(),
            description: None,
            field_type: Some(FieldType {
                type_name: FieldTypeName::Text,
                data_length: 0,
                mem_size: 0,
                file: None,
                field: None,
                resolver: ReferenceResolver::default(),
                unknown_policy: ReferenceUnknownPolicy::default(),
            }),
            alt_names: vec![],
            append_field: None,
            table: None,
            unique,
        };
        let mut schema = Schema::default();
        schema.files.insert(
            "items".to_string(),
            SchemaFile {
                fields: vec![field("policy", false), field("identity", true)],
                ..Default::default()
            },
        );
        let document = DocumentData::parse(
            "policy\tidentity\nrepeat\tfirst\nrepeat\tsecond\nother\tSECOND\n",
            '\t',
        );

        let diagnostics = validate_document("items", &document, Some(&schema), &SymbolIndex::new());
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(
            diagnostics[0].range,
            Range::new(Position::new(3, 6), Position::new(3, 12))
        );
        assert!(
            diagnostics[0]
                .message
                .contains("unique key column 'identity'")
        );
        assert!(!diagnostics[0].message.contains("policy"));
    }
}
