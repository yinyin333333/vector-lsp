use serde::{Deserialize, Deserializer};
use std::path::PathBuf;

use crate::i18n::Locale;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum JsonRuleAction {
    Ignore,
    #[default]
    Warn,
}

impl<'de> Deserialize<'de> for JsonRuleAction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "ignore" => Ok(Self::Ignore),
            "warn" => Ok(Self::Warn),
            // Older clients could request Error, but it ran the same JSON
            // rule set as Warning. Keep the rule enabled while migrating to
            // the two-state ignore/warn contract.
            "error" => Ok(Self::Warn),
            _ => Err(serde::de::Error::custom(format!(
                "expected ignore or warn, got '{value}'"
            ))),
        }
    }
}

impl JsonRuleAction {
    pub fn is_enabled(self) -> bool {
        self != Self::Ignore
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JsonDiagnosticRules {
    pub duplicate_ids: JsonRuleAction,
    pub string_format: JsonRuleAction,
    pub key_usage: JsonRuleAction,
    pub key_usage_id_start: f64,
}

impl Default for JsonDiagnosticRules {
    fn default() -> Self {
        Self {
            duplicate_ids: JsonRuleAction::Warn,
            string_format: JsonRuleAction::Warn,
            key_usage: JsonRuleAction::Ignore,
            key_usage_id_start: 40_000.0,
        }
    }
}

impl JsonDiagnosticRules {
    pub fn any_enabled(self) -> bool {
        self.duplicate_ids.is_enabled()
            || self.string_format.is_enabled()
            || self.key_usage.is_enabled()
    }
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    Auto,
    #[default]
    Utf8,
    #[serde(rename = "utf-16-le")]
    Utf16Le,
    #[serde(rename = "utf-16-be")]
    Utf16Be,
    #[serde(rename = "latin-1")]
    Latin1,
}

impl Encoding {
    /// Decode raw file bytes into a UTF-8 `String` according to this encoding.
    /// A UTF-16 BOM, if present, overrides the declared byte order.
    pub fn decode(&self, bytes: &[u8]) -> anyhow::Result<String> {
        match self {
            Encoding::Auto => Self::decode_auto(bytes),
            Encoding::Utf8 => Ok(String::from_utf8_lossy(
                bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes),
            )
            .into_owned()),
            Encoding::Latin1 => Ok(bytes.iter().map(|&b| b as char).collect()),
            Encoding::Utf16Le => Self::decode_utf16(bytes, false),
            Encoding::Utf16Be => Self::decode_utf16(bytes, true),
        }
    }

    fn decode_auto(bytes: &[u8]) -> anyhow::Result<String> {
        if bytes.starts_with(&[0xFF, 0xFE]) {
            return Self::decode_utf16(bytes, false);
        }
        if bytes.starts_with(&[0xFE, 0xFF]) {
            return Self::decode_utf16(bytes, true);
        }
        if let Some(data) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
            return String::from_utf8(data.to_vec()).map_err(Into::into);
        }
        match String::from_utf8(bytes.to_vec()) {
            Ok(text) => Ok(text),
            Err(_) => Ok(decode_windows_1252(bytes)),
        }
    }

