use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, jsonrpc::Result as LspResult};

use crate::diagnostics;
use crate::document::{DocumentData, utf16_len, utf16_offset_to_byte_index};
use crate::plugin;
use crate::scan::{ScanFailure, ScanPolicy};
use crate::schema::{FieldTypeName, ReferenceResolver, find_loader, format_description};
use crate::settings::VectorLspSettings;
use crate::source_selection::{SourceKind, normalized_file_stem_from_uri};
use crate::workspace::{
    DocumentChangeError, ReferenceContextMode, ValidationTicket, Workspace, WorkspacePhase,
    fixed4_display,
};

enum VectorLspReady {}

enum VectorLspFailed {}

const SCAN_CONCURRENCY: usize = 4;
type ParsedWorkspaceDocument = (Url, std::path::PathBuf, String, Arc<DocumentData>);
type WorkspaceLoadResult = Result<ParsedWorkspaceDocument, ScanFailure>;

fn mark_edge_whitespace(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let first_visible = chars.iter().position(|ch| !ch.is_whitespace());
    let last_visible = chars.iter().rposition(|ch| !ch.is_whitespace());
    chars
        .iter()
        .enumerate()
        .map(|(index, ch)| {
            let at_edge = first_visible.map_or(true, |first| index < first)
                || last_visible.map_or(true, |last| index > last);
            if at_edge && *ch == ' ' {
                '␠'
            } else if at_edge && *ch == '\t' {
                '⇥'
            } else {
                *ch
            }
        })
        .collect()
}

impl tower_lsp::lsp_types::notification::Notification for VectorLspReady {
    type Params = VectorLspReadyParams;
    const METHOD: &'static str = "vectorLsp/ready";
}

impl tower_lsp::lsp_types::notification::Notification for VectorLspFailed {
    type Params = VectorLspFailedParams;
    const METHOD: &'static str = "vectorLsp/failed";
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct VectorLspReadyParams {
    session_generation: u64,
    scan_generation: u64,
    workspace_revision: u64,
    root_uri: Option<Url>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct VectorLspFailedParams {
    session_generation: u64,
    root_uri: Option<Url>,
    reason: String,
}

pub struct Backend {
    pub client: Client,
    // Arc so the same settings can be shared across TCP connections cheaply.
    pub settings: Arc<VectorLspSettings>,
    pub workspace: Arc<RwLock<Workspace>>,
    /// None when no plugins are configured.
    pub plugin_host: Option<plugin::PluginHost>,
    pub publish_gates: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl Backend {
    async fn fail_workspace(&self, reason: impl Into<String>) {
        let reason = reason.into();
        let params = {
            let mut workspace = self.workspace.write().await;
            workspace.mark_failed();
            VectorLspFailedParams {
                session_generation: workspace.session_generation,
                root_uri: workspace.root_uri.clone(),
                reason: reason.clone(),
            }
        };
        self.client.log_message(MessageType::ERROR, reason).await;
        self.client
            .send_notification::<VectorLspFailed>(params)
            .await;
    }

    async fn publish_gate(&self, uri: &Url) -> Arc<Mutex<()>> {
        let key = normalized_file_stem_from_uri(uri)
            .map(|stem| format!("stem:{stem}"))
            .unwrap_or_else(|| format!("uri:{uri}"));
        let mut gates = self.publish_gates.lock().await;
        Arc::clone(gates.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))))
    }

    async fn validate_open_ticket(&self, ticket: &ValidationTicket) -> Option<Vec<Diagnostic>> {
        let (schema_diags, plugin_data) = {
            let ws = self.workspace.read().await;
            if !ws.is_current(ticket) {
                return None;
            }
            let doc = ws.open_documents.get(&ticket.uri)?.clone();
            let stem = Self::file_stem(&ticket.uri);
            let schema_diags = diagnostics::validate_document_for_version(
                &stem,
                &doc,
                ws.schema.as_deref(),
                &ws.symbols,
                ws.reference_version.as_deref(),
            );
            let plugin_data = self
                .plugin_host
                .as_ref()
                .filter(|host| host.validates_file(&stem))
                .map(|_| {
                    let ctx = plugin::build_context(&stem, &doc);
                    let view = ws.plugin_workspace_view();
                    (ctx, view.index, view.snapshot)
                });
            (schema_diags, plugin_data)
        };
        let plugin_diags = match (plugin_data, &self.plugin_host) {
            (Some((ctx, idx, snap)), Some(host)) => host.run(ctx, idx, snap).await,
            _ => vec![],
        };
        let mut diagnostics = schema_diags;
        diagnostics.extend(plugin_diags);
        Some(diagnostics)
    }

    async fn publish_open_if_current(
        &self,
        ticket: &ValidationTicket,
        diagnostics: Vec<Diagnostic>,
    ) -> bool {
        let gate = self.publish_gate(&ticket.uri).await;
        let _guard = gate.lock().await;
        if !self.workspace.read().await.needs_publish(ticket) {
            return false;
        }
        self.client
            .publish_diagnostics(ticket.uri.clone(), diagnostics, Some(ticket.client_version))
            .await;
        self.workspace.write().await.mark_published(ticket)
    }

    async fn validate_and_publish_open(&self, ticket: ValidationTicket) -> bool {
        let Some(diagnostics) = self.validate_open_ticket(&ticket).await else {
            return false;
        };
        self.publish_open_if_current(&ticket, diagnostics).await
    }

    async fn publish_disk_if_current(
        &self,
        scan_generation: u64,
        workspace_revision: u64,
        uri: Url,
        diagnostics: Vec<Diagnostic>,
    ) -> bool {
        let gate = self.publish_gate(&uri).await;
        let _guard = gate.lock().await;
        let current = {
            let ws = self.workspace.read().await;
            ws.scan_generation == scan_generation
                && ws.workspace_revision == workspace_revision
                && matches!(
                    ws.phase,
                    WorkspacePhase::Reconciling | WorkspacePhase::Ready
                )
                && ws.disk_diagnostics_allowed(&uri)
        };
        if current {
            let has_diagnostics = !diagnostics.is_empty();
            self.client
                .publish_diagnostics(uri.clone(), diagnostics, None)
                .await;
            let mut ws = self.workspace.write().await;
            let still_current = ws.scan_generation == scan_generation
                && ws.workspace_revision == workspace_revision
                && matches!(
                    ws.phase,
                    WorkspacePhase::Reconciling | WorkspacePhase::Ready
                )
                && ws.disk_diagnostics_allowed(&uri);
            if still_current {
                ws.record_disk_diagnostics(uri, has_diagnostics);
                true
            } else {
                ws.forget_disk_diagnostics(&uri);
                drop(ws);
                self.client.publish_diagnostics(uri, vec![], None).await;
                false
            }
        } else {
            false
        }
    }

    async fn clear_obsolete_disk_diagnostics_except(&self, replacement_uri: Option<&Url>) {
        let obsolete = self.workspace.read().await.obsolete_disk_diagnostic_uris();
        for uri in obsolete {
            if replacement_uri == Some(&uri) {
                self.workspace.write().await.forget_disk_diagnostics(&uri);
                continue;
            }
            let gate = self.publish_gate(&uri).await;
            let _guard = gate.lock().await;
            if !self
                .workspace
                .read()
                .await
                .obsolete_disk_diagnostic_uris()
                .contains(&uri)
            {
                continue;
            }
            self.client
                .publish_diagnostics(uri.clone(), vec![], None)
                .await;
            self.workspace.write().await.forget_disk_diagnostics(&uri);
        }
    }

    async fn clear_obsolete_disk_diagnostics(&self) {
        self.clear_obsolete_disk_diagnostics_except(None).await;
    }

