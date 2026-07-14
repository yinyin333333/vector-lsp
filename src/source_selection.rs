use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use tower_lsp::lsp_types::Url;

use crate::document::DocumentData;

#[derive(Clone)]
pub struct EffectiveSource {
    pub identity: String,
    pub stem: String,
    pub uri: Option<Url>,
    pub path: Option<PathBuf>,
    pub document: Arc<DocumentData>,
    pub is_open: bool,
    pub kind: SourceKind,
    pub bundled_version: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Open,
    Workspace,
    /// A hidden, direct-child TXT loaded from the parent directory of a
    /// standalone document. It participates in reference resolution only.
    Sibling,
    Bundled,
}

pub fn normalized_file_stem_from_uri(uri: &Url) -> Option<String> {
    uri.to_file_path()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_lowercase)
        })
        .or_else(|| {
            uri.path_segments()
                .and_then(|mut segments| segments.next_back())
                .and_then(|name| name.rfind('.').map(|index| name[..index].to_lowercase()))
        })
        .filter(|stem| !stem.is_empty())
}

pub fn effective_workspace_sources(
    open_documents: &HashMap<Url, Arc<DocumentData>>,
    file_cache: &HashMap<PathBuf, Arc<DocumentData>>,
) -> Vec<EffectiveSource> {
    effective_workspace_sources_with_fallback(
        open_documents,
        file_cache,
        &HashMap::new(),
        &HashSet::new(),
        None,
    )
}

pub fn effective_workspace_sources_with_fallback(
    open_documents: &HashMap<Url, Arc<DocumentData>>,
    file_cache: &HashMap<PathBuf, Arc<DocumentData>>,
    fallback_cache: &HashMap<String, Arc<DocumentData>>,
    workspace_present_stems: &HashSet<String>,
    fallback_version: Option<&str>,
) -> Vec<EffectiveSource> {
    effective_workspace_sources_with_fallback_kind(
        open_documents,
        file_cache,
        fallback_cache,
        workspace_present_stems,
        fallback_version,
        SourceKind::Workspace,
    )
}

pub fn effective_workspace_sources_with_fallback_kind(
    open_documents: &HashMap<Url, Arc<DocumentData>>,
    file_cache: &HashMap<PathBuf, Arc<DocumentData>>,
    fallback_cache: &HashMap<String, Arc<DocumentData>>,
    workspace_present_stems: &HashSet<String>,
    fallback_version: Option<&str>,
    disk_source_kind: SourceKind,
) -> Vec<EffectiveSource> {
    effective_workspace_sources_with_priority_tiers(
        open_documents,
        file_cache,
        workspace_present_stems,
        disk_source_kind,
        &HashMap::new(),
        &HashSet::new(),
        fallback_cache,
        fallback_version,
    )
}

/// Resolve one table per stem using the integrated editor priority:
/// open document > primary disk tier > explicit reference-root tier > bundled.
/// In a standalone session the primary disk tier is the direct sibling folder;
/// otherwise it is the normal workspace and the secondary tier is empty.
pub fn effective_workspace_sources_with_priority_tiers(
    open_documents: &HashMap<Url, Arc<DocumentData>>,
    primary_cache: &HashMap<PathBuf, Arc<DocumentData>>,
    primary_present_stems: &HashSet<String>,
    primary_kind: SourceKind,
    reference_root_cache: &HashMap<PathBuf, Arc<DocumentData>>,
    reference_root_present_stems: &HashSet<String>,
    fallback_cache: &HashMap<String, Arc<DocumentData>>,
    fallback_version: Option<&str>,
) -> Vec<EffectiveSource> {
    debug_assert!(matches!(
        primary_kind,
        SourceKind::Workspace | SourceKind::Sibling
    ));
    let mut open_sources = open_documents
        .iter()
        .filter_map(|(uri, document)| {
            Some(EffectiveSource {
                identity: uri.as_str().to_string(),
                stem: normalized_file_stem_from_uri(uri)?,
                uri: Some(uri.clone()),
                path: uri.to_file_path().ok(),
                document: Arc::clone(document),
                is_open: true,
                kind: SourceKind::Open,
                bundled_version: None,
            })
        })
        .collect::<Vec<_>>();
    sort_sources(&mut open_sources);

    let mut selected_stems = HashSet::new();
    let mut selected = open_sources
        .into_iter()
        .filter(|source| selected_stems.insert(source.stem.clone()))
        .collect::<Vec<_>>();
    let mut primary_disk_sources = disk_sources(primary_cache, primary_kind);
    sort_sources(&mut primary_disk_sources);
    selected.extend(
        primary_disk_sources
            .into_iter()
            .filter(|source| selected_stems.insert(source.stem.clone())),
    );
    // A workspace file that failed to parse still owns its table stem. Do not
    // silently replace it with a bundled table and hide the local failure.
    selected_stems.extend(primary_present_stems.iter().map(|stem| stem.to_lowercase()));

    let mut reference_sources = disk_sources(reference_root_cache, SourceKind::Workspace);
    sort_sources(&mut reference_sources);
    selected.extend(
        reference_sources
            .into_iter()
            .filter(|source| selected_stems.insert(source.stem.clone())),
    );
    selected_stems.extend(
        reference_root_present_stems
            .iter()
            .map(|stem| stem.to_lowercase()),
    );

    let mut fallback_sources = fallback_cache
        .iter()
        .filter_map(|(stem, document)| {
            let stem = stem.to_lowercase();
            if stem.is_empty() || selected_stems.contains(&stem) {
                return None;
            }
            Some(EffectiveSource {
                identity: format!("bundled:{stem}"),
                stem,
                uri: None,
                path: None,
                document: Arc::clone(document),
                is_open: false,
                kind: SourceKind::Bundled,
                bundled_version: fallback_version.map(str::to_string),
            })
        })
        .collect::<Vec<_>>();
    sort_sources(&mut fallback_sources);
    selected.extend(
        fallback_sources
            .into_iter()
            .filter(|source| selected_stems.insert(source.stem.clone())),
    );
    // Every effective table participates in one version-scoped session.  Keep
    // that selected version on local/open sources too so hover and version-
    // dependent plugin semantics do not imply that only bundled rows are
    // versioned.
    for source in &mut selected {
        if source.bundled_version.is_none() {
            source.bundled_version = fallback_version.map(str::to_string);
        }
    }
    sort_sources(&mut selected);
    selected
}

