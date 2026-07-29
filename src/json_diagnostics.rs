//! d2rlint-compatible diagnostics for localization string JSON files.
//!
//! This module deliberately has a narrower source policy than the normal TXT
//! reference resolver.  A scope exists only when a primary TXT document can be
//! traced to `<data>/global/excel` and that same physical `<data>` root contains
//! at least one top-level `local/lng/strings/*.json` file.  Explicit references,
//! bundled data and fallback tables are never inputs.

use serde::de::IgnoredAny;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Url};

use crate::document::{DocumentData, utf16_len};
use crate::i18n::{self, Locale};
use crate::settings::{JsonDiagnosticRules, JsonRuleAction};

const REQUIRED_STRING_FIELDS: &[&str] = &[
    "id", "Key", "enUS", "zhTW", "deDE", "esES", "frFR", "itIT", "koKR", "plPL", "esMX", "jaJP",
    "ptBR", "ruRU", "zhCN",
];

/// Verified ordinary string-table load order in the D2R runtime. Unknown
/// top-level JSON files remain observable for mod diagnostics, but sort after
/// the runtime-owned tables because the game does not enumerate them here.
const STRING_JSON_LOAD_ORDER: &[&str] = &[
    "bnet.json",
    "item-gems.json",
    "item-modifiers.json",
    "item-nameaffixes.json",
    "item-names.json",
    "item-runes.json",
    "keybinds.json",
    "levels.json",
    "mercenaries.json",
    "monsters.json",
    "npcs.json",
    "objects.json",
    "quests.json",
    "shrines.json",
    "skills.json",
    "ui.json",
    "ui-controller.json",
    "vo.json",
    "commands.json",
];

/// Runtime `LoadWorkspace` record types in d2rlint. `hiredesc.txt` is not in
/// this list because its class and loader are commented out upstream;
/// `moncalc.txt` has no record type there either.
const D2RLINT_EXCEL_FILES: &[&str] = &[
    "actinfo.txt",
    "armor.txt",
    "armtype.txt",
    "automagic.txt",
    "automap.txt",
    "belts.txt",
    "bodylocs.txt",
    "books.txt",
    "charstats.txt",
    "colors.txt",
    "compcode.txt",
    "composit.txt",
    "cubemain.txt",
    "cubemod.txt",
    "difficultylevels.txt",
    "elemtypes.txt",
    "events.txt",
    "experience.txt",
    "gamble.txt",
    "gems.txt",
    "hireling.txt",
    "hitclass.txt",
    "inventory.txt",
    "itemratio.txt",
    "itemstatcost.txt",
    "itemtypes.txt",
    "itemuicategories.txt",
    "levelgroups.txt",
    "levels.txt",
    "lowqualityitems.txt",
    "lvlmaze.txt",
    "lvlprest.txt",
    "lvlsub.txt",
    "lvltypes.txt",
    "lvlwarp.txt",
    "magicprefix.txt",
    "magicsuffix.txt",
    "misc.txt",
    "misscalc.txt",
    "missiles.txt",
    "monai.txt",
    "monequip.txt",
    "monlvl.txt",
    "monmode.txt",
    "monpet.txt",
    "monplace.txt",
    "monpreset.txt",
    "monprop.txt",
    "monseq.txt",
    "monsounds.txt",
    "monstats.txt",
    "monstats2.txt",
    "montype.txt",
    "monumod.txt",
    "npc.txt",
    "objects.txt",
    "objgroup.txt",
    "objmode.txt",
    "objpreset.txt",
    "objtype.txt",
    "overlay.txt",
    "pettype.txt",
    "playerclass.txt",
    "plrmode.txt",
    "plrtype.txt",
    "properties.txt",
    "propertygroups.txt",
    "qualityitems.txt",
    "rareprefix.txt",
    "raresuffix.txt",
    "runes.txt",
    "runeworduicategories.txt",
    "setitems.txt",
    "sets.txt",
    "shrines.txt",
    "skillcalc.txt",
    "skilldesc.txt",
    "skills.txt",
    "soundenviron.txt",
    "sounds.txt",
    "states.txt",
    "storepage.txt",
    "superuniques.txt",
    "treasureclassex.txt",
    "uniqueappellation.txt",
    "uniqueitems.txt",
    "uniqueprefix.txt",
    "uniquesuffix.txt",
    "wanderingmon.txt",
    "weapons.txt",
];

/// These runtime record sets are not loaded by d2rlint when its selected
/// version is `legacy`, so they cannot contribute Json/KeyUsage evidence.
const D2RLINT_LEGACY_EXCLUDED_EXCEL_FILES: &[&str] = &[
    "actinfo.txt",
    "itemuicategories.txt",
    "levelgroups.txt",
    "monpet.txt",
    "objpreset.txt",
    "propertygroups.txt",
    "runeworduicategories.txt",
    "wanderingmon.txt",
];

#[derive(Clone)]
pub struct PrimaryTxtDocument {
    pub path: PathBuf,
    pub document: Arc<DocumentData>,
}

#[derive(Debug)]
pub struct JsonDiagnosticBatch {
    pub uri: Url,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Default)]
pub struct JsonDiagnosticReport {
    /// Includes every physical top-level JSON file, even an invalid file that
    /// d2rlint skips, so a caller can clear diagnostics published previously.
    pub batches: Vec<JsonDiagnosticBatch>,
    pub warnings: Vec<JsonLogWarning>,
}

/// Product-authored JSON analysis logs retain a stable key and named data
/// until the backend renders them for the current client locale.
#[derive(Debug, PartialEq)]
pub struct JsonLogWarning {
    pub message_key: &'static str,
    pub message_args: Map<String, Value>,
}

impl JsonLogWarning {
    fn new(
        message_key: &'static str,
        message_args: impl IntoIterator<Item = (&'static str, Value)>,
    ) -> Self {
        Self {
            message_key,
            message_args: i18n::args(message_args),
        }
    }
}

/// Unsaved localization JSON buffers supplied by the editor, keyed by the
/// normalized physical path they shadow. Physical files still establish the
/// scope; this map only replaces their bytes while a matching tab is open.
pub type OpenJsonSources = HashMap<String, String>;

/// Selects the rule dependency set invalidated by an input event.
///
/// A localization string JSON change can affect every rule. TXT and layout
/// changes only affect Json/KeyUsage, allowing the caller to retain cached
/// DuplicateIds and StringFormat results.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JsonAnalysisTrigger {
    #[default]
    All,
    KeyUsageOnly,
}

/// Selects the d2rlint workspace record sets that may prove a localization key
/// is used. Unknown versions use the modern/resurrected superset rather than
/// silently applying legacy exclusions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JsonEvidenceProfile {
    Legacy,
    #[default]
    ResurrectedOrUnknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Span {
    start: usize,
    end: usize,
}

#[derive(Clone, Debug)]
struct Entry {
    fields: HashMap<String, (Value, Span)>,
    span: Span,
}

impl Entry {
    fn field(&self, name: &str) -> Option<&(Value, Span)> {
        self.fields.get(name)
    }

    fn diagnostic_span(&self, preferred_field: &str) -> Span {
        self.field(preferred_field)
            .map(|(_, span)| *span)
            .unwrap_or_else(|| one_char_span(self.span))
    }
}

#[derive(Debug)]
struct StringFile {
    path: PathBuf,
    uri: Url,
    display_stem: String,
    source: String,
    entries: Vec<Entry>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug)]
