use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use crate::runtime::ScriptRuntime;
use crate::schema::registry::LoaderEntry;
use crate::schema::{
    FieldTypeName, ReferenceResolver, Schema, SchemaField, SchemaFile, SchemaLoader,
};

/// Variant names that cannot be used as `schema_variant` values because they
/// conflict with reserved subdirectory names in the contrib layout.
const RESERVED_VARIANT_NAMES: &[&str] = &["plugins"];

// Self-register with the schema loader registry.
inventory::submit!(LoaderEntry {
    id: "d2rdoc",
    create: |variant, patches_dir| Box::new(D2rDocLoader {
        variant,
        patches_dir
    }),
});

/// Schema loader for the d2rdoc JavaScript schema format.
///
/// Schema and plugin files are discovered in two ways:
///
/// 1. **Auto-discovery** (no explicit path in config): resolved relative to the
///    running executable under `contrib/d2rdoc/`.
/// 2. **Explicit override**: `schema_path` / `plugin_path` in config fully
///    replace the auto-discovered paths.
///
/// Plugin directories are tiered:
/// - `{exe}/contrib/d2rdoc/plugins/`          — base plugins, all variants
/// - `{exe}/contrib/d2rdoc/{variant}/plugins/` — variant-specific plugins
pub struct D2rDocLoader {
    /// Selected schema/plugin set, e.g. `"d2r-2.7"`. Empty string means no
    /// auto-discovery (explicit paths must be provided).
    pub variant: String,
    /// Explicit plugin directory from settings, used for `_patches.js` lookups.
    pub patches_dir: Option<PathBuf>,
}

impl D2rDocLoader {
    /// Root of the d2rdoc contrib tree: `{exe_dir}/contrib/d2rdoc/`.
    pub(crate) fn contrib_root() -> PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_owned()))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("contrib")
            .join("d2rdoc")
    }

    /// Resolve the schema directory, falling back to auto-discovery when
    /// `explicit` is `None`.
    fn effective_schema_dir(&self, explicit: Option<&Path>) -> Option<PathBuf> {
        if let Some(p) = explicit {
            return Some(p.to_owned());
        }
        if self.variant.is_empty() {
            return None;
        }
        Some(Self::contrib_root().join(&self.variant).join("schema"))
    }

    /// Resolve the patches directory for `_patches.js` lookup.
    ///
    /// Resolution order:
    /// 1. Explicit `plugin_path` from settings (highest priority).
    /// 2. `{exe}/contrib/d2rdoc/` — global patches, shared across all variants.
    /// 3. `{exe}/contrib/d2rdoc/{variant}/plugins/` — variant-specific patches.
    fn effective_patches_dir(&self) -> Option<PathBuf> {
        if let Some(ref p) = self.patches_dir {
            return Some(p.clone());
        }
        let root = Self::contrib_root();
        let global = root.join("_patches.js");
        if global.exists() {
            return Some(root);
        }
        if self.variant.is_empty() {
            return None;
        }
        let dir = root.join(&self.variant).join("plugins");
        if dir.exists() { Some(dir) } else { None }
    }
}

impl SchemaLoader for D2rDocLoader {
    fn load(&self, explicit_dir: Option<&Path>) -> Result<Schema> {
        if RESERVED_VARIANT_NAMES.contains(&self.variant.as_str()) {
            anyhow::bail!(
                "schema_variant '{}' is reserved and cannot be used",
                self.variant
            );
        }

        let schema_dir = self
            .effective_schema_dir(explicit_dir)
            .ok_or_else(|| anyhow!("no schema directory: set schema_path or schema_variant"))?;

        let patches_dir = self.effective_patches_dir();
        let mut rt = ScriptRuntime::new()?;
        let mut schema = load_js(&mut rt, &schema_dir, patches_dir.as_deref())?;
        if self.variant == "1.13" {
            patch_1_13_reference_semantics(&mut schema);
        }
        if self.variant == "2.4" {
            patch_2_4_reference_semantics(&mut schema);
        }
        if self.variant == "3.2" {
            patch_3_2_reference_semantics(&mut schema);
        }
        Ok(schema)
    }

