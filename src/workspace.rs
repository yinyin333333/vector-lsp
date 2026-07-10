use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::document::{DocumentData, utf16_len};
use crate::plugin;
use crate::runtime::{self, WorkspaceFileSnapshot, WorkspaceIndex};
use crate::schema::Schema;
use crate::source_selection::{effective_workspace_sources, normalized_file_stem_from_uri};

#[derive(Clone)]
pub struct PluginWorkspaceView {
    pub session_generation: u64,
    pub workspace_revision: u64,
    pub index: Arc<WorkspaceIndex>,
    pub snapshot: Arc<WorkspaceFileSnapshot>,
}

/// Cross-file symbol index.
///
/// Key: `(file_stem, column_name, cell_value)` — all three components lowercased.
/// Value: the LSP Location of that cell in the workspace.
///
/// Only columns that are `reference` targets in the schema are stored, keeping
/// memory usage proportional to what go-to-definition actually needs.
#[derive(Clone)]
pub struct SymbolIndex {
    entries: HashMap<(String, String, String), Location>,
    columns: HashSet<(String, String)>,
    files: HashSet<String>,
}

impl SymbolIndex {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            columns: HashSet::new(),
            files: HashSet::new(),
        }
    }

    /// Index the cells of `doc` that belong to columns listed in `ref_targets`.
    pub fn index_document(
        &mut self,
        uri: &Url,
        file_stem: &str,
        doc: &DocumentData,
        ref_targets: &HashSet<(String, String)>,
    ) {
        let stem = file_stem.to_lowercase();
        self.files.insert(stem.clone());
        for header in &doc.headers {
            if !header.is_empty() {
                self.columns.insert((stem.clone(), header.to_lowercase()));
            }
        }
        for row in &doc.rows {
            if row
                .cells
                .first()
                .map(|cell| cell.value.trim_start().starts_with('*'))
                .unwrap_or(false)
            {
                continue;
            }
            for (col_idx, cell) in row.cells.iter().enumerate() {
                if cell.value.trim().is_empty() {
                    continue;
                }
                let col_name = match doc.headers.get(col_idx) {
                    Some(h) => h.as_str(),
                    None => continue,
                };
                let col_lower = col_name.to_lowercase();
                if !ref_targets.contains(&(stem.clone(), col_lower.clone())) {
                    continue;
                }
                let end_char = cell.col_start + utf16_len(&cell.value);
                self.entries.insert(
                    (stem.clone(), col_lower, cell.value.to_lowercase()),
                    Location {
                        uri: uri.clone(),
                        range: Range {
                            start: Position {
                                line: row.line,
                                character: cell.col_start,
                            },
                            end: Position {
                                line: row.line,
                                character: end_char,
                            },
                        },
                    },
                );
            }
        }
    }

    /// Look up the location of a specific value in a specific column of a specific file.
    pub fn lookup(&self, file_stem: &str, column: &str, value: &str) -> Option<&Location> {
        self.entries.get(&(
            file_stem.to_lowercase(),
            column.to_lowercase(),
            value.to_lowercase(),
        ))
    }

    pub fn has_file(&self, file_stem: &str) -> bool {
        self.files.contains(&file_stem.to_lowercase())
    }

    pub fn has_column(&self, file_stem: &str, column: &str) -> bool {
        self.columns
            .contains(&(file_stem.to_lowercase(), column.to_lowercase()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspacePhase {
    Cold,
    LoadingSchema,
    Scanning,
    Reconciling,
    Ready,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationTicket {
    pub session_generation: u64,
    pub scan_generation: u64,
    pub workspace_revision: u64,
    pub uri: Url,
    pub document_epoch: u64,
    pub document_revision: u64,
    pub client_version: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentChangeError {
    NotOpen,
    StaleVersion { current: i32, incoming: i32 },
}

pub struct Workspace {
    pub root_uri: Option<Url>,
    /// Documents currently open in the editor (managed via didOpen/didChange).
    pub open_documents: HashMap<Url, Arc<DocumentData>>,
    /// All other workspace files parsed from disk on startup.
    pub file_cache: HashMap<PathBuf, Arc<DocumentData>>,
    pub symbols: SymbolIndex,
    /// Schema loaded from the configured schema directory, if any.
    /// Stored as `Arc` so it can be shared cheaply with the plugin host.
    pub schema: Option<Arc<Schema>>,
    /// Cached set of `(file_stem, column_name)` reference targets derived from the schema.
    /// Drives what SymbolIndex stores — populated once when the schema loads.
    pub ref_targets: HashSet<(String, String)>,
    pub phase: WorkspacePhase,
    pub session_generation: u64,
    pub scan_generation: u64,
    pub workspace_revision: u64,
    document_versions: HashMap<Url, i32>,
    document_epochs: HashMap<Url, u64>,
    document_revisions: HashMap<Url, u64>,
    published_revisions: HashMap<Url, u64>,
    published_disk_diagnostics: HashSet<Url>,
    next_document_epoch: u64,
    next_document_revision: u64,
    plugin_workspace_view: Mutex<Option<PluginWorkspaceView>>,
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            root_uri: None,
            open_documents: HashMap::new(),
            file_cache: HashMap::new(),
            symbols: SymbolIndex::new(),
            schema: None,
            ref_targets: HashSet::new(),
            phase: WorkspacePhase::Cold,
            session_generation: 0,
            scan_generation: 0,
            workspace_revision: 0,
            document_versions: HashMap::new(),
            document_epochs: HashMap::new(),
            document_revisions: HashMap::new(),
            published_revisions: HashMap::new(),
            published_disk_diagnostics: HashSet::new(),
            next_document_epoch: 0,
            next_document_revision: 0,
            plugin_workspace_view: Mutex::new(None),
        }
    }

    pub fn plugin_workspace_view(&self) -> PluginWorkspaceView {
        let mut cached = self.plugin_workspace_view.lock().unwrap();
        if let Some(view) = cached.as_ref()
            && view.session_generation == self.session_generation
            && view.workspace_revision == self.workspace_revision
        {
            return view.clone();
        }
        let view = PluginWorkspaceView {
            session_generation: self.session_generation,
            workspace_revision: self.workspace_revision,
            index: runtime::build_workspace_index(&self.open_documents, &self.file_cache),
            snapshot: plugin::build_workspace_snapshot(&self.open_documents, &self.file_cache),
        };
        *cached = Some(view.clone());
        view
    }

    pub fn begin_initialization(&mut self, session_generation: u64) {
        self.session_generation = session_generation;
        self.phase = WorkspacePhase::LoadingSchema;
    }

    pub fn begin_scan(&mut self) -> u64 {
        self.scan_generation += 1;
        self.phase = WorkspacePhase::Scanning;
        self.scan_generation
    }

    pub fn begin_reconciliation(&mut self, scan_generation: u64) -> bool {
        if self.scan_generation != scan_generation {
            return false;
        }
        self.workspace_revision += 1;
        self.phase = WorkspacePhase::Reconciling;
        true
    }

    pub fn commit_scan_documents(
        &mut self,
        scan_generation: u64,
        parsed: &[(Url, PathBuf, String, Arc<DocumentData>)],
    ) -> bool {
        if self.scan_generation != scan_generation {
            return false;
        }
        self.file_cache.clear();
        for (_, path, _, document) in parsed {
            self.file_cache.insert(path.clone(), Arc::clone(document));
        }
        self.rebuild_effective_symbols();
        self.begin_reconciliation(scan_generation)
    }

    pub fn rebuild_effective_symbols(&mut self) {
        self.symbols = SymbolIndex::new();
        for source in effective_workspace_sources(&self.open_documents, &self.file_cache) {
            let Some(uri) = source.uri else { continue };
            self.symbols
                .index_document(&uri, &source.stem, &source.document, &self.ref_targets);
        }
    }

    pub fn mark_failed(&mut self) {
        self.phase = WorkspacePhase::Failed;
    }

    pub fn accept_open(
        &mut self,
        uri: Url,
        version: i32,
        document: Arc<DocumentData>,
    ) -> ValidationTicket {
        self.next_document_epoch += 1;
        self.next_document_revision += 1;
        self.workspace_revision += 1;
        self.open_documents.insert(uri.clone(), document);
        self.document_versions.insert(uri.clone(), version);
        self.document_epochs
            .insert(uri.clone(), self.next_document_epoch);
        self.document_revisions
            .insert(uri.clone(), self.next_document_revision);
        self.published_revisions.clear();
        self.current_ticket(&uri).expect("accepted open document")
    }

    pub fn accept_change(
        &mut self,
        uri: &Url,
        version: i32,
        document: Arc<DocumentData>,
    ) -> Result<ValidationTicket, DocumentChangeError> {
        let Some(current_version) = self.document_versions.get(uri).copied() else {
            return Err(DocumentChangeError::NotOpen);
        };
        if version <= current_version {
            return Err(DocumentChangeError::StaleVersion {
                current: current_version,
                incoming: version,
            });
        }
        self.next_document_revision += 1;
        self.workspace_revision += 1;
        self.open_documents.insert(uri.clone(), document);
        self.document_versions.insert(uri.clone(), version);
        self.document_revisions
            .insert(uri.clone(), self.next_document_revision);
        self.published_revisions.clear();
        self.current_ticket(uri).ok_or(DocumentChangeError::NotOpen)
    }

    pub fn close_open_document(&mut self, uri: &Url) -> Option<Arc<DocumentData>> {
        let removed = self.open_documents.remove(uri);
        if removed.is_some() {
            self.workspace_revision += 1;
        }
        self.document_versions.remove(uri);
        self.document_epochs.remove(uri);
        self.document_revisions.remove(uri);
        self.published_revisions.clear();
        removed
    }

    pub fn restore_closed_document(
        &mut self,
        uri: &Url,
        path: Option<PathBuf>,
        disk_document: Option<Arc<DocumentData>>,
    ) -> bool {
        if self.close_open_document(uri).is_none() {
            return false;
        }
        if let Some(path) = path {
            self.file_cache.remove(&path);
            if let Some(document) = disk_document {
                self.file_cache.insert(path, document);
            }
        }
        self.rebuild_effective_symbols();
        true
    }

    pub fn disk_diagnostics_allowed(&self, uri: &Url) -> bool {
        let stem = normalized_file_stem_from_uri(uri);
        !self
            .open_documents
            .keys()
            .any(|open_uri| stem.is_some() && normalized_file_stem_from_uri(open_uri) == stem)
    }

    pub fn disk_documents_for_validation(&self) -> Vec<(Url, String, Arc<DocumentData>)> {
        // Choose the lexical winner before any consumer-specific filtering so a
        // duplicate loser can never replace the shared source.
        effective_workspace_sources(&self.open_documents, &self.file_cache)
            .into_iter()
            .filter_map(|source| {
                if source.is_open {
                    return None;
                }
                let uri = source.uri?;
                self.disk_diagnostics_allowed(&uri)
                    .then_some((uri, source.stem, source.document))
            })
            .collect()
    }

    pub fn record_disk_diagnostics(&mut self, uri: Url, has_diagnostics: bool) {
        if has_diagnostics {
            self.published_disk_diagnostics.insert(uri);
        } else {
            self.published_disk_diagnostics.remove(&uri);
        }
    }

    pub fn forget_disk_diagnostics(&mut self, uri: &Url) {
        self.published_disk_diagnostics.remove(uri);
    }

    pub fn obsolete_disk_diagnostic_uris(&self) -> Vec<Url> {
        let current = self
            .disk_documents_for_validation()
            .into_iter()
            .map(|(uri, _, _)| uri)
            .collect::<HashSet<_>>();
        let mut obsolete = self
            .published_disk_diagnostics
            .difference(&current)
            .cloned()
            .collect::<Vec<_>>();
        obsolete.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        obsolete
    }

    pub fn current_ticket(&self, uri: &Url) -> Option<ValidationTicket> {
        Some(ValidationTicket {
            session_generation: self.session_generation,
            scan_generation: self.scan_generation,
            workspace_revision: self.workspace_revision,
            uri: uri.clone(),
            document_epoch: *self.document_epochs.get(uri)?,
            document_revision: *self.document_revisions.get(uri)?,
            client_version: *self.document_versions.get(uri)?,
        })
    }

    pub fn open_tickets(&self) -> Vec<ValidationTicket> {
        self.open_documents
            .keys()
            .filter_map(|uri| self.current_ticket(uri))
            .collect()
    }

    pub fn pending_open_tickets(&self) -> Vec<ValidationTicket> {
        self.open_tickets()
            .into_iter()
            .filter(|ticket| self.needs_publish(ticket))
            .collect()
    }

    pub fn needs_publish(&self, ticket: &ValidationTicket) -> bool {
        self.is_current(ticket)
            && self.published_revisions.get(&ticket.uri).copied() != Some(ticket.document_revision)
    }

    pub fn is_current(&self, ticket: &ValidationTicket) -> bool {
        self.session_generation == ticket.session_generation
            && self.scan_generation == ticket.scan_generation
            && self.workspace_revision == ticket.workspace_revision
            && self.document_versions.get(&ticket.uri).copied() == Some(ticket.client_version)
            && self.document_epochs.get(&ticket.uri).copied() == Some(ticket.document_epoch)
            && self.document_revisions.get(&ticket.uri).copied() == Some(ticket.document_revision)
    }

    pub fn mark_published(&mut self, ticket: &ValidationTicket) -> bool {
        if !self.is_current(ticket) {
            return false;
        }
        self.published_revisions
            .insert(ticket.uri.clone(), ticket.document_revision);
        true
    }

    pub fn mark_ready_if_reconciled(&mut self, scan_generation: u64) -> bool {
        if self.phase != WorkspacePhase::Reconciling
            || self.scan_generation != scan_generation
            || !self.pending_open_tickets().is_empty()
        {
            return false;
        }
        self.phase = WorkspacePhase::Ready;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{plugin, runtime};

    fn uri() -> Url {
        Url::parse("file:///workspace/items.txt").unwrap()
    }

    fn doc(value: &str) -> Arc<DocumentData> {
        Arc::new(DocumentData::parse(&format!("id\n{value}"), '\t'))
    }

    fn named_uri(name: &str) -> Url {
        Url::parse(&format!("file:///workspace/{name}.txt")).unwrap()
    }

    fn mark_all_open_documents_published(workspace: &mut Workspace) {
        let tickets = workspace.open_tickets();
        for ticket in tickets {
            assert!(workspace.mark_published(&ticket));
        }
        assert!(workspace.pending_open_tickets().is_empty());
    }

    #[test]
    fn plugin_workspace_view_is_paired_per_session_revision_and_preserves_shadowing() {
        let mut workspace = Workspace::new();
        workspace.begin_initialization(7);
        workspace.begin_scan();
        let path = PathBuf::from("C:/workspace/items.txt");
        workspace.file_cache.insert(path.clone(), doc("OLD"));

        let disk_view = workspace.plugin_workspace_view();
        let same_revision = workspace.plugin_workspace_view();
        assert!(Arc::ptr_eq(&disk_view.index, &same_revision.index));
        assert!(Arc::ptr_eq(&disk_view.snapshot, &same_revision.snapshot));
        assert!(disk_view.index.lookup("items", "id", "OLD"));

        let uri = named_uri("items");
        workspace.accept_open(uri.clone(), 1, doc("NEW"));
        let live_view = workspace.plugin_workspace_view();
        assert!(!Arc::ptr_eq(&disk_view.index, &live_view.index));
        assert!(!Arc::ptr_eq(&disk_view.snapshot, &live_view.snapshot));
        assert!(live_view.index.lookup("items", "id", "NEW"));
        assert!(!live_view.index.lookup("items", "id", "OLD"));
        assert_eq!(
            live_view.snapshot.files["items"].rows[0].cells[0].value,
            "NEW"
        );

        assert!(workspace.restore_closed_document(&uri, Some(path), Some(doc("DISK2"))));
        let restored_view = workspace.plugin_workspace_view();
        assert!(restored_view.index.lookup("items", "id", "DISK2"));
        assert!(!restored_view.index.lookup("items", "id", "NEW"));

        workspace.begin_initialization(8);
        let next_session = workspace.plugin_workspace_view();
        assert!(!Arc::ptr_eq(&restored_view.index, &next_session.index));
        assert_eq!(next_session.session_generation, 8);
        assert_eq!(
            next_session.workspace_revision,
            workspace.workspace_revision
        );
    }

    #[test]
    fn document_versions_reject_missing_stale_and_equal_changes() {
        let uri = uri();
        let mut workspace = Workspace::new();
        workspace.begin_initialization(7);
        workspace.begin_scan();
        assert_eq!(
            workspace.accept_change(&uri, 1, doc("MISSING")),
            Err(DocumentChangeError::NotOpen)
        );
        let opened = workspace.accept_open(uri.clone(), 2, doc("V2"));
        assert!(workspace.is_current(&opened));
        assert_eq!(
            workspace.accept_change(&uri, 2, doc("EQUAL")),
            Err(DocumentChangeError::StaleVersion {
                current: 2,
                incoming: 2
            })
        );
        assert_eq!(
            workspace.accept_change(&uri, 1, doc("OLD")),
            Err(DocumentChangeError::StaleVersion {
                current: 2,
                incoming: 1
            })
        );
        assert_eq!(workspace.open_documents[&uri].rows[0].cells[0].value, "V2");
    }

    #[test]
    fn newer_validation_ticket_invalidates_older_completion() {
        let uri = uri();
        let mut workspace = Workspace::new();
        workspace.begin_initialization(9);
        let scan_generation = workspace.begin_scan();
        assert!(workspace.begin_reconciliation(scan_generation));
        let v2 = workspace.accept_open(uri.clone(), 2, doc("V2"));
        let v3 = workspace.accept_change(&uri, 3, doc("V3")).unwrap();

        assert!(!workspace.is_current(&v2));
        assert!(!workspace.mark_published(&v2));
        assert!(workspace.is_current(&v3));
        assert!(workspace.mark_published(&v3));
        assert!(workspace.mark_ready_if_reconciled(scan_generation));
        assert_eq!(workspace.phase, WorkspacePhase::Ready);
    }

    #[test]
    fn close_and_reopen_changes_epoch_even_when_version_is_reused() {
        let uri = uri();
        let mut workspace = Workspace::new();
        workspace.begin_initialization(1);
        workspace.begin_scan();
        let first = workspace.accept_open(uri.clone(), 1, doc("FIRST"));
        workspace.close_open_document(&uri);
        let second = workspace.accept_open(uri, 1, doc("SECOND"));
        assert_ne!(first.document_epoch, second.document_epoch);
        assert!(!workspace.is_current(&first));
        assert!(workspace.is_current(&second));
    }

    #[test]
    fn scan_commit_keeps_live_document_authoritative_over_parsed_disk_copy() {
        let uri = uri();
        let mut workspace = Workspace::new();
        workspace.begin_initialization(4);
        workspace
            .ref_targets
            .insert(("items".to_string(), "id".to_string()));
        let scan_generation = workspace.begin_scan();
        let parsed = vec![(
            uri.clone(),
            PathBuf::from("/workspace/items.txt"),
            "items".to_string(),
            doc("OLD"),
        )];

        let live = workspace.accept_open(uri, 2, doc("NEW"));
        assert!(workspace.commit_scan_documents(scan_generation, &parsed));

        assert!(workspace.symbols.lookup("items", "id", "OLD").is_none());
        assert!(workspace.symbols.lookup("items", "id", "NEW").is_some());
        assert_eq!(
            workspace.open_documents[&live.uri].rows[0].cells[0].value,
            "NEW"
        );
        assert_eq!(workspace.file_cache.len(), 1);
    }

    #[test]
    fn duplicate_stem_contract_uses_lexical_source_and_open_documents_shadow_disk() {
        let root = std::env::temp_dir().join("vlsp-duplicate-stem");
        let path_a = root.join("a").join("items.txt");
        let path_b = root.join("b").join("items.txt");
        let uri_a = Url::from_file_path(&path_a).unwrap();
        let uri_b = Url::from_file_path(&path_b).unwrap();
        let mut workspace = Workspace::new();
        workspace
            .ref_targets
            .insert(("items".to_string(), "id".to_string()));
        workspace.file_cache.insert(path_b, doc("DISK_B"));
        workspace.file_cache.insert(path_a, doc("DISK_A"));
        workspace.rebuild_effective_symbols();
        assert!(workspace.symbols.lookup("items", "id", "DISK_A").is_some());
        assert!(workspace.symbols.lookup("items", "id", "DISK_B").is_none());

        workspace.accept_open(uri_b.clone(), 1, doc("OPEN_B"));
        workspace.rebuild_effective_symbols();
        assert!(workspace.symbols.lookup("items", "id", "OPEN_B").is_some());
        assert!(workspace.symbols.lookup("items", "id", "DISK_A").is_none());

        workspace.accept_open(uri_a.clone(), 1, doc("OPEN_A"));
        workspace.rebuild_effective_symbols();
        let index =
            runtime::build_workspace_index(&workspace.open_documents, &workspace.file_cache);
        let snapshot =
            plugin::build_workspace_snapshot(&workspace.open_documents, &workspace.file_cache);
        assert!(workspace.symbols.lookup("items", "id", "OPEN_A").is_some());
        assert!(workspace.symbols.lookup("items", "id", "OPEN_B").is_none());
        assert!(index.lookup("items", "id", "OPEN_A"));
        assert!(!index.lookup("items", "id", "OPEN_B"));
        assert_eq!(snapshot.files["items"].rows[0].cells[0].value, "OPEN_A");
        assert_eq!(
            workspace
                .symbols
                .lookup("items", "id", "OPEN_A")
                .unwrap()
                .uri,
            uri_a
        );
    }

    #[test]
    fn disk_validation_uses_one_lexical_winner_and_restores_it_after_close() {
        let root = std::env::temp_dir().join("vlsp-duplicate-validation-stem");
        let path_a = root.join("a").join("items.txt");
        let path_b = root.join("b").join("ITEMS.txt");
        let uri_a = Url::from_file_path(&path_a).unwrap();
        let uri_b = Url::from_file_path(&path_b).unwrap();
        let mut workspace = Workspace::new();
        workspace.file_cache.insert(path_b, doc("DISK_B"));
        workspace.file_cache.insert(path_a.clone(), doc("DISK_A"));

        let selected_uris = |workspace: &Workspace| {
            let mut uris = workspace
                .disk_documents_for_validation()
                .into_iter()
                .map(|(uri, _, _)| uri)
                .collect::<Vec<_>>();
            uris.sort_by(|left, right| left.as_str().cmp(right.as_str()));
            uris
        };
        assert_eq!(selected_uris(&workspace), vec![uri_a.clone()]);

        workspace.accept_open(uri_b.clone(), 1, doc("OPEN_B"));
        assert!(selected_uris(&workspace).is_empty());
        workspace.close_open_document(&uri_b);

        workspace.accept_open(uri_a.clone(), 1, doc("OPEN_A"));
        assert!(workspace.restore_closed_document(&uri_a, Some(path_a), Some(doc("DISK_A")),));
        assert_eq!(selected_uris(&workspace), vec![uri_a]);
        assert!(!selected_uris(&workspace).contains(&uri_b));
    }

    #[test]
    fn schema_target_delete_and_restore_each_invalidate_open_source_ticket() {
        let target_uri = named_uri("target");
        let source_uri = named_uri("source");
        let mut workspace = Workspace::new();
        workspace.begin_initialization(12);
        let scan_generation = workspace.begin_scan();
        assert!(workspace.begin_reconciliation(scan_generation));
        workspace
            .ref_targets
            .insert(("target".to_string(), "id".to_string()));

        workspace.accept_open(target_uri.clone(), 1, doc("KEY"));
        workspace.accept_open(source_uri.clone(), 1, doc("KEY"));
        mark_all_open_documents_published(&mut workspace);

        workspace
            .accept_change(&target_uri, 2, doc(""))
            .expect("target key deletion should be accepted");
        assert!(
            workspace
                .pending_open_tickets()
                .iter()
                .any(|ticket| ticket.uri == source_uri),
            "deleting a schema target key must schedule the open source for revalidation"
        );
        mark_all_open_documents_published(&mut workspace);

        workspace
            .accept_change(&target_uri, 3, doc("KEY"))
            .expect("target key restoration should be accepted");
        assert!(
            workspace
                .pending_open_tickets()
                .iter()
                .any(|ticket| ticket.uri == source_uri),
            "restoring a schema target key must schedule the open source so its error clears"
        );
    }

    #[test]
    fn plugin_target_change_conservatively_invalidates_every_open_ticket() {
        let target_uri = named_uri("target");
        let source_uri = named_uri("source");
        let unrelated_uri = named_uri("unrelated");
        let mut workspace = Workspace::new();
        workspace.begin_initialization(13);
        let scan_generation = workspace.begin_scan();
        assert!(workspace.begin_reconciliation(scan_generation));

        // With no declared plugin dependency metadata, any open document may call
        // lookupKey/getColumnValues against the changed target.
        workspace.accept_open(target_uri.clone(), 1, doc("KEY"));
        workspace.accept_open(source_uri.clone(), 1, doc("KEY"));
        workspace.accept_open(unrelated_uri.clone(), 1, doc("OTHER"));
        mark_all_open_documents_published(&mut workspace);

        workspace
            .accept_change(&target_uri, 2, doc(""))
            .expect("plugin lookup target deletion should be accepted");
        let pending = workspace
            .pending_open_tickets()
            .into_iter()
            .map(|ticket| ticket.uri)
            .collect::<HashSet<_>>();

        assert_eq!(
            pending,
            HashSet::from([
                target_uri.clone(),
                source_uri.clone(),
                unrelated_uri.clone(),
            ]),
            "unknown plugin dependencies require conservative open-document revalidation"
        );
        mark_all_open_documents_published(&mut workspace);

        workspace
            .accept_change(&target_uri, 3, doc("KEY"))
            .expect("plugin lookup target restoration should be accepted");
        let pending_after_restore = workspace
            .pending_open_tickets()
            .into_iter()
            .map(|ticket| ticket.uri)
            .collect::<HashSet<_>>();
        assert_eq!(
            pending_after_restore,
            HashSet::from([target_uri, source_uri, unrelated_uri]),
            "restoring a plugin target must conservatively revalidate open dependents so stale errors clear"
        );
    }

    #[test]
    fn did_close_restores_schema_and_plugin_views_from_the_same_disk_generation() {
        for (label, initial_disk, restored_disk, expected) in [
            ("unsaved", Some("OLD"), Some("OLD"), Some("OLD")),
            ("saved", Some("OLD"), Some("NEW"), Some("NEW")),
            ("new-file", None, Some("NEW"), Some("NEW")),
            ("deleted", Some("OLD"), None, None),
        ] {
            let path = std::env::temp_dir()
                .join(format!("vlsp-close-{label}"))
                .join("items.txt");
            let uri = Url::from_file_path(&path).unwrap();
            let mut workspace = Workspace::new();
            workspace.begin_initialization(1);
            workspace
                .ref_targets
                .insert(("items".to_string(), "id".to_string()));
            if let Some(value) = initial_disk {
                workspace.file_cache.insert(path.clone(), doc(value));
            }
            workspace.rebuild_effective_symbols();
            workspace.accept_open(uri.clone(), 1, doc("NEW"));
            workspace.rebuild_effective_symbols();

            assert!(workspace.restore_closed_document(
                &uri,
                Some(path.clone()),
                restored_disk.map(doc),
            ));
            let index =
                runtime::build_workspace_index(&workspace.open_documents, &workspace.file_cache);
            let snapshot =
                plugin::build_workspace_snapshot(&workspace.open_documents, &workspace.file_cache);
            assert!(!workspace.open_documents.contains_key(&uri), "{label}");
            assert_eq!(
                workspace
                    .file_cache
                    .get(&path)
                    .map(|document| document.rows[0].cells[0].value.as_str()),
                expected,
                "{label}",
            );
            assert_eq!(
                workspace.symbols.lookup("items", "id", "NEW").is_some(),
                expected == Some("NEW"),
                "{label}",
            );
            assert_eq!(
                workspace.symbols.lookup("items", "id", "OLD").is_some(),
                expected == Some("OLD"),
                "{label}",
            );
            let definition =
                expected.and_then(|value| workspace.symbols.lookup("items", "id", value).cloned());
            if let Some(definition) = definition {
                assert_eq!(definition.uri, uri, "{label}");
                assert_eq!(definition.range.start, Position::new(1, 0), "{label}");
                assert_eq!(definition.range.end, Position::new(1, 3), "{label}");
            } else {
                assert!(expected.is_none(), "{label}");
            }
            assert_eq!(
                index.lookup("items", "id", "NEW"),
                expected == Some("NEW"),
                "{label}",
            );
            assert_eq!(
                index.lookup("items", "id", "OLD"),
                expected == Some("OLD"),
                "{label}",
            );
            assert_eq!(
                snapshot
                    .files
                    .get("items")
                    .map(|document| document.rows[0].cells[0].value.as_str()),
                expected,
                "{label}",
            );
        }
    }

    #[test]
    fn definition_ranges_use_utf16_code_units() {
        let uri = named_uri("items");
        let document = DocumentData::parse("id\n🙂", '\t');
        let targets = HashSet::from([("items".to_string(), "id".to_string())]);
        let mut symbols = SymbolIndex::new();

        symbols.index_document(&uri, "items", &document, &targets);

        let location = symbols.lookup("items", "id", "🙂").unwrap();
        assert_eq!(location.range.start, Position::new(1, 0));
        assert_eq!(location.range.end, Position::new(1, 2));
    }
}