struct LoadedStringRecord {
    file: String,
    entry: usize,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum JsMapKey {
    Null,
    Bool(bool),
    Number(u64),
    String(String),
    /// JavaScript Maps compare objects by identity. JSON.parse creates a fresh
    /// array/object value for each property occurrence, so composite values do
    /// not collide merely because their serialized content is equal.
    Composite(usize),
}

/// Analyze all physical mod scopes represented by primary TXT documents.
/// File I/O is synchronous so callers should run this function on a blocking
/// worker. Results and diagnostics are deterministic by normalized path.
#[cfg(test)]
pub fn analyze(primary_documents: Vec<PrimaryTxtDocument>) -> JsonDiagnosticReport {
    analyze_with_rules(
        primary_documents,
        JsonDiagnosticRules::default(),
        JsonAnalysisTrigger::All,
    )
}

/// Analyze only the configured rules invalidated by `trigger`.
///
/// `KeyUsageOnly` reports contain Json/KeyUsage diagnostics, or a single
/// Json/Syntax diagnostic when the source cannot be parsed. A caller publishing
/// aggregate diagnostics must merge them with its cached semantic results
/// before replacing diagnostics for a JSON URI.
#[cfg(test)]
pub fn analyze_with_rules(
    primary_documents: Vec<PrimaryTxtDocument>,
    rules: JsonDiagnosticRules,
    trigger: JsonAnalysisTrigger,
) -> JsonDiagnosticReport {
    analyze_with_rules_and_profile(
        primary_documents,
        rules,
        trigger,
        JsonEvidenceProfile::ResurrectedOrUnknown,
    )
}

#[cfg(test)]
pub fn analyze_with_rules_and_profile(
    primary_documents: Vec<PrimaryTxtDocument>,
    rules: JsonDiagnosticRules,
    trigger: JsonAnalysisTrigger,
    evidence_profile: JsonEvidenceProfile,
) -> JsonDiagnosticReport {
    analyze_with_rules_profile_and_open_json(
        primary_documents,
        rules,
        trigger,
        evidence_profile,
        OpenJsonSources::new(),
    )
}

#[cfg(test)]
pub fn analyze_with_rules_profile_and_open_json(
    primary_documents: Vec<PrimaryTxtDocument>,
    rules: JsonDiagnosticRules,
    trigger: JsonAnalysisTrigger,
    evidence_profile: JsonEvidenceProfile,
    open_json_sources: OpenJsonSources,
) -> JsonDiagnosticReport {
    analyze_with_rules_profile_and_open_json_localized(
        primary_documents,
        rules,
        trigger,
        evidence_profile,
        open_json_sources,
        Locale::EnUs,
    )
}

pub fn analyze_with_rules_profile_and_open_json_localized(
    primary_documents: Vec<PrimaryTxtDocument>,
    rules: JsonDiagnosticRules,
    trigger: JsonAnalysisTrigger,
    evidence_profile: JsonEvidenceProfile,
    open_json_sources: OpenJsonSources,
    locale: Locale,
) -> JsonDiagnosticReport {
    let mut grouped: HashMap<String, (PathBuf, Vec<PrimaryTxtDocument>)> = HashMap::new();
    for input in primary_documents {
        let Some(data_root) = data_root_from_excel_txt(&input.path) else {
            continue;
        };
        let identity = local_path_identity(&data_root);
        grouped
            .entry(identity)
            .or_insert_with(|| (data_root, Vec::new()))
            .1
            .push(input);
    }

    let mut scopes = grouped.into_iter().collect::<Vec<_>>();
    scopes.sort_by(|left, right| left.0.cmp(&right.0));

    let mut report = JsonDiagnosticReport::default();
    for (_identity, (data_root, mut txt_documents)) in scopes {
        txt_documents.sort_by(|left, right| {
            local_path_identity(&left.path).cmp(&local_path_identity(&right.path))
        });
        let Some(mut scope_report) = analyze_scope(
            &data_root,
            &txt_documents,
            rules,
            trigger,
            evidence_profile,
            &open_json_sources,
            locale,
        ) else {
            continue;
        };
        report.batches.append(&mut scope_report.batches);
        report.warnings.append(&mut scope_report.warnings);
    }
    report
        .batches
        .sort_by(|left, right| left.uri.as_str().cmp(right.uri.as_str()));
    report
}

fn analyze_scope(
    data_root: &Path,
    txt_documents: &[PrimaryTxtDocument],
    rules: JsonDiagnosticRules,
    trigger: JsonAnalysisTrigger,
    evidence_profile: JsonEvidenceProfile,
    open_json_sources: &OpenJsonSources,
    locale: Locale,
) -> Option<JsonDiagnosticReport> {
    let strings_dir = data_root.join("local").join("lng").join("strings");
    let mut json_paths = direct_files_with_extension(&strings_dir, "json");
    if json_paths.is_empty() {
        return None;
    }
    json_paths.sort_by(|left, right| {
        string_json_load_order_key(left)
            .cmp(&string_json_load_order_key(right))
            .then_with(|| local_path_identity(left).cmp(&local_path_identity(right)))
    });

    let mut warnings = Vec::new();
    let mut observed = Vec::new();
    let mut files = Vec::new();
    for path in json_paths {
        let Ok(uri) = Url::from_file_path(&path) else {
            warnings.push(JsonLogWarning::new(
                "log.json_path_uri",
                [("path", json!(path.display().to_string()))],
            ));
            continue;
        };
        observed.push(uri.clone());
        let open_source = open_json_sources.get(&local_path_identity(&path)).cloned();
        let parsed = match (trigger, open_source) {
            (JsonAnalysisTrigger::All, Some(source)) => {
                parse_string_source_localized(path.clone(), uri, source, locale)
            }
            (JsonAnalysisTrigger::KeyUsageOnly, Some(source)) => {
                parse_string_source_key_usage_localized(path.clone(), uri, source, locale)
            }
            (JsonAnalysisTrigger::All, None) => {
                parse_string_file_localized(path.clone(), uri, locale)
            }
            (JsonAnalysisTrigger::KeyUsageOnly, None) => {
                parse_string_file_key_usage_localized(path.clone(), uri, locale)
            }
        };
        match parsed {
            Ok(file) => files.push(file),
            Err(error) => warnings.push(JsonLogWarning::new(
                "log.json_parse_failed",
                [
                    ("path", json!(path.display().to_string())),
                    ("error", json!(error)),
                ],
            )),
        }
    }

    // Rule order intentionally mirrors d2rlint's three Json rules.
    if trigger == JsonAnalysisTrigger::All
        && let Some(severity) = diagnostic_severity(rules.duplicate_ids)
    {
        apply_duplicate_ids(&mut files, severity, locale);
    }
    if trigger == JsonAnalysisTrigger::All
        && let Some(severity) = diagnostic_severity(rules.string_format)
    {
        apply_string_format(&mut files, severity, locale);
    }
    if let Some(severity) = diagnostic_severity(rules.key_usage) {
        let used_keys = collect_used_keys(data_root, txt_documents, evidence_profile);
        apply_key_usage(
            &mut files,
            &used_keys,
            rules.key_usage_id_start,
            severity,
            locale,
        );
    }

    let mut diagnostics_by_uri = files
        .into_iter()
        .map(|file| {
            debug_assert!(file.path.is_file());
            (file.uri, file.diagnostics)
        })
        .collect::<HashMap<_, _>>();
    observed.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    let batches = observed
        .into_iter()
        .map(|uri| JsonDiagnosticBatch {
            diagnostics: diagnostics_by_uri.remove(&uri).unwrap_or_default(),
            uri,
        })
        .collect();

    Some(JsonDiagnosticReport { batches, warnings })
}

fn parse_string_file_localized(
    path: PathBuf,
    uri: Url,
    locale: Locale,
) -> Result<StringFile, String> {
    let bytes = fs::read(&path).map_err(|error| error.to_string())?;
    let source = String::from_utf8(bytes).map_err(|error| error.to_string())?;
    parse_string_source_localized(path, uri, source, locale)
}

#[cfg(test)]
fn parse_string_source(path: PathBuf, uri: Url, source: String) -> Result<StringFile, String> {
    parse_string_source_localized(path, uri, source, Locale::EnUs)
}

fn parse_string_source_localized(
    path: PathBuf,
    uri: Url,
    source: String,
    locale: Locale,
) -> Result<StringFile, String> {
    let parse_source = source.strip_prefix('\u{feff}').unwrap_or(&source);
    let value: Value = match serde_json::from_str(parse_source) {
        Ok(value) => value,
        Err(error) => return Ok(syntax_error_file(path, uri, source, error, locale)),
    };
    let Value::Array(values) = value else {
        return Ok(StringFile {
            display_stem: display_stem(&path),
            path,
            uri,
            source,
            entries: Vec::new(),
            diagnostics: Vec::new(),
        });
    };

    let spans = top_level_array_value_spans(&source).unwrap_or_default();
    let fallback = Span { start: 0, end: 0 };
    let entries = values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            let span = spans.get(index).copied().unwrap_or(fallback);
            let fields = if value.is_object() {
                object_fields_with_spans(&source, span).unwrap_or_default()
            } else {
                HashMap::new()
            };
            Entry { fields, span }
        })
        .collect();
    Ok(StringFile {
        display_stem: display_stem(&path),
        path,
        uri,
        source,
        entries,
        diagnostics: Vec::new(),
    })
}