    async fn validate_disk_documents_for_revision(
        &self,
        scan_generation: u64,
        workspace_revision: u64,
    ) -> bool {
        self.clear_obsolete_disk_diagnostics().await;
        let (schema, symbols, shared, documents, reference_version) = {
            let ws = self.workspace.read().await;
            if ws.scan_generation != scan_generation
                || ws.workspace_revision != workspace_revision
                || !matches!(
                    ws.phase,
                    WorkspacePhase::Reconciling | WorkspacePhase::Ready
                )
            {
                return false;
            }
            let documents = ws.disk_documents_for_validation();
            let shared = self
                .plugin_host
                .as_ref()
                .filter(|host| {
                    documents
                        .iter()
                        .any(|(_, stem, _)| host.validates_file(stem))
                })
                .map(|_| {
                    let view = ws.plugin_workspace_view();
                    (view.index, view.snapshot)
                });
            (
                ws.schema.clone(),
                Arc::new(ws.symbols.clone()),
                shared,
                documents,
                ws.reference_version.clone(),
            )
        };

        let mut schema_tasks = tokio::task::JoinSet::new();
        for (uri, stem, document) in documents {
            let schema = schema.clone();
            let symbols = Arc::clone(&symbols);
            let reference_version = reference_version.clone();
            schema_tasks.spawn_blocking(move || {
                let diagnostics = diagnostics::validate_document_for_version(
                    &stem,
                    &document,
                    schema.as_deref(),
                    &symbols,
                    reference_version.as_deref(),
                );
                (uri, stem, document, diagnostics)
            });
        }
        let mut results = Vec::new();
        while let Some(result) = schema_tasks.join_next().await {
            if let Ok(result) = result {
                results.push(result);
            }
        }
        results.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));

        for (uri, stem, document, mut diagnostics) in results {
            if let (Some((index, snapshot)), Some(host)) = (&shared, &self.plugin_host)
                && host.validates_file(&stem)
            {
                diagnostics.extend(
                    host.run(
                        plugin::build_context(&stem, &document),
                        Arc::clone(index),
                        Arc::clone(snapshot),
                    )
                    .await,
                );
            }
            if !self
                .publish_disk_if_current(scan_generation, workspace_revision, uri, diagnostics)
                .await
            {
                return false;
            }
        }
        let ws = self.workspace.read().await;
        ws.scan_generation == scan_generation
            && ws.workspace_revision == workspace_revision
            && matches!(
                ws.phase,
                WorkspacePhase::Reconciling | WorkspacePhase::Ready
            )
    }

    async fn revalidate_workspace_after_change(&self) {
        loop {
            let (scan_generation, workspace_revision) = {
                let ws = self.workspace.read().await;
                if ws.phase != WorkspacePhase::Ready {
                    return;
                }
                (ws.scan_generation, ws.workspace_revision)
            };
            if !self
                .validate_disk_documents_for_revision(scan_generation, workspace_revision)
                .await
            {
                continue;
            }
            let pending = self.workspace.read().await.pending_open_tickets();
            for ticket in pending {
                self.validate_and_publish_open(ticket).await;
            }
            let ws = self.workspace.read().await;
            if ws.phase == WorkspacePhase::Ready
                && ws.scan_generation == scan_generation
                && ws.workspace_revision == workspace_revision
                && ws.pending_open_tickets().is_empty()
            {
                return;
            }
        }
    }

    async fn reconcile_open_documents_until_ready(&self, scan_generation: u64) {
        loop {
            let workspace_revision = {
                let ws = self.workspace.read().await;
                if ws.scan_generation != scan_generation || ws.phase != WorkspacePhase::Reconciling
                {
                    return;
                }
                ws.workspace_revision
            };
            if !self
                .validate_disk_documents_for_revision(scan_generation, workspace_revision)
                .await
            {
                continue;
            }
            let pending = self.workspace.read().await.pending_open_tickets();
            if pending.is_empty() {
                let ready = {
                    let mut ws = self.workspace.write().await;
                    if ws.workspace_revision != workspace_revision
                        || !ws.mark_ready_if_reconciled(scan_generation)
                    {
                        None
                    } else {
                        Some(VectorLspReadyParams {
                            session_generation: ws.session_generation,
                            scan_generation: ws.scan_generation,
                            workspace_revision: ws.workspace_revision,
                            root_uri: ws.root_uri.clone(),
                        })
                    }
                };
                if let Some(params) = ready {
                    self.client
                        .send_notification::<VectorLspReady>(params)
                        .await;
                }
                return;
            }
            for ticket in pending {
                self.validate_and_publish_open(ticket).await;
            }
        }
    }

    async fn report_rejected_change(&self, uri: &Url, error: DocumentChangeError) {
        let message = match error {
            DocumentChangeError::NotOpen => {
                format!("Ignored didChange for unopened document {uri}")
            }
            DocumentChangeError::StaleVersion { current, incoming } => format!(
                "Ignored stale didChange for {uri}: incoming version {incoming}, current version {current}"
            ),
        };
        self.client.log_message(MessageType::WARNING, message).await;
    }

    /// Extract the lowercase file stem from a URI (e.g. `"armor"` from `.../armor.txt`).
    fn file_stem(uri: &Url) -> String {
        if let Some(stem) = uri.to_file_path().ok().and_then(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_lowercase)
        }) {
            return stem;
        }
        let name = uri
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .unwrap_or("");
        match name.rfind('.') {
            Some(i) => name[..i].to_lowercase(),
            None => name.to_lowercase(),
        }
    }

    /// Read a file from disk using the configured encoding.
    async fn read_file(&self, path: &std::path::Path) -> anyhow::Result<String> {
        let bytes = tokio::fs::read(path).await?;
        self.settings.encoding.decode(&bytes)
    }

    /// Scan all data files in the workspace root, parse and index them.
    /// Called after the schema (and thus ref_targets) is ready.
    async fn scan_and_index_workspace(
        &self,
        initialized_started: Instant,
        schema_duration: Duration,
    ) {
        let scan_started = Instant::now();
        let (
            root_uri,
            reference_root_uri,
            delimiter,
            ext,
            reference_context_mode,
            scan_generation,
            session_generation,
        ) = {
            let mut ws = self.workspace.write().await;
            (
                ws.root_uri.clone(),
                ws.reference_root_uri.clone(),
                self.settings.delimiter_char(),
                self.settings.extension.clone(),
                ws.reference_context_mode,
                ws.begin_scan(),
                ws.session_generation,
            )
        };

        let Some(root_uri) = root_uri else {
            self.fail_workspace("Workspace scan failed: initialize did not provide a root URI")
                .await;
            return;
        };
        let Ok(root_path) = root_uri.to_file_path() else {
            self.fail_workspace(format!(
                "Workspace scan failed: root URI is not a file path: {root_uri}"
            ))
            .await;
            return;
        };

        let scan_policy = if reference_context_mode == ReferenceContextMode::Sibling {
            ScanPolicy::sibling_txt()
        } else if self.settings.editor_mode {
            ScanPolicy::editor()
        } else {
            ScanPolicy::standalone(&ext)
        };
        let enumerate_started = Instant::now();
        let discovery = match crate::scan::collect_data_files(&root_path, &scan_policy) {
            Ok(discovery) => discovery,
            Err(e) => {
                self.fail_workspace(format!("Workspace scan failed: {e}"))
                    .await;
                return;
            }
        };
        let enumerate_duration = enumerate_started.elapsed();
        let mut failures = discovery.failures;

        // Collect directory entries before spawning so we can log errors on the main task.
        let mut entries: Vec<(Url, std::path::PathBuf, String)> = Vec::new();
        for path in discovery.paths {
            let uri = match Url::from_file_path(&path) {
                Ok(uri) => uri,
                Err(_) => {
                    failures.push(ScanFailure {
                        path,
                        reason: "cannot convert path to a file URI".to_string(),
                    });
                    continue;
                }
            };
            let stem = Self::file_stem(&uri);
            entries.push((uri, path, stem));
        }
        let mut reference_entries: Vec<(Url, std::path::PathBuf, String)> = Vec::new();
        if reference_context_mode == ReferenceContextMode::Sibling {
            if let Some(reference_root_uri) = reference_root_uri {
                match reference_root_uri.to_file_path() {
                    Ok(reference_root_path) if reference_root_path != root_path => {
                        match crate::scan::collect_data_files(
                            &reference_root_path,
                            &ScanPolicy::editor(),
                        ) {
                            Ok(reference_discovery) => {
                                failures.extend(reference_discovery.failures);
                                for path in reference_discovery.paths {
                                    match Url::from_file_path(&path) {
                                        Ok(uri) => {
                                            let stem = Self::file_stem(&uri);
                                            reference_entries.push((uri, path, stem));
                                        }
                                        Err(_) => failures.push(ScanFailure {
                                            path,
                                            reason: "cannot convert explicit reference-root path to a file URI".to_string(),
                                        }),
                                    }
                                }
                            }
                            Err(error) => failures.push(ScanFailure {
                                path: reference_root_path,
                                reason: format!("explicit reference-root scan failed: {error}"),
                            }),
                        }
                    }
                    Ok(_) => {}
                    Err(_) => failures.push(ScanFailure {
                        path: root_path.clone(),
                        reason: format!(
                            "explicit reference root is not a file path: {reference_root_uri}"
                        ),
                    }),
                }
            }
        }
        let present_paths = entries
            .iter()
            .map(|(_, path, _)| path.clone())
            .collect::<std::collections::HashSet<_>>();
        let reference_present_paths = reference_entries
            .iter()
            .map(|(_, path, _)| path.clone())
            .collect::<std::collections::HashSet<_>>();
        {
            let mut workspace = self.workspace.write().await;
            workspace.set_workspace_present_paths(present_paths);
            workspace.set_reference_root_present_paths(reference_present_paths);
        }

        // Keep only a small, fixed number of read+parse tasks in flight so a large
        // workspace cannot allocate one task and one source buffer per file.
        let read_parse_started = Instant::now();
        let settings = Arc::clone(&self.settings);
        let mut parsed: Vec<ParsedWorkspaceDocument> = Vec::new();
        let load_results = run_bounded(entries, SCAN_CONCURRENCY, move |entry| {
            load_workspace_document(Arc::clone(&settings), delimiter, entry)
        })
        .await;
        for result in load_results {
            match result {
                Ok(Ok(item)) => parsed.push(item),
                Ok(Err(failure)) => failures.push(failure),
                Err(error) => failures.push(ScanFailure {
                    path: root_path.clone(),
                    reason: format!("workspace scan task failed: {error}"),
                }),
            }
        }
        let reference_settings = Arc::clone(&self.settings);
        let mut reference_parsed: Vec<ParsedWorkspaceDocument> = Vec::new();
        let reference_load_results =
            run_bounded(reference_entries, SCAN_CONCURRENCY, move |entry| {
                load_workspace_document(Arc::clone(&reference_settings), delimiter, entry)
            })
            .await;
        for result in reference_load_results {
            match result {
                Ok(Ok(item)) => reference_parsed.push(item),
                Ok(Err(failure)) => failures.push(failure),
                Err(error) => failures.push(ScanFailure {
                    path: root_path.clone(),
                    reason: format!("explicit reference-root scan task failed: {error}"),
                }),
            }
        }
        let read_parse_duration = read_parse_started.elapsed();

        let count = parsed.len() + reference_parsed.len();
        let index_started = Instant::now();
        {
            let mut ws = self.workspace.write().await;
            if !ws.commit_scan_documents_with_reference_root(
                scan_generation,
                &parsed,
                &reference_parsed,
            ) {
                return;
            }
        }
        let index_duration = index_started.elapsed();

        for failure in &failures {
            self.client
                .log_message(
                    MessageType::WARNING,
                    format!(
                        "Workspace scan skipped '{}': {}",
                        failure.path.display(),
                        failure.reason
                    ),
                )
                .await;
        }
        self.client
            .log_message(
                MessageType::INFO,
                format!(
                    "Indexed {count} workspace files; skipped {} path(s).",
                    failures.len()
                ),
            )
            .await;

        let reconcile_started = Instant::now();
        self.reconcile_open_documents_until_ready(scan_generation)
            .await;
        let reconcile_duration = reconcile_started.elapsed();

        self.client
            .log_message(
                MessageType::LOG,
                format!(
                    "vlsp perf session={session_generation} files={count} skipped={} schemaMs={:.2} enumerateMs={:.2} readParseMs={:.2} indexMs={:.2} reconcileMs={:.2} scanMs={:.2} startupMs={:.2}",
                    failures.len(),
                    milliseconds(schema_duration),
                    milliseconds(enumerate_duration),
                    milliseconds(read_parse_duration),
                    milliseconds(index_duration),
                    milliseconds(reconcile_duration),
                    milliseconds(scan_started.elapsed()),
                    milliseconds(initialized_started.elapsed())
                ),
            )
            .await;
    }
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