    fn decode_utf16(bytes: &[u8], big_endian: bool) -> anyhow::Result<String> {
        // A BOM at the start overrides the caller-supplied byte order.
        let (data, be) = match bytes.get(..2) {
            Some([0xFF, 0xFE]) => (&bytes[2..], false),
            Some([0xFE, 0xFF]) => (&bytes[2..], true),
            _ => (bytes, big_endian),
        };
        anyhow::ensure!(data.len() % 2 == 0, "UTF-16 data has an odd byte count");
        let u16s: Vec<u16> = data
            .chunks_exact(2)
            .map(|c| {
                if be {
                    u16::from_be_bytes([c[0], c[1]])
                } else {
                    u16::from_le_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16(&u16s).map_err(|e| anyhow::anyhow!("{e}"))
    }
}

fn decode_windows_1252(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| char::from_u32(windows_1252_code_point(*byte)).unwrap())
        .collect()
}

fn windows_1252_code_point(byte: u8) -> u32 {
    match byte {
        0x80 => 0x20AC,
        0x82 => 0x201A,
        0x83 => 0x0192,
        0x84 => 0x201E,
        0x85 => 0x2026,
        0x86 => 0x2020,
        0x87 => 0x2021,
        0x88 => 0x02C6,
        0x89 => 0x2030,
        0x8A => 0x0160,
        0x8B => 0x2039,
        0x8C => 0x0152,
        0x8E => 0x017D,
        0x91 => 0x2018,
        0x92 => 0x2019,
        0x93 => 0x201C,
        0x94 => 0x201D,
        0x95 => 0x2022,
        0x96 => 0x2013,
        0x97 => 0x2014,
        0x98 => 0x02DC,
        0x99 => 0x2122,
        0x9A => 0x0161,
        0x9B => 0x203A,
        0x9C => 0x0153,
        0x9E => 0x017E,
        0x9F => 0x0178,
        _ => u32::from(byte),
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct TcpSettings {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum IoType {
    #[default]
    Stdio,
    Tcp(TcpSettings),
}

#[derive(Debug, Deserialize, Clone)]
pub struct VectorLspSettings {
    #[serde(skip)]
    pub editor_mode: bool,
    #[serde(default)]
    pub io_type: IoType,
    /// Single-character delimiter. Serialized as a string in config (e.g. "\t" or ",").
    #[serde(default = "default_delimiter")]
    pub delimiter: String,
    #[serde(default)]
    pub encoding: Encoding,
    /// File extension to treat as workspace data files (without leading dot).
    #[serde(default = "default_extension")]
    pub extension: String,
    pub schema_path: Option<PathBuf>,
    /// Directory to scan for plugin files (*.ts / *.js).
    pub plugin_path: Option<PathBuf>,
    /// Schema loader driver to use. Must match a registered `LoaderEntry::id`.
    /// Defaults to `"d2rdoc"` (the built-in JavaScript schema format).
    #[serde(default = "default_schema_loader")]
    pub schema_loader: String,
    /// Selects which bundled schema/plugin set to use when `schema_path` and
    /// `plugin_path` are not set. E.g. `"d2r-2.7"`. The name `"plugins"` is
    /// reserved and cannot be used as a variant name.
    #[serde(default)]
    pub schema_variant: String,
    /// Selects one bundled D2 reference dataset for cross-file fallback lookups.
    /// Empty means infer from a supported schema_variant; if neither is known,
    /// fallback remains disabled instead of guessing a game version.
    #[serde(default)]
    pub reference_variant: String,
    /// Workspace directory to use in single-shot mode (and optionally in LSP mode).
    pub workspace_path: Option<PathBuf>,
    /// When true, validate the workspace and exit instead of starting the LSP server.
    #[serde(default)]
    pub single_shot: bool,
    /// Run the d2rlint-compatible localization string JSON diagnostics for JSON
    /// files that physically exist beside the primary mod's Excel directory.
    /// This is intentionally opt-in: reference and bundled data must never turn
    /// these diagnostics on for a mod that does not contain string JSON files.
    #[serde(default, deserialize_with = "deserialize_bool_or_string")]
    pub json_diagnostics: bool,
    /// Default product-message locale for CLI/single-shot runs. LSP sessions
    /// use their negotiated locale and fall back to enUS when it is omitted.
    #[serde(default = "default_locale")]
    pub locale: String,
    /// Action for d2rlint's Json/DuplicateIds rule.
    #[serde(default)]
    pub json_duplicate_ids_action: JsonRuleAction,
    /// Action for d2rlint's Json/StringFormat rule.
    #[serde(default)]
    pub json_string_format_action: JsonRuleAction,
    /// Action for d2rlint's Json/KeyUsage rule.
    #[serde(default = "default_json_key_usage_action")]
    pub json_key_usage_action: JsonRuleAction,
    /// Json/KeyUsage reports only entries whose JavaScript-coerced id is
    /// strictly greater than this value, matching d2rlint's `idStart`.
    #[serde(
        default = "default_json_key_usage_id_start",
        deserialize_with = "deserialize_number_or_string"
    )]
    pub json_key_usage_id_start: f64,
}

impl VectorLspSettings {
    pub fn configured_locale(&self) -> Locale {
        Locale::normalize(Some(&self.locale))
    }

    pub fn delimiter_char(&self) -> char {
        self.delimiter.chars().next().unwrap_or('\t')
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.delimiter.chars().count() == 1,
            "delimiter must contain exactly one character"
        );
        anyhow::ensure!(
            !self.extension.is_empty()
                && !self.extension.starts_with('.')
                && !self.extension.contains('/')
                && !self.extension.contains('\\'),
            "extension must be a non-empty suffix without a leading dot or path separator"
        );
        anyhow::ensure!(
            !self.schema_loader.trim().is_empty(),
            "schema_loader must not be empty"
        );
        anyhow::ensure!(
            self.json_key_usage_id_start.is_finite(),
            "json_key_usage_id_start must be a finite number"
        );
        Ok(())
    }

    pub fn json_diagnostic_rules(&self) -> JsonDiagnosticRules {
        JsonDiagnosticRules {
            duplicate_ids: self.json_duplicate_ids_action,
            string_format: self.json_string_format_action,
            key_usage: self.json_key_usage_action,
            key_usage_id_start: self.json_key_usage_id_start,
        }
    }

    pub fn apply_editor_mode(&mut self) {
        self.editor_mode = true;
        self.io_type = IoType::Stdio;
        self.single_shot = false;
        self.workspace_path = None;
    }

    pub fn effective_summary(&self) -> String {
        let schema = self
            .schema_path
            .as_ref()
            .map(|path| format!("path:{}", path.display()))
            .unwrap_or_else(|| format!("variant:{}", self.schema_variant));
        format!(
            "editorMode={} transport={} singleShot={} jsonDiagnostics={} encoding={:?} schema={} referenceVariant={} pluginPath={}",
            self.editor_mode,
            if matches!(self.io_type, IoType::Stdio) {
                "stdio"
            } else {
                "tcp"
            },
            self.single_shot,
            self.json_diagnostics,
            self.encoding,
            schema,
            if self.reference_variant.trim().is_empty() {
                "inferred-or-disabled"
            } else {
                self.reference_variant.as_str()
            },
            self.plugin_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "none".to_string())
        )
    }
}

impl Default for VectorLspSettings {
    fn default() -> Self {
        Self {
            editor_mode: false,
            io_type: IoType::Stdio,
            delimiter: default_delimiter(),
            encoding: Encoding::Utf8,
            extension: default_extension(),
            schema_path: None,
            plugin_path: None,
            workspace_path: None,
            single_shot: false,
            schema_loader: default_schema_loader(),
            schema_variant: String::new(),
            reference_variant: String::new(),
            json_diagnostics: false,
            locale: default_locale(),
            json_duplicate_ids_action: JsonRuleAction::Warn,
            json_string_format_action: JsonRuleAction::Warn,
            json_key_usage_action: JsonRuleAction::Ignore,
            json_key_usage_id_start: default_json_key_usage_id_start(),
        }
    }
}

fn default_schema_loader() -> String {
    "d2rdoc".to_string()
}

fn default_delimiter() -> String {
    "\t".to_string()
}

fn default_locale() -> String {
    Locale::EnUs.as_str().to_string()
}

fn default_extension() -> String {
    "txt".to_string()
}

fn default_json_key_usage_id_start() -> f64 {
    40_000.0
}

fn default_json_key_usage_action() -> JsonRuleAction {
    JsonRuleAction::Ignore
}

fn deserialize_bool_or_string<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum BoolOrString {
        Bool(bool),
        String(String),
    }

