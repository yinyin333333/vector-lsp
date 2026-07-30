mod format;
pub mod registry;

pub use format::format_description;
pub use registry::find_loader;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Loader trait
// ---------------------------------------------------------------------------

/// Implemented by each contrib schema driver.
///
/// A driver reads a schema directory (whose layout and file format it defines)
/// and returns a fully-populated `Schema`. The driver is selected at startup by
/// the `schema_loader` config key.
pub trait SchemaLoader: Send + Sync {
    /// Load a schema from `dir`, or auto-discover the schema directory if `None`.
    /// May block on IO.
    fn load(&self, dir: Option<&Path>) -> anyhow::Result<Schema>;
    /// Directories to scan for plugin files, in load order (lowest priority first).
    /// Called before `load()` so the plugin host can be set up before the LSP
    /// server starts. The caller appends any explicit `plugin_path` from settings.
    fn default_plugin_dirs(&self) -> Vec<PathBuf> {
        vec![]
    }
}

// ---------------------------------------------------------------------------
// Schema types
// ---------------------------------------------------------------------------

/// The primitive type of a field's value, as declared in the schema.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FieldTypeName {
    Int,
    Float,
    Boolean,
    Text,
    String,
    Array,
    Object,
    /// Calc expression field (parsed separately for sub-cell diagnostics).
    Parse,
    /// Value references a row in another file.
    Reference,
    /// Documentation-only; not a real column.
    Comment,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ReferenceResolver {
    /// Full string, ASCII case-insensitive, with whitespace preserved.
    #[default]
    AsciiCi,
    /// First four raw bytes, space padded, case-sensitive.
    Fixed4,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ReferenceUnknownPolicy {
    Error,
    #[default]
    Warning,
    Ignore,
}

/// Type descriptor attached to each field definition.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FieldType {
    #[serde(rename = "type")]
    pub type_name: FieldTypeName,
    #[serde(default)]
    pub data_length: i64,
    #[serde(default)]
    pub mem_size: i64,
    /// For `reference` fields: the file containing the target rows.
    pub file: Option<String>,
    /// For `reference` fields: the column in the target file to resolve against.
    pub field: Option<String>,
    /// Binary resolver semantics for reference fields.
    #[serde(default)]
    pub resolver: ReferenceResolver,
    /// Severity policy for an unresolved value. Defaults to a hygiene warning
    /// because a generic schema reference does not prove a fatal consumer.
    #[serde(default)]
    pub unknown_policy: ReferenceUnknownPolicy,
}

/// Declares that this field's valid values come from an enum defined in another schema file.
#[derive(Debug, Deserialize, Clone)]
pub struct AppendField {
    #[expect(
        dead_code,
        reason = "Bundled schema append metadata is retained for input-model fidelity."
    )]
    pub file: String,
    #[expect(
        dead_code,
        reason = "Bundled schema append metadata is retained for input-model fidelity."
    )]
    pub field: String,
}

/// A single column definition within a schema file.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SchemaField {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub field_type: Option<FieldType>,
    /// Alternative column names this field may appear under.
    #[serde(default)]
    pub alt_names: Vec<String>,
    /// If set, valid values for this field are drawn from the named enum table.
    #[expect(
        dead_code,
        reason = "Bundled schema append metadata is retained for input-model fidelity."
    )]
    pub append_field: Option<AppendField>,
    /// Enum table for `comment`-type fields in reference-only schema files.
    /// Each inner vec is [code, description].
    pub table: Option<Vec<Vec<serde_json::Value>>>,
    /// Only explicitly declared unique fields participate in duplicate checks.
    #[serde(default)]
    pub unique: bool,
}