async fn load_workspace_document(
    settings: Arc<VectorLspSettings>,
    delimiter: char,
    entry: (Url, std::path::PathBuf, String),
) -> WorkspaceLoadResult {
    let (uri, path, stem) = entry;
    let bytes = tokio::fs::read(&path).await.map_err(|error| ScanFailure {
        path: path.clone(),
        reason: error.to_string(),
    })?;
    let source = settings
        .encoding
        .decode(&bytes)
        .map_err(|error| ScanFailure {
            path: path.clone(),
            reason: error.to_string(),
        })?;
    let document =
        tokio::task::spawn_blocking(move || Arc::new(DocumentData::parse(&source, delimiter)))
            .await
            .map_err(|error| ScanFailure {
                path: path.clone(),
                reason: format!("could not read this TXT file in the background: {error}"),
            })?;
    Ok((uri, path, stem, document))
}

async fn run_bounded<T, R, F, Fut>(
    items: Vec<T>,
    limit: usize,
    operation: F,
) -> Vec<Result<R, tokio::task::JoinError>>
where
    T: Send + 'static,
    R: Send + 'static,
    F: Fn(T) -> Fut,
    Fut: std::future::Future<Output = R> + Send + 'static,
{
    let mut pending = items.into_iter();
    let mut join_set = tokio::task::JoinSet::new();
    for _ in 0..limit.max(1) {
        let Some(item) = pending.next() else { break };
        join_set.spawn(operation(item));
    }
    let mut results = Vec::new();
    while let Some(result) = join_set.join_next().await {
        results.push(result);
        if let Some(item) = pending.next() {
            join_set.spawn(operation(item));
        }
    }
    results
}

fn resolve_plugin_definition(
    expected_identity: (u64, u64),
    current_identity: (u64, u64),
    symbols: &crate::workspace::SymbolIndex,
    target: (&str, &str, &str),
) -> Option<Location> {
    if !workspace_identity_matches(expected_identity, current_identity) {
        return None;
    }
    symbols.lookup(target.0, target.1, target.2).cloned()
}

fn workspace_identity_matches(expected: (u64, u64), current: (u64, u64)) -> bool {
    expected == current
}

/// Rebuild TSV text from a parsed document. Used to seed incremental change application.
fn reconstruct_text(doc: &DocumentData, delimiter: char) -> String {
    let delim_str = delimiter.to_string();
    let header = doc.headers.join(&delim_str);
    let rows: Vec<String> = doc
        .rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|c| c.value.as_str())
                .collect::<Vec<_>>()
                .join(&delim_str)
        })
        .collect();
    std::iter::once(header)
        .chain(rows)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Apply a single LSP incremental content change to a lines buffer.