    match BoolOrString::deserialize(deserializer)? {
        BoolOrString::Bool(value) => Ok(value),
        BoolOrString::String(value) if value.eq_ignore_ascii_case("true") => Ok(true),
        BoolOrString::String(value) if value.eq_ignore_ascii_case("false") => Ok(false),
        BoolOrString::String(value) => Err(serde::de::Error::custom(format!(
            "expected true or false, got '{value}'"
        ))),
    }
}

fn deserialize_number_or_string<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumberOrString {
        Number(f64),
        String(String),
    }

    match NumberOrString::deserialize(deserializer)? {
        NumberOrString::Number(value) => Ok(value),
        NumberOrString::String(value) => value.parse().map_err(serde::de::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_encoding_matches_editor_cp1252_and_bom_detection() {
        assert_eq!(
            Encoding::Auto
                .decode(&[0x80, 0x91, 0x92, 0x96, 0xE9])
                .unwrap(),
            "€‘’–é"
        );
        assert_eq!(
            Encoding::Auto
                .decode(&[0xFF, 0xFE, b'A', 0, b'B', 0])
                .unwrap(),
            "AB"
        );
        assert_eq!(
            Encoding::Auto
                .decode(&[0xFE, 0xFF, 0, b'A', 0, b'B'])
                .unwrap(),
            "AB"
        );
        assert_eq!(
            Encoding::Auto
                .decode(&[0xEF, 0xBB, 0xBF, b'A', b'B'])
                .unwrap(),
            "AB"
        );
    }

    #[test]
    fn explicit_utf8_encoding_strips_a_utf8_bom_before_table_parsing() {
        assert_eq!(
            Encoding::Utf8
                .decode(&[0xEF, 0xBB, 0xBF, b'c', b'o', b'd', b'e'])
                .unwrap(),
            "code"
        );
    }

    #[test]
    fn settings_validation_rejects_ambiguous_delimiters_and_extensions() {
        let mut settings = VectorLspSettings {
            delimiter: "||".to_string(),
            ..VectorLspSettings::default()
        };
        assert!(
            settings
                .validate()
                .unwrap_err()
                .to_string()
                .contains("delimiter")
        );

        settings.delimiter = "\t".to_string();
        settings.extension = ".txt".to_string();
        assert!(
            settings
                .validate()
                .unwrap_err()
                .to_string()
                .contains("extension")
        );
    }

    #[test]
    fn json_diagnostics_is_opt_in_and_accepts_environment_strings() {
        assert!(!VectorLspSettings::default().json_diagnostics);
        let settings: VectorLspSettings = serde_json::from_value(serde_json::json!({
            "json_diagnostics": "true",
            "json_duplicate_ids_action": "ignore",
            "json_string_format_action": "error",
            "json_key_usage_action": "warn",
            "json_key_usage_id_start": "56032"
        }))
        .unwrap();
        assert!(settings.json_diagnostics);
        assert_eq!(settings.json_duplicate_ids_action, JsonRuleAction::Ignore);
        assert_eq!(settings.json_string_format_action, JsonRuleAction::Warn);
        assert_eq!(settings.json_key_usage_action, JsonRuleAction::Warn);
        assert_eq!(settings.json_key_usage_id_start, 56_032.0);
        assert_eq!(
            settings.json_diagnostic_rules(),
            JsonDiagnosticRules {
                duplicate_ids: JsonRuleAction::Ignore,
                string_format: JsonRuleAction::Warn,
                key_usage: JsonRuleAction::Warn,
                key_usage_id_start: 56_032.0,
            }
        );
    }

    #[test]
    fn vlsp_environment_names_map_to_the_flat_json_rule_fields() {
        let source = config::Environment::with_prefix("VLSP").source(Some({
            let mut environment = std::collections::HashMap::new();
            environment.insert("VLSP_JSON_DIAGNOSTICS".into(), "true".into());
            environment.insert("VLSP_JSON_DUPLICATE_IDS_ACTION".into(), "error".into());
            environment.insert("VLSP_JSON_STRING_FORMAT_ACTION".into(), "ignore".into());
            environment.insert("VLSP_JSON_KEY_USAGE_ACTION".into(), "warn".into());
            environment.insert("VLSP_JSON_KEY_USAGE_ID_START".into(), "12345.5".into());
            environment
        }));
        let settings = config::Config::builder()
            .add_source(source)
            .build()
            .unwrap()
            .try_deserialize::<VectorLspSettings>()
            .unwrap();
        assert!(settings.json_diagnostics);
        assert_eq!(settings.json_duplicate_ids_action, JsonRuleAction::Warn);
        assert_eq!(settings.json_string_format_action, JsonRuleAction::Ignore);
        assert_eq!(settings.json_key_usage_action, JsonRuleAction::Warn);
        assert_eq!(settings.json_key_usage_id_start, 12_345.5);
    }

    #[test]
    fn json_rule_defaults_preserve_the_original_master_checkbox_behavior() {
        let settings: VectorLspSettings = serde_json::from_value(serde_json::json!({
            "json_diagnostics": true
        }))
        .unwrap();
        assert!(settings.json_diagnostics);
        assert_eq!(
            settings.json_diagnostic_rules(),
            JsonDiagnosticRules::default()
        );
        assert_eq!(settings.json_key_usage_action, JsonRuleAction::Ignore);
    }

    #[test]
    fn json_key_usage_id_start_must_be_finite() {
        let settings = VectorLspSettings {
            json_key_usage_id_start: f64::INFINITY,
            ..VectorLspSettings::default()
        };
        assert!(
            settings
                .validate()
                .unwrap_err()
                .to_string()
                .contains("finite")
        );
    }
}
