use serde::Deserialize;
use std::path::PathBuf;

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
            Encoding::Utf8 => Ok(String::from_utf8_lossy(bytes).into_owned()),
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
    /// Workspace directory to use in single-shot mode (and optionally in LSP mode).
    pub workspace_path: Option<PathBuf>,
    /// When true, validate the workspace and exit instead of starting the LSP server.
    #[serde(default)]
    pub single_shot: bool,
}

impl VectorLspSettings {
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
        Ok(())
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
            "editorMode={} transport={} singleShot={} encoding={:?} schema={} pluginPath={}",
            self.editor_mode,
            if matches!(self.io_type, IoType::Stdio) {
                "stdio"
            } else {
                "tcp"
            },
            self.single_shot,
            self.encoding,
            schema,
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
        }
    }
}

fn default_schema_loader() -> String {
    "d2rdoc".to_string()
}

fn default_delimiter() -> String {
    "\t".to_string()
}

fn default_extension() -> String {
    "txt".to_string()
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
    fn settings_validation_rejects_ambiguous_delimiters_and_extensions() {
        let mut settings = VectorLspSettings::default();
        settings.delimiter = "||".to_string();
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
}