/// KeyUsage does not inspect localized strings or any fields other than `id`
/// and `Key`. Validate the complete JSON stream, then retain only those two
/// values and their lexical spans instead of materializing the full Value tree
/// and 15-field maps for every entry.
fn parse_string_file_key_usage_localized(
    path: PathBuf,
    uri: Url,
    locale: Locale,
) -> Result<StringFile, String> {
    let bytes = fs::read(&path).map_err(|error| error.to_string())?;
    let source = String::from_utf8(bytes).map_err(|error| error.to_string())?;
    parse_string_source_key_usage_localized(path, uri, source, locale)
}

#[cfg(test)]
fn parse_string_source_key_usage(
    path: PathBuf,
    uri: Url,
    source: String,
) -> Result<StringFile, String> {
    parse_string_source_key_usage_localized(path, uri, source, Locale::EnUs)
}

fn parse_string_source_key_usage_localized(
    path: PathBuf,
    uri: Url,
    source: String,
    locale: Locale,
) -> Result<StringFile, String> {
    let parse_source = source.strip_prefix('\u{feff}').unwrap_or(&source);
    if serde_json::from_str::<IgnoredAny>(parse_source).is_err() {
        // Preserve serde_json's detailed message, but publish it as a document
        // diagnostic instead of reducing an invalid file to an empty batch.
        let error = serde_json::from_str::<Value>(parse_source)
            .expect_err("IgnoredAny and Value must agree on JSON validity");
        return Ok(syntax_error_file(path, uri, source, error, locale));
    }

    let entries = top_level_array_value_spans(&source)
        .unwrap_or_default()
        .into_iter()
        .map(|span| Entry {
            fields: selected_object_fields_with_spans(&source, span, &["id", "Key"])
                .unwrap_or_default(),
            span,
        })
        .collect();
    Ok(StringFile {
        display_stem: display_stem(&path),
        path,
        uri,
        source,
        entries,
        diagnostics: Vec::new(),
    })
}

fn syntax_error_file(
    path: PathBuf,
    uri: Url,
    source: String,
    error: serde_json::Error,
    locale: Locale,
) -> StringFile {
    let span = syntax_error_span(&source, &error);
    let diagnostics = vec![rule_diagnostic(
        &source,
        span,
        "Json/Syntax",
        "invalid-json",
        DiagnosticSeverity::ERROR,
        "json.syntax_invalid",
        i18n::args([("error", serde_json::json!(error.to_string()))]),
        locale,
    )];
    StringFile {
        display_stem: display_stem(&path),
        path,
        uri,
        source,
        entries: Vec::new(),
        diagnostics,
    }
}

fn syntax_error_span(source: &str, error: &serde_json::Error) -> Span {
    let parse_source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let source_prefix_bytes = source.len().saturating_sub(parse_source.len());
    let target_line = error.line().saturating_sub(1);
    let mut line_start = 0usize;
    for _ in 0..target_line {
        let Some(newline) = parse_source[line_start..].find('\n') else {
            line_start = parse_source.len();
            break;
        };
        line_start += newline + 1;
    }
    let line_end = parse_source[line_start..]
        .find('\n')
        .map_or(parse_source.len(), |newline| line_start + newline);
    let line_source = &parse_source[line_start..line_end];
    // serde_json's string reader reports a one-based UTF-8 byte column.
    // Clamp to the line and recover to a character boundary defensively before
    // selecting the offending code point (or an empty EOF range).
    let mut relative = error.column().saturating_sub(1).min(line_source.len());
    while relative > 0 && !line_source.is_char_boundary(relative) {
        relative -= 1;
    }
    let start = line_start + relative;
    let end = parse_source[start..line_end]
        .chars()
        .next()
        .map_or(start, |character| start + character.len_utf8());
    Span {
        start: source_prefix_bytes + start,
        end: source_prefix_bytes + end,
    }
}

fn apply_duplicate_ids(files: &mut [StringFile], severity: DiagnosticSeverity, locale: Locale) {
    let mut global_ids: HashMap<JsMapKey, LoadedStringRecord> = HashMap::new();
    let mut global_keys: HashMap<JsMapKey, LoadedStringRecord> = HashMap::new();
    let mut composite_identity = 0usize;

    for file in files {
        for index in 0..file.entries.len() {
            let entry = &file.entries[index];
            let id_field = entry.field("id");
            let key_field = entry
                .field("Key")
                .filter(|(value, _)| !matches!(value, Value::String(value) if value.is_empty()));
            let id_key = id_field.map(|(id, _)| js_map_key(id, &mut composite_identity));
            let key_key = key_field.map(|(key, _)| js_map_key(key, &mut composite_identity));

            if let Some((id, span)) = id_field {
                if json_number_outside_runtime_id_range(id) {
                    file.diagnostics.push(rule_diagnostic(
                        &file.source,
                        *span,
                        "Json/DuplicateIds",
                        "invalid-id-range",
                        severity,
                        "json.id_out_of_range",
                        i18n::args([
                            ("file", serde_json::json!(file.display_stem)),
                            ("id", serde_json::json!(js_string(id))),
                        ]),
                        locale,
                    ));
                }
            } else {
                file.diagnostics.push(rule_diagnostic(
                    &file.source,
                    entry.diagnostic_span("Key"),
                    "Json/DuplicateIds",
                    "missing-id",
                    severity,
                    "json.missing_id",
                    i18n::args([
                        ("file", serde_json::json!(file.display_stem)),
                        ("entry", serde_json::json!(index + 1)),
                    ]),
                    locale,
                ));
            }

            if let (Some((id, span)), Some(id_key)) = (id_field, id_key.as_ref())
                && let Some(previous_record) = global_ids.get(id_key)
            {
                let same_file = previous_record.file == file.display_stem;
                file.diagnostics.push(rule_diagnostic(
                    &file.source,
                    *span,
                    "Json/DuplicateIds",
                    "duplicate-id",
                    severity,
                    if same_file {
                        "json.duplicate_id_same"
                    } else {
                        "json.duplicate_id_cross"
                    },
                    i18n::args([
                        ("file", serde_json::json!(file.display_stem)),
                        ("id", serde_json::json!(js_string(id))),
                        ("first", serde_json::json!(previous_record.entry + 1)),
                        ("entry", serde_json::json!(index + 1)),
                        ("otherFile", serde_json::json!(previous_record.file)),
                    ]),
                    locale,
                ));
                continue;
            }

            if let (Some((key_value, span)), Some(key_key)) = (key_field, key_key.as_ref())
                && let Some(previous_record) = global_keys.get(key_key)
            {
                let same_file = previous_record.file == file.display_stem;
                file.diagnostics.push(rule_diagnostic(
                    &file.source,
                    *span,
                    "Json/DuplicateIds",
                    "duplicate-key",
                    severity,
                    if same_file {
                        "json.duplicate_key_same"
                    } else {
                        "json.duplicate_key_cross"
                    },
                    i18n::args([
                        ("file", serde_json::json!(file.display_stem)),
                        ("keyValue", serde_json::json!(js_string(key_value))),
                        ("first", serde_json::json!(previous_record.entry + 1)),
                        ("entry", serde_json::json!(index + 1)),
                        ("otherFile", serde_json::json!(previous_record.file)),
                    ]),
                    locale,
                ));
                continue;
            }

            let loaded = LoadedStringRecord {
                file: file.display_stem.clone(),
                entry: index,
            };
            if let Some(id_key) = id_key {
                global_ids.insert(id_key, loaded.clone());
            }
            if let Some(key_key) = key_key {
                global_keys.insert(key_key, loaded);
            }
        }
    }
}