fn apply_change(lines: &mut Vec<String>, range: tower_lsp::lsp_types::Range, new_text: &str) {
    let sl = range.start.line as usize;
    let sc = range.start.character;
    let el = range.end.line as usize;
    let ec = range.end.character;

    let prefix = lines
        .get(sl)
        .map(|line| &line[..utf16_offset_to_byte_index(line, sc)])
        .unwrap_or_default();
    let suffix = lines
        .get(el)
        .map(|line| &line[utf16_offset_to_byte_index(line, ec)..])
        .unwrap_or_default();

    let new_lines: Vec<&str> = new_text.split('\n').collect();
    let replacement: Vec<String> = match new_lines.as_slice() {
        [] | [""] => vec![format!("{prefix}{suffix}")],
        [only] => vec![format!("{prefix}{}{suffix}", only.trim_end_matches('\r'))],
        [first, rest @ ..] => {
            let mut v = vec![format!("{prefix}{}", first.trim_end_matches('\r'))];
            for mid in &rest[..rest.len() - 1] {
                v.push(mid.trim_end_matches('\r').to_string());
            }
            v.push(format!(
                "{}{suffix}",
                rest.last().unwrap().trim_end_matches('\r')
            ));
            v
        }
    };

    while lines.len() <= el {
        lines.push(String::new());
    }
    lines.splice(sl..=el, replacement);
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> LspResult<InitializeResult> {
        let session_generation = params
            .initialization_options
            .as_ref()
            .and_then(|value| value.get("sessionGeneration"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let reference_context_mode = ReferenceContextMode::from_initialization_value(
            params
                .initialization_options
                .as_ref()
                .and_then(|value| value.get("referenceContextMode"))
                .and_then(serde_json::Value::as_str),
        );
        let reference_root_uri = params
            .initialization_options
            .as_ref()
            .and_then(|value| value.get("referenceRootUri"))
            .and_then(serde_json::Value::as_str)
            .and_then(|value| Url::parse(value).ok())
            .filter(|uri| uri.scheme() == "file");
        let root_uri = params
            .workspace_folders
            .as_deref()
            .and_then(|f| f.first())
            .map(|f| f.uri.clone())
            .or(params.root_uri);

        {
            let mut ws = self.workspace.write().await;
            ws.root_uri = root_uri;
            ws.set_reference_context_mode(reference_context_mode);
            ws.set_reference_root_uri(reference_root_uri);
            ws.begin_initialization(session_generation);
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        ..Default::default()
                    },
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "vlsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        let initialized_started = Instant::now();
        let mut schema_duration = Duration::ZERO;
        if self.settings.editor_mode {
            self.client
                .log_message(
                    MessageType::INFO,
                    format!(
                        "Effective vector-lsp config: {}",
                        self.settings.effective_summary()
                    ),
                )
                .await;
        }
        let has_schema =
            self.settings.schema_path.is_some() || !self.settings.schema_variant.is_empty();
        if has_schema {
            let schema_started = Instant::now();
            let loader = match find_loader(
                &self.settings.schema_loader,
                self.settings.schema_variant.clone(),
                self.settings.plugin_path.clone(),
            ) {
                Ok(l) => l,
                Err(e) => {
                    self.fail_workspace(format!("Could not select the schema: {e}"))
                        .await;
                    return;
                }
            };
            let schema_path = self.settings.schema_path.clone();
            let result =
                tokio::task::spawn_blocking(move || loader.load(schema_path.as_deref())).await;

            match result {
                Ok(Ok(schema)) => {
                    let schema = Arc::new(schema);
                    let ref_targets = schema.reference_targets();
                    {
                        let mut ws = self.workspace.write().await;
                        ws.ref_targets = ref_targets;
                        ws.schema = Some(Arc::clone(&schema));
                    }
                    if let Some(ph) = &self.plugin_host {
                        ph.set_schema(schema).await;
                    }
                    self.client
                        .log_message(MessageType::INFO, "Schema loaded successfully.")
                        .await;
                    schema_duration = schema_started.elapsed();
                }
                Ok(Err(e)) => {
                    self.fail_workspace(format!("Could not load the schema: {e:#}"))
                        .await;
                    return;
                }
                Err(e) => {
                    self.fail_workspace(format!(
                        "Could not load the schema because its background task stopped: {e}"
                    ))
                    .await;
                    return;
                }
            }
        }
        let reference_settings = Arc::clone(&self.settings);
        match tokio::task::spawn_blocking(move || {
            crate::reference_data::load_selected_reference_dataset(&reference_settings)
        })
        .await
        {
            Ok(Ok(Some(dataset))) => {
                let count = dataset.documents.len();
                let version = dataset.game_version.clone();
                let digest = dataset.canonical_sha256.clone();
                self.workspace.write().await.install_reference_dataset(
                    dataset.game_version,
                    dataset.canonical_sha256,
                    dataset.documents,
                );
                self.client
                    .log_message(
                        MessageType::INFO,
                        format!(
                            "Loaded {count} hidden reference tables for game version {version} ({digest})."
                        ),
                    )
                    .await;
            }
            Ok(Ok(None)) => {
                self.workspace.write().await.clear_reference_dataset();
                self.client
                    .log_message(
                        MessageType::INFO,
                        "Bundled reference fallback disabled: no explicit or inferable game version.",
                    )
                    .await;
            }
            Ok(Err(error)) => {
                self.workspace.write().await.clear_reference_dataset();
                self.client
                    .log_message(
                        MessageType::WARNING,
                        format!("Bundled reference fallback disabled for this session: {error:#}"),
                    )
                    .await;
            }
            Err(error) => {
                self.workspace.write().await.clear_reference_dataset();
                self.client
                    .log_message(
                        MessageType::WARNING,
                        format!("Bundled reference data could not be loaded: {error}"),
                    )
                    .await;
            }
        }
        if self.workspace.read().await.phase != WorkspacePhase::Failed {
            self.scan_and_index_workspace(initialized_started, schema_duration)
                .await;
        }
    }

    async fn shutdown(&self) -> LspResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let mutation_gate = self.publish_gate(&uri).await;
        let mutation_guard = mutation_gate.lock().await;
        let doc = Arc::new(DocumentData::parse(
            &params.text_document.text,
            self.settings.delimiter_char(),
        ));
        let (ready, equivalent_open, ticket) = {
            let mut ws = self.workspace.write().await;
            let equivalent_open = ws.effective_source_has_same_content(&uri, &doc);
            let ticket = if equivalent_open {
                ws.accept_equivalent_open(uri, version, doc)
            } else {
                ws.accept_open(uri, version, doc)
            };
            ws.rebuild_effective_symbols();
            (ws.phase == WorkspacePhase::Ready, equivalent_open, ticket)
        };
        drop(mutation_guard);
        if ready {
            if equivalent_open {
                self.clear_obsolete_disk_diagnostics_except(Some(&ticket.uri))
                    .await;
                if !self.validate_and_publish_open(ticket).await {
                    self.revalidate_workspace_after_change().await;
                }
            } else {
                self.revalidate_workspace_after_change().await;
            }
        }
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let mutation_gate = self.publish_gate(&uri).await;
        let mutation_guard = mutation_gate.lock().await;
        let delimiter = self.settings.delimiter_char();

        // Reconstruct current text from the stored document, apply each incremental
        // change in order, then re-parse. Avoids receiving the full document over IPC.
        let update_result: Result<bool, DocumentChangeError> = {
            let mut ws = self.workspace.write().await;

            let existing_text = ws
                .open_documents
                .get(&uri)
                .map(|d| reconstruct_text(d, delimiter))
                .unwrap_or_default();

            let mut lines: Vec<String> = existing_text.lines().map(str::to_owned).collect();
            for change in &params.content_changes {
                match change.range {
                    Some(range) => apply_change(&mut lines, range, &change.text),
                    None => lines = change.text.lines().map(str::to_owned).collect(),
                }
            }

            let full_text = lines.join("\n");
            let doc = Arc::new(DocumentData::parse(&full_text, delimiter));
            match ws.accept_change(&uri, params.text_document.version, Arc::clone(&doc)) {
                Ok(_) => {
                    ws.rebuild_effective_symbols();
                    Ok(ws.phase == WorkspacePhase::Ready)
                }
                Err(error) => Err(error),
            }
        };
        drop(mutation_guard);
        match update_result {
            Ok(true) => self.revalidate_workspace_after_change().await,
            Ok(false) => {}
            Err(error) => self.report_rejected_change(&uri, error).await,
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        let mutation_gate = self.publish_gate(&uri).await;
        let mutation_guard = mutation_gate.lock().await;
        if !self
            .workspace
            .read()
            .await
            .open_documents
            .contains_key(&uri)
        {
            return;
        }
        let path = uri.to_file_path().ok();
        let (disk_document, reload_error) = match path.as_deref() {
            Some(path) => match self.read_file(path).await {
                Ok(text) => (
                    Some(Arc::new(DocumentData::parse(
                        &text,
                        self.settings.delimiter_char(),
                    ))),
                    None,
                ),
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|io_error| {
                            io_error.kind() == std::io::ErrorKind::NotFound
                        }) =>
                {
                    (None, None)
                }
                Err(error) => (None, Some(error.to_string())),
            },
            None => (None, Some(format!("Cannot convert {uri} to a file path"))),
        };
        let ready = {
            let mut ws = self.workspace.write().await;
            let path_present = disk_document.is_some() || reload_error.is_some();
            let restored_document = if reload_error.is_some() {
                path.as_ref().and_then(|path| ws.cached_disk_document(path))
            } else {
                disk_document
            };
            ws.restore_closed_document_with_presence(
                &uri,
                path.clone(),
                restored_document,
                path_present,
            ) && ws.phase == WorkspacePhase::Ready
        };
        self.client
            .publish_diagnostics(uri.clone(), vec![], None)
            .await;
        drop(mutation_guard);
        if let Some(error) = reload_error {
            self.client
                .log_message(
                    MessageType::WARNING,
                    format!("didClose disk restore failed for {uri}: {error}"),
                )
                .await;
        }
        if ready {
            self.revalidate_workspace_after_change().await;
        }
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> LspResult<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let file_stem = Self::file_stem(uri);

        // Phase 1: extract cell info and attempt schema-based lookup.
        // Build plugin context data only when the schema has no answer and a plugin host exists.
        let (schema_loc, plugin_data) = {
            let ws = self.workspace.read().await;

            let doc = ws
                .open_documents
                .get(uri)
                .or_else(|| uri.to_file_path().ok().and_then(|p| ws.file_cache.get(&p)));
            let Some(doc) = doc else {
                return Ok(None);
            };

            let Some((col_index, cell)) = doc.cell_at(pos.line, pos.character) else {
                return Ok(None);
            };
            let col_name = match doc.headers.get(col_index) {
                Some(h) => h.clone(),
                None => return Ok(None),
            };
            let cell_value = cell.value.clone();

            let ref_target = ws
                .schema
                .as_ref()
                .and_then(|s| s.find_field(&file_stem, &col_name))
                .and_then(|f| f.field_type.as_ref())
                .filter(|ft| ft.type_name == FieldTypeName::Reference)
                .and_then(|ft| {
                    ft.file
                        .as_ref()
                        .zip(ft.field.as_ref())
                        .map(|(file, field)| (file.to_lowercase(), field.clone(), ft.resolver))
                });

            let schema_loc = ref_target
                .as_ref()
                .and_then(|(ref_file, ref_col, resolver)| {
                    ws.symbols
                        .lookup_resolved(ref_file, ref_col, &cell_value, *resolver)
                        .cloned()
                });

            let plugin_data = if schema_loc.is_none() {
                self.plugin_host
                    .as_ref()
                    .filter(|host| host.defines_file(&file_stem))
                    .map(|_| {
                        let ctx = plugin::build_hover_context(
                            &file_stem,
                            &col_name,
                            &cell_value,
                            pos.line,
                            doc,
                        );
                        let view = ws.plugin_workspace_view();
                        (
                            ctx,
                            view.index,
                            view.snapshot,
                            (view.session_generation, view.workspace_revision),
                            Arc::new(ws.symbols.clone()),
                        )
                    })
            } else {
                None
            };

            (schema_loc, plugin_data)
        }; // read lock released

        if let Some(loc) = schema_loc {
            return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
        }

        // Phase 2: try plugin-based goto definition.
        let Some((ctx, idx, snap, expected_identity, symbols)) = plugin_data else {
            return Ok(None);
        };
        let Some(plugin_host) = &self.plugin_host else {
            return Ok(None);
        };
        let plugin_target = plugin_host.goto_definition(ctx, idx, snap).await;

        let Some((target_file, target_col, target_value)) = plugin_target else {
            return Ok(None);
        };

        // Phase 3: resolve only against the SymbolIndex captured with the same
        // plugin snapshot. A workspace mutation during the await invalidates it.
        let current_identity = {
            let ws = self.workspace.read().await;
            (ws.session_generation, ws.workspace_revision)
        };
        Ok(resolve_plugin_definition(
            expected_identity,
            current_identity,
            &symbols,
            (&target_file, &target_col, &target_value),
        )
        .map(GotoDefinitionResponse::Scalar))
    }

    async fn hover(&self, params: HoverParams) -> LspResult<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let file_stem = Self::file_stem(uri);

        if pos.line == 0 {
            // Header row hover: return the column's schema description.
            let ws = self.workspace.read().await;
            let Some(doc) = ws.open_documents.get(uri) else {
                return Ok(None);
            };
            let Some(col_index) = doc.header_at(pos.character) else {
                return Ok(None);
            };
            let col_name = doc
                .headers
                .get(col_index)
                .map(|s| s.as_str())
                .unwrap_or("unknown");

            let Some((col_start, col_end)) = doc.header_span(col_index) else {
                return Ok(None);
            };

            let description = ws
                .schema
                .as_ref()
                .and_then(|s| s.find_field(&file_stem, col_name))
                .and_then(|f| f.description.as_deref())
                .map(format_description);

            let text = match description {
                Some(desc) => format!("**{col_name}**\n\n{desc}"),
                None => return Ok(None),
            };

            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: text,
                }),
                range: Some(Range {
                    start: Position {
                        line: 0,
                        character: col_start,
                    },
                    end: Position {
                        line: 0,
                        character: col_end,
                    },
                }),
            }));
        }

        // Data row hover: return the cell value plus any plugin-provided context.
        // Column documentation is intentionally omitted here — it belongs on the header.
        let (
            cell_col_start,
            cell_len,
            _col_name,
            cell_value,
            reference_content,
            type29_content,
            hit_summon_mode_content,
            plugin_hover_data,
        ) = {
            let ws = self.workspace.read().await;
            let Some(doc) = ws.open_documents.get(uri) else {
                return Ok(None);
            };
            let Some((col_index, cell)) = doc.cell_at(pos.line, pos.character) else {
                return Ok(None);
            };

            let col_name = doc
                .headers
                .get(col_index)
                .map(|s| s.as_str())
                .unwrap_or("unknown")
                .to_string();
            let cell_value = cell.value.clone();
            let cell_col_start = cell.col_start;
            let cell_len = utf16_len(&cell.value);
            let current_row = doc.rows.iter().find(|row| row.line == pos.line);
            let reference_cell_is_consumed = current_row.is_none_or(|row| {
                diagnostics::reference_cell_is_consumed(&file_stem, doc, row, &col_name)
            });
            let properties_stat_func = current_row.and_then(|row| {
                diagnostics::properties_stat_dispatch_func(&file_stem, doc, row, &col_name)
            });

            let reference_content = ws
                .schema
                .as_ref()
                .and_then(|schema| schema.find_field(&file_stem, &col_name))
                .and_then(|field| field.field_type.as_ref())
                .filter(|field_type| field_type.type_name == FieldTypeName::Reference)
                .and_then(|field_type| {
                    if !reference_cell_is_consumed {
                        return None;
                    }
                    let reference_file = field_type.file.as_deref()?;
                    let reference_column = field_type.field.as_deref()?;
                    let resolved = ws.symbols.resolve(
                        reference_file,
                        reference_column,
                        &cell_value,
                        field_type.resolver,
                    );
                    if resolved.is_none()
                        && diagnostics::is_monpet_consumestat_reference(&file_stem, &col_name)
                        && ws.symbols.has_file(reference_file)
                        && ws.symbols.has_column(reference_file, reference_column)
                    {
                        return Some(format!(
                            "**Unknown stat name**\n\n`{}` is not a known stat. This Consume bonus is not applied; other Consume slots still work. Use the exact Stat name from `itemstatcost.txt`.",
                            cell_value
                        ));
                    }
                    if resolved.is_none()
                        && diagnostics::is_properties_stat_reference(&file_stem, &col_name)
                        && ws.symbols.has_file(reference_file)
                        && ws.symbols.has_column(reference_file, reference_column)
                    {
                        return Some(if properties_stat_func == Some(17) {
                            format!(
                                "**Unknown stat name**\n\n`{}` is not a known stat. This property has no effect. Use the exact Stat name from `itemstatcost.txt`.",
                                cell_value
                            )
                        } else {
                            format!(
                                "**Unknown stat name**\n\n`{}` is not a known stat. Use the exact Stat name from `itemstatcost.txt`.",
                                cell_value
                            )
                        });
                    }
                    let resolved = resolved?;
                    let lookup_value = match field_type.resolver {
                        ReferenceResolver::AsciiCi => cell_value.clone(),
                        ReferenceResolver::Fixed4 => fixed4_display(&cell_value),
                    };
                    let source = match resolved.source_kind {
                        SourceKind::Open => match ws.reference_version.as_deref() {
                            Some(version) => {
                                format!("Open document (game version {version})")
                            }
                            None => "Open document".to_string(),
                        },
                        SourceKind::Workspace => match ws.reference_version.as_deref() {
                            Some(version) => {
                                format!("TXT file in the current workspace (game version {version})")
                            }
                            None => "TXT file in the current workspace".to_string(),
                        },
                        SourceKind::Sibling => match ws.reference_version.as_deref() {
                            Some(version) => {
                                format!("TXT file in the same folder (game version {version})")
                            }
                            None => "TXT file in the same folder".to_string(),
                        },
                        SourceKind::Bundled => format!(
                            "Built-in reference data (game version {})",
                            resolved
                                .bundled_version
                                .as_deref()
                                .unwrap_or("unknown")
                        ),
                    };
                    if file_stem.eq_ignore_ascii_case("skills")
                        && col_name.eq_ignore_ascii_case("range")
                        && field_type.resolver == ReferenceResolver::Fixed4
                    {
                        Some(format!(
                            "**Range code**\n\n`{}` is valid. The game uses range code `{}`.\n\nSource: {}",
                            mark_edge_whitespace(&cell_value),
                            resolved.stored_value,
                            source
                        ))
                    } else {
                        let shown_cell = if field_type.resolver == ReferenceResolver::Fixed4 {
                            mark_edge_whitespace(&cell_value)
                        } else {
                            cell_value.clone()
                        };
                        Some(format!(
                            "**Reference resolved**\n\n`{}` → `{}` in `{}.{}`\n\nSource: {}",
                            shown_cell,
                            if field_type.resolver == ReferenceResolver::Fixed4 {
                                lookup_value.as_str()
                            } else {
                                resolved.stored_value.as_str()
                            },
                            reference_file,
                            reference_column,
                            source
                        ))
                    }
                });

            let type29_content = if diagnostics::is_confirmed_type29_boolean(&file_stem, &col_name)
            {
                diagnostics::parse_type29_boolean(&cell_value).map(|value| {
                    let version = ws.reference_version.as_deref().map_or_else(
                        || "Game version: not selected".to_string(),
                        |version| format!("Game version: {version}"),
                    );
                    format!(
                        "**Boolean value**\n\n`{}` → **{}** (0 means false; any nonzero number means true)\n\n{}",
                        cell_value,
                        if value { "true" } else { "false" },
                        version
                    )
                })
            } else {
                None
            };

            let hit_summon_mode_content = current_row
                .filter(|row| {
                    diagnostics::is_hit_summon_mode_cell(
                        &file_stem,
                        doc,
                        row,
                        &col_name,
                        ws.reference_version.as_deref(),
                    )
                })
                .map(|_| {
                    let result = diagnostics::hit_summon_mode_result(&cell_value);
                    let current = if result.fallback_applied {
                        format!("Current value: `{}` -> 1 (NU)", mark_edge_whitespace(&cell_value))
                    } else {
                        format!(
                            "Current value: `{}` -> {} ({})",
                            if cell_value.is_empty() {
                                "blank".to_string()
                            } else {
                                mark_edge_whitespace(&cell_value)
                            },
                            result.effective,
                            diagnostics::HIT_SUMMON_MODE_CODES[result.effective as usize]
                        )
                    };
                    format!(
                        "**HitSummon monster mode**\n\nThe second server parameter uses a monster mode number from 0 through 15.\n\n0=DT, 1=NU, 2=WL, 3=GH, 4=A1, 5=A2, 6=BL, 7=SC, 8=S1, 9=S2, 10=S3, 11=S4, 12=DD, 13=KB, 14=xx, 15=RN.\n\nValues outside 0 through 15 use 1=NU.\n\n{current}"
                    )
                });

            let plugin_hover_data = self
                .plugin_host
                .as_ref()
                .filter(|host| host.hovers_file(&file_stem))
                .map(|_| {
                    let ctx = plugin::build_hover_context(
                        &file_stem,
                        &col_name,
                        &cell_value,
                        pos.line,
                        doc,
                    );
                    let view = ws.plugin_workspace_view();
                    (
                        ctx,
                        view.index,
                        view.snapshot,
                        (view.session_generation, view.workspace_revision),
                    )
                });

            (
                cell_col_start,
                cell_len,
                col_name,
                cell_value,
                reference_content,
                type29_content,
                hit_summon_mode_content,
                plugin_hover_data,
            )
        }; // read lock released here

        let plugin_content = match (plugin_hover_data, &self.plugin_host) {
            (Some((ctx, idx, snap, expected_identity)), Some(ph)) => {
                let content = ph.hover(ctx, idx, snap).await;
                let ws = self.workspace.read().await;
                if !workspace_identity_matches(
                    expected_identity,
                    (ws.session_generation, ws.workspace_revision),
                ) {
                    return Ok(None);
                }
                content
            }
            _ => None,
        };

        let combined = match (
            plugin_content,
            reference_content,
            type29_content,
            hit_summon_mode_content,
        ) {
            (Some(plugin), Some(reference), _, _) if !plugin.is_empty() => {
                format!("{plugin}\n\n---\n\n{reference}")
            }
            (Some(plugin), _, Some(type29), _) if !plugin.is_empty() => {
                format!("{plugin}\n\n---\n\n{type29}")
            }
            (Some(plugin), _, _, Some(hit_summon)) if !plugin.is_empty() => {
                format!("{plugin}\n\n---\n\n{hit_summon}")
            }
            (Some(plugin), _, _, _) if !plugin.is_empty() => plugin,
            (_, Some(reference), _, _) => reference,
            (_, _, Some(type29), _) => type29,
            (_, _, _, Some(hit_summon)) => hit_summon,
            _ if cell_value.is_empty() => return Ok(None),
            _ => cell_value.clone(),
        };

        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: combined,
            }),
            range: Some(Range {
                start: Position {
                    line: pos.line,
                    character: cell_col_start,
                },
                end: Position {
                    line: pos.line,
                    character: cell_col_start + cell_len,
                },
            }),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn range(start: u32, end: u32) -> Range {
        Range::new(Position::new(0, start), Position::new(0, end))
    }

    fn apply(text: &str, start: u32, end: u32, replacement: &str) -> String {
        let mut lines = vec![text.to_string()];
        apply_change(&mut lines, range(start, end), replacement);
        lines.join("\n")
    }

    #[test]
    fn incremental_changes_use_utf16_offsets_around_supplementary_characters() {
        assert_eq!(apply("A🙂B", 1, 1, "X"), "AX🙂B");
        assert_eq!(apply("A🙂B", 3, 3, "X"), "A🙂XB");
        assert_eq!(apply("A🙂B", 1, 3, ""), "AB");
        assert_eq!(apply("A🙂B\told", 5, 8, "new"), "A🙂B\tnew");
    }

    #[test]
    fn invalid_half_surrogate_offsets_clamp_to_the_code_point_start() {
        assert_eq!(apply("A🙂B", 2, 2, "X"), "AX🙂B");
    }

    #[tokio::test]
    async fn workspace_scan_runner_never_exceeds_its_concurrency_limit() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let release_gate = Arc::clone(&gate);
        let active_for_tasks = Arc::clone(&active);
        let maximum_for_tasks = Arc::clone(&maximum);

        let run = tokio::spawn(run_bounded((0..8).collect(), SCAN_CONCURRENCY, move |_| {
            let active = Arc::clone(&active_for_tasks);
            let maximum = Arc::clone(&maximum_for_tasks);
            let gate = Arc::clone(&gate);
            async move {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                maximum.fetch_max(current, Ordering::SeqCst);
                gate.acquire_owned().await.unwrap().forget();
                active.fetch_sub(1, Ordering::SeqCst);
            }
        }));

        for _ in 0..100 {
            if maximum.load(Ordering::SeqCst) == SCAN_CONCURRENCY {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(maximum.load(Ordering::SeqCst), SCAN_CONCURRENCY);
        release_gate.add_permits(8);
        let results = run.await.unwrap();

        assert_eq!(results.len(), 8);
        assert!(results.into_iter().all(|result| result.is_ok()));
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert!(maximum.load(Ordering::SeqCst) <= SCAN_CONCURRENCY);
    }

    #[tokio::test]
    async fn initialize_consumes_sibling_reference_context_without_guessing_other_values() {
        let workspace = Arc::new(RwLock::new(Workspace::new()));
        let settings = Arc::new(VectorLspSettings::default());
        let publish_gates = Arc::new(Mutex::new(HashMap::new()));
        let workspace_for_service = Arc::clone(&workspace);
        let (service, _socket) = tower_lsp::LspService::new(move |client| Backend {
            client,
            settings: Arc::clone(&settings),
            workspace: Arc::clone(&workspace_for_service),
            plugin_host: None,
            publish_gates: Arc::clone(&publish_gates),
        });
        let mut params = InitializeParams::default();
        params.root_uri = Some(Url::parse("file:///E:/mod").unwrap());
        params.initialization_options = Some(serde_json::json!({
            "sessionGeneration": 42,
            "referenceContextMode": "sibling",
            "referenceRootUri": "file:///E:/explicit-workspace"
        }));
        service.inner().initialize(params).await.unwrap();

        let ws = workspace.read().await;
        assert_eq!(ws.reference_context_mode, ReferenceContextMode::Sibling);
        assert_eq!(ws.session_generation, 42);
        assert_eq!(
            ws.root_uri.as_ref().map(Url::as_str),
            Some("file:///E:/mod")
        );
        assert_eq!(
            ws.reference_root_uri.as_ref().map(Url::as_str),
            Some("file:///E:/explicit-workspace")
        );
    }

    #[test]
    fn plugin_definition_never_mixes_snapshot_and_later_symbol_revisions() {
        let uri = Url::parse("file:///workspace/items.txt").unwrap();
        let document = DocumentData::parse("id\nKEY", '\t');
        let targets = HashSet::from([("items".to_string(), "id".to_string())]);
        let mut symbols = crate::workspace::SymbolIndex::new();
        symbols.index_document(&uri, "items", &document, &targets);

        assert!(
            resolve_plugin_definition((1, 7), (1, 8), &symbols, ("items", "id", "KEY")).is_none()
        );
        assert!(
            resolve_plugin_definition((1, 7), (2, 7), &symbols, ("items", "id", "KEY")).is_none()
        );
        assert_eq!(
            resolve_plugin_definition((1, 7), (1, 7), &symbols, ("items", "id", "KEY"))
                .unwrap()
                .uri,
            uri
        );
    }

    #[test]
    fn plugin_hover_identity_rejects_revision_or_session_changes() {
        assert!(workspace_identity_matches((4, 9), (4, 9)));
        assert!(!workspace_identity_matches((4, 9), (4, 10)));
        assert!(!workspace_identity_matches((4, 9), (5, 9)));
    }

    #[tokio::test]
    async fn d2rdoc_binary_patches_reach_the_actual_header_hover_path() {
        let contrib = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("contrib")
            .join("d2rdoc");
        let schema_dir = contrib.join("3.2").join("schema");
        let loader = find_loader("d2rdoc", "3.2".to_string(), Some(contrib)).unwrap();
        let schema = Arc::new(loader.load(Some(&schema_dir)).unwrap());

        let documents = [
            (
                Url::parse("file:///workspace/monstats.txt").unwrap(),
                "NextInClass\n",
            ),
            (
                Url::parse("file:///workspace/treasureclassex.txt").unwrap(),
                "Picks\tProb1\n",
            ),
            (
                Url::parse("file:///workspace/cubemain.txt").unwrap(),
                "output\toutput b\toutput c\tmod 1\tb mod 1\tc mod 1\tilvl\tb ilvl\tc ilvl\n",
            ),
            (
                Url::parse("file:///workspace/missiles.txt").unwrap(),
                "Explosion\tNoMultiShot\n2\t-1\n",
            ),
        ];

        let workspace = Arc::new(RwLock::new(Workspace::new()));
        {
            let mut ws = workspace.write().await;
            ws.schema = Some(schema);
            ws.reference_version = Some("3.2".to_string());
            for (uri, text) in &documents {
                ws.open_documents
                    .insert(uri.clone(), Arc::new(DocumentData::parse(text, '\t')));
            }
        }

        let settings = Arc::new(VectorLspSettings::default());
        let publish_gates = Arc::new(Mutex::new(HashMap::new()));
        let workspace_for_service = Arc::clone(&workspace);
        let (service, _socket) = tower_lsp::LspService::new(move |client| Backend {
            client,
            settings: Arc::clone(&settings),
            workspace: Arc::clone(&workspace_for_service),
            plugin_host: None,
            publish_gates: Arc::clone(&publish_gates),
        });

        let cases = [
            (
                "file:///workspace/monstats.txt",
                0,
                "contiguous row order are not required",
            ),
            (
                "file:///workspace/treasureclassex.txt",
                0,
                "does not have to equal the Prob# total",
            ),
            (
                "file:///workspace/treasureclassex.txt",
                7,
                "positive Prob# values select numbered positions",
            ),
            (
                "file:///workspace/cubemain.txt",
                1,
                "use input 1, input 2, or input 3",
            ),
            (
                "file:///workspace/cubemain.txt",
                8,
                "use input 1, input 2, or input 3",
            ),
            (
                "file:///workspace/cubemain.txt",
                17,
                "use input 1, input 2, or input 3",
            ),
            (
                "file:///workspace/cubemain.txt",
                26,
                "Letter case does not matter",
            ),
            (
                "file:///workspace/cubemain.txt",
                31,
                "Letter case does not matter",
            ),
            (
                "file:///workspace/cubemain.txt",
                39,
                "Letter case does not matter",
            ),
            ("file:///workspace/cubemain.txt", 47, "ilvl uses input 1"),
            ("file:///workspace/cubemain.txt", 52, "b ilvl uses input 2"),
            ("file:///workspace/cubemain.txt", 59, "c ilvl uses input 3"),
            (
                "file:///workspace/missiles.txt",
                1,
                "Numeric 0 means false. Any numeric nonzero value means true",
            ),
            (
                "file:///workspace/missiles.txt",
                12,
                "including negative values",
            ),
        ];

        for (uri, character, expected) in cases {
            let hover = service
                .inner()
                .hover(HoverParams {
                    text_document_position_params: TextDocumentPositionParams {
                        text_document: TextDocumentIdentifier {
                            uri: Url::parse(uri).unwrap(),
                        },
                        position: Position::new(0, character),
                    },
                    work_done_progress_params: WorkDoneProgressParams::default(),
                })
                .await
                .unwrap()
                .expect("patched header hover");
            let HoverContents::Markup(markup) = hover.contents else {
                panic!("header hover should be Markdown markup");
            };
            assert!(
                markup.value.contains(expected),
                "header hover at {uri}:{character} did not contain {expected:?}: {}",
                markup.value
            );
        }

        for (character, expected_value, expected_truth) in
            [(0, "`2`", "**true**"), (3, "`-1`", "**true**")]
        {
            let hover = service
                .inner()
                .hover(HoverParams {
                    text_document_position_params: TextDocumentPositionParams {
                        text_document: TextDocumentIdentifier {
                            uri: Url::parse("file:///workspace/missiles.txt").unwrap(),
                        },
                        position: Position::new(1, character),
                    },
                    work_done_progress_params: WorkDoneProgressParams::default(),
                })
                .await
                .unwrap()
                .expect("type-29 value hover");
            let HoverContents::Markup(markup) = hover.contents else {
                panic!("type-29 hover should be Markdown markup");
            };
            assert!(markup.value.contains(expected_value), "{}", markup.value);
            assert!(markup.value.contains(expected_truth), "{}", markup.value);
            assert!(
                markup.value.contains("Game version: 3.2"),
                "{}",
                markup.value
            );
        }
    }

    #[tokio::test]
    async fn hit_summon_mode_hover_is_limited_to_server_parameter_two_in_3_2() {
        let contrib = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("contrib")
            .join("d2rdoc");
        let schema_dir = contrib.join("3.2").join("schema");
        let loader = find_loader("d2rdoc", "3.2".to_string(), Some(contrib)).unwrap();
        let schema = Arc::new(loader.load(Some(&schema_dir)).unwrap());
        let uri = Url::parse("file:///workspace/missiles.txt").unwrap();
        let workspace = Arc::new(RwLock::new(Workspace::new()));
        {
            let mut ws = workspace.write().await;
            ws.schema = Some(schema);
            ws.reference_version = Some("3.2".to_string());
            ws.open_documents.insert(
                uri.clone(),
                Arc::new(DocumentData::parse(
                    "pSrvHitFunc\tsHitPar2\tcHitPar2\n6\tNU\tNU",
                    '\t',
                )),
            );
        }
        let settings = Arc::new(VectorLspSettings::default());
        let publish_gates = Arc::new(Mutex::new(HashMap::new()));
        let workspace_for_service = Arc::clone(&workspace);
        let (service, _socket) = tower_lsp::LspService::new(move |client| Backend {
            client,
            settings: Arc::clone(&settings),
            workspace: Arc::clone(&workspace_for_service),
            plugin_host: None,
            publish_gates: Arc::clone(&publish_gates),
        });

        let hover_at = |character| HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position::new(1, character),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        let hover = service
            .inner()
            .hover(hover_at(2))
            .await
            .unwrap()
            .expect("HitSummon sHitPar2 hover");
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("HitSummon hover should be Markdown markup");
        };
        assert!(
            markup.value.contains("HitSummon monster mode"),
            "{}",
            markup.value
        );
        assert!(markup.value.contains("0=DT, 1=NU"), "{}", markup.value);
        assert!(markup.value.contains("15=RN"), "{}", markup.value);
        assert!(
            markup.value.contains("outside 0 through 15 use 1=NU"),
            "{}",
            markup.value
        );
        assert!(
            markup.value.contains("Current value: `NU` -> 1 (NU)"),
            "{}",
            markup.value
        );

        let client_hover = service
            .inner()
            .hover(hover_at(5))
            .await
            .unwrap()
            .expect("generic cHitPar2 hover");
        let HoverContents::Markup(client_markup) = client_hover.contents else {
            panic!("cHitPar2 hover should be Markdown markup");
        };
        assert!(!client_markup.value.contains("HitSummon monster mode"));
    }

    #[tokio::test]
    async fn monpet_consumestat_miss_hover_uses_plain_slot_skip_explanation() {
        let contrib = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("contrib")
            .join("d2rdoc");
        let schema_dir = contrib.join("3.1").join("schema");
        let loader = find_loader("d2rdoc", "3.1".to_string(), Some(contrib)).unwrap();
        let schema = Arc::new(loader.load(Some(&schema_dir)).unwrap());
        let uri = Url::parse("file:///workspace/monpet.txt").unwrap();

        let workspace = Arc::new(RwLock::new(Workspace::new()));
        {
            let mut ws = workspace.write().await;
            ws.schema = Some(schema);
            ws.reference_version = Some("3.1".to_string());
            ws.open_documents.insert(
                uri.clone(),
                Arc::new(DocumentData::parse(
                    "monster\tconsumestat1\nrow\titem_addsksrc _tab\n",
                    '\t',
                )),
            );
            let targets = HashSet::from([("itemstatcost".to_string(), "stat".to_string())]);
            ws.symbols.index_effective_document(
                None,
                "itemstatcost",
                &DocumentData::parse("Stat\nstrength\n", '\t'),
                &targets,
                SourceKind::Bundled,
                Some("3.1"),
            );
        }

        let settings = Arc::new(VectorLspSettings::default());
        let publish_gates = Arc::new(Mutex::new(HashMap::new()));
        let workspace_for_service = Arc::clone(&workspace);
        let (service, _socket) = tower_lsp::LspService::new(move |client| Backend {
            client,
            settings: Arc::clone(&settings),
            workspace: Arc::clone(&workspace_for_service),
            plugin_host: None,
            publish_gates: Arc::clone(&publish_gates),
        });

        let hover = service
            .inner()
            .hover(HoverParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri },
                    position: Position::new(1, 6),
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .await
            .unwrap()
            .expect("unresolved consumestat hover");
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("consumestat hover should be Markdown");
        };
        assert!(markup.value.contains("Unknown stat name"));
        assert!(markup.value.contains("This Consume bonus is not applied"));
        assert!(markup.value.contains("other Consume slots still work"));
        assert!(markup.value.contains("Use the exact Stat name"));
        assert!(!markup.value.contains("0xFFFF"));
        assert!(!markup.value.contains("loader"));
    }

    #[tokio::test]
    async fn properties_stat_hover_reports_only_reachable_active_slots() {
        let contrib = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("contrib")
            .join("d2rdoc");
        let schema_dir = contrib.join("3.2").join("schema");
        let loader = find_loader("d2rdoc", "3.2".to_string(), Some(contrib)).unwrap();
        let schema = Arc::new(loader.load(Some(&schema_dir)).unwrap());
        let uri = Url::parse("file:///workspace/properties.txt").unwrap();

        let workspace = Arc::new(RwLock::new(Workspace::new()));
        {
            let mut ws = workspace.write().await;
            ws.schema = Some(schema);
            ws.reference_version = Some("3.2".to_string());
            ws.open_documents.insert(
                uri.clone(),
                Arc::new(DocumentData::parse(
                    "code\tfunc1\tstat1\nactive\t17\tunknown\ninactive\t0\tunknown\ngeneric\t1\tunknown\n",
                    '\t',
                )),
            );
            let targets = HashSet::from([("itemstatcost".to_string(), "stat".to_string())]);
            ws.symbols.index_effective_document(
                None,
                "itemstatcost",
                &DocumentData::parse("Stat\nstrength\n", '\t'),
                &targets,
                SourceKind::Bundled,
                Some("3.2"),
            );
        }

        let settings = Arc::new(VectorLspSettings::default());
        let publish_gates = Arc::new(Mutex::new(HashMap::new()));
        let workspace_for_service = Arc::clone(&workspace);
        let (service, _socket) = tower_lsp::LspService::new(move |client| Backend {
            client,
            settings: Arc::clone(&settings),
            workspace: Arc::clone(&workspace_for_service),
            plugin_host: None,
            publish_gates: Arc::clone(&publish_gates),
        });

        let hover_at = |line| HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position::new(line, 12),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        let active = service
            .inner()
            .hover(hover_at(1))
            .await
            .unwrap()
            .expect("active unresolved property stat hover");
        let HoverContents::Markup(active_markup) = active.contents else {
            panic!("property stat hover should be Markdown");
        };
        assert!(active_markup.value.contains("Unknown stat name"));
        assert!(active_markup.value.contains("This property has no effect"));
        assert!(active_markup.value.contains("Use the exact Stat name"));

        let inactive = service
            .inner()
            .hover(hover_at(2))
            .await
            .unwrap()
            .expect("inactive cell still returns its plain cell value");
        let HoverContents::Markup(inactive_markup) = inactive.contents else {
            panic!("inactive cell hover should be Markdown");
        };
        assert_eq!(inactive_markup.value, "unknown");

        let generic = service
            .inner()
            .hover(hover_at(3))
            .await
            .unwrap()
            .expect("reachable non-func17 property stat hover");
        let HoverContents::Markup(generic_markup) = generic.contents else {
            panic!("generic property stat hover should be Markdown");
        };
        assert!(generic_markup.value.contains("Unknown stat name"));
        assert!(generic_markup.value.contains("Use the exact Stat name"));
        assert!(!generic_markup.value.contains("has no effect"));
    }

    #[tokio::test]
    async fn skills_range_hover_marks_trailing_space_and_reports_effective_code() {
        let contrib = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("contrib")
            .join("d2rdoc");
        let schema_dir = contrib.join("3.2").join("schema");
        let loader = find_loader("d2rdoc", "3.2".to_string(), Some(contrib)).unwrap();
        let schema = Arc::new(loader.load(Some(&schema_dir)).unwrap());
        let uri = Url::parse("file:///workspace/skills.txt").unwrap();

        let workspace = Arc::new(RwLock::new(Workspace::new()));
        {
            let mut ws = workspace.write().await;
            ws.schema = Some(schema);
            ws.reference_version = Some("3.2".to_string());
            ws.open_documents.insert(
                uri.clone(),
                Arc::new(DocumentData::parse("skill|range\nrow|rng \n", '|')),
            );
            let targets = HashSet::from([("enums".to_string(), "skill ranges".to_string())]);
            ws.symbols.index_effective_document(
                None,
                "enums",
                &DocumentData::parse("Skill Ranges\nrng\n", '\t'),
                &targets,
                SourceKind::Bundled,
                Some("3.2"),
            );
        }

        let settings = Arc::new(VectorLspSettings::default());
        let publish_gates = Arc::new(Mutex::new(HashMap::new()));
        let workspace_for_service = Arc::clone(&workspace);
        let (service, _socket) = tower_lsp::LspService::new(move |client| Backend {
            client,
            settings: Arc::clone(&settings),
            workspace: Arc::clone(&workspace_for_service),
            plugin_host: None,
            publish_gates: Arc::clone(&publish_gates),
        });

        let hover = service
            .inner()
            .hover(HoverParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri },
                    position: Position::new(1, 4),
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .await
            .unwrap()
            .expect("range hover");
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("range hover should be Markdown");
        };
        assert!(markup.value.contains("`rng␠` is valid"), "{}", markup.value);
        assert!(
            markup.value.contains("game uses range code `rng`"),
            "{}",
            markup.value
        );
        assert!(
            markup
                .value
                .contains("Built-in reference data (game version 3.2)"),
            "{}",
            markup.value
        );
    }

    #[tokio::test]
    async fn fixed4_reference_hover_reports_selected_bundled_version_and_open_shadow() {
        let contrib = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("contrib")
            .join("d2rdoc");
        let schema_dir = contrib.join("3.2").join("schema");
        let loader = find_loader("d2rdoc", "3.2".to_string(), Some(contrib.clone())).unwrap();
        let schema = Arc::new(loader.load(Some(&schema_dir)).unwrap());
        let reference_dataset =
            crate::reference_data::load_reference_dataset(&contrib, "3.2").unwrap();
        assert_eq!(reference_dataset.documents.len(), 91);
        assert!(reference_dataset.documents.contains_key("itemtypes"));
        let source_uri = Url::parse("file:///workspace/magicprefix.txt").unwrap();
        let target_uri = Url::parse("file:///workspace/itemtypes.txt").unwrap();
        let target_path = std::path::PathBuf::from("C:/workspace/itemtypes.txt");

        let workspace = Arc::new(RwLock::new(Workspace::new()));
        {
            let mut ws = workspace.write().await;
            ws.ref_targets = schema.reference_targets();
            ws.schema = Some(schema);
            ws.open_documents.insert(
                source_uri.clone(),
                Arc::new(DocumentData::parse("Name\titype1\nrow\tstaff\n", '\t')),
            );
            ws.install_reference_dataset(
                reference_dataset.game_version,
                reference_dataset.canonical_sha256,
                reference_dataset.documents,
            );
            ws.rebuild_effective_symbols();
        }

        let settings = Arc::new(VectorLspSettings::default());
        let publish_gates = Arc::new(Mutex::new(HashMap::new()));
        let workspace_for_service = Arc::clone(&workspace);
        let (service, _socket) = tower_lsp::LspService::new(move |client| Backend {
            client,
            settings: Arc::clone(&settings),
            workspace: Arc::clone(&workspace_for_service),
            plugin_host: None,
            publish_gates: Arc::clone(&publish_gates),
        });
        let params = || HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: source_uri.clone(),
                },
                position: Position::new(1, 5),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        let definition_params = || GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: source_uri.clone(),
                },
                position: Position::new(1, 5),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };

        let hover = service.inner().hover(params()).await.unwrap().unwrap();
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("fixed4 hover must be Markdown");
        };
        assert!(
            markup.value.contains("`staff` → `staf`"),
            "{}",
            markup.value
        );
        assert!(
            markup
                .value
                .contains("Built-in reference data (game version 3.2)"),
            "{}",
            markup.value
        );

        {
            let mut ws = workspace.write().await;
            ws.set_reference_context_mode(ReferenceContextMode::Sibling);
            ws.set_workspace_present_paths([target_path.clone()]);
            ws.file_cache.insert(
                target_path.clone(),
                Arc::new(DocumentData::parse("Code\nstaf\n", '\t')),
            );
            ws.rebuild_effective_symbols();
        }
        let sibling = service.inner().hover(params()).await.unwrap().unwrap();
        let HoverContents::Markup(sibling_markup) = sibling.contents else {
            panic!("sibling hover must be Markdown");
        };
        assert!(
            sibling_markup
                .value
                .contains("TXT file in the same folder (game version 3.2)"),
            "{}",
            sibling_markup.value
        );
        assert!(
            service
                .inner()
                .goto_definition(definition_params())
                .await
                .unwrap()
                .is_none(),
            "hidden sibling references must not open an editor tab via go-to-definition"
        );

        {
            let mut ws = workspace.write().await;
            ws.set_reference_context_mode(ReferenceContextMode::Workspace);
            ws.rebuild_effective_symbols();
        }
        assert!(
            service
                .inner()
                .goto_definition(definition_params())
                .await
                .unwrap()
                .is_some(),
            "ordinary workspace references must retain go-to-definition"
        );
        {
            let mut ws = workspace.write().await;
            ws.set_reference_context_mode(ReferenceContextMode::Sibling);
            ws.rebuild_effective_symbols();
        }

        {
            let mut ws = workspace.write().await;
            ws.open_documents.insert(
                target_uri.clone(),
                Arc::new(DocumentData::parse("Code\nxxxx\n", '\t')),
            );
            ws.rebuild_effective_symbols();
        }
        let shadowed = service.inner().hover(params()).await.unwrap().unwrap();
        let HoverContents::Markup(shadowed_markup) = shadowed.contents else {
            panic!("shadowed hover must be Markdown");
        };
        assert!(!shadowed_markup.value.contains("Reference resolved"));

        {
            let mut ws = workspace.write().await;
            ws.open_documents.remove(&target_uri);
            ws.rebuild_effective_symbols();
        }
        let restored = service.inner().hover(params()).await.unwrap().unwrap();
        let HoverContents::Markup(restored_markup) = restored.contents else {
            panic!("restored hover must be Markdown");
        };
        assert!(
            restored_markup
                .value
                .contains("TXT file in the same folder")
        );

        {
            let mut ws = workspace.write().await;
            ws.file_cache.remove(&target_path);
            ws.set_workspace_present_paths(std::iter::empty::<std::path::PathBuf>());
            ws.rebuild_effective_symbols();
        }
        let deleted = service.inner().hover(params()).await.unwrap().unwrap();
        let HoverContents::Markup(deleted_markup) = deleted.contents else {
            panic!("bundled restore hover must be Markdown");
        };
        assert!(
            deleted_markup
                .value
                .contains("Built-in reference data (game version 3.2)")
        );
    }

    #[tokio::test]
    async fn workspace_load_failure_preserves_the_skipped_path_and_decode_reason() {
        let path =
            std::env::temp_dir().join(format!("vector-lsp-odd-utf16-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, [0x41]).unwrap();
        let uri = Url::from_file_path(&path).unwrap();
        let mut settings = VectorLspSettings::default();
        settings.encoding = crate::settings::Encoding::Utf16Le;

        let result = load_workspace_document(
            Arc::new(settings),
            '\t',
            (uri, path.clone(), "odd-utf16".to_string()),
        )
        .await;
        let Err(failure) = result else {
            panic!("odd UTF-16 input must be reported as a skipped path");
        };

        assert_eq!(failure.path, path);
        assert!(failure.reason.contains("odd byte count"));
        let _ = std::fs::remove_file(failure.path);
    }
}
