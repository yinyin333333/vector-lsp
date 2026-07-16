use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, jsonrpc::Result as LspResult};

use crate::diagnostics;
use crate::document::{DocumentData, utf16_len, utf16_offset_to_byte_index};
use crate::json_diagnostics::{
    JsonAnalysisTrigger, JsonDiagnosticBatch, JsonDiagnosticReport, JsonEvidenceProfile,
    PrimaryTxtDocument, analyze_with_rules_profile_and_open_json, data_root_from_excel_txt,
    data_root_from_localization_json, local_path_identity as json_path_identity,
};
use crate::plugin;
use crate::scan::{ScanFailure, ScanPolicy};
use crate::schema::{FieldTypeName, ReferenceResolver, find_loader, format_description};
use crate::settings::VectorLspSettings;
use crate::source_selection::{SourceKind, normalized_file_stem_from_uri};
use crate::workspace::{
    DocumentChangeError, ReferenceContextMode, ValidationTicket, Workspace, WorkspacePhase,
    fixed4_display, local_path_is_within, same_local_path,
};

enum VectorLspReady {}

enum VectorLspFailed {}

const SCAN_CONCURRENCY: usize = 4;
const WATCHED_FILES_QUIET_WINDOW: Duration = Duration::from_millis(250);
const INITIAL_WATCH_REGISTRATION_GRACE: Duration = Duration::from_millis(50);
type ParsedWorkspaceDocument = (Url, std::path::PathBuf, String, Arc<DocumentData>);
type WorkspaceLoadResult = Result<ParsedWorkspaceDocument, ScanFailure>;
type PendingJsonWatchRegistration = (String, String, u64, u64, WatchRegistrationPlan);

#[derive(Clone, Debug, PartialEq, Eq)]
struct JsonValidationTicket {
    session_generation: u64,
    scan_generation: u64,
    workspace_revision: u64,
    json_input_generation: u64,
    scope_identities: Vec<String>,
    evidence_profile: JsonEvidenceProfile,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WatchRegistrationPlan {
    label: String,
    base_uri: Url,
    patterns: Vec<String>,
    kind: Option<WatchKind>,
}

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

#[derive(Clone)]
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
    fn json_diagnostics_enabled(&self) -> bool {
        self.settings.json_diagnostics && self.settings.json_diagnostic_rules().any_enabled()
    }

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

