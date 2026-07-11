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
use crate::schema::{FieldTypeName, find_loader, format_description};
use crate::settings::VectorLspSettings;
use crate::source_selection::normalized_file_stem_from_uri;
use crate::workspace::{DocumentChangeError, ValidationTicket, Workspace, WorkspacePhase};

enum VectorLspReady {}

enum VectorLspFailed {}

const SCAN_CONCURRENCY: usize = 4;
type ParsedWorkspaceDocument = (Url, std::path::PathBuf, String, Arc<DocumentData>);
type WorkspaceLoadResult = Result<ParsedWorkspaceDocument, ScanFailure>;

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
            let schema_diags =
                diagnostics::validate_document(&stem, &doc, ws.schema.as_deref(), &ws.symbols);
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
        let (schema, symbols, shared, documents) = {
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
            )
        };

        let mut schema_tasks = tokio::task::JoinSet::new();
        for (uri, stem, document) in documents {
            let schema = schema.clone();
            let symbols = Arc::clone(&symbols);
            schema_tasks.spawn_blocking(move || {
                let diagnostics =
                    diagnostics::validate_document(&stem, &document, schema.as_deref(), &symbols);
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
        let (root_uri, delimiter, ext, scan_generation, session_generation) = {
            let mut ws = self.workspace.write().await;
            (
                ws.root_uri.clone(),
                self.settings.delimiter_char(),
                self.settings.extension.clone(),
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

        let scan_policy = if self.settings.editor_mode {
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
        let read_parse_duration = read_parse_started.elapsed();

        let count = parsed.len();
        let index_started = Instant::now();
        {
            let mut ws = self.workspace.write().await;
            if !ws.commit_scan_documents(scan_generation, &parsed) {
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
                reason: format!("parser task failed: {error}"),
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
        let root_uri = params
            .workspace_folders
            .as_deref()
            .and_then(|f| f.first())
            .map(|f| f.uri.clone())
            .or(params.root_uri);

        {
            let mut ws = self.workspace.write().await;
            ws.root_uri = root_uri;
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
                    self.fail_workspace(format!("Schema loader selection failed: {e}"))
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
                    self.fail_workspace(format!("Schema load failed: {e:#}"))
                        .await;
                    return;
                }
                Err(e) => {
                    self.fail_workspace(format!("Schema task panicked: {e}"))
                        .await;
                    return;
                }
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
            let restored_document = if reload_error.is_some() {
                path.as_ref()
                    .and_then(|path| ws.file_cache.get(path))
                    .cloned()
            } else {
                disk_document
            };
            ws.restore_closed_document(&uri, path.clone(), restored_document)
                && ws.phase == WorkspacePhase::Ready
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
                .and_then(|ft| ft.file.as_ref().zip(ft.field.as_ref()))
                .map(|(f, c)| (f.to_lowercase(), c.clone()));

            let schema_loc = ref_target.as_ref().and_then(|(ref_file, ref_col)| {
                ws.symbols.lookup(ref_file, ref_col, &cell_value).cloned()
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
        let (cell_col_start, cell_len, _col_name, cell_value, plugin_hover_data) = {
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

        let combined = match plugin_content {
            Some(extra) if !extra.is_empty() => extra,
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
