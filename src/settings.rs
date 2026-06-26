use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
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
            Encoding::Utf8 => Ok(String::from_utf8_lossy(bytes).into_owned()),
            Encoding::Latin1 => Ok(bytes.iter().map(|&b| b as char).collect()),
            Encoding::Utf16Le => Self::decode_utf16(bytes, false),
            Encoding::Utf16Be => Self::decode_utf16(bytes, true),
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
}

impl Default for VectorLspSettings {
    fn default() -> Self {
        Self {
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