/// Top-level schema entry for a single data file (or a reference-only pseudo-file).
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SchemaFile {
    /// Human-readable title (usually the filename).
    #[expect(
        dead_code,
        reason = "Bundled schema presentation metadata is retained for input-model fidelity."
    )]
    pub title: Option<String>,
    /// Human-readable overview of what this file controls.
    #[expect(
        dead_code,
        reason = "Bundled schema presentation metadata is retained for input-model fidelity."
    )]
    pub overview: Option<String>,
    /// If true, this entry is a reference table only and has no corresponding data file.
    #[serde(default)]
    pub guide_only: bool,
    /// Schema files whose fields are merged into this one for reference purposes.
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "Bundled schema reference metadata is retained for input-model fidelity."
    )]
    pub reference_files: Vec<String>,
    /// Schema files whose fields are prepended to this file's field list.
    #[serde(default)]
    pub append_files: Vec<String>,
    /// Ordered column definitions for this file.
    #[serde(default)]
    pub fields: Vec<SchemaField>,
    /// Columns that exist in the data file but are intentionally undocumented / ignored.
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "Bundled schema compatibility metadata is retained for input-model fidelity."
    )]
    pub ignore_fields: Vec<String>,
    #[expect(
        dead_code,
        reason = "Bundled schema compatibility metadata is retained for input-model fidelity."
    )]
    pub code_dependency: Option<String>,
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "Bundled schema presentation metadata is retained for input-model fidelity."
    )]
    pub not_searchable: bool,
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "Bundled schema presentation metadata is retained for input-model fidelity."
    )]
    pub no_html: bool,
}

impl SchemaFile {
    /// Find a field by its primary name or any alt name (case-insensitive).
    pub fn find_field(&self, col_name: &str) -> Option<&SchemaField> {
        let lower = col_name.to_lowercase();
        self.fields.iter().find(|f| {
            f.name.to_lowercase() == lower || f.alt_names.iter().any(|a| a.to_lowercase() == lower)
        })
    }
}

/// The full parsed schema for a workspace — a map from file stem to its definition.
#[derive(Debug, Default)]
pub struct Schema {
    pub files: HashMap<String, SchemaFile>,
}

impl Schema {
    /// Look up the schema entry for a file by its stem (e.g. `"armor"` for `armor.txt`).
    pub fn get_file(&self, stem: &str) -> Option<&SchemaFile> {
        self.files
            .get(stem)
            .or_else(|| self.files.get(&stem.to_lowercase()))
            .or_else(|| {
                let lower = stem.to_lowercase();
                self.files
                    .iter()
                    .find_map(|(key, file)| (key.to_lowercase() == lower).then_some(file))
            })
    }

    /// Find a field definition for `col_name` in `file_stem`, following the
    /// `appendFiles` chain. Returns the first match found.
    /// A visited set prevents infinite loops if the schema has cycles.
    pub fn find_field<'a>(&'a self, file_stem: &str, col_name: &str) -> Option<&'a SchemaField> {
        self.find_field_inner(file_stem, col_name, &mut HashSet::new())
    }

    fn find_field_inner<'a>(
        &'a self,
        file_stem: &str,
        col_name: &str,
        visited: &mut HashSet<String>,
    ) -> Option<&'a SchemaField> {
        if !visited.insert(file_stem.to_lowercase()) {
            return None;
        }
        let sf = self.get_file(file_stem)?;
        if let Some(f) = sf.find_field(col_name) {
            return Some(f);
        }
        for stem in &sf.append_files {
            if let Some(f) = self.find_field_inner(stem, col_name, visited) {
                return Some(f);
            }
        }
        None
    }

    /// For a `reference` target that is a guide-only (no data file) entry, return the
    /// valid values from the field's `table` (first column, skipping the header row).
    /// Returns `None` for real files — their values come from the SymbolIndex.
    pub fn enum_values_for_target(&self, file_stem: &str, col_name: &str) -> Option<Vec<String>> {
        let sf = self.get_file(file_stem)?;
        if !sf.guide_only {
            return None;
        }
        let field = sf.find_field(col_name)?;
        let table = field.table.as_ref()?;
        let values: Vec<String> = table
            .iter()
            .skip(1)
            .filter_map(|row| row.first())
            .filter_map(|v| v.as_str())
            .map(|s| s.to_string())
            .collect();
        if values.is_empty() {
            None
        } else {
            Some(values)
        }
    }

    /// Return the set of `(file_stem, column_name)` pairs that are pointed at by
    /// `reference`-typed fields anywhere in the schema. Drives SymbolIndex population.
    pub fn reference_targets(&self) -> HashSet<(String, String)> {
        let mut targets = HashSet::new();
        for schema_file in self.files.values() {
            for field in &schema_file.fields {
                let Some(ft) = &field.field_type else {
                    continue;
                };
                if ft.type_name != FieldTypeName::Reference {
                    continue;
                }
                let (Some(file), Some(col)) = (&ft.file, &ft.field) else {
                    continue;
                };
                targets.insert((file.to_lowercase(), col.to_lowercase()));
            }
        }
        targets
    }
}