fn disk_sources(
    cache: &HashMap<PathBuf, Arc<DocumentData>>,
    kind: SourceKind,
) -> Vec<EffectiveSource> {
    cache
        .iter()
        .filter_map(|(path, document)| {
            let stem = path.file_stem()?.to_str()?.to_lowercase();
            if stem.is_empty() {
                return None;
            }
            let uri = Url::from_file_path(path).ok();
            let identity = uri
                .as_ref()
                .map(|uri| uri.as_str().to_string())
                .unwrap_or_else(|| path.to_string_lossy().replace('\\', "/"));
            Some(EffectiveSource {
                identity,
                stem,
                uri,
                path: Some(path.clone()),
                document: Arc::clone(document),
                is_open: false,
                kind,
                bundled_version: None,
            })
        })
        .collect()
}

fn sort_sources(sources: &mut [EffectiveSource]) {
    sources.sort_by(|left, right| {
        (left.stem.as_str(), left.identity.as_str())
            .cmp(&(right.stem.as_str(), right.identity.as_str()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(value: &str) -> Arc<DocumentData> {
        Arc::new(DocumentData::parse(&format!("Code\n{value}\n"), '\t'))
    }

    fn selected_value(sources: &[EffectiveSource], stem: &str) -> String {
        sources
            .iter()
            .find(|source| source.stem == stem)
            .and_then(|source| source.document.rows.first())
            .and_then(|row| row.cells.first())
            .map(|cell| cell.value.clone())
            .expect("selected table value")
    }

    #[test]
    fn open_then_workspace_then_bundled_precedence_and_close_restore_are_stable() {
        let uri = Url::parse("file:///E:/workspace/itemtypes.txt").unwrap();
        let path = PathBuf::from(r"E:\workspace\itemtypes.txt");
        let mut open = HashMap::new();
        let mut disk = HashMap::new();
        let fallback = HashMap::from([("itemtypes".to_string(), document("bundled"))]);

        disk.insert(path.clone(), document("disk"));
        open.insert(uri.clone(), document("open"));
        let sources = effective_workspace_sources_with_fallback(
            &open,
            &disk,
            &fallback,
            &HashSet::from(["itemtypes".to_string()]),
            Some("3.2"),
        );
        assert_eq!(selected_value(&sources, "itemtypes"), "open");
        assert_eq!(
            sources.iter().find(|s| s.stem == "itemtypes").unwrap().kind,
            SourceKind::Open
        );
        assert_eq!(
            sources
                .iter()
                .find(|s| s.stem == "itemtypes")
                .unwrap()
                .bundled_version
                .as_deref(),
            Some("3.2")
        );

        open.remove(&uri);
        let sources = effective_workspace_sources_with_fallback(
            &open,
            &disk,
            &fallback,
            &HashSet::from(["itemtypes".to_string()]),
            Some("3.2"),
        );
        assert_eq!(selected_value(&sources, "itemtypes"), "disk");
        assert_eq!(
            sources.iter().find(|s| s.stem == "itemtypes").unwrap().kind,
            SourceKind::Workspace
        );

        disk.remove(&path);
        let sources = effective_workspace_sources_with_fallback(
            &open,
            &disk,
            &fallback,
            &HashSet::new(),
            Some("3.2"),
        );
        let selected = sources.iter().find(|s| s.stem == "itemtypes").unwrap();
        assert_eq!(selected_value(&sources, "itemtypes"), "bundled");
        assert_eq!(selected.kind, SourceKind::Bundled);
        assert_eq!(selected.bundled_version.as_deref(), Some("3.2"));
        assert!(selected.uri.is_none() && selected.path.is_none());
    }

    #[test]
    fn present_but_unparsed_workspace_table_blocks_bundled_replacement() {
        let fallback = HashMap::from([("itemtypes".to_string(), document("bundled"))]);
        let sources = effective_workspace_sources_with_fallback(
            &HashMap::new(),
            &HashMap::new(),
            &fallback,
            &HashSet::from(["ITEMTYPES".to_string()]),
            Some("3.2"),
        );
        assert!(sources.iter().all(|source| source.stem != "itemtypes"));
    }

    #[test]
    fn sibling_mode_preserves_open_sibling_bundled_precedence_and_source_tiers() {
        let uri = Url::parse("file:///E:/mod/itemtypes.txt").unwrap();
        let path = PathBuf::from(r"E:\mod\itemtypes.txt");
        let fallback = HashMap::from([("itemtypes".to_string(), document("bundled"))]);
        let mut open = HashMap::new();
        let mut siblings = HashMap::from([(path, document("sibling"))]);

        open.insert(uri.clone(), document("open"));
        let sources = effective_workspace_sources_with_fallback_kind(
            &open,
            &siblings,
            &fallback,
            &HashSet::from(["itemtypes".to_string()]),
            Some("3.2"),
            SourceKind::Sibling,
        );
        let selected = sources.iter().find(|s| s.stem == "itemtypes").unwrap();
        assert_eq!(selected_value(&sources, "itemtypes"), "open");
        assert_eq!(selected.kind, SourceKind::Open);

        open.remove(&uri);
        let sources = effective_workspace_sources_with_fallback_kind(
            &open,
            &siblings,
            &fallback,
            &HashSet::from(["itemtypes".to_string()]),
            Some("3.2"),
            SourceKind::Sibling,
        );
        let selected = sources.iter().find(|s| s.stem == "itemtypes").unwrap();
        assert_eq!(selected_value(&sources, "itemtypes"), "sibling");
        assert_eq!(selected.kind, SourceKind::Sibling);
        assert_eq!(selected.bundled_version.as_deref(), Some("3.2"));

        siblings.clear();
        let sources = effective_workspace_sources_with_fallback_kind(
            &open,
            &siblings,
            &fallback,
            &HashSet::new(),
            Some("3.2"),
            SourceKind::Sibling,
        );
        let selected = sources.iter().find(|s| s.stem == "itemtypes").unwrap();
        assert_eq!(selected_value(&sources, "itemtypes"), "bundled");
        assert_eq!(selected.kind, SourceKind::Bundled);
    }

    #[test]
    fn sibling_then_explicit_reference_root_then_bundled_precedence_is_stable() {
        let sibling_path = PathBuf::from(r"E:\mod\itemtypes.txt");
        let reference_path = PathBuf::from(r"E:\workspace\itemtypes.txt");
        let sibling = HashMap::from([(sibling_path, document("sibling"))]);
        let reference = HashMap::from([(reference_path, document("reference-root"))]);
        let fallback = HashMap::from([("itemtypes".to_string(), document("bundled"))]);

        let sources = effective_workspace_sources_with_priority_tiers(
            &HashMap::new(),
            &sibling,
            &HashSet::from(["itemtypes".to_string()]),
            SourceKind::Sibling,
            &reference,
            &HashSet::from(["itemtypes".to_string()]),
            &fallback,
            Some("3.2"),
        );
        let selected = sources
            .iter()
            .find(|source| source.stem == "itemtypes")
            .unwrap();
        assert_eq!(selected_value(&sources, "itemtypes"), "sibling");
        assert_eq!(selected.kind, SourceKind::Sibling);

        let sources = effective_workspace_sources_with_priority_tiers(
            &HashMap::new(),
            &HashMap::new(),
            &HashSet::new(),
            SourceKind::Sibling,
            &reference,
            &HashSet::from(["itemtypes".to_string()]),
            &fallback,
            Some("3.2"),
        );
        let selected = sources
            .iter()
            .find(|source| source.stem == "itemtypes")
            .unwrap();
        assert_eq!(selected_value(&sources, "itemtypes"), "reference-root");
        assert_eq!(selected.kind, SourceKind::Workspace);
        assert_eq!(selected.bundled_version.as_deref(), Some("3.2"));

        let sources = effective_workspace_sources_with_priority_tiers(
            &HashMap::new(),
            &HashMap::new(),
            &HashSet::new(),
            SourceKind::Sibling,
            &HashMap::new(),
            &HashSet::new(),
            &fallback,
            Some("3.2"),
        );
        assert_eq!(selected_value(&sources, "itemtypes"), "bundled");
    }
}