    /// Returns plugin directories in load order (base first, then variant).
    /// The caller appends any explicit `plugin_path` from settings on top.
    fn default_plugin_dirs(&self) -> Vec<PathBuf> {
        let root = Self::contrib_root();
        let mut dirs = vec![root.join("plugins")];
        if !self.variant.is_empty() {
            dirs.push(root.join(&self.variant).join("plugins"));
        }
        dirs
    }
}

fn patch_1_13_reference_semantics(schema: &mut Schema) {
    if let Some(id) = schema_field_mut(schema, "monprop", "Id") {
        if let Some(field_type) = id.field_type.as_mut() {
            field_type.type_name = FieldTypeName::String;
        }
        id.description = Some(
            "Defines the unique MonProp name referenced by the MonProp field in monstats.txt"
                .to_string(),
        );
    }

    // D2Common's 1.13 loader stores these as uint8_t/uint16_t fields.  oninit
    // is consumed as zero/nonzero, so it must not use the schema Boolean
    // validator, which accepts only the canonical spellings 0 and 1.
    for (field, mem_size) in [("oninit", 8), ("level", 16), ("mod#", 8)] {
        if let Some(field) = schema_field_mut(schema, "monequip", field) {
            if let Some(field_type) = field.field_type.as_mut() {
                field_type.type_name = FieldTypeName::Int;
                field_type.data_length = 0;
                field_type.mem_size = mem_size;
            }
        }
    }
}

fn patch_2_4_reference_semantics(schema: &mut Schema) {
    for (file, field) in [
        ("shareditems", "TMogType"),
        ("monstats", "Id"),
        ("monseq", "sequence"),
        ("automagic", "transformcolor"),
    ] {
        if let Some(field) = schema_field_mut(schema, file, field) {
            if let Some(field_type) = field.field_type.as_mut() {
                field_type.type_name = FieldTypeName::String;
            }
        }
    }

    if let Some(monprop_id) = schema_field_mut(schema, "monprop", "Id") {
        if let Some(field_type) = monprop_id.field_type.as_mut() {
            field_type.type_name = FieldTypeName::Reference;
            field_type.data_length = 47;
            field_type.mem_size = 16;
            field_type.file = Some("monstats".to_string());
            field_type.field = Some("Id".to_string());
        }
    }
}

fn schema_field_mut<'a>(
    schema: &'a mut Schema,
    file_name: &str,
    field_name: &str,
) -> Option<&'a mut SchemaField> {
    schema
        .files
        .iter_mut()
        .find(|(name, _)| name.eq_ignore_ascii_case(file_name))
        .and_then(|(_, file)| {
            file.fields
                .iter_mut()
                .find(|field| field.name.eq_ignore_ascii_case(field_name))
        })
}

fn patch_3_2_reference_semantics(schema: &mut Schema) {
    let Some(skills) = schema.files.get_mut("skills") else {
        return;
    };
    let Some(range) = skills
        .fields
        .iter_mut()
        .find(|field| field.name.eq_ignore_ascii_case("range"))
    else {
        return;
    };
    if let Some(field_type) = range.field_type.as_mut() {
        field_type.resolver = ReferenceResolver::Fixed4;
    }
}

// ---------------------------------------------------------------------------
// JS schema loading
// ---------------------------------------------------------------------------

fn load_js(runtime: &mut ScriptRuntime, dir: &Path, patches_dir: Option<&Path>) -> Result<Schema> {
    runtime.exec("__init__", "var files = {};")?;

    let mut paths: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().map_or(false, |ext| ext == "js")
                && p.file_name().map_or(true, |n| n != "_patches.js")
        })
        .collect();
    paths.sort();

    for path in paths {
        runtime.exec_file(&path)?;
    }

    // _patches.js is executed last so it can override any schema entry.
    if let Some(pd) = patches_dir {
        let patches = pd.join("_patches.js");
        if patches.exists() {
            runtime.exec_file(&patches)?;
        }
    }

    let json = runtime.eval_json("files")?;
    let map = json
        .as_object()
        .ok_or_else(|| anyhow!("files is not an object"))?;
    let mut files: HashMap<String, SchemaFile> = HashMap::new();
    for (key, val) in map {
        if val.is_null() {
            continue;
        }
        match serde_json::from_value::<SchemaFile>(val.clone()) {
            Ok(sf) => {
                files.insert(key.clone(), sf);
            }
            Err(e) => return Err(anyhow!("schema key '{}': {}", key, e)),
        }
    }
    Ok(Schema { files })
}