fn apply_string_format(files: &mut [StringFile], severity: DiagnosticSeverity, locale: Locale) {
    for file in files {
        for (index, entry) in file.entries.iter().enumerate() {
            let missing = REQUIRED_STRING_FIELDS
                .iter()
                .filter(|field| entry.field(field).is_none())
                .copied()
                .collect::<Vec<_>>();
            if missing.is_empty() {
                continue;
            }
            let key = entry
                .field("Key")
                .map(|(value, _)| js_nullish_default(value, "unknown key"))
                .unwrap_or_else(|| "unknown key".to_string());
            file.diagnostics.push(rule_diagnostic(
                &file.source,
                entry.diagnostic_span("Key"),
                "Json/StringFormat",
                "missing-fields",
                severity,
                "json.missing_fields",
                i18n::args([
                    ("file", serde_json::json!(file.display_stem)),
                    ("entry", serde_json::json!(index + 1)),
                    ("keyValue", serde_json::json!(key)),
                    ("fields", serde_json::json!(missing.join(", "))),
                ]),
                locale,
            ));
        }
    }
}

fn apply_key_usage(
    files: &mut [StringFile],
    used_keys: &HashSet<String>,
    id_start: f64,
    severity: DiagnosticSeverity,
    locale: Locale,
) {
    for file in files {
        for entry in &file.entries {
            let Some((id, _)) = entry.field("id") else {
                continue;
            };
            let Some((key, key_span)) = entry.field("Key") else {
                continue;
            };
            if !js_greater_than(id, id_start)
                || matches!(key, Value::String(value) if value.is_empty())
                || matches!(key, Value::String(value) if used_keys.contains(value))
            {
                continue;
            }
            file.diagnostics.push(rule_diagnostic(
                &file.source,
                *key_span,
                "Json/KeyUsage",
                "unused-key",
                severity,
                "json.unused_key",
                i18n::args([
                    ("file", serde_json::json!(file.display_stem)),
                    ("keyValue", serde_json::json!(js_string(key))),
                    ("id", serde_json::json!(js_string(id))),
                ]),
                locale,
            ));
        }
    }
}

fn collect_used_keys(
    data_root: &Path,
    txt_documents: &[PrimaryTxtDocument],
    evidence_profile: JsonEvidenceProfile,
) -> HashSet<String> {
    let known = D2RLINT_EXCEL_FILES.iter().copied().collect::<HashSet<_>>();
    let legacy_excluded = D2RLINT_LEGACY_EXCLUDED_EXCEL_FILES
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let mut used = HashSet::new();
    for input in txt_documents {
        if data_root_from_excel_txt(&input.path)
            .is_none_or(|root| local_path_identity(&root) != local_path_identity(data_root))
        {
            continue;
        }
        let Some(file_name) = input
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_ascii_lowercase)
        else {
            continue;
        };
        if !known.contains(file_name.as_str())
            || evidence_profile == JsonEvidenceProfile::Legacy
                && legacy_excluded.contains(file_name.as_str())
        {
            continue;
        }

        // ParseExcel lowercases headers, excludes only '*' headers, and writes
        // duplicate property names using the last matching column.
        let mut property_columns: HashMap<String, usize> = HashMap::new();
        let mut header_field_count = 0usize;
        let mut skip_docs_columns = Vec::new();
        for (index, header) in input.document.headers.iter().enumerate() {
            let header = header.to_lowercase();
            if header.starts_with('*') {
                continue;
            }
            header_field_count += 1;
            if header == "@skipdocs" {
                skip_docs_columns.push(index);
            }
            property_columns.insert(header, index);
        }
        for row in &input.document.rows {
            if row.cells.len() < header_field_count {
                continue;
            }
            for index in property_columns.values() {
                if let Some(value) = row.cells.get(*index).map(|cell| cell.value.as_str())
                    && !value.is_empty()
                {
                    used.insert(value.to_string());
                }
            }
            // ParseExcel synthesizes an own boolean `skipInDocs` property.
            if skip_docs_columns.iter().any(|index| {
                // In JavaScript an out-of-range `line[idx]` is `undefined`,
                // which is also `!== ""` in ParseExcel.
                row.cells
                    .get(*index)
                    .is_none_or(|cell| !cell.value.is_empty())
            }) {
                used.insert("true".to_string());
            }
        }
    }

    let layouts = data_root.join("global").join("ui").join("layouts");
    let mut layout_paths = direct_files_with_extension(&layouts, "json");
    layout_paths.sort_by_key(|path| file_sort_key(path));
    for path in layout_paths {
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        collect_layout_at_keys(&content, &mut used);
    }
    used
}

fn collect_layout_at_keys(content: &str, used: &mut HashSet<String>) {
    let bytes = content.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'@' {
            index += 1;
            continue;
        }
        let start = index + 1;
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
            end += 1;
        }
        if end > start {
            // The accepted character class is ASCII, therefore these bounds
            // are guaranteed UTF-8 boundaries.
            used.insert(content[start..end].to_string());
            index = end;
        } else {
            index += 1;
        }
    }
}

fn rule_diagnostic(
    source: &str,
    span: Span,
    rule: &str,
    kind: &str,
    severity: DiagnosticSeverity,
    message_key: &'static str,
    message_args: serde_json::Map<String, Value>,
    locale: Locale,
) -> Diagnostic {
    i18n::localized_diagnostic(
        locale,
        message_key,
        message_args,
        Diagnostic {
            range: source_range(source, span),
            severity: Some(severity),
            code: Some(NumberOrString::String(rule.to_string())),
            source: Some("d2rlint".to_string()),
            data: Some(serde_json::json!({ "kind": kind })),
            ..Default::default()
        },
    )
}

fn diagnostic_severity(action: JsonRuleAction) -> Option<DiagnosticSeverity> {
    match action {
        JsonRuleAction::Ignore => None,
        JsonRuleAction::Warn => Some(DiagnosticSeverity::WARNING),
    }
}

fn source_range(source: &str, span: Span) -> Range {
    let start = span.start.min(source.len());
    let end = span.end.max(start).min(source.len());
    Range {
        start: position_at(source, start),
        end: position_at(source, end),
    }
}

fn position_at(source: &str, byte_offset: usize) -> Position {
    let prefix = source.get(..byte_offset).unwrap_or(source);
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let current_line = prefix.rsplit_once('\n').map_or(prefix, |(_, line)| line);
    // Editors decode the UTF-8 signature and expose the first JSON token at
    // character zero. Keep byte spans in the original source, but do not count
    // a leading BOM as an LSP character on line zero.
    let current_line = if line == 0 {
        current_line
            .strip_prefix('\u{feff}')
            .unwrap_or(current_line)
    } else {
        current_line
    };
    Position {
        line,
        character: utf16_len(current_line),
    }
}

fn one_char_span(span: Span) -> Span {
    Span {
        start: span.start,
        end: span.start.saturating_add(1).min(span.end),
    }
}

fn js_map_key(value: &Value, composite_identity: &mut usize) -> JsMapKey {
    match value {
        Value::Null => JsMapKey::Null,
        Value::Bool(value) => JsMapKey::Bool(*value),
        Value::Number(value) => {
            let number = value.as_f64().unwrap_or(f64::NAN);
            let normalized = if number == 0.0 { 0.0 } else { number };
            JsMapKey::Number(normalized.to_bits())
        }
        Value::String(value) => JsMapKey::String(value.clone()),
        Value::Array(_) | Value::Object(_) => {
            let identity = *composite_identity;
            *composite_identity += 1;
            JsMapKey::Composite(identity)
        }
    }
}

fn json_number_outside_runtime_id_range(value: &Value) -> bool {
    matches!(value, Value::Number(number)
        if number.as_f64().is_some_and(|number| !(0.0..=65535.0).contains(&number)))
}

fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(values) => values.iter().map(js_string).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".to_string(),
    }
}

fn js_nullish_default(value: &Value, default: &str) -> String {
    if value.is_null() {
        default.to_string()
    } else {
        js_string(value)
    }
}

fn js_greater_than(value: &Value, threshold: f64) -> bool {
    js_number(value).is_some_and(|number| number > threshold)
}

fn js_number(value: &Value) -> Option<f64> {
    match value {
        Value::Null => Some(0.0),
        Value::Bool(false) => Some(0.0),
        Value::Bool(true) => Some(1.0),
        Value::Number(value) => value.as_f64(),
        Value::String(value) => js_string_number(value),
        // JavaScript first applies Array#toString (join with commas), then
        // converts that primitive string to a number for relational `>`.
        Value::Array(values) => js_string_number(&js_array_string(values)),
        Value::Object(_) => None,
    }
}

