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
            })
        })
        .collect::<Vec<_>>();
    sort_sources(&mut open_sources);

    let mut selected_stems = HashSet::new();
    let mut selected = open_sources
        .into_iter()
        .filter(|source| selected_stems.insert(source.stem.clone()))
        .collect::<Vec<_>>();
    let mut disk_sources = file_cache
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
            })
        })
        .collect::<Vec<_>>();
    sort_sources(&mut disk_sources);
    selected.extend(
        disk_sources
            .into_iter()
            .filter(|source| selected_stems.insert(source.stem.clone())),
    );
    sort_sources(&mut selected);
    selected
}

fn sort_sources(sources: &mut [EffectiveSource]) {
    sources.sort_by(|left, right| {
        (left.stem.as_str(), left.identity.as_str())
            .cmp(&(right.stem.as_str(), right.identity.as_str()))
    });
}