    async fn fail_workspace_if_current_scan(
        &self,
        session_generation: u64,
        scan_generation: u64,
        reason: impl Into<String>,
    ) -> bool {
        let reason = reason.into();
        let params = {
            let mut workspace = self.workspace.write().await;
            if workspace.session_generation != session_generation
                || workspace.scan_generation != scan_generation
            {
                return false;
            }
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
        true
    }

    async fn publish_gate(&self, uri: &Url) -> Arc<Mutex<()>> {
        let key = normalized_file_stem_from_uri(uri)
            .map(|stem| format!("stem:{stem}"))
            .unwrap_or_else(|| format!("uri:{uri}"));
        let mut gates = self.publish_gates.lock().await;
        Arc::clone(gates.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))))
    }

    async fn json_publish_gate(&self, uri: &Url) -> Arc<Mutex<()>> {
        let key = format!("json-uri:{uri}");
        let mut gates = self.publish_gates.lock().await;
        Arc::clone(gates.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))))
    }

    fn json_scope_identities(documents: &[PrimaryTxtDocument]) -> Vec<String> {
        let mut identities = documents
            .iter()
            .filter_map(|input| data_root_from_excel_txt(&input.path))
            .map(|root| json_path_identity(&root))
            .collect::<Vec<_>>();
        identities.sort();
        identities.dedup();
        identities
    }

    fn json_root_identities(roots: &[std::path::PathBuf]) -> Vec<String> {
        let mut identities = roots
            .iter()
            .map(|root| json_path_identity(root))
            .collect::<Vec<_>>();
        identities.sort();
        identities.dedup();
        identities
    }

    fn is_json_uri(uri: &Url) -> bool {
        uri.to_file_path().ok().is_some_and(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("json"))
        })
    }

    fn localization_json_uri_in_scope(workspace: &Workspace, uri: &Url) -> bool {
        let Ok(path) = uri.to_file_path() else {
            return false;
        };
        if !path.is_file() && workspace.open_json_version(uri).is_none() {
            return false;
        }
        let Some(data_root) = data_root_from_localization_json(&path) else {
            return false;
        };
        workspace
            .primary_json_data_roots()
            .iter()
            .any(|root| same_local_path(root, &data_root))
    }

    fn successful_current_json_registration_needs_catch_up(
        finished: bool,
        succeeded: bool,
        still_desired: bool,
    ) -> bool {
        finished && succeeded && still_desired
    }

    fn primary_discovery_contains_path(
        primary_identities: &std::collections::HashSet<String>,
        path: &std::path::Path,
    ) -> bool {
        primary_identities.contains(&json_path_identity(path))
    }

    fn merge_key_usage_batches(
        previous: &HashMap<Url, Vec<Diagnostic>>,
        batches: Vec<JsonDiagnosticBatch>,
    ) -> Vec<JsonDiagnosticBatch> {
        batches
            .into_iter()
            .map(|batch| {
                let mut diagnostics = previous.get(&batch.uri).cloned().unwrap_or_default();
                diagnostics.retain(|diagnostic| {
                    !matches!(
                        diagnostic.code.as_ref(),
                        Some(NumberOrString::String(code)) if code == "Json/KeyUsage"
                    )
                });
                diagnostics.extend(batch.diagnostics.into_iter().filter(|diagnostic| {
                    matches!(
                        diagnostic.code.as_ref(),
                        Some(NumberOrString::String(code)) if code == "Json/KeyUsage"
                    )
                }));
                JsonDiagnosticBatch {
                    uri: batch.uri,
                    diagnostics,
                }
            })
            .collect()
    }

    fn json_evidence_profile(
        settings: &VectorLspSettings,
        workspace: &Workspace,
    ) -> JsonEvidenceProfile {
        let selected = workspace.reference_version.clone().or_else(|| {
            crate::reference_data::selected_reference_variant(settings)
                .ok()
                .flatten()
        });
        match selected.as_deref() {
            Some(version)
                if version.eq_ignore_ascii_case("1.13")
                    || version.eq_ignore_ascii_case("1.13c")
                    || version.eq_ignore_ascii_case("legacy") =>
            {
                JsonEvidenceProfile::Legacy
            }
            _ => JsonEvidenceProfile::ResurrectedOrUnknown,
        }
    }

    fn json_ticket_is_current(
        settings: &VectorLspSettings,
        workspace: &Workspace,
        ticket: &JsonValidationTicket,
    ) -> bool {
        if workspace.session_generation != ticket.session_generation
            || workspace.scan_generation != ticket.scan_generation
            || workspace.workspace_revision != ticket.workspace_revision
            || workspace.json_input_generation() != ticket.json_input_generation
            || Self::json_evidence_profile(settings, workspace) != ticket.evidence_profile
            || !matches!(
                workspace.phase,
                WorkspacePhase::Reconciling | WorkspacePhase::Ready
            )
        {
            return false;
        }
        Self::json_root_identities(&workspace.primary_json_data_roots()) == ticket.scope_identities
    }

    fn json_publication_matches(
        previous: &[Diagnostic],
        previous_version: Option<Option<i32>>,
        next: &[Diagnostic],
        next_version: Option<i32>,
    ) -> bool {
        previous == next && previous_version == Some(next_version)
    }

    async fn publish_json_batch_if_current(
        &self,
        ticket: &JsonValidationTicket,
        batch: JsonDiagnosticBatch,
    ) -> bool {
        let gate = self.json_publish_gate(&batch.uri).await;
        let _guard = gate.lock().await;
        let (current, previous, previous_version, version) = {
            let workspace = self.workspace.read().await;
            (
                Self::json_ticket_is_current(&self.settings, &workspace, ticket),
                workspace.json_diagnostics_for_uri(&batch.uri),
                workspace.json_diagnostics_version_for_uri(&batch.uri),
                workspace.open_json_version(&batch.uri),
            )
        };
        if !current {
            return false;
        }
        if Self::json_publication_matches(&previous, previous_version, &batch.diagnostics, version)
        {
            return true;
        }
        let had_diagnostics = !previous.is_empty();
        let has_diagnostics = !batch.diagnostics.is_empty();
        if has_diagnostics || had_diagnostics {
            self.client
                .publish_diagnostics(batch.uri.clone(), batch.diagnostics.clone(), version)
                .await;
        }
        let mut workspace = self.workspace.write().await;
        if !Self::json_ticket_is_current(&self.settings, &workspace, ticket) {
            // The speculative publication above is no longer authoritative.
            // Keep the server-side snapshot aligned with the clear sent to the
            // client so a retry whose result equals the old snapshot cannot be
            // incorrectly suppressed by the unchanged-result fast path.
            let clear_version = workspace.open_json_version(&batch.uri);
            workspace.record_json_diagnostics_for_version(
                batch.uri.clone(),
                Vec::new(),
                clear_version,
            );
            drop(workspace);
            if has_diagnostics || had_diagnostics {
                self.client
                    .publish_diagnostics(batch.uri, vec![], clear_version)
                    .await;
            }
            return false;
        }
        workspace.record_json_diagnostics_for_version(batch.uri, batch.diagnostics, version);
        true
    }

    async fn clear_obsolete_json_if_current(
        &self,
        ticket: &JsonValidationTicket,
        observed: &std::collections::HashSet<Url>,
    ) -> bool {
        let obsolete = self
            .workspace
            .read()
            .await
            .published_json_diagnostic_uris()
            .into_iter()
            .filter(|uri| !observed.contains(uri))
            .collect::<Vec<_>>();
        for uri in obsolete {
            let batch = JsonDiagnosticBatch {
                uri,
                diagnostics: Vec::new(),
            };
            if !self.publish_json_batch_if_current(ticket, batch).await {
                return false;
            }
        }
        true
    }

    async fn validate_json_diagnostics_for_revision(
        &self,
        scan_generation: u64,
        workspace_revision: u64,
        trigger: JsonAnalysisTrigger,
    ) -> bool {
        // JSON OFF is intentionally a true fast path: do not enumerate TXT
        // inputs, sort roots, or touch the filesystem.
        let rules = self.settings.json_diagnostic_rules();
        if !self.json_diagnostics_enabled()
            || trigger == JsonAnalysisTrigger::KeyUsageOnly && !rules.key_usage.is_enabled()
        {
            return true;
        }
        let (ticket, documents, open_json_sources) = {
            let workspace = self.workspace.read().await;
            if workspace.scan_generation != scan_generation
                || workspace.workspace_revision != workspace_revision
                || !matches!(
                    workspace.phase,
                    WorkspacePhase::Reconciling | WorkspacePhase::Ready
                )
            {
                return false;
            }
            let documents = workspace.primary_txt_documents_for_json();
            (
                JsonValidationTicket {
                    session_generation: workspace.session_generation,
                    scan_generation,
                    workspace_revision,
                    json_input_generation: workspace.json_input_generation(),
                    scope_identities: Self::json_scope_identities(&documents),
                    evidence_profile: Self::json_evidence_profile(&self.settings, &workspace),
                },
                documents,
                workspace.open_json_sources(),
            )
        };

        let evidence_profile = ticket.evidence_profile;
        let JsonDiagnosticReport { batches, warnings } =
            match tokio::task::spawn_blocking(move || {
                analyze_with_rules_profile_and_open_json(
                    documents,
                    rules,
                    trigger,
                    evidence_profile,
                    open_json_sources,
                )
            })
            .await
            {
                Ok(report) => report,
                Err(error) => {
                    self.client
                        .log_message(
                            MessageType::WARNING,
                            format!("Localization JSON diagnostics stopped: {error}"),
                        )
                        .await;
                    let workspace = self.workspace.read().await;
                    return Self::json_ticket_is_current(&self.settings, &workspace, &ticket);
                }
            };

        // A KeyUsage-only pass must not overwrite diagnostics produced by the
        // two JSON-only rules. Merge it into the last successfully published
        // snapshot before comparing and publishing. Preserve those diagnostics
        // only for JSON files observed by this pass; carrying the entire prior
        // snapshot forward would keep deleted/out-of-scope files alive forever.
        let batches = if trigger == JsonAnalysisTrigger::KeyUsageOnly {
            let previous = self.workspace.read().await.published_json_diagnostics();
            Self::merge_key_usage_batches(&previous, batches)
        } else {
            batches
        };

        let ticket_is_current = {
            let workspace = self.workspace.read().await;
            Self::json_ticket_is_current(&self.settings, &workspace, &ticket)
        };
        if !ticket_is_current {
            return false;
        }
        for warning in warnings {
            self.client.log_message(MessageType::WARNING, warning).await;
        }

        let observed = batches
            .iter()
            .map(|batch| batch.uri.clone())
            .collect::<std::collections::HashSet<_>>();
        for batch in batches {
            if !self.publish_json_batch_if_current(&ticket, batch).await {
                return false;
            }
        }
        self.clear_obsolete_json_if_current(&ticket, &observed)
            .await
    }

    fn extension_glob(extension: &str) -> String {
        extension
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphabetic() {
                    format!("[{}{}]", ch.to_ascii_lowercase(), ch.to_ascii_uppercase())
                } else {
                    ch.to_string()
                }
            })
            .collect()
    }

    fn txt_watch_patterns(recursive: bool, extensions: &[&str]) -> Vec<String> {
        let prefix = if recursive { "**/*" } else { "*" };
        extensions
            .iter()
            .map(|extension| format!("{prefix}.{}", Self::extension_glob(extension)))
            .collect()
    }

    fn json_file_watch_pattern() -> String {
        format!("*.{}", Self::extension_glob("json"))
    }

    fn json_target_watch_plans(
        data_root: &std::path::Path,
        target: &std::path::Path,
        label: &str,
    ) -> Vec<(String, WatchRegistrationPlan)> {
        let mut plans = Vec::new();
        let mut base = target.to_path_buf();
        while !base.is_dir() {
            let Some(parent) = base.parent() else {
                return plans;
            };
            base = parent.to_path_buf();
        }
        if !local_path_is_within(&base, data_root) {
            return plans;
        }

        if target.is_dir() {
            let base_uri = match Url::from_directory_path(target) {
                Ok(uri) => uri,
                Err(_) => return plans,
            };
            let pattern = Self::json_file_watch_pattern();
            let identity = format!(
                "{}|{label}|files|{}|{pattern}",
                json_path_identity(data_root),
                json_path_identity(target)
            );
            plans.push((
                identity,
                WatchRegistrationPlan {
                    label: format!("{label}-files"),
                    base_uri,
                    patterns: vec![pattern],
                    kind: None,
                },
            ));
        }

        // Keep one non-recursive guard at the deepest existing ancestor. If a
        // missing component is created or renamed into place, the server can
        // advance the registration one directory at a time without watching
        // the entire data tree recursively. The final parent guard also makes
        // deletion/recreation of an existing target recoverable.
        let guarded_path = if target.is_dir() {
            target.to_path_buf()
        } else {
            let Ok(relative) = target.strip_prefix(&base) else {
                return plans;
            };
            let Some(component) = relative.components().next() else {
                return plans;
            };
            base.join(component.as_os_str())
        };
        let Some(guard_base) = guarded_path.parent() else {
            return plans;
        };
        let Some(pattern) = guarded_path
            .file_name()
            .and_then(|component| component.to_str())
            .map(ToString::to_string)
        else {
            return plans;
        };
        let Ok(base_uri) = Url::from_directory_path(guard_base) else {
            return plans;
        };
        let identity = format!(
            "{}|{label}|guard|{}|{pattern}",
            json_path_identity(data_root),
            json_path_identity(guard_base)
        );
        plans.push((
            identity,
            WatchRegistrationPlan {
                label: format!("{label}-guard"),
                base_uri,
                patterns: vec![pattern],
                kind: Some(WatchKind::Create | WatchKind::Delete),
            },
        ));
        plans
    }

    fn add_watch_plan(
        plans: &mut HashMap<String, WatchRegistrationPlan>,
        label: &str,
        base_uri: Url,
        patterns: impl IntoIterator<Item = String>,
    ) {
        let key = base_uri.as_str().trim_end_matches('/').to_ascii_lowercase();
        let plan = plans.entry(key).or_insert_with(|| WatchRegistrationPlan {
            label: label.to_string(),
            base_uri,
            patterns: Vec::new(),
            kind: None,
        });
        for pattern in patterns {
            if !plan.patterns.contains(&pattern) {
                plan.patterns.push(pattern);
            }
        }
        plan.patterns.sort();
    }

    fn watch_registration_plans(&self, workspace: &Workspace) -> Vec<WatchRegistrationPlan> {
        if !workspace.supports_dynamic_watched_files() {
            return Vec::new();
        }
        let Some(root_uri) = workspace
            .root_uri
            .clone()
            .filter(|uri| uri.scheme() == "file")
        else {
            return Vec::new();
        };
        let mut plans = HashMap::new();
        let (primary_recursive, primary_extensions): (bool, Vec<&str>) =
            if workspace.reference_context_mode == ReferenceContextMode::Sibling {
                (false, vec!["txt"])
            } else if self.settings.editor_mode {
                (
                    workspace.include_subfolders,
                    crate::scan::EDITOR_EXTENSIONS.to_vec(),
                )
            } else {
                (true, vec![self.settings.extension.as_str()])
            };
        Self::add_watch_plan(
            &mut plans,
            "primary",
            root_uri.clone(),
            Self::txt_watch_patterns(primary_recursive, &primary_extensions),
        );

        if workspace.reference_context_mode == ReferenceContextMode::Sibling
            && let Some(reference_uri) = workspace
                .reference_root_uri
                .clone()
                .filter(|uri| uri.scheme() == "file")
            && reference_uri != root_uri
        {
            Self::add_watch_plan(
                &mut plans,
                "reference",
                reference_uri,
                Self::txt_watch_patterns(
                    workspace.include_subfolders,
                    crate::scan::EDITOR_EXTENSIONS,
                ),
            );
        }

        let mut plans = plans.into_values().collect::<Vec<_>>();
        plans.sort_by(|left, right| left.base_uri.as_str().cmp(right.base_uri.as_str()));
        plans
    }

    fn successful_txt_registration_needs_catch_up(phase: WorkspacePhase) -> bool {
        matches!(
            phase,
            WorkspacePhase::Scanning | WorkspacePhase::Reconciling | WorkspacePhase::Ready
        )
    }

    async fn register_watched_files(&self) -> Option<tokio::sync::oneshot::Receiver<()>> {
        let (session_generation, plans) = {
            let workspace = self.workspace.read().await;
            (
                workspace.session_generation,
                self.watch_registration_plans(&workspace),
            )
        };
        if plans.is_empty() {
            return None;
        }
        let mut registration_tasks = Vec::with_capacity(plans.len());
        for (index, plan) in plans.into_iter().enumerate() {
            let id = format!(
                "vector-lsp-watched-files-{session_generation}-{}-{index}",
                plan.label
            );
            let worker = self.clone();
            registration_tasks.push(tokio::spawn(async move {
                if !worker.register_watch_plan(id.clone(), plan).await {
                    return;
                }
                let (stale_registration, start_catch_up_worker) = {
                    let mut workspace = worker.workspace.write().await;
                    if workspace.session_generation != session_generation {
                        (true, false)
                    } else if matches!(workspace.phase, WorkspacePhase::Ready) {
                        let (_, start_worker) = workspace.queue_watched_changes(true);
                        (false, start_worker)
                    } else if Self::successful_txt_registration_needs_catch_up(workspace.phase) {
                        // Do not invalidate an in-progress initial scan. Merge
                        // late registration gaps into one catch-up after Ready.
                        workspace.defer_txt_watch_registration_catch_up();
                        (false, false)
                    } else {
                        (false, false)
                    }
                };
                if stale_registration {
                    worker.unregister_json_watch_ids(vec![id]);
                } else if start_catch_up_worker {
                    worker.spawn_watched_change_worker(session_generation);
                }
            }));
        }
        let (completed_tx, completed_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            for task in registration_tasks {
                let _ = task.await;
            }
            let _ = completed_tx.send(());
        });
        Some(completed_rx)
    }

    fn spawn_watched_change_worker(&self, session_generation: u64) {
        let worker = self.clone();
        tokio::spawn(async move {
            worker.process_watched_changes(session_generation).await;
        });
    }

    async fn register_watch_plan(&self, id: String, plan: WatchRegistrationPlan) -> bool {
        let watchers = plan
            .patterns
            .iter()
            .map(|pattern| FileSystemWatcher {
                glob_pattern: GlobPattern::Relative(RelativePattern {
                    base_uri: OneOf::Right(plan.base_uri.clone()),
                    pattern: pattern.clone(),
                }),
                kind: plan.kind,
            })
            .collect::<Vec<_>>();
        let register_options =
            match serde_json::to_value(DidChangeWatchedFilesRegistrationOptions { watchers }) {
                Ok(value) => value,
                Err(error) => {
                    self.client
                        .log_message(
                            MessageType::WARNING,
                            format!("Could not prepare watched-files registration: {error}"),
                        )
                        .await;
                    return false;
                }
            };
        let registration = Registration {
            id,
            method: "workspace/didChangeWatchedFiles".to_string(),
            register_options: Some(register_options),
        };
        if let Err(error) = self.client.register_capability(vec![registration]).await {
            self.client
                .log_message(
                    MessageType::WARNING,
                    format!(
                        "File watching is unavailable for '{}': {error}",
                        plan.base_uri
                    ),
                )
                .await;
            return false;
        }
        true
    }

    async fn register_json_watch_roots(&self, roots: Vec<std::path::PathBuf>) {
        let (unregistrations, registrations) = {
            let mut workspace = self.workspace.write().await;
            self.reserve_json_watch_registrations(&mut workspace, roots)
        };
        self.unregister_json_watch_ids(unregistrations);
        for (id, identity, registration_session, sequence, plan) in registrations {
            let worker = self.clone();
            tokio::spawn(async move {
                let succeeded = worker.register_watch_plan(id.clone(), plan).await;
                let (catch_up, stale_ids) = {
                    let mut workspace = worker.workspace.write().await;
                    let finished = workspace.finish_watched_json_registration(
                        &identity,
                        registration_session,
                        sequence,
                        &id,
                        succeeded,
                    );
                    let desired = Self::json_watch_candidates(workspace.primary_json_data_roots())
                        .into_iter()
                        .map(|(identity, _)| identity)
                        .collect::<std::collections::HashSet<_>>();
                    let still_desired = desired.contains(&identity);
                    let stale_ids = workspace.take_obsolete_watched_json_registrations(&desired);
                    let mut stale_ids = stale_ids;
                    if succeeded && !finished {
                        // The client accepted this registration after its
                        // session/sequence slot became stale. It is not present
                        // in Workspace state, so retire it explicitly instead
                        // of leaking a live watcher in the client.
                        stale_ids.push(id.clone());
                    }
                    (
                        Self::successful_current_json_registration_needs_catch_up(
                            finished,
                            succeeded,
                            still_desired,
                        ),
                        stale_ids,
                    )
                };
                worker.unregister_json_watch_ids(stale_ids);
                if catch_up {
                    worker
                        // Registration may have been unavailable while physical
                        // JSON changed. Invalidate any in-flight snapshot before
                        // performing the catch-up pass.
                        .queue_json_analysis(JsonAnalysisTrigger::All, true)
                        .await;
                }
            });
        }
    }

    fn unregister_json_watch_ids(&self, ids: Vec<String>) {
        let worker = self.clone();
        tokio::spawn(async move {
            let start_worker = worker
                .workspace
                .write()
                .await
                .queue_watched_json_unregistrations(ids);
            if start_worker {
                worker.process_json_watch_unregistrations().await;
            }
        });
    }

    async fn process_json_watch_unregistrations(&self) {
        loop {
            let ids = self
                .workspace
                .write()
                .await
                .take_watched_json_unregistrations();
            if ids.is_empty() {
                self.workspace
                    .write()
                    .await
                    .finish_watched_json_unregistration_worker_if_idle();
                return;
            }
            let unregistrations = ids
                .iter()
                .cloned()
                .map(|id| Unregistration {
                    id,
                    method: "workspace/didChangeWatchedFiles".to_string(),
                })
                .collect();
            if let Err(error) = self.client.unregister_capability(unregistrations).await {
                self.client
                    .log_message(
                        MessageType::WARNING,
                        format!(
                            "Could not retire stale JSON file watchers; the next JSON scope sync will retry: {error}"
                        ),
                    )
                    .await;
                let retry_now = self
                    .workspace
                    .write()
                    .await
                    .defer_watched_json_unregistrations(ids);
                if retry_now {
                    continue;
                }
                return;
            }
            if self
                .workspace
                .write()
                .await
                .finish_watched_json_unregistration_worker_if_idle()
            {
                return;
            }
        }
    }

    fn reserve_json_watch_registrations(
        &self,
        workspace: &mut Workspace,
        roots: Vec<std::path::PathBuf>,
    ) -> (Vec<String>, Vec<PendingJsonWatchRegistration>) {
        if !self.json_diagnostics_enabled() {
            return (Vec::new(), Vec::new());
        }
        let candidates = Self::json_watch_candidates(roots);
        let desired = candidates
            .iter()
            .map(|(identity, _)| identity.clone())
            .collect::<std::collections::HashSet<_>>();
        let unregistrations = workspace.take_obsolete_watched_json_registrations(&desired);
        let mut registrations = Vec::new();
        for (identity, plan) in candidates {
            let Some(sequence) = workspace.reserve_watched_json_registration(&identity) else {
                continue;
            };
            registrations.push((
                format!(
                    "vector-lsp-watched-files-{}-json-{sequence}",
                    workspace.session_generation
                ),
                identity,
                workspace.session_generation,
                sequence,
                plan,
            ));
        }
        (unregistrations, registrations)
    }

    fn json_watch_candidates(
        mut roots: Vec<std::path::PathBuf>,
    ) -> Vec<(String, WatchRegistrationPlan)> {
        roots.sort_by_key(|root| json_path_identity(root));
        roots.dedup_by(|left, right| same_local_path(left, right));
        let mut candidates = Vec::new();
        for root in roots {
            for (identity, plan) in [
                (root.join("local").join("lng").join("strings"), "strings"),
                (root.join("global").join("ui").join("layouts"), "layouts"),
            ]
            .into_iter()
            .flat_map(|(target, label)| Self::json_target_watch_plans(&root, &target, label))
            {
                candidates.push((identity, plan));
            }
        }
        candidates
    }

    fn extension_matches(path: &std::path::Path, extensions: &[&str]) -> bool {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extensions
                    .iter()
                    .any(|candidate| extension.eq_ignore_ascii_case(candidate))
            })
    }

    fn path_matches_root(path: &std::path::Path, root: &std::path::Path, recursive: bool) -> bool {
        if recursive {
            local_path_is_within(path, root) && !same_local_path(path, root)
        } else {
            path.parent()
                .is_some_and(|parent| same_local_path(parent, root))
        }
    }

    fn is_watched_txt_path(&self, workspace: &Workspace, path: &std::path::Path) -> bool {
        if workspace.open_documents.keys().any(|uri| {
            uri.to_file_path()
                .ok()
                .is_some_and(|open_path| same_local_path(&open_path, path))
        }) {
            return false;
        }
        let primary = workspace
            .root_uri
            .as_ref()
            .and_then(|uri| uri.to_file_path().ok())
            .is_some_and(|root| {
                if workspace.reference_context_mode == ReferenceContextMode::Sibling {
                    Self::path_matches_root(path, &root, false)
                        && Self::extension_matches(path, &["txt"])
                } else if self.settings.editor_mode {
                    Self::path_matches_root(path, &root, workspace.include_subfolders)
                        && Self::extension_matches(path, crate::scan::EDITOR_EXTENSIONS)
                } else {
                    Self::path_matches_root(path, &root, true)
                        && Self::extension_matches(path, &[self.settings.extension.as_str()])
                }
            });
        if primary {
            return true;
        }
        workspace.reference_context_mode == ReferenceContextMode::Sibling
            && workspace
                .reference_root_uri
                .as_ref()
                .and_then(|uri| uri.to_file_path().ok())
                .is_some_and(|root| {
                    Self::path_matches_root(path, &root, workspace.include_subfolders)
                        && Self::extension_matches(path, crate::scan::EDITOR_EXTENSIONS)
                })
    }

    fn watched_json_event(
        workspace: &Workspace,
        path: &std::path::Path,
    ) -> (Option<JsonAnalysisTrigger>, bool) {
        let is_json = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
        let mut trigger = None;
        let mut bootstrap = false;
        for data_root in workspace.primary_json_data_roots() {
            let strings = data_root.join("local").join("lng").join("strings");
            let layouts = data_root.join("global").join("ui").join("layouts");
            if is_json
                && path
                    .parent()
                    .is_some_and(|parent| same_local_path(parent, &strings))
            {
                trigger = Some(JsonAnalysisTrigger::All);
            } else if is_json
                && path
                    .parent()
                    .is_some_and(|parent| same_local_path(parent, &layouts))
                && trigger.is_none()
            {
                trigger = Some(JsonAnalysisTrigger::KeyUsageOnly);
            }
            bootstrap |= (same_local_path(path, &strings)
                || same_local_path(path, &layouts)
                || local_path_is_within(&strings, path)
                || local_path_is_within(&layouts, path))
                && local_path_is_within(path, &data_root)
                && !same_local_path(path, &data_root);
        }
        (trigger, bootstrap)
    }

    async fn process_watched_changes(&self, session_generation: u64) {
        loop {
            let Some(generation) = self
                .workspace
                .read()
                .await
                .watched_change_generation_for_worker(session_generation)
            else {
                return;
            };
            tokio::time::sleep(WATCHED_FILES_QUIET_WINDOW).await;
            let Some(pending) = self
                .workspace
                .write()
                .await
                .take_watched_changes(session_generation, generation)
            else {
                continue;
            };

            if pending.txt {
                self.scan_and_index_workspace(Instant::now(), Duration::ZERO)
                    .await;
            }

            if self
                .workspace
                .write()
                .await
                .finish_watched_change_worker_if_idle(session_generation)
            {
                return;
            }
        }
    }

    async fn queue_json_analysis(
        &self,
        trigger: JsonAnalysisTrigger,
        invalidate_physical_inputs: bool,
    ) {
        let rules = self.settings.json_diagnostic_rules();
        if !self.json_diagnostics_enabled()
            || trigger == JsonAnalysisTrigger::KeyUsageOnly && !rules.key_usage.is_enabled()
        {
            return;
        }
        let (session_generation, start_worker) = {
            let mut workspace = self.workspace.write().await;
            if invalidate_physical_inputs {
                workspace.bump_json_input_generation();
            }
            let (_, start_worker) =
                workspace.queue_json_analysis(trigger == JsonAnalysisTrigger::All);
            (workspace.session_generation, start_worker)
        };
        if start_worker {
            self.spawn_json_analysis_worker(session_generation);
        }
    }

    fn spawn_json_analysis_worker(&self, session_generation: u64) {
        let worker = self.clone();
        let future: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
            Box::pin(async move {
                worker.process_json_analysis(session_generation).await;
            });
        tokio::spawn(future);
    }

    async fn process_json_analysis(&self, session_generation: u64) {
        loop {
            let Some(generation) = self
                .workspace
                .read()
                .await
                .json_analysis_generation_for_worker(session_generation)
            else {
                return;
            };
            tokio::time::sleep(WATCHED_FILES_QUIET_WINDOW).await;
            let Some(pending) = self
                .workspace
                .write()
                .await
                .take_json_analysis(session_generation, generation)
            else {
                continue;
            };
            let revision = {
                let workspace = self.workspace.read().await;
                (workspace.phase == WorkspacePhase::Ready)
                    .then_some((workspace.scan_generation, workspace.workspace_revision))
            };
            if let Some((scan_generation, workspace_revision)) = revision {
                // Registration failures roll their Pending slot back. Every
                // subsequent event-driven JSON pass is therefore a bounded
                // retry opportunity without introducing a polling loop.
                let roots = self.workspace.read().await.primary_json_data_roots();
                self.register_json_watch_roots(roots).await;
                let trigger = if pending.all_rules {
                    JsonAnalysisTrigger::All
                } else {
                    JsonAnalysisTrigger::KeyUsageOnly
                };
                let completed = self
                    .validate_json_diagnostics_for_revision(
                        scan_generation,
                        workspace_revision,
                        trigger,
                    )
                    .await;
                if !completed {
                    // Retry the same strength. In particular an All pass must
                    // never degrade into KeyUsage-only, and an unrelated TXT
                    // revision must not silently discard a primary evidence
                    // update that was already in flight.
                    if !self
                        .workspace
                        .write()
                        .await
                        .requeue_json_analysis(session_generation, pending)
                    {
                        return;
                    }
                }
            } else {
                let keep_running = self.workspace.write().await.defer_json_analysis(
                    session_generation,
                    generation,
                    pending,
                );
                if keep_running {
                    continue;
                }
                return;
            }
            if self
                .workspace
                .write()
                .await
                .finish_json_analysis_worker_if_idle(session_generation)
            {
                return;
            }
        }
    }

    async fn validate_open_ticket(&self, ticket: &ValidationTicket) -> Option<Vec<Diagnostic>> {
        let (schema_diags, plugin_data) = {
            let ws = self.workspace.read().await;
            if !ws.is_current(ticket) {
                return None;
            }
            let doc = ws.open_documents.get(&ticket.uri)?.clone();
            let stem = Self::file_stem(&ticket.uri);
            let symbols = ws.symbols_for_uri(&ticket.uri);
            let schema_diags = diagnostics::validate_document_for_version(
                &stem,
                &doc,
                ws.schema.as_deref(),
                &symbols,
                ws.reference_version.as_deref(),
            );
            let plugin_data = self
                .plugin_host
                .as_ref()
                .filter(|host| host.validates_file(&stem))
                .map(|_| {
                    let ctx = plugin::build_context(&stem, &doc);
                    let view = ws.plugin_workspace_view_for_uri(&ticket.uri);
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
        let (schema, documents, reference_version) = {
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
            let documents = documents
                .into_iter()
                .map(|(uri, stem, document)| {
                    let symbols = ws.symbols_for_uri(&uri);
                    let plugin_view = self
                        .plugin_host
                        .as_ref()
                        .filter(|host| host.validates_file(&stem))
                        .map(|_| ws.plugin_workspace_view_for_uri(&uri));
                    (uri, stem, document, symbols, plugin_view)
                })
                .collect::<Vec<_>>();
            (ws.schema.clone(), documents, ws.reference_version.clone())
        };

        let mut schema_tasks = tokio::task::JoinSet::new();
        for (uri, stem, document, symbols, plugin_view) in documents {
            let schema = schema.clone();
            let reference_version = reference_version.clone();
            schema_tasks.spawn_blocking(move || {
                let diagnostics = diagnostics::validate_document_for_version(
                    &stem,
                    &document,
                    schema.as_deref(),
                    &symbols,
                    reference_version.as_deref(),
                );
                (uri, stem, document, plugin_view, diagnostics)
            });
        }
        let mut results = Vec::new();
        while let Some(result) = schema_tasks.join_next().await {
            if let Ok(result) = result {
                results.push(result);
            }
        }
        results.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));

        for (uri, stem, document, plugin_view, mut diagnostics) in results {
            if let (Some(view), Some(host)) = (plugin_view, &self.plugin_host) {
                diagnostics.extend(
                    host.run(
                        plugin::build_context(&stem, &document),
                        view.index,
                        view.snapshot,
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
                        let queue_startup_json = ws.claim_json_startup_analysis()
                            && self.json_diagnostics_enabled()
                            && !ws.primary_json_data_roots().is_empty();
                        let queue_txt_registration_catch_up =
                            ws.take_txt_watch_registration_catch_up();
                        Some((
                            VectorLspReadyParams {
                                session_generation: ws.session_generation,
                                scan_generation: ws.scan_generation,
                                workspace_revision: ws.workspace_revision,
                                root_uri: ws.root_uri.clone(),
                            },
                            queue_startup_json,
                            queue_txt_registration_catch_up,
                        ))
                    }
                };
                if let Some((params, queue_startup_json, queue_txt_registration_catch_up)) = ready {
                    self.client
                        .send_notification::<VectorLspReady>(params)
                        .await;
                    if queue_startup_json {
                        self.queue_json_analysis(JsonAnalysisTrigger::All, false)
                            .await;
                    }
                    if queue_txt_registration_catch_up {
                        let (session_generation, start_worker) = {
                            let mut workspace = self.workspace.write().await;
                            let (_, start_worker) = workspace.queue_watched_changes(true);
                            (workspace.session_generation, start_worker)
                        };
                        if start_worker {
                            self.spawn_watched_change_worker(session_generation);
                        }
                    }
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
            include_subfolders,
            scan_generation,
            session_generation,
            json_was_initialized,
            json_scope_before_scan,
        ) = {
            let mut ws = self.workspace.write().await;
            let json_scope_before_scan = Self::json_root_identities(&ws.primary_json_data_roots());
            let scan_generation = ws.begin_scan();
            (
                ws.root_uri.clone(),
                ws.reference_root_uri.clone(),
                self.settings.delimiter_char(),
                self.settings.extension.clone(),
                ws.reference_context_mode,
                ws.include_subfolders,
                scan_generation,
                ws.session_generation,
                ws.json_startup_analysis_queued(),
                json_scope_before_scan,
            )
        };

        let Some(root_uri) = root_uri else {
            self.fail_workspace_if_current_scan(
                session_generation,
                scan_generation,
                "Workspace scan failed: initialize did not provide a root URI",
            )
            .await;
            return;
        };
        let Ok(root_path) = root_uri.to_file_path() else {
            self.fail_workspace_if_current_scan(
                session_generation,
                scan_generation,
                format!("Workspace scan failed: root URI is not a file path: {root_uri}"),
            )
            .await;
            return;
        };

        let scan_policy = if reference_context_mode == ReferenceContextMode::Sibling {
            ScanPolicy::sibling_txt()
        } else if self.settings.editor_mode {
            ScanPolicy::editor_with_subfolders(include_subfolders)
        } else {
            ScanPolicy::standalone(&ext)
        };
        let enumerate_started = Instant::now();
        let discovery = match crate::scan::collect_data_files(&root_path, &scan_policy) {
            Ok(discovery) => discovery,
            Err(e) => {
                self.fail_workspace_if_current_scan(
                    session_generation,
                    scan_generation,
                    format!("Workspace scan failed: {e}"),
                )
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
        let primary_discovered_paths = entries
            .iter()
            .map(|(_, path, _)| json_path_identity(path))
            .collect::<std::collections::HashSet<_>>();
        let mut reference_entries: Vec<(Url, std::path::PathBuf, String)> = Vec::new();
        if reference_context_mode == ReferenceContextMode::Sibling
            && let Some(reference_root_uri) = reference_root_uri
        {
            match reference_root_uri.to_file_path() {
                Ok(reference_root_path) if reference_root_path != root_path => {
                    match crate::scan::collect_data_files(
                        &reference_root_path,
                        &ScanPolicy::editor_with_subfolders(include_subfolders),
                    ) {
                        Ok(reference_discovery) => {
                            failures.extend(reference_discovery.failures);
                            for path in reference_discovery.paths {
                                if Self::primary_discovery_contains_path(
                                    &primary_discovered_paths,
                                    &path,
                                ) {
                                    // Remove only exact paths selected by
                                    // primary discovery. In sibling mode a
                                    // nested descendant belongs exclusively
                                    // to the explicit-reference tier.
                                    continue;
                                }
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
            if !workspace.commit_scan_present_paths(
                scan_generation,
                present_paths,
                reference_present_paths,
            ) {
                return;
            }
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
        let (json_data_roots, json_scope_changed) = {
            let mut ws = self.workspace.write().await;
            if !ws.commit_scan_documents_with_reference_root(
                scan_generation,
                &parsed,
                &reference_parsed,
            ) {
                return;
            }
            let roots = ws.primary_json_data_roots();
            let changed = Self::json_root_identities(&roots) != json_scope_before_scan;
            (roots, changed)
        };
        if self.json_diagnostics_enabled() {
            self.register_json_watch_roots(json_data_roots).await;
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
        if json_was_initialized && self.workspace.read().await.phase == WorkspacePhase::Ready {
            self.queue_json_analysis(
                if json_scope_changed {
                    JsonAnalysisTrigger::All
                } else {
                    JsonAnalysisTrigger::KeyUsageOnly
                },
                false,
            )
            .await;
        }
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
        let watched_files_capabilities = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.did_change_watched_files.as_ref());
        let watched_files_dynamic_registration = watched_files_capabilities
            .and_then(|capabilities| capabilities.dynamic_registration)
            .unwrap_or(false);
        let watched_files_relative_pattern_support = watched_files_capabilities
            .and_then(|capabilities| capabilities.relative_pattern_support)
            .unwrap_or(false);
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
        let include_subfolders = params
            .initialization_options
            .as_ref()
            .and_then(|value| value.get("includeSubfolders"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let workspace_directory_scopes = params
            .initialization_options
            .as_ref()
            .and_then(|value| value.get("workspaceDirectoryScopes"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
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
            ws.set_editor_workspace_options(include_subfolders, workspace_directory_scopes);
            ws.set_reference_root_uri(reference_root_uri);
            ws.begin_initialization(session_generation);
            ws.set_watched_files_client_capabilities(
                watched_files_dynamic_registration,
                watched_files_relative_pattern_support,
            );
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
        let watch_registration_completion =
            if self.workspace.read().await.phase != WorkspacePhase::Failed {
                self.register_watched_files().await
            } else {
                None
            };
        if let Some(completion) = watch_registration_completion {
            // Fast clients normally install the watchers before the first scan,
            // preserving a single startup scan. A missing/slow response cannot
            // hold Ready indefinitely; late success records one post-Ready
            // catch-up instead.
            let _ = tokio::time::timeout(INITIAL_WATCH_REGISTRATION_GRACE, completion).await;
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
        let is_json = Self::is_json_uri(&uri);
        let mutation_gate = if is_json {
            self.json_publish_gate(&uri).await
        } else {
            self.publish_gate(&uri).await
        };
        let mutation_guard = mutation_gate.lock().await;
        let json_enabled = self.json_diagnostics_enabled();
        if is_json {
            let accepted = {
                let mut workspace = self.workspace.write().await;
                if json_enabled && Self::localization_json_uri_in_scope(&workspace, &uri) {
                    workspace.accept_open_json(uri, version, params.text_document.text);
                    true
                } else {
                    false
                }
            };
            drop(mutation_guard);
            if accepted {
                self.queue_json_analysis(JsonAnalysisTrigger::All, false)
                    .await;
            }
            return;
        }
        let doc = Arc::new(DocumentData::parse(
            &params.text_document.text,
            self.settings.delimiter_char(),
        ));
        let (ready, equivalent_open, ticket, json_trigger, json_roots) = {
            let mut ws = self.workspace.write().await;
            let roots_before = if json_enabled {
                ws.primary_json_data_roots()
            } else {
                Vec::new()
            };
            let equivalent_open = ws.effective_source_has_same_content(&uri, &doc);
            let ticket = if equivalent_open {
                ws.accept_equivalent_open(uri.clone(), version, doc)
            } else {
                ws.accept_open(uri.clone(), version, doc)
            };
            ws.rebuild_effective_symbols();
            let roots_after = if json_enabled {
                ws.primary_json_data_roots()
            } else {
                Vec::new()
            };
            let json_trigger = if Self::json_root_identities(&roots_before)
                != Self::json_root_identities(&roots_after)
            {
                Some(JsonAnalysisTrigger::All)
            } else if json_enabled
                && !equivalent_open
                && ws.primary_json_data_root_for_uri(&uri).is_some()
            {
                Some(JsonAnalysisTrigger::KeyUsageOnly)
            } else {
                None
            };
            (
                ws.phase == WorkspacePhase::Ready,
                equivalent_open,
                ticket,
                json_trigger,
                roots_after,
            )
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
        if let Some(trigger) = json_trigger {
            self.register_json_watch_roots(json_roots).await;
            self.queue_json_analysis(trigger, false).await;
        }
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let is_json = Self::is_json_uri(&uri);
        let mutation_gate = if is_json {
            self.json_publish_gate(&uri).await
        } else {
            self.publish_gate(&uri).await
        };
        let mutation_guard = mutation_gate.lock().await;
        let json_enabled = self.json_diagnostics_enabled();
        let delimiter = self.settings.delimiter_char();

        if is_json {
            let update_result = {
                let mut workspace = self.workspace.write().await;
                // An accepted didOpen owns its buffer until didClose. A watched
                // TXT rescan may temporarily remove the diagnostic scope, but
                // rejecting changes here would desynchronize the live document.
                if !json_enabled || workspace.open_json_version(&uri).is_none() {
                    Err(DocumentChangeError::NotOpen)
                } else {
                    let existing = workspace.open_json_text(&uri).unwrap_or_default();
                    let mut lines = existing.lines().map(str::to_owned).collect::<Vec<_>>();
                    let mut full_text = None;
                    for change in &params.content_changes {
                        match change.range {
                            Some(range) if full_text.is_none() => {
                                apply_change(&mut lines, range, &change.text)
                            }
                            Some(range) => {
                                let mut current = full_text
                                    .take()
                                    .unwrap_or_else(|| lines.join("\n"))
                                    .lines()
                                    .map(str::to_owned)
                                    .collect::<Vec<_>>();
                                apply_change(&mut current, range, &change.text);
                                full_text = Some(current.join("\n"));
                            }
                            None => full_text = Some(change.text.clone()),
                        }
                    }
                    workspace.accept_change_json(
                        &uri,
                        params.text_document.version,
                        full_text.unwrap_or_else(|| lines.join("\n")),
                    )
                }
            };
            drop(mutation_guard);
            match update_result {
                Ok(()) => {
                    self.queue_json_analysis(JsonAnalysisTrigger::All, false)
                        .await
                }
                Err(error) => self.report_rejected_change(&uri, error).await,
            }
            return;
        }

        // Reconstruct current text from the stored document, apply each incremental
        // change in order, then re-parse. Avoids receiving the full document over IPC.
        let update_result: Result<(bool, bool), DocumentChangeError> = {
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
                    Ok((
                        ws.phase == WorkspacePhase::Ready,
                        json_enabled && ws.primary_json_data_root_for_uri(&uri).is_some(),
                    ))
                }
                Err(error) => Err(error),
            }
        };
        drop(mutation_guard);
        let json_relevant = update_result
            .as_ref()
            .is_ok_and(|(_, json_relevant)| *json_relevant);
        match update_result {
            Ok((true, _)) => self.revalidate_workspace_after_change().await,
            Ok((false, _)) => {}
            Err(error) => self.report_rejected_change(&uri, error).await,
        }
        if json_relevant {
            self.queue_json_analysis(JsonAnalysisTrigger::KeyUsageOnly, false)
                .await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        let is_json = Self::is_json_uri(&uri);
        let mutation_gate = if is_json {
            self.json_publish_gate(&uri).await
        } else {
            self.publish_gate(&uri).await
        };
        let mutation_guard = mutation_gate.lock().await;
        let json_enabled = self.json_diagnostics_enabled();
        if is_json {
            let closed = self.workspace.write().await.close_open_json(&uri);
            drop(mutation_guard);
            if closed && json_enabled {
                self.queue_json_analysis(JsonAnalysisTrigger::All, false)
                    .await;
            }
            return;
        }
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
        let (ready, json_trigger, json_roots) = {
            let mut ws = self.workspace.write().await;
            let roots_before = if json_enabled {
                ws.primary_json_data_roots()
            } else {
                Vec::new()
            };
            let was_json_relevant =
                json_enabled && ws.primary_json_data_root_for_uri(&uri).is_some();
            let path_present = disk_document.is_some() || reload_error.is_some();
            let restored_document = if reload_error.is_some() {
                path.as_ref().and_then(|path| ws.cached_disk_document(path))
            } else {
                disk_document
            };
            let restored = ws.restore_closed_document_with_presence(
                &uri,
                path.clone(),
                restored_document,
                path_present,
            );
            let roots_after = if json_enabled {
                ws.primary_json_data_roots()
            } else {
                Vec::new()
            };
            let json_trigger = if restored
                && Self::json_root_identities(&roots_before)
                    != Self::json_root_identities(&roots_after)
            {
                Some(JsonAnalysisTrigger::All)
            } else if restored && was_json_relevant {
                Some(JsonAnalysisTrigger::KeyUsageOnly)
            } else {
                None
            };
            (
                restored && ws.phase == WorkspacePhase::Ready,
                json_trigger,
                roots_after,
            )
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
        if let Some(trigger) = json_trigger {
            self.register_json_watch_roots(json_roots).await;
            self.queue_json_analysis(trigger, false).await;
        }
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let (txt, json_trigger, bootstrap, json_roots) = {
            let workspace = self.workspace.read().await;
            let mut txt = false;
            let mut json_trigger = None;
            let mut bootstrap = false;
            for change in params.changes {
                let Ok(path) = change.uri.to_file_path() else {
                    continue;
                };
                txt |= self.is_watched_txt_path(&workspace, &path);
                if self.json_diagnostics_enabled() {
                    let (event_trigger, event_bootstrap) =
                        Self::watched_json_event(&workspace, &path);
                    if event_trigger == Some(JsonAnalysisTrigger::All)
                        || json_trigger.is_none() && event_trigger.is_some()
                    {
                        json_trigger = event_trigger;
                    }
                    bootstrap |= event_bootstrap;
                }
            }
            (
                txt,
                json_trigger,
                bootstrap,
                workspace.primary_json_data_roots(),
            )
        };
        if bootstrap {
            self.register_json_watch_roots(json_roots).await;
        }
        if let Some(trigger) =
            json_trigger.or_else(|| bootstrap.then_some(JsonAnalysisTrigger::All))
        {
            self.queue_json_analysis(trigger, true).await;
        }
        if txt {
            let (session_generation, start_worker) = {
                let mut workspace = self.workspace.write().await;
                let (_, start_worker) = workspace.queue_watched_changes(true);
                (workspace.session_generation, start_worker)
            };
            if start_worker {
                self.spawn_watched_change_worker(session_generation);
            }
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
            let symbols = ws.symbols_for_uri(uri);

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
                    symbols
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
                        let view = ws.plugin_workspace_view_for_uri(uri);
                        (
                            ctx,
                            view.index,
                            view.snapshot,
                            (view.session_generation, view.workspace_revision),
                            Arc::clone(&symbols),
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
            let symbols = ws.symbols_for_uri(uri);
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
                    let resolved = symbols.resolve(
                        reference_file,
                        reference_column,
                        &cell_value,
                        field_type.resolver,
                    );
                    if resolved.is_none()
                        && diagnostics::is_monpet_consumestat_reference(&file_stem, &col_name)
                        && symbols.has_file(reference_file)
                        && symbols.has_column(reference_file, reference_column)
                    {
                        return Some(format!(
                            "**Unknown stat name**\n\n`{}` is not a known stat. This Consume bonus is not applied; other Consume slots still work. Use the exact Stat name from `itemstatcost.txt`.",
                            cell_value
                        ));
                    }
                    if resolved.is_none()
                        && diagnostics::is_properties_stat_reference(&file_stem, &col_name)
                        && symbols.has_file(reference_file)
                        && symbols.has_column(reference_file, reference_column)
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
                    let view = ws.plugin_workspace_view_for_uri(uri);
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
    use futures::StreamExt;
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

    async fn wait_for_watched_change_worker(workspace: &Arc<RwLock<Workspace>>) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if !workspace.read().await.watched_change_worker_running() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("watched-files worker did not become idle");
    }

    async fn wait_for_json_analysis_worker(workspace: &Arc<RwLock<Workspace>>) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if !workspace.read().await.json_analysis_worker_running() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("JSON analysis worker did not become idle");
    }

    #[test]
    fn key_usage_merge_preserves_other_rules_only_for_currently_observed_json() {
        let kept = Url::parse("file:///mod/data/local/lng/strings/skills.json").unwrap();
        let deleted = Url::parse("file:///mod/data/local/lng/strings/ui.json").unwrap();
        let diagnostic = |code: &str, message: &str| Diagnostic {
            code: Some(NumberOrString::String(code.to_string())),
            message: message.to_string(),
            ..Diagnostic::default()
        };
        let previous = HashMap::from([
            (
                kept.clone(),
                vec![
                    diagnostic("Json/DuplicateIds", "duplicate"),
                    diagnostic("Json/KeyUsage", "old key result"),
                ],
            ),
            (
                deleted.clone(),
                vec![diagnostic("Json/StringFormat", "deleted file")],
            ),
        ]);
        let merged = Backend::merge_key_usage_batches(
            &previous,
            vec![JsonDiagnosticBatch {
                uri: kept.clone(),
                diagnostics: vec![diagnostic("Json/KeyUsage", "new key result")],
            }],
        );

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].uri, kept);
        assert!(merged.iter().all(|batch| batch.uri != deleted));
        assert_eq!(
            merged[0]
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.as_str())
                .collect::<Vec<_>>(),
            vec!["duplicate", "new key result"]
        );
    }

    #[test]
    fn identical_json_diagnostics_are_republished_when_the_version_changes() {
        let diagnostics = vec![Diagnostic {
            message: "same diagnostics".to_string(),
            ..Diagnostic::default()
        }];

        assert!(Backend::json_publication_matches(
            &diagnostics,
            Some(Some(2)),
            &diagnostics,
            Some(2),
        ));
        assert!(!Backend::json_publication_matches(
            &diagnostics,
            Some(None),
            &diagnostics,
            Some(1),
        ));
        assert!(!Backend::json_publication_matches(
            &diagnostics,
            Some(Some(1)),
            &diagnostics,
            Some(2),
        ));
        assert!(!Backend::json_publication_matches(
            &diagnostics,
            Some(Some(2)),
            &diagnostics,
            None,
        ));
    }

    #[test]
    fn successful_current_json_registration_always_requests_an_all_rule_catch_up() {
        assert!(Backend::successful_current_json_registration_needs_catch_up(true, true, true));
        assert!(!Backend::successful_current_json_registration_needs_catch_up(false, true, true));
        assert!(!Backend::successful_current_json_registration_needs_catch_up(true, false, true));
        assert!(!Backend::successful_current_json_registration_needs_catch_up(true, true, false));
    }

    #[test]
    fn delayed_txt_registration_catches_up_without_waiting_before_initial_scan() {
        assert!(!Backend::successful_txt_registration_needs_catch_up(
            WorkspacePhase::LoadingSchema
        ));
        assert!(Backend::successful_txt_registration_needs_catch_up(
            WorkspacePhase::Scanning
        ));
        assert!(Backend::successful_txt_registration_needs_catch_up(
            WorkspacePhase::Reconciling
        ));
        assert!(Backend::successful_txt_registration_needs_catch_up(
            WorkspacePhase::Ready
        ));
        assert!(!Backend::successful_txt_registration_needs_catch_up(
            WorkspacePhase::Failed
        ));
    }

    #[test]
    fn reference_scan_deduplicates_only_exact_primary_discovery_paths() {
        let primary_root = std::env::temp_dir().join("vlsp-exact-primary-discovery");
        let direct = primary_root.join("items.txt");
        let nested_reference = primary_root.join("nested/skills.txt");
        let primary = HashSet::from([json_path_identity(&direct)]);

        assert!(Backend::primary_discovery_contains_path(&primary, &direct));
        assert!(!Backend::primary_discovery_contains_path(
            &primary,
            &nested_reference
        ));
    }

    #[tokio::test]
    async fn txt_watch_registration_scheduling_does_not_wait_for_a_client_response() {
        let workspace = Arc::new(RwLock::new(Workspace::new()));
        {
            let mut ws = workspace.write().await;
            ws.root_uri = Url::parse("file:///workspace").ok();
            ws.session_generation = 71;
            ws.phase = WorkspacePhase::LoadingSchema;
            ws.set_watched_files_client_capabilities(true, true);
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

        tokio::time::timeout(
            Duration::from_millis(100),
            service.inner().register_watched_files(),
        )
        .await
        .expect("registration scheduling must not await client/registerCapability");
    }

    #[tokio::test]
    async fn json_queue_ignores_unrelated_open_documents_and_tracks_primary_txt_changes() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "vector-lsp-json-trigger-scope-{}-{nonce}",
            std::process::id()
        ));
        let excel = base.join("mod/data/global/excel");
        std::fs::create_dir_all(&excel).unwrap();
        let unrelated_uri = Url::from_file_path(base.join("notes.txt")).unwrap();
        let primary_uri = Url::from_file_path(excel.join("skills.txt")).unwrap();

        let workspace = Arc::new(RwLock::new(Workspace::new()));
        {
            let mut ws = workspace.write().await;
            ws.root_uri = Url::from_directory_path(&base).ok();
            ws.session_generation = 101;
            ws.scan_generation = 3;
            ws.workspace_revision = 7;
            ws.phase = WorkspacePhase::Ready;
        }
        let settings = Arc::new(VectorLspSettings {
            json_diagnostics: true,
            json_key_usage_action: crate::settings::JsonRuleAction::Warn,
            ..VectorLspSettings::default()
        });
        let publish_gates = Arc::new(Mutex::new(HashMap::new()));
        let workspace_for_service = Arc::clone(&workspace);
        let (service, mut socket) = tower_lsp::LspService::new(move |client| Backend {
            client,
            settings: Arc::clone(&settings),
            workspace: Arc::clone(&workspace_for_service),
            plugin_host: None,
            publish_gates: Arc::clone(&publish_gates),
        });
        let socket_task = tokio::spawn(async move { while socket.next().await.is_some() {} });

        service
            .inner()
            .did_open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem::new(
                    unrelated_uri.clone(),
                    "txt".to_string(),
                    1,
                    "id\nUNRELATED\n".to_string(),
                ),
            })
            .await;
        assert_eq!(workspace.read().await.json_analysis_generation(), 0);

        service
            .inner()
            .did_open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem::new(
                    primary_uri.clone(),
                    "txt".to_string(),
                    1,
                    "id\nPRIMARY\n".to_string(),
                ),
            })
            .await;
        assert!(workspace.read().await.pending_json_analysis().all_rules);
        wait_for_json_analysis_worker(&workspace).await;

        let generation = workspace.read().await.json_analysis_generation();
        service
            .inner()
            .did_change(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier::new(unrelated_uri, 2),
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: "id\nSTILL-UNRELATED\n".to_string(),
                }],
            })
            .await;
        assert_eq!(
            workspace.read().await.json_analysis_generation(),
            generation
        );

        service
            .inner()
            .did_change(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier::new(primary_uri, 2),
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: "id\nPRIMARY-CHANGED\n".to_string(),
                }],
            })
            .await;
        let ws = workspace.read().await;
        assert!(ws.json_analysis_generation() > generation);
        assert!(ws.pending_json_analysis().key_usage);
        drop(ws);
        wait_for_json_analysis_worker(&workspace).await;
        std::fs::remove_dir_all(base).unwrap();
        socket_task.abort();
    }

    #[tokio::test]
    async fn localization_json_open_buffer_survives_temporary_scope_loss_and_close_restores_disk() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "vector-lsp-json-open-buffer-{}-{nonce}",
            std::process::id()
        ));
        let excel_path = base.join("data/global/excel/skills.txt");
        let json_path = base.join("data/local/lng/strings/skills.json");
        std::fs::create_dir_all(excel_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(json_path.parent().unwrap()).unwrap();
        std::fs::write(&excel_path, "skill\nnone\n").unwrap();
        std::fs::write(&json_path, "[]").unwrap();
        let json_uri = Url::from_file_path(&json_path).unwrap();

        let workspace = Arc::new(RwLock::new(Workspace::new()));
        {
            let mut ws = workspace.write().await;
            ws.root_uri = Url::from_directory_path(&base).ok();
            ws.session_generation = 102;
            ws.scan_generation = 4;
            ws.workspace_revision = 8;
            ws.phase = WorkspacePhase::Ready;
            ws.file_cache.insert(
                excel_path.clone(),
                Arc::new(DocumentData::parse("skill\nnone\n", '\t')),
            );
        }
        let settings = Arc::new(VectorLspSettings {
            json_diagnostics: true,
            ..VectorLspSettings::default()
        });
        let publish_gates = Arc::new(Mutex::new(HashMap::new()));
        let workspace_for_service = Arc::clone(&workspace);
        let (service, mut socket) = tower_lsp::LspService::new(move |client| Backend {
            client,
            settings: Arc::clone(&settings),
            workspace: Arc::clone(&workspace_for_service),
            plugin_host: None,
            publish_gates: Arc::clone(&publish_gates),
        });
        let socket_task = tokio::spawn(async move { while socket.next().await.is_some() {} });

        service
            .inner()
            .did_open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem::new(
                    json_uri.clone(),
                    "json".to_string(),
                    1,
                    "[{\"id\":41001}]".to_string(),
                ),
            })
            .await;
        wait_for_json_analysis_worker(&workspace).await;
        assert_eq!(workspace.read().await.open_json_version(&json_uri), Some(1));

        workspace.write().await.file_cache.clear();
        assert!(
            workspace.read().await.primary_json_data_roots().is_empty(),
            "the accepted LSP document must outlive a temporary loss of TXT scope"
        );

        service
            .inner()
            .did_change(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier::new(json_uri.clone(), 2),
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: "[{\"id\":41002}]".to_string(),
                }],
            })
            .await;
        wait_for_json_analysis_worker(&workspace).await;
        let ws = workspace.read().await;
        assert_eq!(ws.open_json_version(&json_uri), Some(2));
        assert_eq!(ws.open_json_text(&json_uri), Some("[{\"id\":41002}]"));
        drop(ws);

        workspace.write().await.file_cache.insert(
            excel_path,
            Arc::new(DocumentData::parse("skill\nnone\n", '\t')),
        );
        assert_eq!(workspace.read().await.primary_json_data_roots().len(), 1);

        service
            .inner()
            .did_close(DidCloseTextDocumentParams {
                text_document: TextDocumentIdentifier::new(json_uri.clone()),
            })
            .await;
        wait_for_json_analysis_worker(&workspace).await;
        assert_eq!(workspace.read().await.open_json_version(&json_uri), None);

        std::fs::remove_dir_all(base).unwrap();
        socket_task.abort();
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
    fn json_input_generation_invalidates_an_in_flight_filesystem_analysis() {
        let mut workspace = Workspace::new();
        workspace.session_generation = 9;
        workspace.scan_generation = 4;
        workspace.workspace_revision = 6;
        workspace.phase = WorkspacePhase::Ready;
        let mut settings = VectorLspSettings::default();
        let ticket = JsonValidationTicket {
            session_generation: 9,
            scan_generation: 4,
            workspace_revision: 6,
            json_input_generation: workspace.json_input_generation(),
            scope_identities: Vec::new(),
            evidence_profile: JsonEvidenceProfile::ResurrectedOrUnknown,
        };
        assert!(Backend::json_ticket_is_current(
            &settings, &workspace, &ticket
        ));
        workspace.reference_version = Some("1.13".to_string());
        assert!(!Backend::json_ticket_is_current(
            &settings, &workspace, &ticket
        ));
        workspace.reference_version = None;
        assert!(Backend::json_ticket_is_current(
            &settings, &workspace, &ticket
        ));
        settings.schema_variant = "1.13".to_string();
        assert!(!Backend::json_ticket_is_current(
            &settings, &workspace, &ticket
        ));
        settings.schema_variant.clear();
        workspace.bump_json_input_generation();
        assert!(!Backend::json_ticket_is_current(
            &settings, &workspace, &ticket
        ));
    }

    #[tokio::test]
    async fn stale_scan_failure_cannot_fail_a_newer_scan() {
        let workspace = Arc::new(RwLock::new(Workspace::new()));
        let (session_generation, stale_scan, current_scan) = {
            let mut ws = workspace.write().await;
            ws.begin_initialization(71);
            let stale = ws.begin_scan();
            let current = ws.begin_scan();
            (ws.session_generation, stale, current)
        };
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

        assert!(
            !service
                .inner()
                .fail_workspace_if_current_scan(
                    session_generation,
                    stale_scan,
                    "stale scan failure",
                )
                .await
        );
        let ws = workspace.read().await;
        assert_eq!(ws.scan_generation, current_scan);
        assert_eq!(ws.phase, WorkspacePhase::Scanning);
    }

    #[tokio::test]
    async fn dynamic_registration_bootstraps_missing_json_directories_non_recursively() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "vector-lsp-watch-registration-{}-{nonce}",
            std::process::id()
        ));
        let data_root = base.join("mod/data");
        let global_root = data_root.join("global");
        let excel_dir = global_root.join("excel");
        let txt_path = excel_dir.join("skills.txt");
        std::fs::create_dir_all(&excel_dir).unwrap();
        std::fs::write(&txt_path, "id\n1\n").unwrap();

        let workspace = Arc::new(RwLock::new(Workspace::new()));
        {
            let mut ws = workspace.write().await;
            ws.root_uri = Url::from_directory_path(&global_root).ok();
            ws.session_generation = 61;
            ws.set_watched_files_client_capabilities(true, true);
        }
        let settings = Arc::new(VectorLspSettings {
            editor_mode: true,
            json_diagnostics: true,
            ..VectorLspSettings::default()
        });
        let publish_gates = Arc::new(Mutex::new(HashMap::new()));
        let workspace_for_service = Arc::clone(&workspace);
        let (service, _socket) = tower_lsp::LspService::new(move |client| Backend {
            client,
            settings: Arc::clone(&settings),
            workspace: Arc::clone(&workspace_for_service),
            plugin_host: None,
            publish_gates: Arc::clone(&publish_gates),
        });
        let txt_plans = service
            .inner()
            .watch_registration_plans(&*workspace.read().await);
        assert_eq!(txt_plans.len(), 1);
        assert_eq!(
            txt_plans[0].base_uri,
            Url::from_directory_path(&global_root).unwrap()
        );
        assert!(
            txt_plans[0]
                .patterns
                .contains(&"**/*.[tT][xX][tT]".to_string())
        );

        let discovered_root = data_root_from_excel_txt(&txt_path).unwrap();
        let (json_registrations, repeated_registrations) = {
            let mut ws = workspace.write().await;
            let (_, first) = service
                .inner()
                .reserve_json_watch_registrations(&mut ws, vec![discovered_root.clone()]);
            let (_, repeated) = service
                .inner()
                .reserve_json_watch_registrations(&mut ws, vec![discovered_root]);
            (first, repeated)
        };
        assert_eq!(json_registrations.len(), 2);
        assert!(repeated_registrations.is_empty());
        let patterns = json_registrations
            .iter()
            .flat_map(|(_, _, _, _, plan)| plan.patterns.iter().map(String::as_str))
            .collect::<HashSet<_>>();
        assert_eq!(patterns, HashSet::from(["local", "ui"]));
        assert!(json_registrations.iter().all(|(_, _, _, _, plan)| {
            plan.kind == Some(WatchKind::Create | WatchKind::Delete)
        }));
        assert!(json_registrations.iter().any(|(_, _, _, _, plan)| {
            plan.base_uri == Url::from_directory_path(&data_root).unwrap()
                && plan.patterns == vec!["local"]
        }));
        assert!(json_registrations.iter().any(|(_, _, _, _, plan)| {
            plan.base_uri == Url::from_directory_path(&global_root).unwrap()
                && plan.patterns == vec!["ui"]
        }));

        std::fs::create_dir_all(data_root.join("local/lng/strings")).unwrap();
        std::fs::create_dir_all(data_root.join("global/ui/layouts")).unwrap();
        let exact = Backend::json_watch_candidates(vec![data_root.clone()]);
        let file_plans = exact
            .iter()
            .filter(|(_, plan)| plan.kind.is_none())
            .collect::<Vec<_>>();
        assert_eq!(file_plans.len(), 2);
        assert!(file_plans.iter().all(|(_, plan)| {
            plan.patterns == vec!["*.[jJ][sS][oO][nN]"]
                && (plan.base_uri
                    == Url::from_directory_path(data_root.join("local/lng/strings")).unwrap()
                    || plan.base_uri
                        == Url::from_directory_path(data_root.join("global/ui/layouts")).unwrap())
        }));
        std::fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn deleted_json_target_retires_exact_watch_and_recreation_can_register_again() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "vector-lsp-json-watch-recreate-{}-{nonce}",
            std::process::id()
        ));
        let data_root = base.join("mod/data");
        let strings = data_root.join("local/lng/strings");
        let layouts = data_root.join("global/ui/layouts");
        std::fs::create_dir_all(&strings).unwrap();
        std::fs::create_dir_all(&layouts).unwrap();

        let workspace = Arc::new(RwLock::new(Workspace::new()));
        {
            let mut ws = workspace.write().await;
            ws.session_generation = 91;
            ws.set_watched_files_client_capabilities(true, true);
        }
        let settings = Arc::new(VectorLspSettings {
            json_diagnostics: true,
            ..VectorLspSettings::default()
        });
        let publish_gates = Arc::new(Mutex::new(HashMap::new()));
        let workspace_for_service = Arc::clone(&workspace);
        let (service, _socket) = tower_lsp::LspService::new(move |client| Backend {
            client,
            settings: Arc::clone(&settings),
            workspace: Arc::clone(&workspace_for_service),
            plugin_host: None,
            publish_gates: Arc::clone(&publish_gates),
        });

        {
            let mut ws = workspace.write().await;
            let (_, registrations) = service
                .inner()
                .reserve_json_watch_registrations(&mut ws, vec![data_root.clone()]);
            assert_eq!(registrations.len(), 4);
            for (id, identity, registration_session, sequence, _) in registrations {
                assert!(ws.finish_watched_json_registration(
                    &identity,
                    registration_session,
                    sequence,
                    &id,
                    true,
                ));
            }
        }

        std::fs::remove_dir(&strings).unwrap();
        {
            let mut ws = workspace.write().await;
            let (unregistrations, registrations) = service
                .inner()
                .reserve_json_watch_registrations(&mut ws, vec![data_root.clone()]);
            assert_eq!(unregistrations.len(), 1);
            assert!(registrations.is_empty());
        }

        std::fs::create_dir(&strings).unwrap();
        {
            let mut ws = workspace.write().await;
            let (_, registrations) = service
                .inner()
                .reserve_json_watch_registrations(&mut ws, vec![data_root.clone()]);
            assert_eq!(registrations.len(), 1);
            assert_eq!(registrations[0].4.patterns, vec!["*.[jJ][sS][oO][nN]"]);
        }
        std::fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn watched_file_burst_returns_immediately_and_runs_one_latest_scan() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "vector-lsp-watched-burst-{}-{nonce}",
            std::process::id()
        ));
        let excel_dir = base.join("mod/data/global/excel");
        let txt_path = excel_dir.join("skills.txt");
        std::fs::create_dir_all(&excel_dir).unwrap();
        std::fs::write(&txt_path, "id\nDISK-OLD\n").unwrap();
        let txt_uri = Url::from_file_path(&txt_path).unwrap();

        let workspace = Arc::new(RwLock::new(Workspace::new()));
        {
            let mut ws = workspace.write().await;
            ws.root_uri = Url::from_directory_path(&excel_dir).ok();
            ws.reference_context_mode = ReferenceContextMode::Sibling;
            ws.session_generation = 81;
            ws.scan_generation = 2;
            ws.workspace_revision = 7;
            ws.phase = WorkspacePhase::Ready;
            ws.set_workspace_present_paths([txt_path.clone()]);
            ws.file_cache.insert(
                txt_path.clone(),
                Arc::new(DocumentData::parse("id\nDISK-OLD\n", '\t')),
            );
        }

        let settings = Arc::new(VectorLspSettings::default());
        let publish_gates = Arc::new(Mutex::new(HashMap::new()));
        let workspace_for_service = Arc::clone(&workspace);
        let (service, mut socket) = tower_lsp::LspService::new(move |client| Backend {
            client,
            settings: Arc::clone(&settings),
            workspace: Arc::clone(&workspace_for_service),
            plugin_host: None,
            publish_gates: Arc::clone(&publish_gates),
        });
        let socket_task = tokio::spawn(async move { while socket.next().await.is_some() {} });

        const BURST: u64 = 8;
        for index in 0..BURST {
            std::fs::write(&txt_path, format!("id\nDISK-{index}\n")).unwrap();
            tokio::time::timeout(
                Duration::from_millis(50),
                service
                    .inner()
                    .did_change_watched_files(DidChangeWatchedFilesParams {
                        changes: vec![FileEvent::new(txt_uri.clone(), FileChangeType::CHANGED)],
                    }),
            )
            .await
            .expect("watched-files notification occupied an LSP handler slot");
        }
        wait_for_watched_change_worker(&workspace).await;

        let ws = workspace.read().await;
        assert_eq!(ws.phase, WorkspacePhase::Ready);
        assert_eq!(ws.scan_generation, 2 + BURST + 1);
        assert_eq!(ws.workspace_revision, 8);
        assert_eq!(
            ws.file_cache
                .iter()
                .find(|(path, _)| same_local_path(path, &txt_path))
                .map(|(_, document)| document.rows[0].cells[0].value.as_str()),
            Some("DISK-7")
        );
        drop(ws);
        std::fs::remove_dir_all(base).unwrap();
        socket_task.abort();
    }

    #[tokio::test]
    async fn watched_json_events_add_clear_and_restore_sibling_publications_without_fallback() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "vector-lsp-json-publication-{}-{nonce}",
            std::process::id()
        ));
        let data_root = base.join("mod/data");
        let excel_dir = data_root.join("global/excel");
        let strings_dir = data_root.join("local/lng/strings");
        let txt_path = excel_dir.join("skills.txt");
        let json_path = strings_dir.join("mod.json");
        std::fs::create_dir_all(&excel_dir).unwrap();
        std::fs::create_dir_all(&strings_dir).unwrap();
        std::fs::write(&txt_path, "name\nnone\n").unwrap();
        std::fs::write(&json_path, r#"[{"id":1,"Key":"A"},{"id":1,"Key":"A"}]"#).unwrap();

        // A lower-priority reference root also has JSON. Removing the local
        // file below must clear diagnostics instead of linting this fallback.
        let reference_excel = base.join("reference/data/global/excel");
        let reference_strings = base.join("reference/data/local/lng/strings");
        std::fs::create_dir_all(&reference_excel).unwrap();
        std::fs::create_dir_all(&reference_strings).unwrap();
        std::fs::write(
            reference_strings.join("reference.json"),
            r#"[{"id":2,"Key":"R"},{"id":2,"Key":"R"}]"#,
        )
        .unwrap();

        let workspace = Arc::new(RwLock::new(Workspace::new()));
        {
            let mut ws = workspace.write().await;
            ws.root_uri = Url::from_directory_path(&excel_dir).ok();
            ws.reference_root_uri = Url::from_directory_path(&reference_excel).ok();
            ws.reference_context_mode = ReferenceContextMode::Sibling;
            ws.session_generation = 41;
            ws.scan_generation = 3;
            ws.workspace_revision = 7;
            ws.phase = WorkspacePhase::Ready;
            let document = Arc::new(DocumentData::parse("name\nnone\n", '\t'));
            ws.file_cache
                .insert(txt_path.clone(), Arc::clone(&document));
            ws.open_documents
                .insert(Url::from_file_path(&txt_path).unwrap(), document);
        }

        let settings = Arc::new(VectorLspSettings {
            json_diagnostics: true,
            ..VectorLspSettings::default()
        });
        let publish_gates = Arc::new(Mutex::new(HashMap::new()));
        let workspace_for_service = Arc::clone(&workspace);
        let (service, mut socket) = tower_lsp::LspService::new(move |client| Backend {
            client,
            settings: Arc::clone(&settings),
            workspace: Arc::clone(&workspace_for_service),
            plugin_host: None,
            publish_gates: Arc::clone(&publish_gates),
        });
        let socket_task = tokio::spawn(async move { while socket.next().await.is_some() {} });

        assert!(
            service
                .inner()
                .validate_json_diagnostics_for_revision(3, 7, JsonAnalysisTrigger::All)
                .await
        );
        let json_uri = Url::from_file_path(&json_path).unwrap();
        assert_eq!(
            workspace.read().await.published_json_diagnostic_uris(),
            vec![json_uri.clone()]
        );

        std::fs::write(&json_path, "[]").unwrap();
        service
            .inner()
            .did_change_watched_files(DidChangeWatchedFilesParams {
                changes: vec![FileEvent::new(json_uri.clone(), FileChangeType::CHANGED)],
            })
            .await;
        wait_for_json_analysis_worker(&workspace).await;
        assert!(
            workspace
                .read()
                .await
                .published_json_diagnostic_uris()
                .is_empty()
        );

        std::fs::write(&json_path, r#"[{"id":1,"Key":"A"},{"id":1,"Key":"A"}]"#).unwrap();
        service
            .inner()
            .did_change_watched_files(DidChangeWatchedFilesParams {
                changes: vec![FileEvent::new(json_uri.clone(), FileChangeType::CHANGED)],
            })
            .await;
        wait_for_json_analysis_worker(&workspace).await;
        assert_eq!(
            workspace.read().await.published_json_diagnostic_uris(),
            vec![json_uri.clone()]
        );

        std::fs::remove_file(&json_path).unwrap();
        service
            .inner()
            .did_change_watched_files(DidChangeWatchedFilesParams {
                changes: vec![FileEvent::new(json_uri, FileChangeType::DELETED)],
            })
            .await;
        wait_for_json_analysis_worker(&workspace).await;
        assert!(
            workspace
                .read()
                .await
                .published_json_diagnostic_uris()
                .is_empty()
        );
        std::fs::write(&json_path, r#"[{"id":1,"Key":"A"},{"id":1,"Key":"A"}]"#).unwrap();
        service
            .inner()
            .did_change_watched_files(DidChangeWatchedFilesParams {
                changes: vec![FileEvent::new(
                    Url::from_file_path(&json_path).unwrap(),
                    FileChangeType::CREATED,
                )],
            })
            .await;
        wait_for_json_analysis_worker(&workspace).await;
        assert_eq!(
            workspace.read().await.published_json_diagnostic_uris(),
            vec![Url::from_file_path(&json_path).unwrap()]
        );
        std::fs::remove_dir_all(base).unwrap();
        socket_task.abort();
    }

    #[tokio::test]
    async fn open_save_event_is_suppressed_but_sibling_event_rescans_and_restores_latest_disk() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "vector-lsp-watched-txt-{}-{nonce}",
            std::process::id()
        ));
        let excel_dir = base.join("mod/data/global/excel");
        let txt_path = excel_dir.join("skills.txt");
        let sibling_path = excel_dir.join("itemtypes.txt");
        std::fs::create_dir_all(&excel_dir).unwrap();
        std::fs::write(&txt_path, "id\nDISK-OLD\n").unwrap();
        std::fs::write(&sibling_path, "id\nSIBLING-OLD\n").unwrap();
        let txt_uri = Url::from_file_path(&txt_path).unwrap();
        let sibling_uri = Url::from_file_path(&sibling_path).unwrap();

        let workspace = Arc::new(RwLock::new(Workspace::new()));
        {
            let mut ws = workspace.write().await;
            ws.root_uri = Url::from_directory_path(&excel_dir).ok();
            ws.reference_context_mode = ReferenceContextMode::Sibling;
            ws.session_generation = 51;
            ws.scan_generation = 2;
            ws.workspace_revision = 7;
            ws.phase = WorkspacePhase::Ready;
            ws.ref_targets
                .insert(("skills".to_string(), "id".to_string()));
            ws.ref_targets
                .insert(("itemtypes".to_string(), "id".to_string()));
            ws.set_workspace_present_paths([txt_path.clone(), sibling_path.clone()]);
            ws.file_cache.insert(
                txt_path.clone(),
                Arc::new(DocumentData::parse("id\nDISK-OLD\n", '\t')),
            );
            ws.file_cache.insert(
                sibling_path.clone(),
                Arc::new(DocumentData::parse("id\nSIBLING-OLD\n", '\t')),
            );
            ws.accept_open(
                txt_uri.clone(),
                1,
                Arc::new(DocumentData::parse("id\nOPEN\n", '\t')),
            );
            ws.rebuild_effective_symbols();
        }

        let settings = Arc::new(VectorLspSettings::default());
        let publish_gates = Arc::new(Mutex::new(HashMap::new()));
        let workspace_for_service = Arc::clone(&workspace);
        let (service, mut socket) = tower_lsp::LspService::new(move |client| Backend {
            client,
            settings: Arc::clone(&settings),
            workspace: Arc::clone(&workspace_for_service),
            plugin_host: None,
            publish_gates: Arc::clone(&publish_gates),
        });
        let socket_task = tokio::spawn(async move { while socket.next().await.is_some() {} });

        let scan_generation_before_save = workspace.read().await.scan_generation;
        std::fs::write(&txt_path, "id\nDISK-SAVED\n").unwrap();
        {
            let ws = workspace.read().await;
            assert!(!service.inner().is_watched_txt_path(&ws, &txt_path));
        }
        service
            .inner()
            .did_change_watched_files(DidChangeWatchedFilesParams {
                changes: vec![FileEvent::new(txt_uri.clone(), FileChangeType::CHANGED)],
            })
            .await;
        wait_for_watched_change_worker(&workspace).await;
        {
            let ws = workspace.read().await;
            assert_eq!(ws.scan_generation, scan_generation_before_save);
            assert_eq!(ws.file_cache[&txt_path].rows[0].cells[0].value, "DISK-OLD");
        }

        std::fs::write(&sibling_path, "id\nSIBLING-NEW\n").unwrap();
        {
            let ws = workspace.read().await;
            assert!(service.inner().is_watched_txt_path(&ws, &sibling_path));
        }
        service
            .inner()
            .did_change_watched_files(DidChangeWatchedFilesParams {
                changes: vec![FileEvent::new(sibling_uri.clone(), FileChangeType::CHANGED)],
            })
            .await;
        wait_for_watched_change_worker(&workspace).await;

        {
            let ws = workspace.read().await;
            let disk = ws
                .file_cache
                .iter()
                .find(|(path, _)| same_local_path(path, &txt_path))
                .map(|(_, document)| document)
                .unwrap_or_else(|| panic!("missing disk cache; keys={:?}", ws.file_cache.keys()));
            assert_eq!(ws.phase, WorkspacePhase::Ready);
            assert_eq!(ws.open_documents[&txt_uri].rows[0].cells[0].value, "OPEN");
            assert_eq!(disk.rows[0].cells[0].value, "DISK-SAVED");
            let sibling = ws
                .file_cache
                .iter()
                .find(|(path, _)| same_local_path(path, &sibling_path))
                .map(|(_, document)| document)
                .unwrap();
            assert_eq!(sibling.rows[0].cells[0].value, "SIBLING-NEW");
            assert!(ws.symbols.lookup("skills", "id", "OPEN").is_some());
            assert!(ws.symbols.lookup("skills", "id", "DISK-SAVED").is_none());
            assert!(ws.pending_open_tickets().is_empty());
        }

        service
            .inner()
            .did_close(DidCloseTextDocumentParams {
                text_document: TextDocumentIdentifier::new(txt_uri.clone()),
            })
            .await;
        {
            let ws = workspace.read().await;
            let disk = ws
                .file_cache
                .iter()
                .find(|(path, _)| same_local_path(path, &txt_path))
                .map(|(_, document)| document)
                .unwrap();
            assert!(!ws.open_documents.contains_key(&txt_uri));
            assert_eq!(disk.rows[0].cells[0].value, "DISK-SAVED");
            assert!(
                ws.symbols
                    .resolve("skills", "id", "DISK-SAVED", ReferenceResolver::AsciiCi)
                    .is_some()
            );
        }
        std::fs::remove_dir_all(base).unwrap();
        socket_task.abort();
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
            "referenceRootUri": "file:///E:/explicit-workspace",
            "includeSubfolders": false,
            "workspaceDirectoryScopes": true
        }));
        service.inner().initialize(params).await.unwrap();

        let ws = workspace.read().await;
        assert_eq!(ws.reference_context_mode, ReferenceContextMode::Sibling);
        assert_eq!(ws.session_generation, 42);
        assert!(!ws.include_subfolders);
        assert!(ws.workspace_directory_scopes);
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