fn js_array_string(values: &[Value]) -> String {
    values
        .iter()
        .map(|value| match value {
            // Array#join renders null (and undefined, which JSON cannot hold)
            // as an empty field rather than the string "null".
            Value::Null => String::new(),
            Value::Array(values) => js_array_string(values),
            Value::Object(_) => "[object Object]".to_string(),
            _ => js_string(value),
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn is_ecmascript_trim_character(ch: char) -> bool {
    // TrimString uses ECMAScript WhiteSpace plus LineTerminator. Rust's
    // Unicode White_Space property additionally includes U+0085 NEXT LINE.
    matches!(
        ch,
        '\u{0009}'..='\u{000d}'
            | '\u{0020}'
            | '\u{00a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

fn js_string_number(value: &str) -> Option<f64> {
    let value = value.trim_matches(is_ecmascript_trim_character);
    if value.is_empty() {
        return Some(0.0);
    }
    match value {
        "Infinity" | "+Infinity" => return Some(f64::INFINITY),
        "-Infinity" => return Some(f64::NEG_INFINITY),
        _ => {}
    }

    // Number("0x…"), Number("0b…") and Number("0o…") accept unsigned
    // radix literals. A leading '+' or '-' makes those forms NaN.
    for (prefixes, radix) in [(["0x", "0X"], 16), (["0b", "0B"], 2), (["0o", "0O"], 8)] {
        if let Some(digits) = value
            .strip_prefix(prefixes[0])
            .or_else(|| value.strip_prefix(prefixes[1]))
        {
            if digits.is_empty() {
                return None;
            }
            let mut number = 0.0;
            for digit in digits.chars() {
                let digit = digit.to_digit(radix)?;
                number = number * f64::from(radix) + f64::from(digit);
            }
            return Some(number);
        }
    }
    let unsigned_word = value
        .strip_prefix('+')
        .or_else(|| value.strip_prefix('-'))
        .unwrap_or(value);
    if unsigned_word.eq_ignore_ascii_case("inf") || unsigned_word.eq_ignore_ascii_case("infinity") {
        // Rust's float parser accepts case-insensitive `inf` spellings that
        // ECMAScript Number deliberately treats as NaN.
        return None;
    }
    value.parse::<f64>().ok()
}

fn top_level_array_value_spans(source: &str) -> Option<Vec<Span>> {
    let mut index = skip_ws_and_bom(source, 0);
    if source.as_bytes().get(index) != Some(&b'[') {
        return None;
    }
    index += 1;
    let mut spans = Vec::new();
    loop {
        index = skip_ws(source, index);
        if source.as_bytes().get(index) == Some(&b']') {
            return Some(spans);
        }
        let start = index;
        let end = scan_json_value(source, start)?;
        spans.push(Span { start, end });
        index = skip_ws(source, end);
        match source.as_bytes().get(index) {
            Some(b',') => index += 1,
            Some(b']') => return Some(spans),
            _ => return None,
        }
    }
}

fn object_fields_with_spans(
    source: &str,
    object_span: Span,
) -> Option<HashMap<String, (Value, Span)>> {
    object_fields_with_spans_selected(source, object_span, None)
}

fn selected_object_fields_with_spans(
    source: &str,
    object_span: Span,
    selected_fields: &[&str],
) -> Option<HashMap<String, (Value, Span)>> {
    object_fields_with_spans_selected(source, object_span, Some(selected_fields))
}

fn object_fields_with_spans_selected(
    source: &str,
    object_span: Span,
    selected_fields: Option<&[&str]>,
) -> Option<HashMap<String, (Value, Span)>> {
    let mut index = skip_ws(source, object_span.start);
    if source.as_bytes().get(index) != Some(&b'{') {
        return None;
    }
    index += 1;
    let mut fields = HashMap::new();
    loop {
        index = skip_ws(source, index);
        if source.as_bytes().get(index) == Some(&b'}') {
            return Some(fields);
        }
        let key_start = index;
        let key_end = scan_json_string(source, key_start)?;
        let name: String = serde_json::from_str(&source[key_start..key_end]).ok()?;
        index = skip_ws(source, key_end);
        if source.as_bytes().get(index) != Some(&b':') {
            return None;
        }
        index = skip_ws(source, index + 1);
        let value_start = index;
        let value_end = scan_json_value(source, value_start)?;
        if selected_fields.is_none_or(|selected| selected.contains(&name.as_str())) {
            let value = serde_json::from_str(&source[value_start..value_end]).ok()?;
            fields.insert(
                name,
                (
                    value,
                    Span {
                        start: value_start,
                        end: value_end,
                    },
                ),
            );
        }
        index = skip_ws(source, value_end);
        match source.as_bytes().get(index) {
            Some(b',') => index += 1,
            Some(b'}') => return Some(fields),
            _ => return None,
        }
    }
}

fn scan_json_value(source: &str, start: usize) -> Option<usize> {
    match source.as_bytes().get(start)? {
        b'"' => scan_json_string(source, start),
        b'{' => scan_json_container(source, start, b'{', b'}'),
        b'[' => scan_json_container(source, start, b'[', b']'),
        _ => {
            let mut end = start;
            while let Some(byte) = source.as_bytes().get(end) {
                if byte.is_ascii_whitespace() || matches!(byte, b',' | b']' | b'}') {
                    break;
                }
                end += 1;
            }
            (end > start).then_some(end)
        }
    }
}

fn scan_json_string(source: &str, start: usize) -> Option<usize> {
    if source.as_bytes().get(start) != Some(&b'"') {
        return None;
    }
    let mut index = start + 1;
    let mut escaped = false;
    while let Some(byte) = source.as_bytes().get(index) {
        if escaped {
            escaped = false;
        } else if *byte == b'\\' {
            escaped = true;
        } else if *byte == b'"' {
            return Some(index + 1);
        }
        index += 1;
    }
    None
}

fn scan_json_container(source: &str, start: usize, open: u8, close: u8) -> Option<usize> {
    if source.as_bytes().get(start) != Some(&open) {
        return None;
    }
    let mut stack = vec![close];
    let mut index = start + 1;
    while let Some(byte) = source.as_bytes().get(index).copied() {
        match byte {
            b'"' => index = scan_json_string(source, index)?,
            b'{' => {
                stack.push(b'}');
                index += 1;
            }
            b'[' => {
                stack.push(b']');
                index += 1;
            }
            b'}' | b']' => {
                if stack.pop() != Some(byte) {
                    return None;
                }
                index += 1;
                if stack.is_empty() {
                    return Some(index);
                }
            }
            _ => index += 1,
        }
    }
    None
}

fn skip_ws(source: &str, mut index: usize) -> usize {
    while source
        .as_bytes()
        .get(index)
        .is_some_and(u8::is_ascii_whitespace)
    {
        index += 1;
    }
    index
}

fn skip_ws_and_bom(source: &str, mut index: usize) -> usize {
    if source[index..].starts_with('\u{feff}') {
        index += '\u{feff}'.len_utf8();
    }
    skip_ws(source, index)
}

fn direct_files_with_extension(directory: &Path, extension: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            let path = entry.path();
            (file_type.is_file()
                && path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case(extension)))
            .then_some(path)
        })
        .collect()
}

pub fn data_root_from_excel_txt(path: &Path) -> Option<PathBuf> {
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("txt"))
    {
        return None;
    }
    let mut ancestor = path.parent();
    while let Some(directory) = ancestor {
        if path_name_is(directory, "excel")
            && directory
                .parent()
                .is_some_and(|parent| path_name_is(parent, "global"))
        {
            return directory.parent()?.parent().map(Path::to_path_buf);
        }
        ancestor = directory.parent();
    }
    None
}

pub fn data_root_from_localization_json(path: &Path) -> Option<PathBuf> {
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("json"))
    {
        return None;
    }
    let strings = path.parent()?;
    let lng = strings.parent()?;
    let local = lng.parent()?;
    let data = local.parent()?;
    (path_name_is(strings, "strings")
        && path_name_is(lng, "lng")
        && path_name_is(local, "local")
        && path_name_is(data, "data"))
    .then(|| data.to_path_buf())
}

fn path_name_is(path: &Path, expected: &str) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

fn display_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_string()
}

