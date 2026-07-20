use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower_lsp::lsp_types::{Diagnostic, Location, Position, Range, Url};

use crate::document::{DocumentData, utf16_len};
use crate::i18n::Locale;
use crate::json_diagnostics::{
    OpenJsonSources, PrimaryTxtDocument, data_root_from_excel_txt,
    local_path_identity as json_path_identity,
};
use crate::plugin;
use crate::runtime::{self, WorkspaceFileSnapshot, WorkspaceIndex};
use crate::schema::{ReferenceResolver, Schema};
use crate::source_selection::{
    EffectiveSource, SourceKind, effective_workspace_sources,
    effective_workspace_sources_with_priority_tiers, normalized_file_stem_from_uri,
};

#[cfg(windows)]
pub(crate) fn local_path_identity(path: &std::path::Path) -> String {
    let value = path.to_string_lossy().replace('/', "\\");
    let without_extended_prefix = if let Some(rest) = value.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else if let Some(rest) = value.strip_prefix("\\\\?\\") {
        rest.to_string()
    } else {
        value
    };
    let normalized = without_extended_prefix.to_lowercase();
    if normalized.len() > 3 {
        normalized.trim_end_matches(['\\', '/']).to_string()
    } else {
        normalized
    }
}

#[cfg(not(windows))]
pub(crate) fn local_path_identity(path: &std::path::Path) -> String {
    let normalized = path.to_string_lossy();
    if normalized.len() > 1 {
        normalized.trim_end_matches('/').to_string()
    } else {
        normalized.into_owned()
    }
}

pub(crate) fn same_local_path(left: &std::path::Path, right: &std::path::Path) -> bool {
    local_path_identity(left) == local_path_identity(right)
}

fn path_parent_is(path: &std::path::Path, parent: &std::path::Path) -> bool {
    path.parent()
        .is_some_and(|candidate| same_local_path(candidate, parent))
}

fn uri_parent_is(uri: &Url, parent: &std::path::Path) -> bool {
    uri.to_file_path()
        .ok()
        .is_some_and(|path| path_parent_is(&path, parent))
}

fn uri_parents_match(left: &Url, right: &Url) -> bool {
    let Some(left_parent) = left
        .to_file_path()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
    else {
        return false;
    };
    uri_parent_is(right, &left_parent)
}

#[cfg(windows)]
pub(crate) fn local_path_is_within(path: &std::path::Path, root: &std::path::Path) -> bool {
    let path = local_path_identity(path);
    let root = local_path_identity(root);
    let root = root.trim_end_matches(['\\', '/']);
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('\\') || suffix.starts_with('/'))
}

#[cfg(not(windows))]
pub(crate) fn local_path_is_within(path: &std::path::Path, root: &std::path::Path) -> bool {
    path.starts_with(root)
}

#[derive(Clone)]
pub struct PluginWorkspaceView {
    pub session_generation: u64,
    pub workspace_revision: u64,
    pub index: Arc<WorkspaceIndex>,
    pub snapshot: Arc<WorkspaceFileSnapshot>,
}

#[derive(Clone)]
struct DirectorySymbolView {
    session_generation: u64,
    symbol_generation: u64,
    index: Arc<SymbolIndex>,
}

#[derive(Clone, Debug)]
struct PublishedJsonDiagnosticSnapshot {
    diagnostics: Vec<Diagnostic>,
    version: Option<i32>,
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
    ascii_ci_entries: HashMap<(String, String, String), SymbolEntry>,
    fixed4_entries: HashMap<(String, String, String), SymbolEntry>,
    columns: HashSet<(String, String)>,
    files: HashSet<String>,
}

#[derive(Clone, Debug)]
pub struct SymbolEntry {
    pub location: Option<Location>,
    pub source_kind: SourceKind,
    pub bundled_version: Option<String>,
    pub stored_value: String,
}

impl SymbolIndex {
    pub fn new() -> Self {
        Self {
            ascii_ci_entries: HashMap::new(),
            fixed4_entries: HashMap::new(),
            columns: HashSet::new(),
            files: HashSet::new(),
        }
    }

    pub fn remove_file(&mut self, file_stem: &str) {
        let stem = file_stem.to_ascii_lowercase();
        self.ascii_ci_entries
            .retain(|(entry_stem, _, _), _| entry_stem != &stem);
        self.fixed4_entries
            .retain(|(entry_stem, _, _), _| entry_stem != &stem);
        self.columns.retain(|(entry_stem, _)| entry_stem != &stem);
        self.files.remove(&stem);
    }

    /// Index the cells of `doc` that belong to columns listed in `ref_targets`.
    pub fn index_document(
        &mut self,
        uri: &Url,
        file_stem: &str,
        doc: &DocumentData,
        ref_targets: &HashSet<(String, String)>,
    ) {
        self.index_effective_document(
            Some(uri),
            file_stem,
            doc,
            ref_targets,
            SourceKind::Workspace,
            None,
        );
    }

    pub fn index_effective_document(
        &mut self,
        uri: Option<&Url>,
        file_stem: &str,
        doc: &DocumentData,
        ref_targets: &HashSet<(String, String)>,
        source_kind: SourceKind,
        bundled_version: Option<&str>,
    ) {
        let stem = file_stem.to_ascii_lowercase();
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
                let location = match source_kind {
                    SourceKind::Open | SourceKind::Workspace => uri.map(|uri| Location {
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
                    }),
                    SourceKind::Sibling | SourceKind::Bundled => None,
                };
                let entry = SymbolEntry {
                    location,
                    source_kind,
                    bundled_version: bundled_version.map(str::to_string),
                    stored_value: cell.value.clone(),
                };
                self.ascii_ci_entries.insert(
                    (
                        stem.clone(),
                        col_lower.clone(),
                        cell.value.to_ascii_lowercase(),
                    ),
                    entry.clone(),
                );
                self.fixed4_entries
                    .insert((stem.clone(), col_lower, fixed4_key(&cell.value)), entry);
            }
        }
    }

    /// Look up the location of a specific value in a specific column of a specific file.
    pub fn lookup(&self, file_stem: &str, column: &str, value: &str) -> Option<&Location> {
        self.resolve(file_stem, column, value, ReferenceResolver::AsciiCi)
            .and_then(|entry| entry.location.as_ref())
    }

    pub fn lookup_resolved(
        &self,
        file_stem: &str,
        column: &str,
        value: &str,
        resolver: ReferenceResolver,
    ) -> Option<&Location> {
        self.resolve(file_stem, column, value, resolver)
            .and_then(|entry| entry.location.as_ref())
    }

    pub fn contains_resolved(
        &self,
        file_stem: &str,
        column: &str,
        value: &str,
        resolver: ReferenceResolver,
    ) -> bool {
        self.resolve(file_stem, column, value, resolver).is_some()
    }

    pub fn resolve(
        &self,
        file_stem: &str,
        column: &str,
        value: &str,
        resolver: ReferenceResolver,
    ) -> Option<&SymbolEntry> {
        let stem = file_stem.to_ascii_lowercase();
        let column = column.to_ascii_lowercase();
        match resolver {
            ReferenceResolver::AsciiCi => {
                self.ascii_ci_entries
                    .get(&(stem, column, value.to_ascii_lowercase()))
            }
            ReferenceResolver::Fixed4 => {
                self.fixed4_entries.get(&(stem, column, fixed4_key(value)))
            }
        }
    }

    pub fn has_file(&self, file_stem: &str) -> bool {
        self.files.contains(&file_stem.to_lowercase())
    }

    pub fn has_column(&self, file_stem: &str, column: &str) -> bool {
        self.columns
            .contains(&(file_stem.to_lowercase(), column.to_lowercase()))
    }
}

