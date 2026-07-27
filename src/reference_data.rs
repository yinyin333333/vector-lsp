use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::document::DocumentData;
use crate::settings::{Encoding, VectorLspSettings};

const EXPECTED_ROOT_SHA256: &str =
    "6930d9c39b5380fd4c242bae4df24a9b0115386bdc074cb8482007e2a033cab0";
const EXPECTED_DATASETS: &[(&str, &str, &str, usize, u64, &str)] = &[
    (
        "1.13",
        "1.13c",
        "113c",
        64,
        2_920_000,
        "80ae8704937825906ec456c2d843fa173b5e940300a38eb9ab67ea615b2aa71f",
    ),
    (
        "2.4",
        "2.4",
        "69270",
        85,
        4_585_088,
        "1e3e03fa3138debd1b87c6eec0a68d68fc76069842ccf6abffefa7be0d42c008",
    ),
    (
        "3.1",
        "3.1",
        "92198",
        91,
        5_077_001,
        "8479a35241ad05196fc99c2219d8bd934ee3fc7820e0c0f5553fed07847d0152",
    ),
    (
        "3.2",
        "3.2",
        "92777a",
        91,
        5_144_477,
        "7149352429c5d5ff3e641adb75ce6ff683ce4db6c390651c928f336f8dcddc75",
    ),
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReferenceManifest {
    format_version: u32,
    total_file_count: usize,
    total_bytes: u64,
    canonical_sha256: String,
    datasets: Vec<DatasetManifest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DatasetManifest {
    schema_variant: String,
    game_version: String,
    source: DatasetSource,
    resource_path: String,
    file_count: usize,
    total_bytes: u64,
    canonical_sha256: String,
    files: Vec<FileManifest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DatasetSource {
    dataset_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileManifest {
    path: String,
    bytes: u64,
    encoding: String,
    sha256: String,
}

#[derive(Clone)]
pub struct ReferenceDataset {
    #[cfg(test)]
    pub schema_variant: String,
    pub game_version: String,
    pub canonical_sha256: String,
    pub documents: HashMap<String, Arc<DocumentData>>,
}

pub fn selected_reference_variant(settings: &VectorLspSettings) -> Result<Option<String>> {
    let requested = if settings.reference_variant.trim().is_empty() {
        settings.schema_variant.trim()
    } else {
        settings.reference_variant.trim()
    };
    if requested.is_empty() {
        return Ok(None);
    }
    let normalized = match requested.to_ascii_lowercase().as_str() {
        "1.13" | "1.13c" => "1.13",
        "2.4" => "2.4",
        "3.1" => "3.1",
        "3.2" => "3.2",
        _ => bail!("unsupported reference variant '{requested}'; choose 1.13c, 2.4, 3.1, or 3.2"),
    };
    Ok(Some(normalized.to_string()))
}

pub fn load_selected_reference_dataset(
    settings: &VectorLspSettings,
) -> Result<Option<ReferenceDataset>> {
    let Some(schema_variant) = selected_reference_variant(settings)? else {
        return Ok(None);
    };
    let contrib_root = crate::contrib::d2rdoc::D2rDocLoader::contrib_root();
    load_reference_dataset(&contrib_root, &schema_variant).map(Some)
}

pub(crate) fn load_reference_dataset(
    contrib_root: &Path,
    schema_variant: &str,
) -> Result<ReferenceDataset> {
    let manifest_path = contrib_root.join("reference-manifest.json");
    let manifest_bytes = std::fs::read(&manifest_path)
        .with_context(|| format!("cannot read {}", manifest_path.display()))?;
    let manifest: ReferenceManifest = serde_json::from_slice(&manifest_bytes)
        .with_context(|| format!("invalid {}", manifest_path.display()))?;
    ensure!(
        manifest.format_version == 1,
        "unsupported reference manifest format {}",
        manifest.format_version
    );
    ensure!(
        manifest
            .canonical_sha256
            .eq_ignore_ascii_case(EXPECTED_ROOT_SHA256),
        "reference manifest root digest mismatch"
    );
    ensure!(
        manifest.total_file_count == 331 && manifest.total_bytes == 17_726_566,
        "reference manifest inventory mismatch"
    );

    verify_manifest(&manifest)?;

    let dataset = manifest
        .datasets
        .iter()
        .find(|dataset| dataset.schema_variant.eq_ignore_ascii_case(schema_variant))
        .ok_or_else(|| anyhow!("reference dataset '{schema_variant}' is not in the manifest"))?;

    let resource_root = safe_join(contrib_root, &dataset.resource_path)?;
    let mut documents = HashMap::with_capacity(dataset.file_count);
    let mut seen_paths = HashSet::with_capacity(dataset.file_count);
    let mut canonical_lines = Vec::with_capacity(dataset.file_count);
    let mut actual_total = 0u64;

    let mut files = dataset.files.iter().collect::<Vec<_>>();
    files.sort_by_key(|file| file.path.to_ascii_lowercase());
    for file in files {
        let lower_path = file.path.replace('\\', "/").to_ascii_lowercase();
        ensure!(
            seen_paths.insert(lower_path.clone()),
            "duplicate reference manifest path '{}'",
            file.path
        );
        let path = safe_join(&resource_root, &file.path)?;
        let bytes = std::fs::read(&path)
            .with_context(|| format!("cannot read bundled reference file {}", path.display()))?;
        let actual_hash = sha256_hex(&bytes);
        ensure!(
            bytes.len() as u64 == file.bytes && actual_hash.eq_ignore_ascii_case(&file.sha256),
            "bundled reference file '{}' failed size/hash verification",
            file.path
        );
        actual_total += bytes.len() as u64;
        canonical_lines.push(format!("{}  {}\n", actual_hash, lower_path));

        let (text, detected_encoding) = decode_reference_bytes(&bytes)?;
        ensure!(
            detected_encoding == file.encoding,
            "bundled reference file '{}' encoding mismatch: manifest={}, detected={}",
            file.path,
            file.encoding,
            detected_encoding
        );
        let stem = Path::new(&file.path)
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| anyhow!("invalid reference filename '{}'", file.path))?
            .to_ascii_lowercase();
        ensure!(
            documents
                .insert(stem.clone(), Arc::new(DocumentData::parse(&text, '\t')))
                .is_none(),
            "duplicate reference table stem '{stem}'"
        );
    }

    ensure!(
        actual_total == dataset.total_bytes,
        "reference dataset '{}' total byte count mismatch",
        dataset.game_version
    );
    let canonical_hash = sha256_hex(canonical_lines.concat().as_bytes());
    ensure!(
        canonical_hash.eq_ignore_ascii_case(&dataset.canonical_sha256),
        "reference dataset '{}' canonical digest mismatch",
        dataset.game_version
    );

    Ok(ReferenceDataset {
        #[cfg(test)]
        schema_variant: dataset.schema_variant.clone(),
        game_version: dataset.game_version.clone(),
        canonical_sha256: dataset.canonical_sha256.clone(),
        documents,
    })
}

fn verify_manifest(manifest: &ReferenceManifest) -> Result<()> {
    ensure!(
        manifest.datasets.len() == EXPECTED_DATASETS.len(),
        "reference manifest dataset count mismatch"
    );
    let mut seen_variants = HashSet::with_capacity(EXPECTED_DATASETS.len());
    let mut root_canonical = String::new();
    let mut total_files = 0usize;
    let mut total_bytes = 0u64;

    // Canonical root order is a product invariant, not manifest array order.
    for (schema_variant, game_version, dataset_id, file_count, bytes, digest) in EXPECTED_DATASETS {
        let dataset = manifest
            .datasets
            .iter()
            .find(|candidate| {
                candidate
                    .schema_variant
                    .eq_ignore_ascii_case(schema_variant)
            })
            .ok_or_else(|| anyhow!("reference dataset '{schema_variant}' is missing"))?;
        ensure!(
            seen_variants.insert(dataset.schema_variant.to_ascii_lowercase()),
            "duplicate reference schema variant '{}'",
            dataset.schema_variant
        );
        ensure!(
            dataset.schema_variant == *schema_variant
                && dataset.game_version == *game_version
                && dataset.source.dataset_id == *dataset_id
                && dataset.resource_path == format!("{schema_variant}/reference")
                && dataset.file_count == *file_count
                && dataset.total_bytes == *bytes
                && dataset.canonical_sha256.eq_ignore_ascii_case(digest)
                && dataset.files.len() == dataset.file_count,
            "reference dataset '{}'/{} mapping, inventory, or digest does not match the bundled baseline",
            dataset.schema_variant,
            dataset.game_version
        );

        let mut paths = HashSet::with_capacity(dataset.file_count);
        let mut dataset_canonical = String::new();
        let mut files = dataset.files.iter().collect::<Vec<_>>();
        files.sort_by_key(|file| file.path.to_ascii_lowercase());
        for file in files {
            let lower_path = file.path.replace('\\', "/").to_ascii_lowercase();
            ensure!(
                paths.insert(lower_path.clone()),
                "duplicate reference manifest path '{}' in {}",
                file.path,
                dataset.game_version
            );
            safe_join(Path::new("."), &file.path)?;
            ensure!(
                file.sha256.len() == 64
                    && file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && matches!(file.encoding.as_str(), "utf-8" | "windows-1252"),
                "invalid hash or encoding metadata for '{}'/{}",
                dataset.game_version,
                file.path
            );
            dataset_canonical.push_str(&file.sha256.to_ascii_lowercase());
            dataset_canonical.push_str("  ");
            dataset_canonical.push_str(&lower_path);
            dataset_canonical.push('\n');
            root_canonical.push_str(&file.sha256.to_ascii_lowercase());
            root_canonical.push_str("  ");
            root_canonical.push_str(game_version);
            root_canonical.push('/');
            root_canonical.push_str(&lower_path);
            root_canonical.push('\n');
        }
        ensure!(
            sha256_hex(dataset_canonical.as_bytes()).eq_ignore_ascii_case(digest),
            "reference dataset '{}' manifest file list digest mismatch",
            game_version
        );
        total_files += dataset.file_count;
        total_bytes += dataset.total_bytes;
    }

    ensure!(
        total_files == manifest.total_file_count
            && total_bytes == manifest.total_bytes
            && sha256_hex(root_canonical.as_bytes()).eq_ignore_ascii_case(EXPECTED_ROOT_SHA256),
        "reference manifest aggregate inventory or canonical digest mismatch"
    );
    Ok(())
}

fn safe_join(root: &Path, relative: &str) -> Result<PathBuf> {
    let relative = Path::new(relative);
    ensure!(
        !relative.is_absolute()
            && relative
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
        "unsafe bundled reference path '{}'",
        relative.display()
    );
    Ok(root.join(relative))
}

fn decode_reference_bytes(bytes: &[u8]) -> Result<(String, String)> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok((text.to_string(), "utf-8".to_string())),
        Err(_) => Ok((Encoding::Auto.decode(bytes)?, "windows-1252".to_string())),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_versions_are_explicit_and_1_13_maps_to_1_13c_resources() {
        let mut settings = VectorLspSettings::default();
        settings.schema_variant = "1.13".to_string();
        assert_eq!(
            selected_reference_variant(&settings).unwrap().as_deref(),
            Some("1.13")
        );

        settings.reference_variant = "1.13c".to_string();
        assert_eq!(
            selected_reference_variant(&settings).unwrap().as_deref(),
            Some("1.13")
        );

        settings.reference_variant = "custom".to_string();
        assert!(selected_reference_variant(&settings).is_err());
    }

    #[test]
    fn no_schema_or_explicit_reference_variant_disables_fallback() {
        let settings = VectorLspSettings::default();
        assert_eq!(selected_reference_variant(&settings).unwrap(), None);
    }

    #[test]
    fn every_packaged_dataset_matches_its_pinned_version_inventory() {
        let contrib_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("contrib")
            .join("d2rdoc");
        for (schema_variant, game_version, _, file_count, _, digest) in EXPECTED_DATASETS {
            let dataset = load_reference_dataset(&contrib_root, schema_variant).unwrap();
            assert_eq!(dataset.schema_variant, *schema_variant);
            assert_eq!(dataset.game_version, *game_version);
            assert_eq!(dataset.documents.len(), *file_count);
            assert_eq!(dataset.canonical_sha256, *digest);
        }
    }
}
