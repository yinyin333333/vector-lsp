use std::path::PathBuf;

use super::SchemaLoader;

/// Registration record submitted by each contrib loader via `inventory::submit!`.
///
/// # Example — registering a new loader in `contrib/myloader/mod.rs`
/// ```ignore
/// inventory::submit!(LoaderEntry {
///     id: "myloader",
///     create: |variant, patches_dir| Box::new(MyLoader { variant, patches_dir }),
/// });
/// ```
pub struct LoaderEntry {
    /// Identifier used in the `schema_loader` config key to select this driver.
    pub id: &'static str,
    /// Factory called once at startup to produce a configured loader instance.
    /// `variant` selects the bundled schema/plugin set; `patches_dir` is the
    /// explicit `plugin_path` from settings (used for `_patches.js` overrides).
    pub create: fn(variant: String, patches_dir: Option<PathBuf>) -> Box<dyn SchemaLoader>,
}

inventory::collect!(LoaderEntry);

/// Find and instantiate the loader registered under `id`.
/// Returns an error listing all available loader IDs if none matches.
pub fn find_loader(
    id: &str,
    variant: String,
    patches_dir: Option<PathBuf>,
) -> anyhow::Result<Box<dyn SchemaLoader>> {
    for entry in inventory::iter::<LoaderEntry>() {
        if entry.id == id || (id.is_empty() && entry.id == "d2rdoc") {
            return Ok((entry.create)(variant, patches_dir));
        }
    }
    let available: Vec<&str> = inventory::iter::<LoaderEntry>().map(|e| e.id).collect();
    anyhow::bail!(
        "unknown schema loader '{id}'. Available: {}",
        if available.is_empty() {
            "none registered".to_string()
        } else {
            available.join(", ")
        }
    )
}