pub fn fixed4_key(value: &str) -> String {
    let mut bytes = [b' '; 4];
    for (index, byte) in value.as_bytes().iter().take(4).enumerate() {
        bytes[index] = *byte;
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn fixed4_display(value: &str) -> String {
    let mut bytes = [b' '; 4];
    for (index, byte) in value.as_bytes().iter().take(4).enumerate() {
        bytes[index] = *byte;
    }
    String::from_utf8_lossy(&bytes).into_owned()
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReferenceContextMode {
    #[default]
    Workspace,
    /// Direct sibling TXT files are hidden lookup-only inputs for a standalone
    /// editor document. They must never publish their own diagnostics.
    Sibling,
}

impl ReferenceContextMode {
    pub fn from_initialization_value(value: Option<&str>) -> Self {
        match value {
            Some(value) if value.eq_ignore_ascii_case("sibling") => Self::Sibling,
            _ => Self::Workspace,
        }
    }

    fn disk_source_kind(self) -> SourceKind {
        match self {
            Self::Workspace => SourceKind::Workspace,
            Self::Sibling => SourceKind::Sibling,
        }
    }
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PendingWatchedChanges {
    pub txt: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PendingJsonAnalysis {
    pub all_rules: bool,
    pub key_usage: bool,
}

impl PendingJsonAnalysis {
    pub fn requested(self) -> bool {
        self.all_rules || self.key_usage
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WatchedRegistrationState {
    Pending {
        session_generation: u64,
        sequence: u64,
    },
    Active(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentChangeError {
    NotOpen,
    StaleVersion { current: i32, incoming: i32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenJsonDocument {
    pub version: i32,
    pub text: Arc<str>,
}

pub struct Workspace {
    /// Negotiated locale for this LSP service/session. It lives beside the
    /// workspace state so TCP clients cannot observe one another's setting.
    pub locale: Locale,
    pub root_uri: Option<Url>,
    pub reference_root_uri: Option<Url>,
    pub reference_context_mode: ReferenceContextMode,
    /// Editor sessions may opt out of recursive discovery while keeping the
    /// workspace root as the session identity.
    pub include_subfolders: bool,
    /// In an editor workspace, each physical directory is an independent
    /// lookup and diagnostics context.
    pub workspace_directory_scopes: bool,
    /// Documents currently open in the editor (managed via didOpen/didChange).
    pub open_documents: HashMap<Url, Arc<DocumentData>>,
    /// Physical localization JSON files currently shadowed by unsaved editor
    /// buffers. They never participate in TXT symbols or reference lookup.
    open_json_documents: HashMap<Url, OpenJsonDocument>,
    /// All other workspace files parsed from disk on startup.
    pub file_cache: HashMap<PathBuf, Arc<DocumentData>>,
    /// Lower-priority explicit mod/workspace reference root used only while a
    /// standalone sibling session is active.
    pub reference_root_cache: HashMap<PathBuf, Arc<DocumentData>>,
    /// Hidden selected-version baseline tables. They participate in lookups
    /// only and never receive diagnostics or a public URI.
    pub fallback_cache: HashMap<String, Arc<DocumentData>>,
    /// Includes local TXT paths that failed to parse, so a bundled table cannot
    /// silently replace an existing workspace table.
    pub workspace_present_stems: HashSet<String>,
    /// Path-level presence is retained so didClose can remove a deleted path
    /// without accidentally unblocking a duplicate stem that still exists.
    pub workspace_present_paths: HashSet<PathBuf>,
    pub reference_root_present_stems: HashSet<String>,
    pub reference_root_present_paths: HashSet<PathBuf>,
    pub reference_version: Option<String>,
    pub reference_digest: Option<String>,
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
    symbol_generation: u64,
    document_versions: HashMap<Url, i32>,
    document_epochs: HashMap<Url, u64>,
    document_revisions: HashMap<Url, u64>,
    published_revisions: HashMap<Url, u64>,
    published_disk_diagnostics: HashSet<Url>,
    /// JSON publications are tracked separately from stem-selected TXT
    /// diagnostics. Diagnostics and their LSP document version form one
    /// atomic snapshot so unchanged-result suppression cannot observe a
    /// partially updated pair. `None` is a physical-disk publication.
    published_json_diagnostics: HashMap<Url, PublishedJsonDiagnosticSnapshot>,
    /// Invalidates JSON analyses when an external editor changes a physical
    /// localization JSON or layout input without an LSP document version.
    json_input_generation: u64,
    watched_files_dynamic_registration: bool,
    watched_files_relative_pattern_support: bool,
    watched_registration_sequence: u64,
    watched_json_data_roots: HashMap<String, WatchedRegistrationState>,
    pending_watched_json_unregistrations: HashSet<String>,
    watched_json_unregistration_worker_running: bool,
    watched_change_generation: u64,
    pending_watched_changes: PendingWatchedChanges,
    watched_change_worker_running: bool,
    pending_txt_watch_registration_catch_up: bool,
    json_analysis_generation: u64,
    pending_json_analysis: PendingJsonAnalysis,
    json_analysis_worker_running: bool,
    json_startup_analysis_queued: bool,
    workspace_revalidation_worker_session: Option<u64>,
    schema_preview_workers: HashSet<Url>,
    next_document_epoch: u64,
    next_document_revision: u64,
    plugin_workspace_view: Mutex<Option<PluginWorkspaceView>>,
    plugin_directory_views: Mutex<HashMap<String, PluginWorkspaceView>>,
    directory_symbol_views: Mutex<HashMap<String, DirectorySymbolView>>,
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            locale: Locale::EnUs,
            root_uri: None,
            reference_root_uri: None,
            reference_context_mode: ReferenceContextMode::Workspace,
            include_subfolders: true,
            workspace_directory_scopes: false,
            open_documents: HashMap::new(),
            open_json_documents: HashMap::new(),
            file_cache: HashMap::new(),
            reference_root_cache: HashMap::new(),
            fallback_cache: HashMap::new(),
            workspace_present_stems: HashSet::new(),
            workspace_present_paths: HashSet::new(),
            reference_root_present_stems: HashSet::new(),
            reference_root_present_paths: HashSet::new(),
            reference_version: None,
            reference_digest: None,
            symbols: SymbolIndex::new(),
            schema: None,
            ref_targets: HashSet::new(),
            phase: WorkspacePhase::Cold,
            session_generation: 0,
            scan_generation: 0,
            workspace_revision: 0,
            symbol_generation: 0,
            document_versions: HashMap::new(),
            document_epochs: HashMap::new(),
            document_revisions: HashMap::new(),
            published_revisions: HashMap::new(),
            published_disk_diagnostics: HashSet::new(),
            published_json_diagnostics: HashMap::new(),
            json_input_generation: 0,
            watched_files_dynamic_registration: false,
            watched_files_relative_pattern_support: false,
            watched_registration_sequence: 0,
            watched_json_data_roots: HashMap::new(),
            pending_watched_json_unregistrations: HashSet::new(),
            watched_json_unregistration_worker_running: false,
            watched_change_generation: 0,
            pending_watched_changes: PendingWatchedChanges::default(),
            watched_change_worker_running: false,
            pending_txt_watch_registration_catch_up: false,
            json_analysis_generation: 0,
            pending_json_analysis: PendingJsonAnalysis::default(),
            json_analysis_worker_running: false,
            json_startup_analysis_queued: false,
            workspace_revalidation_worker_session: None,
            schema_preview_workers: HashSet::new(),
            next_document_epoch: 0,
            next_document_revision: 0,
            plugin_workspace_view: Mutex::new(None),
            plugin_directory_views: Mutex::new(HashMap::new()),
            directory_symbol_views: Mutex::new(HashMap::new()),
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
        let sources = self.effective_sources();
        let view = self.build_plugin_workspace_view(&sources);
        *cached = Some(view.clone());
        view
    }

    pub fn plugin_workspace_view_for_uri(&self, uri: &Url) -> PluginWorkspaceView {
        if !self.uses_directory_scopes() {
            return self.plugin_workspace_view();
        }
        let Some(key) = self.directory_scope_key_for_uri(uri) else {
            return self.plugin_workspace_view();
        };
        let mut cached = self.plugin_directory_views.lock().unwrap();
        if let Some(view) = cached.get(&key)
            && view.session_generation == self.session_generation
            && view.workspace_revision == self.workspace_revision
        {
            return view.clone();
        }
        let view = self.build_plugin_workspace_view(&self.effective_sources_for_uri(uri));
        cached.insert(key, view.clone());
        view
    }

    fn build_plugin_workspace_view(&self, sources: &[EffectiveSource]) -> PluginWorkspaceView {
        let index = runtime::build_workspace_index_from_sources(sources);
        let mut snapshot = plugin::build_workspace_snapshot_from_sources(sources);
        let snapshot_data = Arc::get_mut(&mut snapshot)
            .expect("new plugin workspace snapshot must be uniquely owned");
        for source in snapshot_data.sources.values_mut() {
            // The source tier and selected session version are both useful to
            // hover providers, even when a local/open document supplies data.
            source.version = self.reference_version.clone();
        }
        PluginWorkspaceView {
            session_generation: self.session_generation,
            workspace_revision: self.workspace_revision,
            index,
            snapshot,
        }
    }

    pub fn begin_initialization(&mut self, session_generation: u64) {
        self.session_generation = session_generation;
        self.json_input_generation = self.json_input_generation.wrapping_add(1);
        self.watched_registration_sequence = 0;
        self.pending_watched_json_unregistrations.extend(
            self.watched_json_data_roots
                .values()
                .filter_map(|state| match state {
                    WatchedRegistrationState::Active(id) => Some(id.clone()),
                    WatchedRegistrationState::Pending { .. } => None,
                }),
        );
        self.watched_json_data_roots.clear();
        self.watched_change_generation = self.watched_change_generation.wrapping_add(1);
        self.pending_watched_changes = PendingWatchedChanges::default();
        self.watched_change_worker_running = false;
        self.pending_txt_watch_registration_catch_up = false;
        self.json_analysis_generation = self.json_analysis_generation.wrapping_add(1);
        self.pending_json_analysis = PendingJsonAnalysis::default();
        self.json_analysis_worker_running = false;
        self.json_startup_analysis_queued = false;
        self.workspace_revalidation_worker_session = None;
        self.schema_preview_workers.clear();
        self.phase = WorkspacePhase::LoadingSchema;
    }

    pub fn reserve_schema_preview_worker(&mut self, uri: &Url) -> Option<u64> {
        self.schema_preview_workers
            .insert(uri.clone())
            .then_some(self.session_generation)
    }

    pub fn finish_schema_preview_worker_if_current(&mut self, ticket: &ValidationTicket) -> bool {
        if !self.is_current(ticket) {
            return false;
        }
        self.schema_preview_workers.remove(&ticket.uri);
        true
    }

    pub fn finish_schema_preview_worker(&mut self, uri: &Url) {
        self.schema_preview_workers.remove(uri);
    }

    pub fn start_workspace_revalidation_worker(&mut self) -> Option<u64> {
        if self.phase != WorkspacePhase::Ready
            || self.workspace_revalidation_worker_session == Some(self.session_generation)
        {
            return None;
        }
        self.workspace_revalidation_worker_session = Some(self.session_generation);
        Some(self.session_generation)
    }

    pub fn workspace_revalidation_worker_should_continue(
        &mut self,
        session_generation: u64,
    ) -> bool {
        if self.workspace_revalidation_worker_session != Some(session_generation) {
            return false;
        }
        if self.phase == WorkspacePhase::Ready && !self.pending_open_tickets().is_empty() {
            return true;
        }
        self.workspace_revalidation_worker_session = None;
        false
    }

    pub fn set_watched_files_client_capabilities(
        &mut self,
        dynamic_registration: bool,
        relative_pattern_support: bool,
    ) {
        self.watched_files_dynamic_registration = dynamic_registration;
        self.watched_files_relative_pattern_support = relative_pattern_support;
    }

    pub fn supports_dynamic_watched_files(&self) -> bool {
        self.watched_files_dynamic_registration && self.watched_files_relative_pattern_support
    }

    pub fn reserve_watched_json_registration(&mut self, identity: &str) -> Option<u64> {
        if !self.supports_dynamic_watched_files() {
            return None;
        }
        if self.watched_json_data_roots.contains_key(identity) {
            return None;
        }
        self.watched_registration_sequence = self.watched_registration_sequence.wrapping_add(1);
        let sequence = self.watched_registration_sequence;
        self.watched_json_data_roots.insert(
            identity.to_string(),
            WatchedRegistrationState::Pending {
                session_generation: self.session_generation,
                sequence,
            },
        );
        Some(sequence)
    }

    pub fn finish_watched_json_registration(
        &mut self,
        identity: &str,
        session_generation: u64,
        sequence: u64,
        registration_id: &str,
        succeeded: bool,
    ) -> bool {
        if self.session_generation != session_generation
            || self.watched_json_data_roots.get(identity)
                != Some(&WatchedRegistrationState::Pending {
                    session_generation,
                    sequence,
                })
        {
            return false;
        }
        if succeeded {
            self.watched_json_data_roots.insert(
                identity.to_string(),
                WatchedRegistrationState::Active(registration_id.to_string()),
            );
        } else {
            self.watched_json_data_roots.remove(identity);
        }
        true
    }

    pub fn take_obsolete_watched_json_registrations(
        &mut self,
        desired_identities: &HashSet<String>,
    ) -> Vec<String> {
        let inactive = self
            .watched_json_data_roots
            .iter()
            .filter_map(|(identity, state)| {
                (!desired_identities.contains(identity))
                    .then(|| match state {
                        WatchedRegistrationState::Active(id) => {
                            Some((identity.clone(), id.clone()))
                        }
                        WatchedRegistrationState::Pending { .. } => None,
                    })
                    .flatten()
            })
            .collect::<Vec<_>>();
        for (identity, _) in &inactive {
            self.watched_json_data_roots.remove(identity);
        }
        inactive.into_iter().map(|(_, id)| id).collect()
    }

    pub fn queue_watched_json_unregistrations<I>(&mut self, ids: I) -> bool
    where
        I: IntoIterator<Item = String>,
    {
        self.pending_watched_json_unregistrations.extend(ids);
        if self.pending_watched_json_unregistrations.is_empty() {
            return false;
        }
        let start_worker = !self.watched_json_unregistration_worker_running;
        self.watched_json_unregistration_worker_running = true;
        start_worker
    }

    pub fn take_watched_json_unregistrations(&mut self) -> Vec<String> {
        if !self.watched_json_unregistration_worker_running {
            return Vec::new();
        }
        let mut ids = self
            .pending_watched_json_unregistrations
            .drain()
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    pub fn finish_watched_json_unregistration_worker_if_idle(&mut self) -> bool {
        if !self.pending_watched_json_unregistrations.is_empty() {
            return false;
        }
        self.watched_json_unregistration_worker_running = false;
        true
    }

    pub fn defer_watched_json_unregistrations(&mut self, ids: Vec<String>) -> bool {
        let newer_request_waiting = !self.pending_watched_json_unregistrations.is_empty();
        self.pending_watched_json_unregistrations.extend(ids);
        self.watched_json_unregistration_worker_running = newer_request_waiting;
        newer_request_waiting
    }

    #[cfg(test)]
    pub fn pending_watched_json_unregistrations(&self) -> Vec<String> {
        let mut ids = self
            .pending_watched_json_unregistrations
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    pub fn queue_watched_changes(&mut self, txt: bool) -> (u64, bool) {
        self.watched_change_generation = self.watched_change_generation.wrapping_add(1);
        self.pending_watched_changes.txt |= txt;
        if txt {
            // Invalidate every validation ticket immediately. The quiet-window
            // rescan will allocate the generation that it actually commits.
            self.begin_scan();
        }
        let start_worker = !self.watched_change_worker_running;
        self.watched_change_worker_running = true;
        (self.watched_change_generation, start_worker)
    }

    pub fn defer_txt_watch_registration_catch_up(&mut self) {
        self.pending_txt_watch_registration_catch_up = true;
    }

    pub fn take_txt_watch_registration_catch_up(&mut self) -> bool {
        std::mem::take(&mut self.pending_txt_watch_registration_catch_up)
    }

    #[cfg(test)]
    pub fn has_pending_txt_watch_registration_catch_up(&self) -> bool {
        self.pending_txt_watch_registration_catch_up
    }

    pub fn watched_change_generation_for_worker(&self, session_generation: u64) -> Option<u64> {
        (self.session_generation == session_generation && self.watched_change_worker_running)
            .then_some(self.watched_change_generation)
    }

    pub fn take_watched_changes(
        &mut self,
        session_generation: u64,
        generation: u64,
    ) -> Option<PendingWatchedChanges> {
        if self.session_generation != session_generation
            || !self.watched_change_worker_running
            || self.watched_change_generation != generation
        {
            return None;
        }
        let pending = std::mem::take(&mut self.pending_watched_changes);
        pending.txt.then_some(pending)
    }

    pub fn finish_watched_change_worker_if_idle(&mut self, session_generation: u64) -> bool {
        if self.session_generation != session_generation {
            return true;
        }
        if self.pending_watched_changes.txt {
            return false;
        }
        self.watched_change_worker_running = false;
        true
    }

    #[cfg(test)]
    pub fn watched_change_worker_running(&self) -> bool {
        self.watched_change_worker_running
    }

    pub fn queue_json_analysis(&mut self, all_rules: bool) -> (u64, bool) {
        self.json_analysis_generation = self.json_analysis_generation.wrapping_add(1);
        if all_rules {
            self.pending_json_analysis.all_rules = true;
        } else {
            self.pending_json_analysis.key_usage = true;
        }
        let start_worker = !self.json_analysis_worker_running;
        self.json_analysis_worker_running = true;
        (self.json_analysis_generation, start_worker)
    }

    pub fn requeue_json_analysis(
        &mut self,
        session_generation: u64,
        pending: PendingJsonAnalysis,
    ) -> bool {
        if self.session_generation != session_generation || !pending.requested() {
            return false;
        }
        self.json_analysis_generation = self.json_analysis_generation.wrapping_add(1);
        self.pending_json_analysis.all_rules |= pending.all_rules;
        self.pending_json_analysis.key_usage |= pending.key_usage;
        self.json_analysis_worker_running = true;
        true
    }

    pub fn json_analysis_generation_for_worker(&self, session_generation: u64) -> Option<u64> {
        (self.session_generation == session_generation && self.json_analysis_worker_running)
            .then_some(self.json_analysis_generation)
    }

    pub fn take_json_analysis(
        &mut self,
        session_generation: u64,
        generation: u64,
    ) -> Option<PendingJsonAnalysis> {
        if self.session_generation != session_generation
            || !self.json_analysis_worker_running
            || self.json_analysis_generation != generation
        {
            return None;
        }
        let pending = std::mem::take(&mut self.pending_json_analysis);
        pending.requested().then_some(pending)
    }

    pub fn finish_json_analysis_worker_if_idle(&mut self, session_generation: u64) -> bool {
        if self.session_generation != session_generation {
            return true;
        }
        if self.pending_json_analysis.requested() {
            return false;
        }
        self.json_analysis_worker_running = false;
        true
    }

    pub fn defer_json_analysis(
        &mut self,
        session_generation: u64,
        worker_generation: u64,
        pending: PendingJsonAnalysis,
    ) -> bool {
        if self.session_generation != session_generation {
            return false;
        }
        self.pending_json_analysis.all_rules |= pending.all_rules;
        self.pending_json_analysis.key_usage |= pending.key_usage;
        if self.json_analysis_generation != worker_generation || self.phase == WorkspacePhase::Ready
        {
            // A newer request or Ready transition raced the worker's phase
            // observation. Keep this worker alive so the merged request cannot
            // be stranded without a wakeup.
            self.json_analysis_worker_running = true;
            return true;
        }
        self.json_analysis_worker_running = false;
        false
    }

    #[cfg(test)]
    pub fn json_analysis_worker_running(&self) -> bool {
        self.json_analysis_worker_running
    }

    #[cfg(test)]
    pub fn json_analysis_generation(&self) -> u64 {
        self.json_analysis_generation
    }

    #[cfg(test)]
    pub fn pending_json_analysis(&self) -> PendingJsonAnalysis {
        self.pending_json_analysis
    }

    pub fn claim_json_startup_analysis(&mut self) -> bool {
        if self.json_startup_analysis_queued {
            return false;
        }
        self.json_startup_analysis_queued = true;
        true
    }

    pub fn json_startup_analysis_queued(&self) -> bool {
        self.json_startup_analysis_queued
    }

    pub fn set_reference_context_mode(&mut self, mode: ReferenceContextMode) {
        self.reference_context_mode = mode;
        self.clear_contextual_views();
    }

    pub fn set_editor_workspace_options(
        &mut self,
        include_subfolders: bool,
        workspace_directory_scopes: bool,
    ) {
        self.include_subfolders = include_subfolders;
        self.workspace_directory_scopes = workspace_directory_scopes;
        self.clear_contextual_views();
    }

    pub fn set_reference_root_uri(&mut self, uri: Option<Url>) {
        self.reference_root_uri = uri;
        self.reference_root_cache.clear();
        self.reference_root_present_paths.clear();
        self.reference_root_present_stems.clear();
        self.clear_contextual_views();
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
        self.commit_scan_documents_with_reference_root(scan_generation, parsed, &[])
    }

    pub fn commit_scan_documents_with_reference_root(
        &mut self,
        scan_generation: u64,
        parsed: &[(Url, PathBuf, String, Arc<DocumentData>)],
        reference_root_parsed: &[(Url, PathBuf, String, Arc<DocumentData>)],
    ) -> bool {
        if self.scan_generation != scan_generation {
            return false;
        }
        self.file_cache.clear();
        for (_, path, _, document) in parsed {
            self.file_cache.insert(path.clone(), Arc::clone(document));
        }
        self.reference_root_cache.clear();
        for (_, path, _, document) in reference_root_parsed {
            self.reference_root_cache
                .insert(path.clone(), Arc::clone(document));
        }
        // A disk or explicit-reference rescan can change symbols used by any
        // open document even when that document's own version is unchanged.
        // Requeue every open publication against the newly committed snapshot.
        self.published_revisions.clear();
        self.rebuild_effective_symbols();
        self.begin_reconciliation(scan_generation)
    }

    pub fn rebuild_effective_symbols(&mut self) {
        self.symbols = SymbolIndex::new();
        for source in self.effective_sources() {
            self.symbols.index_effective_document(
                source.uri.as_ref(),
                &source.stem,
                &source.document,
                &self.ref_targets,
                source.kind,
                source.bundled_version.as_deref(),
            );
        }
        self.symbol_generation = self.symbol_generation.wrapping_add(1);
        self.clear_contextual_views();
    }

    fn replace_symbol_source(
        index: &mut SymbolIndex,
        source: &EffectiveSource,
        ref_targets: &HashSet<(String, String)>,
    ) {
        index.remove_file(&source.stem);
        index.index_effective_document(
            source.uri.as_ref(),
            &source.stem,
            &source.document,
            ref_targets,
            source.kind,
            source.bundled_version.as_deref(),
        );
    }

    pub fn refresh_open_document_symbols(&mut self, uri: &Url) {
        let Some(stem) = normalized_file_stem_from_uri(uri) else {
            self.rebuild_effective_symbols();
            return;
        };
        let global_source = self
            .effective_sources()
            .into_iter()
            .find(|source| source.stem.eq_ignore_ascii_case(&stem));
        let scoped_source = self
            .uses_directory_scopes()
            .then(|| {
                self.effective_sources_for_uri(uri)
                    .into_iter()
                    .find(|source| source.stem.eq_ignore_ascii_case(&stem))
            })
            .flatten();
        let Some(global_source) = global_source else {
            self.rebuild_effective_symbols();
            return;
        };

        Self::replace_symbol_source(&mut self.symbols, &global_source, &self.ref_targets);
        self.symbol_generation = self.symbol_generation.wrapping_add(1);

        let cache_key = if self.uses_directory_scopes() {
            self.directory_scope_key_for_uri(uri)
        } else {
            Some(String::new())
        };
        let Some(cache_key) = cache_key else {
            return;
        };
        let source = scoped_source.as_ref().unwrap_or(&global_source);
        let mut cached = self.directory_symbol_views.lock().unwrap();
        if let Some(view) = cached.get_mut(&cache_key) {
            Self::replace_symbol_source(Arc::make_mut(&mut view.index), source, &self.ref_targets);
            view.session_generation = self.session_generation;
            view.symbol_generation = self.symbol_generation;
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
        self.accept_open_with_dependency_invalidation(uri, version, document, true)
    }

    pub fn install_reference_dataset(
        &mut self,
        game_version: String,
        canonical_sha256: String,
        documents: HashMap<String, Arc<DocumentData>>,
    ) {
        self.reference_version = Some(game_version);
        self.reference_digest = Some(canonical_sha256);
        self.fallback_cache = documents;
        self.clear_contextual_views();
    }

    pub fn clear_reference_dataset(&mut self) {
        self.reference_version = None;
        self.reference_digest = None;
        self.fallback_cache.clear();
        self.clear_contextual_views();
    }

    pub fn set_workspace_present_paths<I>(&mut self, paths: I)
    where
        I: IntoIterator<Item = PathBuf>,
    {
        self.workspace_present_paths = paths.into_iter().collect();
        self.rebuild_workspace_present_stems();
        self.clear_contextual_views();
    }

    pub fn set_reference_root_present_paths<I>(&mut self, paths: I)
    where
        I: IntoIterator<Item = PathBuf>,
    {
        self.reference_root_present_paths = paths.into_iter().collect();
        self.rebuild_reference_root_present_stems();
        self.clear_contextual_views();
    }

    pub fn commit_scan_present_paths(
        &mut self,
        scan_generation: u64,
        workspace_paths: HashSet<PathBuf>,
        reference_root_paths: HashSet<PathBuf>,
    ) -> bool {
        if self.scan_generation != scan_generation {
            return false;
        }
        self.set_workspace_present_paths(workspace_paths);
        self.set_reference_root_present_paths(reference_root_paths);
        true
    }

    fn rebuild_reference_root_present_stems(&mut self) {
        self.reference_root_present_stems = self
            .reference_root_present_paths
            .iter()
            .filter_map(|path| path.file_stem().and_then(|stem| stem.to_str()))
            .map(str::to_ascii_lowercase)
            .collect();
    }

    fn rebuild_workspace_present_stems(&mut self) {
        self.workspace_present_stems = self
            .workspace_present_paths
            .iter()
            .filter_map(|path| path.file_stem().and_then(|stem| stem.to_str()))
            .map(str::to_ascii_lowercase)
            .collect();
    }

    pub fn accept_equivalent_open(
        &mut self,
        uri: Url,
        version: i32,
        document: Arc<DocumentData>,
    ) -> ValidationTicket {
        self.accept_open_with_dependency_invalidation(uri, version, document, false)
    }

    fn accept_open_with_dependency_invalidation(
        &mut self,
        uri: Url,
        version: i32,
        document: Arc<DocumentData>,
        invalidate_dependents: bool,
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
        if invalidate_dependents {
            self.published_revisions.clear();
        } else {
            self.published_revisions.remove(&uri);
        }
        self.current_ticket(&uri).expect("accepted open document")
    }

    pub fn effective_source_has_same_content(&self, uri: &Url, document: &DocumentData) -> bool {
        let Some(stem) = normalized_file_stem_from_uri(uri) else {
            return false;
        };
        self.effective_sources_for_uri(uri)
            .into_iter()
            .find(|source| source.stem == stem)
            .is_some_and(|source| source.document.as_ref() == document)
    }

    fn effective_sources(&self) -> Vec<EffectiveSource> {
        effective_workspace_sources_with_priority_tiers(
            &self.open_documents,
            &self.file_cache,
            &self.workspace_present_stems,
            self.reference_context_mode.disk_source_kind(),
            &self.reference_root_cache,
            &self.reference_root_present_stems,
            &self.fallback_cache,
            self.reference_version.as_deref(),
        )
    }

    fn uses_directory_scopes(&self) -> bool {
        self.workspace_directory_scopes
            && self.reference_context_mode == ReferenceContextMode::Workspace
    }

    fn directory_scope_key_for_uri(&self, uri: &Url) -> Option<String> {
        uri.to_file_path()
            .ok()
            .and_then(|path| path.parent().map(local_path_identity))
    }

    fn clear_contextual_views(&self) {
        *self.plugin_workspace_view.lock().unwrap() = None;
        self.plugin_directory_views.lock().unwrap().clear();
        self.directory_symbol_views.lock().unwrap().clear();
    }

    fn effective_sources_for_uri(&self, uri: &Url) -> Vec<EffectiveSource> {
        if !self.uses_directory_scopes() {
            return self.effective_sources();
        }
        let Some(target_parent) = uri
            .to_file_path()
            .ok()
            .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        else {
            return self.effective_sources();
        };
        let open_documents = self
            .open_documents
            .iter()
            .filter(|(source_uri, _)| uri_parent_is(source_uri, &target_parent))
            .map(|(source_uri, document)| (source_uri.clone(), Arc::clone(document)))
            .collect::<HashMap<_, _>>();
        let file_cache = self
            .file_cache
            .iter()
            .filter(|(path, _)| path_parent_is(path, &target_parent))
            .map(|(path, document)| (path.clone(), Arc::clone(document)))
            .collect::<HashMap<_, _>>();
        let present_stems = self
            .workspace_present_paths
            .iter()
            .filter(|path| path_parent_is(path, &target_parent))
            .filter_map(|path| path.file_stem().and_then(|stem| stem.to_str()))
            .map(str::to_ascii_lowercase)
            .collect::<HashSet<_>>();
        effective_workspace_sources_with_priority_tiers(
            &open_documents,
            &file_cache,
            &present_stems,
            SourceKind::Workspace,
            &HashMap::new(),
            &HashSet::new(),
            &self.fallback_cache,
            self.reference_version.as_deref(),
        )
    }

    pub fn symbols_for_uri(&self, uri: &Url) -> Arc<SymbolIndex> {
        if !self.uses_directory_scopes() {
            return self.cached_symbols_for_key("", || self.symbols.clone());
        }
        let Some(key) = self.directory_scope_key_for_uri(uri) else {
            return self.cached_symbols_for_key("", || self.symbols.clone());
        };
        self.cached_symbols_for_key(&key, || {
            let mut symbols = SymbolIndex::new();
            for source in self.effective_sources_for_uri(uri) {
                symbols.index_effective_document(
                    source.uri.as_ref(),
                    &source.stem,
                    &source.document,
                    &self.ref_targets,
                    source.kind,
                    source.bundled_version.as_deref(),
                );
            }
            symbols
        })
    }

    fn cached_symbols_for_key(
        &self,
        key: &str,
        build: impl FnOnce() -> SymbolIndex,
    ) -> Arc<SymbolIndex> {
        let mut cached = self.directory_symbol_views.lock().unwrap();
        if let Some(view) = cached.get(key)
            && view.session_generation == self.session_generation
            && view.symbol_generation == self.symbol_generation
        {
            return Arc::clone(&view.index);
        }
        let index = Arc::new(build());
        cached.insert(
            key.to_string(),
            DirectorySymbolView {
                session_generation: self.session_generation,
                symbol_generation: self.symbol_generation,
                index: Arc::clone(&index),
            },
        );
        index
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

    pub fn accept_open_json(&mut self, uri: Url, version: i32, text: String) {
        self.open_json_documents.insert(
            uri,
            OpenJsonDocument {
                version,
                text: Arc::from(text),
            },
        );
        self.bump_json_input_generation();
    }

    pub fn accept_change_json(
        &mut self,
        uri: &Url,
        version: i32,
        text: String,
    ) -> Result<(), DocumentChangeError> {
        let Some(current) = self.open_json_documents.get(uri) else {
            return Err(DocumentChangeError::NotOpen);
        };
        if version <= current.version {
            return Err(DocumentChangeError::StaleVersion {
                current: current.version,
                incoming: version,
            });
        }
        self.open_json_documents.insert(
            uri.clone(),
            OpenJsonDocument {
                version,
                text: Arc::from(text),
            },
        );
        self.bump_json_input_generation();
        Ok(())
    }

    pub fn close_open_json(&mut self, uri: &Url) -> bool {
        let removed = self.open_json_documents.remove(uri).is_some();
        if removed {
            self.bump_json_input_generation();
        }
        removed
    }

    pub fn open_json_text(&self, uri: &Url) -> Option<&str> {
        self.open_json_documents
            .get(uri)
            .map(|document| document.text.as_ref())
    }

    pub fn open_json_version(&self, uri: &Url) -> Option<i32> {
        self.open_json_documents
            .get(uri)
            .map(|document| document.version)
    }

    pub fn open_json_sources(&self) -> OpenJsonSources {
        self.open_json_documents
            .iter()
            .filter_map(|(uri, document)| {
                let path = uri.to_file_path().ok()?;
                Some((json_path_identity(&path), document.text.to_string()))
            })
            .collect()
    }

    pub fn restore_closed_document(
        &mut self,
        uri: &Url,
        path: Option<PathBuf>,
        disk_document: Option<Arc<DocumentData>>,
    ) -> bool {
        let path_present = disk_document.is_some();
        self.restore_closed_document_with_presence(uri, path, disk_document, path_present)
    }

    pub fn restore_closed_document_with_presence(
        &mut self,
        uri: &Url,
        path: Option<PathBuf>,
        disk_document: Option<Arc<DocumentData>>,
        path_present: bool,
    ) -> bool {
        let is_reference_root = self.is_reference_root_document(uri, path.as_deref());
        if self.close_open_document(uri).is_none() {
            return false;
        }
        if let Some(path) = path {
            if is_reference_root {
                self.reference_root_cache
                    .retain(|existing, _| !same_local_path(existing, &path));
                self.reference_root_present_paths
                    .retain(|existing| !same_local_path(existing, &path));
                if let Some(document) = disk_document {
                    self.reference_root_cache.insert(path.clone(), document);
                }
                if path_present {
                    self.reference_root_present_paths.insert(path.clone());
                }
                self.rebuild_reference_root_present_stems();
            } else {
                // An explicit reference root may be an ancestor of the primary
                // workspace. A primary file must never remain duplicated in the
                // lower-priority reference tier after close/delete restoration.
                self.reference_root_cache
                    .retain(|existing, _| !same_local_path(existing, &path));
                self.reference_root_present_paths
                    .retain(|existing| !same_local_path(existing, &path));
                self.file_cache
                    .retain(|existing, _| !same_local_path(existing, &path));
                self.workspace_present_paths
                    .retain(|existing| !same_local_path(existing, &path));
                if let Some(document) = disk_document {
                    self.file_cache.insert(path.clone(), document);
                }
                if path_present {
                    self.workspace_present_paths.insert(path.clone());
                }
                self.rebuild_workspace_present_stems();
                self.rebuild_reference_root_present_stems();
            }
        }
        self.rebuild_effective_symbols();
        true
    }

    fn is_reference_root_document(&self, uri: &Url, path: Option<&std::path::Path>) -> bool {
        let resolved_path = path
            .map(std::path::Path::to_path_buf)
            .or_else(|| uri.to_file_path().ok());
        resolved_path.is_some_and(|path| self.reference_tier_owns_path(&path))
    }

    fn cache_contains_local_path(
        cache: &HashMap<PathBuf, Arc<DocumentData>>,
        path: &std::path::Path,
    ) -> bool {
        cache.keys().any(|existing| same_local_path(existing, path))
    }

    fn set_contains_local_path(paths: &HashSet<PathBuf>, path: &std::path::Path) -> bool {
        paths.iter().any(|existing| same_local_path(existing, path))
    }

    fn primary_scope_contains_path(&self, path: &std::path::Path) -> bool {
        let Some(primary_root) = self
            .root_uri
            .as_ref()
            .and_then(|root| root.to_file_path().ok())
        else {
            return false;
        };
        if self.reference_context_mode == ReferenceContextMode::Sibling || !self.include_subfolders
        {
            path_parent_is(path, &primary_root)
        } else {
            local_path_is_within(path, &primary_root)
        }
    }

    fn reference_tier_owns_path(&self, path: &std::path::Path) -> bool {
        let exact_primary = Self::cache_contains_local_path(&self.file_cache, path)
            || Self::set_contains_local_path(&self.workspace_present_paths, path);
        if exact_primary {
            return false;
        }
        if self.primary_scope_contains_path(path) {
            return false;
        }
        let exact_reference = Self::cache_contains_local_path(&self.reference_root_cache, path)
            || Self::set_contains_local_path(&self.reference_root_present_paths, path);
        if exact_reference {
            return true;
        }
        let Some(reference_root) = self
            .reference_root_uri
            .as_ref()
            .and_then(|root| root.to_file_path().ok())
        else {
            return false;
        };
        local_path_is_within(path, &reference_root)
    }

    pub fn cached_disk_document(&self, path: &std::path::Path) -> Option<Arc<DocumentData>> {
        let cache = if self.reference_tier_owns_path(path) {
            &self.reference_root_cache
        } else {
            &self.file_cache
        };
        cache.get(path).cloned().or_else(|| {
            cache
                .iter()
                .find(|(existing, _)| same_local_path(existing, path))
                .map(|(_, document)| Arc::clone(document))
        })
    }

    pub fn disk_diagnostics_allowed(&self, uri: &Url) -> bool {
        if self.reference_context_mode == ReferenceContextMode::Sibling {
            return false;
        }
        let stem = normalized_file_stem_from_uri(uri);
        !self.open_documents.keys().any(|open_uri| {
            stem.is_some()
                && normalized_file_stem_from_uri(open_uri) == stem
                && (!self.uses_directory_scopes() || uri_parents_match(open_uri, uri))
        })
    }

    pub fn disk_documents_for_validation(&self) -> Vec<(Url, String, Arc<DocumentData>)> {
        if self.reference_context_mode == ReferenceContextMode::Sibling {
            return Vec::new();
        }
        if self.uses_directory_scopes() {
            let mut paths = self.file_cache.keys().cloned().collect::<Vec<_>>();
            paths.sort_by_key(|path| local_path_identity(path));
            let mut seen = HashSet::new();
            return paths
                .into_iter()
                .filter_map(|path| {
                    let stem = path.file_stem()?.to_str()?.to_ascii_lowercase();
                    let parent = path.parent()?;
                    let key = (local_path_identity(parent), stem.clone());
                    if !seen.insert(key) {
                        return None;
                    }
                    let uri = Url::from_file_path(&path).ok()?;
                    if !self.disk_diagnostics_allowed(&uri) {
                        return None;
                    }
                    self.file_cache
                        .get(&path)
                        .map(|document| (uri, stem, Arc::clone(document)))
                })
                .collect();
        }
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

    /// Primary, physical TXT inputs for the adjacent JSON lint scope. Open
    /// buffers replace the same disk path, while explicit reference-root and
    /// bundled documents are intentionally absent.
    pub fn primary_txt_documents_for_json(&self) -> Vec<PrimaryTxtDocument> {
        let active_data_roots = self.primary_json_data_roots();
        if active_data_roots.is_empty() {
            return Vec::new();
        }
        let mut documents = HashMap::<String, PrimaryTxtDocument>::new();
        for (path, document) in &self.file_cache {
            if data_root_from_excel_txt(path).is_some_and(|root| {
                active_data_roots
                    .iter()
                    .any(|active| same_local_path(&root, active))
            }) {
                documents.insert(
                    json_path_identity(path),
                    PrimaryTxtDocument {
                        path: path.clone(),
                        document: Arc::clone(document),
                    },
                );
            }
        }

        let primary_root = self
            .root_uri
            .as_ref()
            .and_then(|uri| uri.to_file_path().ok());
        for (uri, document) in &self.open_documents {
            let Ok(path) = uri.to_file_path() else {
                continue;
            };
            if data_root_from_excel_txt(&path).is_none_or(|root| {
                !active_data_roots
                    .iter()
                    .any(|active| same_local_path(&root, active))
            }) || primary_root
                .as_ref()
                .is_some_and(|root| !local_path_is_within(&path, root))
            {
                continue;
            }
            documents.insert(
                json_path_identity(&path),
                PrimaryTxtDocument {
                    path,
                    document: Arc::clone(document),
                },
            );
        }

        let mut documents = documents.into_values().collect::<Vec<_>>();
        documents.sort_by(|left, right| {
            json_path_identity(&left.path).cmp(&json_path_identity(&right.path))
        });
        documents
    }

    /// Physical mod scopes selected by primary workspace TXT documents.
    /// Disk documents discovered by Open Folder establish the scope even when
    /// no tab is open; open buffers then shadow those same disk paths in
    /// `primary_txt_documents_for_json`. Explicit reference-root and bundled
    /// documents are intentionally absent.
    pub fn primary_json_data_roots(&self) -> Vec<PathBuf> {
        let Some(primary_root) = self
            .root_uri
            .as_ref()
            .and_then(|uri| uri.to_file_path().ok())
        else {
            return Vec::new();
        };
        let mut roots = self
            .file_cache
            .keys()
            .filter(|path| local_path_is_within(path, &primary_root))
            .filter_map(|path| data_root_from_excel_txt(path))
            .collect::<Vec<_>>();
        roots.extend(
            self.open_documents
                .keys()
                .filter_map(|uri| self.primary_json_data_root_for_uri(uri)),
        );
        roots.sort_by_key(|root| json_path_identity(root));
        roots.dedup_by(|left, right| same_local_path(left, right));
        roots
    }

    pub fn primary_json_data_root_for_uri(&self, uri: &Url) -> Option<PathBuf> {
        let path = uri.to_file_path().ok()?;
        let primary_root = self.root_uri.as_ref()?.to_file_path().ok()?;
        if !local_path_is_within(&path, &primary_root) {
            return None;
        }
        data_root_from_excel_txt(&path)
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

    pub fn published_json_diagnostic_uris(&self) -> Vec<Url> {
        let mut uris = self
            .published_json_diagnostics
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        uris.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        uris
    }

    pub fn published_json_diagnostics(&self) -> HashMap<Url, Vec<Diagnostic>> {
        self.published_json_diagnostics
            .iter()
            .map(|(uri, snapshot)| (uri.clone(), snapshot.diagnostics.clone()))
            .collect()
    }

    pub fn json_diagnostics_for_uri(&self, uri: &Url) -> Vec<Diagnostic> {
        self.published_json_diagnostics
            .get(uri)
            .map(|snapshot| snapshot.diagnostics.clone())
            .unwrap_or_default()
    }

    pub fn json_diagnostics_version_for_uri(&self, uri: &Url) -> Option<Option<i32>> {
        self.published_json_diagnostics
            .get(uri)
            .map(|snapshot| snapshot.version)
    }

    pub fn record_json_diagnostics(&mut self, uri: Url, diagnostics: Vec<Diagnostic>) {
        self.record_json_diagnostics_for_version(uri, diagnostics, None);
    }

    pub fn record_json_diagnostics_for_version(
        &mut self,
        uri: Url,
        diagnostics: Vec<Diagnostic>,
        version: Option<i32>,
    ) {
        if diagnostics.is_empty() {
            self.published_json_diagnostics.remove(&uri);
        } else {
            self.published_json_diagnostics.insert(
                uri,
                PublishedJsonDiagnosticSnapshot {
                    diagnostics,
                    version,
                },
            );
        }
    }

    pub fn json_input_generation(&self) -> u64 {
        self.json_input_generation
    }

    pub fn bump_json_input_generation(&mut self) -> u64 {
        self.json_input_generation = self.json_input_generation.wrapping_add(1);
        self.json_input_generation
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

    /// Change only the presentation locale.  The semantic workspace and
    /// source files are untouched, while clearing publication markers makes
    /// every currently open document eligible for a re-rendered diagnostic.
    pub fn set_locale(&mut self, locale: Locale) -> bool {
        if self.locale == locale {
            return false;
        }
        self.locale = locale;
        self.workspace_revision = self.workspace_revision.wrapping_add(1);
        self.published_revisions.clear();
        true
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
    fn json_primary_inputs_use_open_buffers_and_exclude_reference_sources() {
        let base = std::env::temp_dir().join("vlsp-json-primary-inputs");
        let primary_excel = base.join("mod/data/global/excel");
        let primary_path = primary_excel.join("skills.txt");
        let reference_excel = base.join("reference/data/global/excel");
        let reference_path = reference_excel.join("skills.txt");
        let primary_uri = Url::from_file_path(&primary_path).unwrap();
        let reference_uri = Url::from_file_path(&reference_path).unwrap();

        let mut workspace = Workspace::new();
        workspace.root_uri = Url::from_directory_path(&primary_excel).ok();
        workspace.reference_root_uri = Url::from_directory_path(&reference_excel).ok();
        workspace.file_cache.insert(
            primary_path,
            Arc::new(DocumentData::parse("name\nDISK", '\t')),
        );
        workspace.open_documents.insert(
            primary_uri,
            Arc::new(DocumentData::parse("name\nOPEN", '\t')),
        );
        workspace.open_documents.insert(
            reference_uri,
            Arc::new(DocumentData::parse("name\nREFERENCE", '\t')),
        );

        let inputs = workspace.primary_txt_documents_for_json();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].document.rows[0].cells[0].value, "OPEN");
    }

    #[test]
    fn json_scope_is_the_union_of_primary_disk_roots_before_any_tab_is_open() {
        let base = std::env::temp_dir().join("vlsp-json-open-root-union");
        let root_a = base.join("a/data/global/excel");
        let root_b = base.join("b/data/global/excel");
        let a_skills = root_a.join("skills.txt");
        let a_items = root_a.join("itemtypes.txt");
        let b_skills = root_b.join("skills.txt");
        let mut workspace = Workspace::new();
        workspace.root_uri = Url::from_directory_path(&base).ok();
        for path in [&a_skills, &a_items, &b_skills] {
            workspace.file_cache.insert(path.clone(), doc("DISK"));
        }
        workspace
            .open_documents
            .insert(Url::from_file_path(&a_skills).unwrap(), doc("OPEN-A"));

        let roots = workspace.primary_json_data_roots();
        assert_eq!(roots.len(), 2);
        assert!(
            roots
                .iter()
                .any(|root| same_local_path(root, &base.join("a/data")))
        );
        assert!(
            roots
                .iter()
                .any(|root| same_local_path(root, &base.join("b/data")))
        );
        let inputs = workspace.primary_txt_documents_for_json();
        assert_eq!(inputs.len(), 3);
        assert!(inputs.iter().any(|input| {
            same_local_path(&input.path, &a_skills)
                && input.document.rows[0].cells[0].value == "OPEN-A"
        }));

        workspace
            .open_documents
            .insert(Url::from_file_path(&b_skills).unwrap(), doc("OPEN-B"));
        assert_eq!(workspace.primary_json_data_roots().len(), 2);
        assert_eq!(workspace.primary_txt_documents_for_json().len(), 3);
    }

    #[test]
    fn nested_primary_root_wins_over_a_reference_ancestor_for_json_scope() {
        let base = std::env::temp_dir().join("vlsp-json-nested-primary");
        let primary = base.join("reference/mod/data/global/excel");
        let path = primary.join("skills.txt");
        let uri = Url::from_file_path(&path).unwrap();
        let mut workspace = Workspace::new();
        workspace.root_uri = Url::from_directory_path(&primary).ok();
        workspace.reference_root_uri = Url::from_directory_path(base.join("reference")).ok();
        workspace.open_documents.insert(uri.clone(), doc("OPEN"));
        workspace.file_cache.insert(path.clone(), doc("DISK"));
        workspace
            .reference_root_cache
            .insert(path.clone(), doc("STALE-REFERENCE-DUPLICATE"));
        workspace.set_workspace_present_paths([path.clone()]);
        workspace.set_reference_root_present_paths([path.clone()]);

        assert_eq!(workspace.primary_json_data_roots().len(), 1);
        assert_eq!(workspace.primary_txt_documents_for_json().len(), 1);
        assert_eq!(
            workspace.cached_disk_document(&path).unwrap().rows[0].cells[0].value,
            "DISK"
        );
        assert!(workspace.restore_closed_document_with_presence(
            &uri,
            Some(path.clone()),
            Some(doc("LATEST-PRIMARY-DISK")),
            true,
        ));
        assert_eq!(
            workspace.cached_disk_document(&path).unwrap().rows[0].cells[0].value,
            "LATEST-PRIMARY-DISK"
        );
        assert!(workspace.reference_root_cache.is_empty());
        assert!(workspace.reference_root_present_paths.is_empty());
    }

    #[test]
    fn stale_all_json_analysis_is_requeued_without_losing_rule_strength() {
        let mut workspace = Workspace::new();
        workspace.begin_initialization(7);
        workspace.phase = WorkspacePhase::Ready;
        let (generation, start_worker) = workspace.queue_json_analysis(true);
        assert!(start_worker);
        let pending = workspace
            .take_json_analysis(7, generation)
            .expect("initial All analysis");
        assert!(pending.all_rules);
        assert!(workspace.requeue_json_analysis(7, pending));

        let retry_generation = workspace.json_analysis_generation();
        let retry = workspace
            .take_json_analysis(7, retry_generation)
            .expect("stale All analysis must be retried");
        assert!(retry.all_rules);
        assert!(!retry.key_usage);
    }

    #[test]
    fn ready_and_new_request_racing_a_defer_keep_the_json_worker_awake() {
        let mut workspace = Workspace::new();
        workspace.begin_initialization(9);
        workspace.phase = WorkspacePhase::Reconciling;
        let (worker_generation, start_worker) = workspace.queue_json_analysis(true);
        assert!(start_worker);
        let pending = workspace
            .take_json_analysis(9, worker_generation)
            .expect("request observed while reconciling");

        // This transition and queue happen after the worker observed the old
        // phase but before it reacquires the write lock to defer its request.
        workspace.phase = WorkspacePhase::Ready;
        let (_, second_worker) = workspace.queue_json_analysis(false);
        assert!(!second_worker);
        assert!(workspace.defer_json_analysis(9, worker_generation, pending));
        assert!(workspace.json_analysis_worker_running());
        assert_eq!(
            workspace.pending_json_analysis(),
            PendingJsonAnalysis {
                all_rules: true,
                key_usage: true,
            }
        );
    }

    #[test]
    fn scanning_json_watch_catch_up_stays_all_until_readiness_arrives() {
        let mut workspace = Workspace::new();
        workspace.begin_initialization(11);
        workspace.phase = WorkspacePhase::Scanning;
        let (generation, _) = workspace.queue_json_analysis(true);
        let pending = workspace.take_json_analysis(11, generation).unwrap();
        assert!(!workspace.defer_json_analysis(11, generation, pending));
        assert!(!workspace.json_analysis_worker_running());

        workspace.phase = WorkspacePhase::Ready;
        let (_, start_worker) = workspace.queue_json_analysis(false);
        assert!(start_worker);
        assert_eq!(
            workspace.pending_json_analysis(),
            PendingJsonAnalysis {
                all_rules: true,
                key_usage: true,
            }
        );
    }

    #[test]
    fn late_txt_registration_defers_without_invalidating_the_initial_scan() {
        let mut workspace = Workspace::new();
        workspace.begin_initialization(13);
        let initial_scan = workspace.begin_scan();
        assert_eq!(initial_scan, 1);

        // A registration response arriving after the bounded startup grace is
        // recorded, but the active scan remains authoritative until Ready.
        workspace.defer_txt_watch_registration_catch_up();
        assert_eq!(workspace.scan_generation, initial_scan);
        assert_eq!(workspace.phase, WorkspacePhase::Scanning);
        assert!(workspace.has_pending_txt_watch_registration_catch_up());
        assert!(!workspace.watched_change_worker_running());

        workspace.phase = WorkspacePhase::Ready;
        assert!(workspace.take_txt_watch_registration_catch_up());
        let (_, start_worker) = workspace.queue_watched_changes(true);
        assert!(start_worker);
        assert_eq!(workspace.scan_generation, initial_scan + 1);
    }

    #[test]
    fn nested_reference_descendant_below_a_primary_folder_keeps_its_tier() {
        let base = std::env::temp_dir().join("vlsp-nested-reference-descendant");
        let reference_root = base.join("reference");
        let primary_root = reference_root.join("primary");
        let path = primary_root.join("nested/items.txt");
        let uri = Url::from_file_path(&path).unwrap();
        let mut workspace = Workspace::new();
        workspace.root_uri = Url::from_directory_path(&primary_root).ok();
        workspace.reference_root_uri = Url::from_directory_path(&reference_root).ok();
        workspace.set_reference_context_mode(ReferenceContextMode::Sibling);
        workspace.set_reference_root_present_paths([path.clone()]);
        workspace
            .reference_root_cache
            .insert(path.clone(), doc("REFERENCE-DISK"));
        workspace.open_documents.insert(uri.clone(), doc("OPEN"));

        assert_eq!(
            workspace.cached_disk_document(&path).unwrap().rows[0].cells[0].value,
            "REFERENCE-DISK"
        );
        assert!(workspace.restore_closed_document_with_presence(
            &uri,
            Some(path.clone()),
            Some(doc("LATEST-REFERENCE-DISK")),
            true,
        ));
        assert!(workspace.file_cache.is_empty());
        assert!(workspace.workspace_present_paths.is_empty());
        assert_eq!(
            workspace.reference_root_cache[&path].rows[0].cells[0].value,
            "LATEST-REFERENCE-DISK"
        );
        assert!(workspace.reference_root_present_paths.contains(&path));
    }

    #[test]
    fn failed_json_watch_registration_rolls_back_and_active_registration_can_retire() {
        let mut workspace = Workspace::new();
        workspace.set_watched_files_client_capabilities(true, true);
        let session_generation = workspace.session_generation;
        let identity = "c:/mod/data|strings|files|c:/mod/data/strings|*.json";
        let first = workspace
            .reserve_watched_json_registration(identity)
            .unwrap();
        assert!(workspace.finish_watched_json_registration(
            identity,
            session_generation,
            first,
            "registration-1",
            false,
        ));
        let second = workspace
            .reserve_watched_json_registration(identity)
            .unwrap();
        assert!(workspace.finish_watched_json_registration(
            identity,
            session_generation,
            second,
            "registration-2",
            true,
        ));
        assert!(
            workspace
                .reserve_watched_json_registration(identity)
                .is_none()
        );
        assert_eq!(
            workspace.take_obsolete_watched_json_registrations(&HashSet::new()),
            vec!["registration-2"]
        );
    }

    #[test]
    fn late_json_watch_registration_response_cannot_activate_a_new_session_slot() {
        let mut workspace = Workspace::new();
        workspace.set_watched_files_client_capabilities(true, true);
        let identity = "c:/mod/data|strings|files|c:/mod/data/strings|*.json";
        workspace.begin_initialization(17);
        let old_sequence = workspace
            .reserve_watched_json_registration(identity)
            .unwrap();

        workspace.begin_initialization(18);
        let new_sequence = workspace
            .reserve_watched_json_registration(identity)
            .unwrap();
        assert_eq!(old_sequence, new_sequence, "sequence resets per session");
        assert!(!workspace.finish_watched_json_registration(
            identity,
            17,
            old_sequence,
            "old-session-registration",
            true,
        ));
        assert!(workspace.finish_watched_json_registration(
            identity,
            18,
            new_sequence,
            "current-registration",
            true,
        ));
    }

    #[test]
    fn json_watch_retirement_survives_session_reset_and_failed_unregistration() {
        let mut workspace = Workspace::new();
        workspace.set_watched_files_client_capabilities(true, true);
        workspace.begin_initialization(23);
        let identity = "c:/mod/data|strings|files|c:/mod/data/strings|*.json";
        let sequence = workspace
            .reserve_watched_json_registration(identity)
            .unwrap();
        assert!(workspace.finish_watched_json_registration(
            identity,
            23,
            sequence,
            "active-registration",
            true,
        ));

        workspace.begin_initialization(24);
        assert_eq!(
            workspace.pending_watched_json_unregistrations(),
            vec!["active-registration"]
        );
        assert!(workspace.queue_watched_json_unregistrations(Vec::new()));
        let first_attempt = workspace.take_watched_json_unregistrations();
        assert_eq!(first_attempt, vec!["active-registration"]);
        assert!(!workspace.defer_watched_json_unregistrations(first_attempt));
        assert_eq!(
            workspace.pending_watched_json_unregistrations(),
            vec!["active-registration"]
        );

        // A later event-driven scope sync wakes the retained retry; no polling
        // worker remains active after the failed request.
        assert!(workspace.queue_watched_json_unregistrations(Vec::new()));
        let retry = workspace.take_watched_json_unregistrations();
        assert_eq!(retry, vec!["active-registration"]);
        assert!(workspace.finish_watched_json_unregistration_worker_if_idle());
        assert!(workspace.pending_watched_json_unregistrations().is_empty());
    }

    #[test]
    fn new_retirement_request_racing_a_failure_keeps_the_worker_awake() {
        let mut workspace = Workspace::new();
        assert!(workspace.queue_watched_json_unregistrations(vec!["old".to_string()]));
        let failed = workspace.take_watched_json_unregistrations();
        assert!(!workspace.queue_watched_json_unregistrations(vec!["new".to_string()]));
        assert!(workspace.defer_watched_json_unregistrations(failed));
        assert_eq!(
            workspace.pending_watched_json_unregistrations(),
            vec!["new", "old"]
        );
    }

    #[test]
    fn json_publications_are_tracked_by_uri_not_txt_stem() {
        let mut workspace = Workspace::new();
        let json_uri = Url::parse("file:///workspace/monsters.json").unwrap();
        let txt_uri = Url::parse("file:///workspace/monsters.txt").unwrap();
        workspace.record_json_diagnostics(json_uri.clone(), vec![Diagnostic::default()]);
        workspace.record_disk_diagnostics(txt_uri, true);
        assert_eq!(workspace.published_json_diagnostic_uris(), vec![json_uri]);
    }

    #[test]
    fn json_publication_versions_follow_the_authoritative_snapshot() {
        let mut workspace = Workspace::new();
        let uri = Url::parse("file:///workspace/skills.json").unwrap();
        let diagnostics = vec![Diagnostic {
            message: "same diagnostics".to_string(),
            ..Diagnostic::default()
        }];

        workspace.record_json_diagnostics_for_version(uri.clone(), diagnostics.clone(), None);
        assert_eq!(workspace.json_diagnostics_version_for_uri(&uri), Some(None));

        workspace.record_json_diagnostics_for_version(uri.clone(), diagnostics, Some(7));
        assert_eq!(
            workspace.json_diagnostics_version_for_uri(&uri),
            Some(Some(7))
        );

        workspace.record_json_diagnostics_for_version(uri.clone(), Vec::new(), Some(7));
        assert_eq!(workspace.json_diagnostics_version_for_uri(&uri), None);
    }

    #[test]
    fn stale_json_publication_clear_removes_the_unchanged_result_snapshot() {
        let mut workspace = Workspace::new();
        let uri = Url::parse("file:///workspace/skills.json").unwrap();
        let diagnostics = vec![Diagnostic {
            message: "same retry result".to_string(),
            ..Diagnostic::default()
        }];
        workspace.record_json_diagnostics(uri.clone(), diagnostics.clone());

        // This mirrors publish_json_batch_if_current's stale-after-await path:
        // clearing both client and cache makes an identical retry publishable.
        workspace.record_json_diagnostics(uri.clone(), Vec::new());
        assert!(workspace.json_diagnostics_for_uri(&uri).is_empty());
        assert_ne!(workspace.json_diagnostics_for_uri(&uri), diagnostics);
    }

    #[test]
    fn equivalent_open_preserves_unrelated_publications_and_only_queues_the_opened_uri() {
        let mut workspace = Workspace::new();
        workspace.begin_initialization(7);
        workspace.begin_scan();
        workspace
            .file_cache
            .insert(PathBuf::from("C:/workspace/items.txt"), doc("SAME"));

        let unrelated_uri = named_uri("other");
        workspace.accept_open(unrelated_uri.clone(), 1, doc("OTHER"));
        mark_all_open_documents_published(&mut workspace);

        let items_uri = named_uri("items");
        assert!(workspace.effective_source_has_same_content(&items_uri, &doc("SAME")));
        assert!(!workspace.effective_source_has_same_content(&items_uri, &doc("CHANGED")));

        let items_ticket = workspace.accept_equivalent_open(items_uri.clone(), 1, doc("SAME"));
        let pending = workspace.pending_open_tickets();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0], items_ticket);
        assert!(!workspace.needs_publish(&workspace.current_ticket(&unrelated_uri).unwrap()));
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
    fn editor_workspace_directory_scopes_keep_same_named_tables_independent() {
        let mut workspace = Workspace::new();
        workspace.set_editor_workspace_options(true, true);
        workspace
            .ref_targets
            .insert(("items".to_string(), "id".to_string()));
        let root_path = std::env::temp_dir().join("vector-lsp-directory-scopes");
        let root_items_path = root_path.join("items.txt");
        let base_items_path = root_path.join("base").join("items.txt");
        workspace
            .file_cache
            .insert(root_items_path.clone(), doc("ROOT"));
        workspace
            .file_cache
            .insert(base_items_path.clone(), doc("BASE"));
        workspace.set_workspace_present_paths([root_items_path, base_items_path]);

        let root_uri = Url::from_file_path(root_path.join("consumer.txt")).unwrap();
        let base_uri = Url::from_file_path(root_path.join("base").join("consumer.txt")).unwrap();
        let root_symbols = workspace.symbols_for_uri(&root_uri);
        let base_symbols = workspace.symbols_for_uri(&base_uri);
        assert!(Arc::ptr_eq(
            &root_symbols,
            &workspace.symbols_for_uri(&root_uri)
        ));
        assert!(root_symbols.contains_resolved("items", "id", "ROOT", ReferenceResolver::AsciiCi));
        assert!(!root_symbols.contains_resolved("items", "id", "BASE", ReferenceResolver::AsciiCi));
        assert!(base_symbols.contains_resolved("items", "id", "BASE", ReferenceResolver::AsciiCi));
        assert!(!base_symbols.contains_resolved("items", "id", "ROOT", ReferenceResolver::AsciiCi));

        let disk_uris = workspace
            .disk_documents_for_validation()
            .into_iter()
            .map(|(uri, _, _)| uri.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            disk_uris,
            vec![
                Url::from_file_path(root_path.join("base").join("items.txt"))
                    .unwrap()
                    .to_string(),
                Url::from_file_path(root_path.join("items.txt"))
                    .unwrap()
                    .to_string()
            ]
        );

        let base_items_uri = Url::from_file_path(root_path.join("base").join("items.txt")).unwrap();
        workspace.accept_open(base_items_uri, 1, doc("OPEN_BASE"));
        let root_view = workspace.plugin_workspace_view_for_uri(&root_uri);
        let base_view = workspace.plugin_workspace_view_for_uri(&base_uri);
        assert!(root_view.index.lookup("items", "id", "ROOT"));
        assert!(!root_view.index.lookup("items", "id", "OPEN_BASE"));
        assert!(base_view.index.lookup("items", "id", "OPEN_BASE"));
        assert!(!base_view.index.lookup("items", "id", "ROOT"));
        assert_eq!(workspace.disk_documents_for_validation().len(), 1);
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
    fn open_document_symbol_refresh_replaces_only_the_changed_file() {
        let uri = uri();
        let mut workspace = Workspace::new();
        workspace.begin_initialization(8);
        workspace.workspace_directory_scopes = true;
        workspace
            .ref_targets
            .insert(("items".to_string(), "id".to_string()));
        workspace.accept_open(
            uri.clone(),
            1,
            Arc::new(DocumentData::parse("id\tlabel\nKEY\told", '\t')),
        );
        workspace.rebuild_effective_symbols();
        let first = workspace.symbols_for_uri(&uri);
        assert!(first.lookup("items", "id", "KEY").is_some());

        let label_edit = Arc::new(DocumentData::parse("id\tlabel\nKEY\tnew", '\t'));
        workspace.accept_change(&uri, 2, label_edit).unwrap();
        workspace.refresh_open_document_symbols(&uri);
        let after_label_edit = workspace.symbols_for_uri(&uri);
        assert!(after_label_edit.lookup("items", "id", "KEY").is_some());

        let id_edit = Arc::new(DocumentData::parse("id\tlabel\nOTHER\tnew", '\t'));
        workspace.accept_change(&uri, 3, id_edit).unwrap();
        workspace.refresh_open_document_symbols(&uri);
        let after_id_edit = workspace.symbols_for_uri(&uri);
        assert!(after_id_edit.lookup("items", "id", "KEY").is_none());
        assert!(after_id_edit.lookup("items", "id", "OTHER").is_some());
        assert!(first.lookup("items", "id", "KEY").is_some());
    }

    #[test]
    fn live_diagnostic_workers_coalesce_and_follow_the_current_session() {
        let uri = uri();
        let mut workspace = Workspace::new();
        workspace.begin_initialization(21);
        workspace.phase = WorkspacePhase::Ready;
        let first = workspace.accept_open(uri.clone(), 1, doc("V1"));

        assert_eq!(workspace.reserve_schema_preview_worker(&uri), Some(21));
        assert_eq!(workspace.reserve_schema_preview_worker(&uri), None);
        let second = workspace.accept_change(&uri, 2, doc("V2")).unwrap();
        assert!(!workspace.finish_schema_preview_worker_if_current(&first));
        assert!(workspace.finish_schema_preview_worker_if_current(&second));

        assert_eq!(workspace.start_workspace_revalidation_worker(), Some(21));
        assert_eq!(workspace.start_workspace_revalidation_worker(), None);
        assert!(workspace.workspace_revalidation_worker_should_continue(21));
        assert!(workspace.mark_published(&second));
        assert!(!workspace.workspace_revalidation_worker_should_continue(21));

        workspace.begin_initialization(22);
        workspace.phase = WorkspacePhase::Ready;
        assert_eq!(workspace.reserve_schema_preview_worker(&uri), Some(22));
        assert_eq!(workspace.start_workspace_revalidation_worker(), Some(22));
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
        assert!(workspace.mark_published(&live));
        assert!(workspace.pending_open_tickets().is_empty());
        assert!(workspace.commit_scan_documents(scan_generation, &parsed));

        assert!(workspace.symbols.lookup("items", "id", "OLD").is_none());
        assert!(workspace.symbols.lookup("items", "id", "NEW").is_some());
        assert_eq!(
            workspace.open_documents[&live.uri].rows[0].cells[0].value,
            "NEW"
        );
        assert_eq!(workspace.file_cache.len(), 1);
        assert_eq!(workspace.pending_open_tickets().len(), 1);
        assert_eq!(workspace.pending_open_tickets()[0].uri, live.uri);
    }

    #[test]
    fn stale_scan_cannot_overwrite_newer_presence_sets() {
        let mut workspace = Workspace::new();
        workspace.begin_initialization(5);
        let stale_generation = workspace.begin_scan();
        let current_generation = workspace.begin_scan();
        let stale_path = PathBuf::from("/workspace/stale/items.txt");
        let current_path = PathBuf::from("/workspace/current/items.txt");

        assert!(workspace.commit_scan_present_paths(
            current_generation,
            HashSet::from([current_path.clone()]),
            HashSet::new(),
        ));
        assert!(!workspace.commit_scan_present_paths(
            stale_generation,
            HashSet::from([stale_path]),
            HashSet::new(),
        ));
        assert_eq!(
            workspace.workspace_present_paths,
            HashSet::from([current_path])
        );
    }

    #[test]
    fn local_path_identity_ignores_a_directory_uris_trailing_separator() {
        #[cfg(windows)]
        assert!(same_local_path(
            std::path::Path::new(r"C:\Mods\Example\data\global\excel\"),
            std::path::Path::new(r"c:\mods\example\data\global\excel"),
        ));
        #[cfg(not(windows))]
        assert!(same_local_path(
            std::path::Path::new("/mods/example/data/global/excel/"),
            std::path::Path::new("/mods/example/data/global/excel"),
        ));
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
    fn locale_change_republishes_open_documents_without_changing_content() {
        let uri = named_uri("locale");
        let mut workspace = Workspace::new();
        workspace.begin_initialization(1);
        workspace.accept_open(uri, 1, doc("KEY"));
        mark_all_open_documents_published(&mut workspace);

        assert!(workspace.set_locale(Locale::KoKr));
        assert_eq!(workspace.locale, Locale::KoKr);
        assert_eq!(workspace.pending_open_tickets().len(), 1);
        assert!(!workspace.set_locale(Locale::KoKr));
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
    fn close_restores_disk_then_bundled_fallback_when_the_workspace_path_is_deleted() {
        let path = std::env::temp_dir()
            .join("vlsp-reference-close-fallback")
            .join("items.txt");
        let uri = Url::from_file_path(&path).unwrap();
        let mut workspace = Workspace::new();
        workspace
            .ref_targets
            .insert(("items".to_string(), "id".to_string()));
        workspace.install_reference_dataset(
            "3.2".to_string(),
            "fixture-digest".to_string(),
            HashMap::from([("items".to_string(), doc("BUNDLED"))]),
        );
        workspace.set_workspace_present_paths([path.clone()]);
        workspace.file_cache.insert(path.clone(), doc("DISK"));
        workspace.rebuild_effective_symbols();
        assert_eq!(
            workspace
                .symbols
                .resolve("items", "id", "DISK", ReferenceResolver::AsciiCi)
                .unwrap()
                .source_kind,
            SourceKind::Workspace
        );
        let disk_view = workspace.plugin_workspace_view();
        assert_eq!(disk_view.snapshot.sources["items"].kind, "workspace");
        assert_eq!(
            disk_view.snapshot.sources["items"].version.as_deref(),
            Some("3.2")
        );

        workspace.accept_open(uri.clone(), 1, doc("OPEN"));
        workspace.rebuild_effective_symbols();
        assert_eq!(
            workspace
                .symbols
                .resolve("items", "id", "OPEN", ReferenceResolver::AsciiCi)
                .unwrap()
                .source_kind,
            SourceKind::Open
        );
        let open_view = workspace.plugin_workspace_view();
        assert_eq!(open_view.snapshot.sources["items"].kind, "open");
        assert_eq!(
            open_view.snapshot.sources["items"].version.as_deref(),
            Some("3.2")
        );
        assert!(workspace.restore_closed_document(&uri, Some(path.clone()), Some(doc("DISK2")),));
        assert_eq!(
            workspace
                .symbols
                .resolve("items", "id", "DISK2", ReferenceResolver::AsciiCi)
                .unwrap()
                .source_kind,
            SourceKind::Workspace
        );

        workspace.accept_open(uri.clone(), 1, doc("OPEN2"));
        workspace.rebuild_effective_symbols();
        assert!(workspace.restore_closed_document(&uri, Some(path.clone()), None));
        let restored = workspace
            .symbols
            .resolve("items", "id", "BUNDLED", ReferenceResolver::AsciiCi)
            .expect("deleted workspace path should reveal the bundled table");
        assert_eq!(restored.source_kind, SourceKind::Bundled);
        assert_eq!(restored.bundled_version.as_deref(), Some("3.2"));
        assert!(restored.location.is_none());
        assert!(!workspace.workspace_present_paths.contains(&path));

        let plugin_view = workspace.plugin_workspace_view();
        assert_eq!(
            plugin_view.snapshot.files["items"].rows[0].cells[0].value,
            "BUNDLED"
        );
        assert_eq!(plugin_view.snapshot.sources["items"].kind, "bundled");
        assert_eq!(
            plugin_view.snapshot.sources["items"].version.as_deref(),
            Some("3.2")
        );
    }

    #[test]
    fn sibling_context_is_hidden_shadows_and_restores_without_masking_read_failures() {
        let path = std::env::temp_dir()
            .join("vlsp-sibling-reference-context")
            .join("items.txt");
        let uri = Url::from_file_path(&path).unwrap();
        let mut workspace = Workspace::new();
        workspace.set_reference_context_mode(ReferenceContextMode::Sibling);
        workspace
            .ref_targets
            .insert(("items".to_string(), "id".to_string()));
        workspace.install_reference_dataset(
            "3.2".to_string(),
            "fixture-digest".to_string(),
            HashMap::from([("items".to_string(), doc("BUNDLED"))]),
        );
        workspace.set_workspace_present_paths([path.clone()]);
        workspace.file_cache.insert(path.clone(), doc("SIBLING"));
        workspace.rebuild_effective_symbols();

        let sibling = workspace
            .symbols
            .resolve("items", "id", "SIBLING", ReferenceResolver::AsciiCi)
            .expect("direct sibling should supply the effective table");
        assert_eq!(sibling.source_kind, SourceKind::Sibling);
        assert!(
            sibling.location.is_none(),
            "hidden sibling references must not navigate into an editor tab"
        );
        assert!(workspace.disk_documents_for_validation().is_empty());
        assert!(!workspace.disk_diagnostics_allowed(&uri));
        let sibling_view = workspace.plugin_workspace_view();
        assert_eq!(sibling_view.snapshot.sources["items"].kind, "sibling");
        assert_eq!(
            sibling_view.snapshot.sources["items"].version.as_deref(),
            Some("3.2")
        );

        workspace.accept_open(uri.clone(), 4, doc("OPEN"));
        workspace.rebuild_effective_symbols();
        let opened = workspace
            .symbols
            .resolve("items", "id", "OPEN", ReferenceResolver::AsciiCi)
            .unwrap();
        assert_eq!(opened.source_kind, SourceKind::Open);
        assert!(opened.location.is_some());
        assert_eq!(
            workspace.accept_change(&uri, 3, doc("STALE")),
            Err(DocumentChangeError::StaleVersion {
                current: 4,
                incoming: 3,
            })
        );
        assert!(workspace.restore_closed_document_with_presence(
            &uri,
            Some(path.clone()),
            Some(doc("LATEST-DISK")),
            true,
        ));
        let latest_sibling = workspace
            .symbols
            .resolve("items", "id", "LATEST-DISK", ReferenceResolver::AsciiCi)
            .unwrap();
        assert_eq!(latest_sibling.source_kind, SourceKind::Sibling);
        assert!(latest_sibling.location.is_none());

        workspace.accept_open(uri.clone(), 5, doc("OPEN-AGAIN"));
        workspace.rebuild_effective_symbols();
        assert!(workspace.restore_closed_document_with_presence(
            &uri,
            Some(path.clone()),
            None,
            true,
        ));
        assert!(workspace.workspace_present_paths.contains(&path));
        assert!(
            workspace
                .symbols
                .resolve("items", "id", "BUNDLED", ReferenceResolver::AsciiCi)
                .is_none(),
            "an existing unreadable sibling must block bundled fallback"
        );

        workspace.accept_open(uri.clone(), 6, doc("OPEN-LAST"));
        workspace.rebuild_effective_symbols();
        assert!(workspace.restore_closed_document_with_presence(
            &uri,
            Some(path.clone()),
            None,
            false,
        ));
        let bundled = workspace
            .symbols
            .resolve("items", "id", "BUNDLED", ReferenceResolver::AsciiCi)
            .expect("deleted sibling should reveal bundled fallback");
        assert_eq!(bundled.source_kind, SourceKind::Bundled);
        assert!(!workspace.workspace_present_paths.contains(&path));
    }

    #[test]
    fn reference_root_close_restores_its_own_tier_and_presence_state() {
        let reference_root = std::env::temp_dir().join("vlsp-explicit-reference-root");
        let path = reference_root.join("items.txt");
        let uri = Url::from_file_path(&path).unwrap();
        let mut workspace = Workspace::new();
        workspace.set_reference_context_mode(ReferenceContextMode::Sibling);
        workspace.set_reference_root_uri(Some(
            Url::from_directory_path(&reference_root).expect("absolute reference root"),
        ));
        workspace
            .ref_targets
            .insert(("items".to_string(), "id".to_string()));
        workspace.install_reference_dataset(
            "3.2".to_string(),
            "fixture-digest".to_string(),
            HashMap::from([("items".to_string(), doc("BUNDLED"))]),
        );
        workspace.set_reference_root_present_paths([path.clone()]);
        workspace
            .reference_root_cache
            .insert(path.clone(), doc("REFERENCE-DISK"));
        workspace.rebuild_effective_symbols();

        let initial = workspace
            .symbols
            .resolve("items", "id", "REFERENCE-DISK", ReferenceResolver::AsciiCi)
            .expect("explicit reference root should supply the table");
        assert_eq!(initial.source_kind, SourceKind::Workspace);
        assert!(workspace.file_cache.is_empty());
        assert!(workspace.workspace_present_paths.is_empty());

        workspace.accept_open(uri.clone(), 1, doc("OPEN"));
        workspace.rebuild_effective_symbols();
        assert!(workspace.restore_closed_document_with_presence(
            &uri,
            Some(path.clone()),
            Some(doc("LATEST-REFERENCE-DISK")),
            true,
        ));
        assert!(workspace.file_cache.is_empty());
        assert!(workspace.workspace_present_paths.is_empty());
        assert!(workspace.reference_root_present_paths.contains(&path));
        assert_eq!(
            workspace.reference_root_cache[&path].rows[0].cells[0].value,
            "LATEST-REFERENCE-DISK"
        );
        let latest = workspace
            .symbols
            .resolve(
                "items",
                "id",
                "LATEST-REFERENCE-DISK",
                ReferenceResolver::AsciiCi,
            )
            .expect("close should restore the latest explicit-root disk document");
        assert_eq!(latest.source_kind, SourceKind::Workspace);

        workspace.accept_open(uri.clone(), 2, doc("OPEN-AGAIN"));
        workspace.rebuild_effective_symbols();
        assert!(workspace.restore_closed_document_with_presence(
            &uri,
            Some(path.clone()),
            None,
            true,
        ));
        assert!(workspace.reference_root_cache.is_empty());
        assert!(workspace.reference_root_present_paths.contains(&path));
        assert!(workspace.reference_root_present_stems.contains("items"));
        assert!(workspace.file_cache.is_empty());
        assert!(
            workspace
                .symbols
                .resolve("items", "id", "BUNDLED", ReferenceResolver::AsciiCi)
                .is_none(),
            "an unreadable explicit-root table must block bundled fallback without stale data"
        );

        workspace.accept_open(uri.clone(), 3, doc("OPEN-LAST"));
        workspace.rebuild_effective_symbols();
        assert!(workspace.restore_closed_document_with_presence(
            &uri,
            Some(path.clone()),
            None,
            false,
        ));
        assert!(workspace.reference_root_cache.is_empty());
        assert!(!workspace.reference_root_present_paths.contains(&path));
        assert!(!workspace.reference_root_present_stems.contains("items"));
        assert!(workspace.file_cache.is_empty());
        assert!(workspace.workspace_present_paths.is_empty());
        let bundled = workspace
            .symbols
            .resolve("items", "id", "BUNDLED", ReferenceResolver::AsciiCi)
            .expect("deleting the explicit-root table should reveal bundled fallback");
        assert_eq!(bundled.source_kind, SourceKind::Bundled);
    }

    #[test]
    fn cached_disk_document_uses_the_explicit_reference_root_tier() {
        let reference_root = std::env::temp_dir().join("vlsp-reference-root-cached-restore");
        let reference_path = reference_root.join("items.txt");
        let workspace_path = std::env::temp_dir()
            .join("vlsp-workspace-cached-restore")
            .join("items.txt");
        let mut workspace = Workspace::new();
        workspace.set_reference_root_uri(Some(
            Url::from_directory_path(&reference_root).expect("absolute reference root"),
        ));
        workspace
            .reference_root_cache
            .insert(reference_path.clone(), doc("REFERENCE"));
        workspace
            .file_cache
            .insert(workspace_path.clone(), doc("WORKSPACE"));

        assert_eq!(
            workspace
                .cached_disk_document(&reference_path)
                .expect("reference-root cache entry")
                .rows[0]
                .cells[0]
                .value,
            "REFERENCE"
        );
        assert_eq!(
            workspace
                .cached_disk_document(&workspace_path)
                .expect("workspace cache entry")
                .rows[0]
                .cells[0]
                .value,
            "WORKSPACE"
        );
    }

    #[test]
    fn unknown_reference_context_mode_keeps_full_workspace_behavior() {
        assert_eq!(
            ReferenceContextMode::from_initialization_value(Some("sibling")),
            ReferenceContextMode::Sibling
        );
        assert_eq!(
            ReferenceContextMode::from_initialization_value(Some("unexpected")),
            ReferenceContextMode::Workspace
        );
        assert_eq!(
            ReferenceContextMode::from_initialization_value(None),
            ReferenceContextMode::Workspace
        );
    }

    #[test]
    fn fixed4_identity_preserves_truncated_utf8_bytes_without_lossy_collisions() {
        assert_eq!(fixed4_key("가x"), fixed4_key("가xA"));
        assert_ne!(fixed4_key("abcé"), fixed4_key("abc€"));
        assert_eq!(fixed4_key("ring  "), fixed4_key("ring"));
        assert_eq!(fixed4_display("staff"), "staf");
    }

    #[cfg(windows)]
    #[test]
    fn close_matches_windows_extended_path_keys_for_latest_restore_and_delete() {
        let scanned_path = PathBuf::from(r"\\?\E:\mods\items.txt");
        let close_path = PathBuf::from(r"E:\mods\items.txt");
        let uri = Url::from_file_path(&close_path).unwrap();
        assert!(same_local_path(&scanned_path, &close_path));

        let mut workspace = Workspace::new();
        workspace.set_reference_context_mode(ReferenceContextMode::Sibling);
        workspace
            .ref_targets
            .insert(("items".to_string(), "id".to_string()));
        workspace.install_reference_dataset(
            "3.2".to_string(),
            "fixture-digest".to_string(),
            HashMap::from([("items".to_string(), doc("BUNDLED"))]),
        );
        workspace.set_workspace_present_paths([scanned_path.clone()]);
        workspace
            .file_cache
            .insert(scanned_path.clone(), doc("SCANNED"));
        assert_eq!(
            workspace.cached_disk_document(&close_path).unwrap().rows[0].cells[0].value,
            "SCANNED"
        );

        workspace.accept_open(uri.clone(), 1, doc("OPEN"));
        assert!(workspace.restore_closed_document_with_presence(
            &uri,
            Some(close_path.clone()),
            Some(doc("LATEST")),
            true,
        ));
        assert!(!workspace.file_cache.contains_key(&scanned_path));
        assert_eq!(workspace.file_cache.len(), 1);
        assert_eq!(
            workspace.cached_disk_document(&close_path).unwrap().rows[0].cells[0].value,
            "LATEST"
        );

        workspace.accept_open(uri.clone(), 2, doc("OPEN-AGAIN"));
        assert!(workspace.restore_closed_document_with_presence(
            &uri,
            Some(close_path.clone()),
            None,
            false,
        ));
        assert!(workspace.file_cache.is_empty());
        assert!(workspace.workspace_present_paths.is_empty());
        let bundled = workspace
            .symbols
            .resolve("items", "id", "BUNDLED", ReferenceResolver::AsciiCi)
            .expect("deleting the non-verbatim path must remove its verbatim scan key");
        assert_eq!(bundled.source_kind, SourceKind::Bundled);
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