fn file_sort_key(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn string_json_load_order_key(path: &Path) -> (usize, String) {
    let file = file_sort_key(path);
    let rank = STRING_JSON_LOAD_ORDER
        .iter()
        .position(|candidate| *candidate == file)
        .unwrap_or(STRING_JSON_LOAD_ORDER.len());
    (rank, file)
}

#[cfg(windows)]
pub fn local_path_identity(path: &Path) -> String {
    let value = path.to_string_lossy().replace('/', "\\");
    let without_extended_prefix = if let Some(rest) = value.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else if let Some(rest) = value.strip_prefix("\\\\?\\") {
        rest.to_string()
    } else {
        value
    };
    without_extended_prefix.to_lowercase()
}

#[cfg(not(windows))]
pub fn local_path_identity(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempTree(PathBuf);

    impl TempTree {
        fn new(name: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "vector-lsp-json-{name}-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(root.join("data/global/excel")).unwrap();
            Self(root)
        }

        fn data_root(&self) -> PathBuf {
            self.0.join("data")
        }

        fn excel(&self, name: &str, text: &str) -> PrimaryTxtDocument {
            let path = self.data_root().join("global/excel").join(name);
            fs::write(&path, text).unwrap();
            PrimaryTxtDocument {
                path,
                document: Arc::new(DocumentData::parse(text, '\t')),
            }
        }

        fn json(&self, name: &str, text: &str) {
            let directory = self.data_root().join("local/lng/strings");
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join(name), text.as_bytes()).unwrap();
        }

        fn layout(&self, name: &str, text: &str) {
            let directory = self.data_root().join("global/ui/layouts");
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join(name), text.as_bytes()).unwrap();
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn complete_entry(id: &str, key: &str) -> String {
        format!(
            r#"{{"id":{id},"Key":{key},"enUS":"","zhTW":"","deDE":"","esES":"","frFR":"","itIT":"","koKR":"","plPL":"","esMX":"","jaJP":"","ptBR":"","ruRU":"","zhCN":""}}"#
        )
    }

    fn diagnostic_messages(report: &JsonDiagnosticReport) -> Vec<String> {
        report
            .batches
            .iter()
            .flat_map(|batch| batch.diagnostics.iter())
            .map(|diagnostic| diagnostic.message.clone())
            .collect()
    }

    fn diagnostics(report: &JsonDiagnosticReport) -> Vec<&Diagnostic> {
        report
            .batches
            .iter()
            .flat_map(|batch| batch.diagnostics.iter())
            .collect()
    }

    #[test]
    fn invalid_json_has_one_error_at_the_exact_utf16_range() {
        let source = "\u{feff}[\r\n  {\"Key\":\"🙂\",}\r\n]".to_string();
        let path = PathBuf::from("emoji.json");
        let uri = Url::parse("file:///emoji.json").unwrap();
        let expected_start = source.find('}').unwrap();

        for file in [
            parse_string_source(path.clone(), uri.clone(), source.clone()).unwrap(),
            parse_string_source_key_usage(path.clone(), uri.clone(), source.clone()).unwrap(),
        ] {
            assert_eq!(file.diagnostics.len(), 1);
            let diagnostic = &file.diagnostics[0];
            assert_eq!(
                diagnostic.code,
                Some(NumberOrString::String("Json/Syntax".to_string()))
            );
            assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::ERROR));
            assert_eq!(diagnostic.range.start, Position::new(1, 14));
            assert_eq!(diagnostic.range.end, Position::new(1, 15));

            let error =
                serde_json::from_str::<Value>(source.strip_prefix('\u{feff}').unwrap_or(&source))
                    .unwrap_err();
            assert_eq!(
                syntax_error_span(&source, &error),
                Span {
                    start: expected_start,
                    end: expected_start + 1,
                }
            );
        }
    }

    #[test]
    fn syntax_diagnostic_disappears_after_the_json_is_valid_again() {
        let path = PathBuf::from("recover.json");
        let uri = Url::parse("file:///recover.json").unwrap();
        let invalid =
            parse_string_source(path.clone(), uri.clone(), "[{\"id\":1".to_string()).unwrap();
        assert_eq!(invalid.diagnostics.len(), 1);
        assert_eq!(
            invalid.diagnostics[0].code,
            Some(NumberOrString::String("Json/Syntax".to_string()))
        );

        let valid =
            parse_string_source(path, uri, "[{\"id\":1,\"Key\":\"Recovered\"}]".to_string())
                .unwrap();
        assert!(valid.diagnostics.is_empty());
        assert_eq!(valid.entries.len(), 1);
    }

    #[test]
    fn d2rlint_known_file_contract_excludes_hiredesc_and_moncalc() {
        assert_eq!(D2RLINT_EXCEL_FILES.len(), 90);
        assert_eq!(D2RLINT_LEGACY_EXCLUDED_EXCEL_FILES.len(), 8);
        assert!(
            D2RLINT_LEGACY_EXCLUDED_EXCEL_FILES
                .iter()
                .all(|file| D2RLINT_EXCEL_FILES.contains(file))
        );
        assert!(!D2RLINT_EXCEL_FILES.contains(&"hiredesc.txt"));
        assert!(!D2RLINT_EXCEL_FILES.contains(&"moncalc.txt"));
        assert!(D2RLINT_EXCEL_FILES.contains(&"skills.txt"));
    }

    #[test]
    fn no_physical_string_json_means_no_scope_or_diagnostics() {
        let tree = TempTree::new("absent");
        let report = analyze(vec![tree.excel("skills.txt", "skill\tstr name\nA\tUsed")]);
        assert!(report.batches.is_empty());
    }

    #[test]
    fn open_json_source_shadows_the_same_physical_file_without_creating_scope() {
        let tree = TempTree::new("open-shadow");
        let duplicate = complete_entry("41001", "\"Duplicate\"");
        tree.json("skills.json", &format!("[{duplicate},{duplicate}]"));
        let primary = vec![tree.excel("skills.txt", "skill\nnone")];
        assert!(
            diagnostic_messages(&analyze(primary.clone()))
                .iter()
                .any(|message| message.contains("duplicate id 41001"))
        );

        let json_path = tree.data_root().join("local/lng/strings/skills.json");
        let fixed = format!(
            "[{},{}]",
            complete_entry("41001", "\"First\""),
            complete_entry("41002", "\"Second\"")
        );
        let open_sources =
            OpenJsonSources::from([(local_path_identity(&json_path), fixed.clone())]);
        let report = analyze_with_rules_profile_and_open_json(
            primary,
            JsonDiagnosticRules::default(),
            JsonAnalysisTrigger::All,
            JsonEvidenceProfile::ResurrectedOrUnknown,
            open_sources,
        );
        assert!(
            !diagnostic_messages(&report)
                .iter()
                .any(|message| message.contains("duplicate id 41001"))
        );

        fs::remove_file(&json_path).unwrap();
        let report_without_physical_scope = analyze_with_rules_profile_and_open_json(
            vec![tree.excel("skills.txt", "skill\nnone")],
            JsonDiagnosticRules::default(),
            JsonAnalysisTrigger::All,
            JsonEvidenceProfile::ResurrectedOrUnknown,
            OpenJsonSources::from([(local_path_identity(&json_path), fixed)]),
        );
        assert!(report_without_physical_scope.batches.is_empty());
    }

    #[test]
    fn duplicate_and_format_rules_preserve_js_presence_and_map_semantics() {
        let tree = TempTree::new("duplicate-format");
        let first = complete_entry("null", "null");
        let second = complete_entry("null", "\"RejectedWithId\"");
        let third = complete_entry("3", "null");
        let numeric = complete_entry("1", "1");
        let string = complete_entry("\"1\"", "\"1\"");
        let empty_values = complete_entry("2", "\"\"");
        let missing = r#"{"Key":"missing"}"#;
        tree.json(
            "a.json",
            &format!("[{first},{second},{third},{numeric},{string},{empty_values},{missing}]"),
        );
        let report = analyze(vec![tree.excel("skills.txt", "skill\nnone")]);
        let messages = diagnostic_messages(&report);
        assert!(
            messages
                .iter()
                .any(|message| message.contains("duplicate id null"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("duplicate Key 'null'"))
        );
        assert!(
            !messages
                .iter()
                .any(|message| message.contains("duplicate id 1"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("missing id on entry 7"))
        );
        assert!(
            messages.iter().any(|message| {
                message.contains("entry 7 (missing) is missing fields: id, enUS")
            })
        );
        assert!(!messages.iter().any(|message| message.contains("entry 6")));
    }

    #[test]
    fn duplicate_rules_follow_runtime_file_order_and_explain_first_wins() {
        let tree = TempTree::new("runtime-string-order");
        tree.json(
            "commands.json",
            &format!(
                "[{},{},{},{},{}]",
                complete_entry("50000", "\"RejectedIdKey\""),
                complete_entry("60000", "\"SharedKey\""),
                complete_entry("60000", "\"AcceptedAfterKeyRejection\""),
                complete_entry("60001", "\"RejectedIdKey\""),
                complete_entry("60002", "\"Unique\"")
            ),
        );
        tree.json(
            "bnet.json",
            &format!(
                "[{},{}]",
                complete_entry("50000", "\"WinnerIdKey\""),
                complete_entry("50001", "\"SharedKey\"")
            ),
        );

        let report = analyze(vec![tree.excel("skills.txt", "skill\nnone")]);
        let commands = report
            .batches
            .iter()
            .find(|batch| batch.uri.path().ends_with("/commands.json"))
            .unwrap();
        assert_eq!(commands.diagnostics.len(), 2);
        assert!(commands.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("duplicate id 50000 found in bnet.json")
                && diagnostic
                    .message
                    .contains("entry 1 is ignored, so neither its ID nor Key is registered")
        }));
        assert!(commands.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("duplicate Key 'SharedKey' found in bnet.json")
                && diagnostic
                    .message
                    .contains("entry 2 is ignored, so neither its ID nor Key is registered")
        }));
        let bnet = report
            .batches
            .iter()
            .find(|batch| batch.uri.path().ends_with("/bnet.json"))
            .unwrap();
        assert!(bnet.diagnostics.is_empty());
    }

    #[test]
    fn duplicate_rule_reports_numeric_ids_outside_the_runtime_uint16_range() {
        let tree = TempTree::new("runtime-id-range");
        tree.json(
            "skills.json",
            &format!(
                "[{},{},{}]",
                complete_entry("-1", "\"Negative\""),
                complete_entry("65535", "\"Boundary\""),
                complete_entry("65536", "\"Overflow\"")
            ),
        );

        let messages = diagnostic_messages(&analyze(vec![tree.excel("skills.txt", "skill\nnone")]));
        assert!(messages.iter().any(|message| {
            message.contains("id -1 is outside the runtime string ID range 0..65535")
        }));
        assert!(!messages.iter().any(|message| {
            message.contains("id 65535 is outside the runtime string ID range")
        }));
        assert!(messages.iter().any(|message| {
            message.contains("id 65536 is outside the runtime string ID range 0..65535")
        }));
    }

    #[test]
    fn key_usage_is_exact_strict_and_uses_known_txt_and_raw_layout_only() {
        let tree = TempTree::new("key-usage");
        let entries = [
            complete_entry("40000", "\"Boundary\""),
            complete_entry("40001", "\"Exact\""),
            complete_entry("\"40002\"", "\"FromLayout\""),
            complete_entry("40003", "\"StarOnly\""),
            complete_entry("40004", "\"MoncalcOnly\""),
            complete_entry("40005", "40005"),
        ];
        tree.json("keys.json", &format!("[{}]", entries.join(",")));
        tree.layout("layout.json", "// JSONC is intentional: @FromLayout");
        let skills = tree.excel(
            "skills.txt",
            "name\tstr key\t*comment\nA\tExact\tStarOnly\n",
        );
        let moncalc = tree.excel("moncalc.txt", "name\nMoncalcOnly\n");
        let report = analyze_with_rules(
            vec![skills, moncalc],
            JsonDiagnosticRules {
                key_usage: JsonRuleAction::Warn,
                ..JsonDiagnosticRules::default()
            },
            JsonAnalysisTrigger::All,
        );
        let messages = diagnostic_messages(&report);
        assert!(!messages.iter().any(|message| message.contains("Boundary")));
        assert!(!messages.iter().any(|message| message.contains("'Exact'")));
        assert!(
            !messages
                .iter()
                .any(|message| message.contains("FromLayout"))
        );
        assert!(messages.iter().any(|message| message.contains("StarOnly")));
        assert!(
            messages
                .iter()
                .any(|message| message.contains("MoncalcOnly"))
        );
        // A numeric JSON Key never matches the Set<string> populated from TXT.
        assert!(
            messages
                .iter()
                .any(|message| message.contains("Key '40005'"))
        );
    }

    #[test]
    fn monpet_only_evidence_is_ignored_by_the_legacy_profile() {
        let tree = TempTree::new("legacy-evidence");
        tree.json(
            "keys.json",
            &format!("[{}]", complete_entry("50001", "\"MonPetOnly\"")),
        );
        let documents = vec![tree.excel("monpet.txt", "name\nMonPetOnly\n")];
        let rules = JsonDiagnosticRules {
            key_usage: JsonRuleAction::Warn,
            ..JsonDiagnosticRules::default()
        };
        let modern = analyze_with_rules_and_profile(
            documents.clone(),
            rules,
            JsonAnalysisTrigger::All,
            JsonEvidenceProfile::ResurrectedOrUnknown,
        );
        let legacy = analyze_with_rules_and_profile(
            documents,
            rules,
            JsonAnalysisTrigger::All,
            JsonEvidenceProfile::Legacy,
        );
        assert!(diagnostic_messages(&modern).is_empty());
        assert!(
            diagnostic_messages(&legacy)
                .iter()
                .any(|message| message.contains("MonPetOnly"))
        );
    }

    #[test]
    fn configured_actions_gate_rules_and_map_severity() {
        let tree = TempTree::new("actions");
        tree.json(
            "rules.json",
            r#"[{"id":50001,"Key":"First"},{"id":50001,"Key":"Second"}]"#,
        );
        let report = analyze_with_rules(
            vec![tree.excel("skills.txt", "name\nnone")],
            JsonDiagnosticRules {
                duplicate_ids: JsonRuleAction::Warn,
                string_format: JsonRuleAction::Ignore,
                key_usage: JsonRuleAction::Warn,
                key_usage_id_start: 40_000.0,
            },
            JsonAnalysisTrigger::All,
        );
        let diagnostics = diagnostics(&report);
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == Some(NumberOrString::String("Json/DuplicateIds".to_string()))
                && diagnostic.severity == Some(DiagnosticSeverity::WARNING)
        }));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == Some(NumberOrString::String("Json/KeyUsage".to_string()))
                && diagnostic.severity == Some(DiagnosticSeverity::WARNING)
        }));
        assert!(!diagnostics.iter().any(|diagnostic| {
            diagnostic.code == Some(NumberOrString::String("Json/StringFormat".to_string()))
        }));
    }

    #[test]
    fn key_usage_only_trigger_skips_json_content_only_rules() {
        let tree = TempTree::new("key-only");
        tree.json(
            "rules.json",
            r#"[{"id":50001,"Key":"First"},{"id":50001,"Key":"First"}]"#,
        );
        let report = analyze_with_rules(
            vec![tree.excel("skills.txt", "name\nnone")],
            JsonDiagnosticRules {
                duplicate_ids: JsonRuleAction::Warn,
                string_format: JsonRuleAction::Warn,
                key_usage: JsonRuleAction::Warn,
                key_usage_id_start: 40_000.0,
            },
            JsonAnalysisTrigger::KeyUsageOnly,
        );
        let diagnostics = diagnostics(&report);
        assert!(!diagnostics.is_empty());
        assert!(diagnostics.iter().all(|diagnostic| {
            diagnostic.code == Some(NumberOrString::String("Json/KeyUsage".to_string()))
                && diagnostic.severity == Some(DiagnosticSeverity::WARNING)
        }));
    }

    #[test]
    fn key_usage_only_parser_matches_full_parser_for_duplicate_properties_and_ranges() {
        let tree = TempTree::new("key-only-parser-equivalence");
        tree.json(
            "rules.json",
            concat!(
                "\u{feff}[\r\n",
                "  {\"unused\":{\"deep\":[1,2,3]},",
                "\"id\":1,\"id\":50001,",
                "\"Key\":\"Earlier\",\"Key\":\"🙂Final\"}\r\n",
                "]"
            ),
        );
        let rules = JsonDiagnosticRules {
            duplicate_ids: JsonRuleAction::Ignore,
            string_format: JsonRuleAction::Ignore,
            key_usage: JsonRuleAction::Warn,
            key_usage_id_start: 40_000.0,
        };
        let documents = vec![tree.excel("skills.txt", "name\nnone")];
        let full = analyze_with_rules(documents.clone(), rules, JsonAnalysisTrigger::All);
        let optimized = analyze_with_rules(documents, rules, JsonAnalysisTrigger::KeyUsageOnly);
        assert_eq!(full.warnings, optimized.warnings);
        assert_eq!(full.batches.len(), 1);
        assert_eq!(optimized.batches.len(), 1);
        assert_eq!(
            full.batches[0].diagnostics,
            optimized.batches[0].diagnostics
        );
        let diagnostic = &optimized.batches[0].diagnostics[0];
        assert!(diagnostic.message.contains("🙂Final"));
        assert!(diagnostic.message.contains("id: 50001"));
        assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::WARNING));
    }

    #[test]
    fn key_usage_only_parser_still_validates_the_complete_json_document() {
        let tree = TempTree::new("key-only-invalid");
        tree.json(
            "invalid.json",
            r#"[{"id":50001,"Key":"LooksValid","ignored":[1,]}]"#,
        );
        let documents = vec![tree.excel("skills.txt", "name\nnone")];
        let full = analyze_with_rules(
            documents.clone(),
            JsonDiagnosticRules::default(),
            JsonAnalysisTrigger::All,
        );
        let optimized = analyze_with_rules(
            documents,
            JsonDiagnosticRules::default(),
            JsonAnalysisTrigger::KeyUsageOnly,
        );
        assert_eq!(full.warnings, optimized.warnings);
        assert!(full.warnings.is_empty());
        assert_eq!(full.batches.len(), 1);
        assert_eq!(optimized.batches.len(), 1);
        for batch in [&full.batches[0], &optimized.batches[0]] {
            assert_eq!(batch.diagnostics.len(), 1);
            assert_eq!(
                batch.diagnostics[0].code,
                Some(NumberOrString::String("Json/Syntax".to_string()))
            );
            assert_eq!(
                batch.diagnostics[0].severity,
                Some(DiagnosticSeverity::ERROR)
            );
        }
    }

    #[test]
    fn key_usage_id_start_is_a_strict_configurable_boundary() {
        let tree = TempTree::new("id-start");
        tree.json(
            "rules.json",
            &format!(
                "[{},{}]",
                complete_entry("56032", "\"Boundary\""),
                complete_entry("56033", "\"Above\"")
            ),
        );
        let report = analyze_with_rules(
            vec![tree.excel("skills.txt", "name\nnone")],
            JsonDiagnosticRules {
                duplicate_ids: JsonRuleAction::Ignore,
                string_format: JsonRuleAction::Ignore,
                key_usage: JsonRuleAction::Warn,
                key_usage_id_start: 56_032.0,
            },
            JsonAnalysisTrigger::All,
        );
        let messages = diagnostic_messages(&report);
        assert!(!messages.iter().any(|message| message.contains("Boundary")));
        assert!(messages.iter().any(|message| message.contains("Above")));
    }

    #[test]
    fn key_usage_id_coercion_matches_javascript_number_conversion() {
        assert_eq!(js_number(&serde_json::json!("0x10")), Some(16.0));
        assert_eq!(js_number(&serde_json::json!("0b101")), Some(5.0));
        assert_eq!(js_number(&serde_json::json!("0o10")), Some(8.0));
        assert_eq!(
            js_number(&serde_json::json!("\u{feff} 12 \u{feff}")),
            Some(12.0)
        );
        assert_eq!(
            js_number(&serde_json::json!("\u{00a0}50001")),
            Some(50001.0)
        );
        assert!(js_number(&serde_json::json!("\u{0085}50001")).is_none());
        assert_eq!(js_number(&serde_json::json!([])), Some(0.0));
        assert_eq!(js_number(&serde_json::json!([null])), Some(0.0));
        assert_eq!(js_number(&serde_json::json!([[" 7 "]])), Some(7.0));
        assert!(js_number(&serde_json::json!("+0x10")).is_none());
        assert!(js_number(&serde_json::json!("infinity")).is_none());
        assert!(js_number(&serde_json::json!("inf")).is_none());
        assert!(js_number(&serde_json::json!("+inf")).is_none());
        assert!(js_number(&serde_json::json!("-INFINITY")).is_none());
        assert!(js_number(&serde_json::json!([1, 2])).is_none());
        assert!(js_number(&serde_json::json!({ "value": 10 })).is_none());
    }

    #[test]
    fn all_ignored_rules_produce_no_diagnostics() {
        let tree = TempTree::new("all-ignored");
        tree.json("rules.json", r#"[{"id":1},{"id":1}]"#);
        let report = analyze_with_rules(
            vec![tree.excel("skills.txt", "name\nnone")],
            JsonDiagnosticRules {
                duplicate_ids: JsonRuleAction::Ignore,
                string_format: JsonRuleAction::Ignore,
                key_usage: JsonRuleAction::Ignore,
                key_usage_id_start: 40_000.0,
            },
            JsonAnalysisTrigger::All,
        );
        assert!(diagnostic_messages(&report).is_empty());
    }

    #[test]
    fn top_level_only_and_bom_crlf_utf16_ranges_are_stable() {
        let tree = TempTree::new("ranges");
        tree.json(
            "emoji.json",
            "\u{feff}[\r\n  {\"id\":1,\"Key\":\"🙂\"},\r\n  {\"id\":1,\"Key\":\"Other\"},\r\n  {\"id\":2,\"Key\":\"🙂\"}\r\n]",
        );
        let metadata = tree.data_root().join("local/lng/strings/metadata");
        fs::create_dir_all(&metadata).unwrap();
        fs::write(metadata.join("ignored.json"), "[{}]").unwrap();
        let report = analyze(vec![tree.excel("skills.txt", "name\nnone")]);
        assert_eq!(report.batches.len(), 1);
        let duplicate_id = report.batches[0]
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.message.contains("duplicate id"))
            .unwrap();
        assert_eq!(duplicate_id.range.start.line, 2);
        assert_eq!(duplicate_id.range.start.character, 8);
        let duplicate_key = report.batches[0]
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.message.contains("duplicate Key"))
            .unwrap();
        assert_eq!(duplicate_key.range.start.line, 3);
        assert_eq!(duplicate_key.range.start.character, 16);
        assert_eq!(duplicate_key.range.end.character, 20); // quotes + surrogate pair
    }

    #[test]
    fn compact_bom_json_ranges_start_at_editor_visible_columns() {
        let tree = TempTree::new("compact-bom-range");
        let visible_json = format!(
            "[{},{},{}]",
            complete_entry("1", "\"A\""),
            complete_entry("1", "\"B\""),
            complete_entry("2", "\"A\"")
        );
        tree.json("compact.json", &format!("\u{feff}{visible_json}"));
        let report = analyze(vec![tree.excel("skills.txt", "name\nnone")]);
        let duplicate_id = report.batches[0]
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.message.contains("duplicate id"))
            .unwrap();
        let duplicate_key = report.batches[0]
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.message.contains("duplicate Key"))
            .unwrap();
        let expected_id_byte = visible_json.rfind("\"id\":1").unwrap() + "\"id\":".len();
        let expected_key_byte = visible_json.rfind("\"Key\":\"A\"").unwrap() + "\"Key\":".len();
        assert_eq!(duplicate_id.range.start.line, 0);
        assert_eq!(
            duplicate_id.range.start.character,
            utf16_len(&visible_json[..expected_id_byte])
        );
        assert_eq!(
            duplicate_key.range.start.character,
            utf16_len(&visible_json[..expected_key_byte])
        );
    }

    #[test]
    fn at_skipdocs_adds_the_synthetic_true_value_like_parse_excel() {
        let tree = TempTree::new("skip-docs");
        tree.json(
            "keys.json",
            &format!("[{}]", complete_entry("40001", "\"true\"")),
        );
        let report = analyze(vec![tree.excel("skills.txt", "name\t@skipdocs\nA\tyes\n")]);
        assert!(diagnostic_messages(&report).is_empty());
    }
}
