use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Url};

use crate::document::DocumentData;
use crate::i18n::{self, Locale};
use crate::runtime::{ScriptRuntime, WorkspaceFileSnapshot, WorkspaceIndex, WorkspaceSourceInfo};
use crate::schema::Schema;
#[cfg(test)]
use crate::source_selection::effective_workspace_sources;
use crate::source_selection::{
    EffectiveSource, SourceKind, effective_workspace_sources_with_fallback,
};

// ---------------------------------------------------------------------------
// Wire types for validate()
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawDiag {
    line: u32,
    col: u32,
    #[serde(rename = "endCol", default)]
    end_col: u32,
    /// "error" | "warning" | "info" | "hint" — defaults to "warning"
    #[serde(default)]
    severity: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(rename = "messageKey", default)]
    message_key: Option<String>,
    #[serde(rename = "messageArgs", default)]
    message_args: Option<serde_json::Map<String, Value>>,
    /// Compatibility text supplied by bundled plugins for enUS only. It is
    /// never analyzed or translated; keyed localization remains authoritative.
    #[serde(rename = "legacyMessage", default)]
    legacy_message: Option<String>,
    #[serde(default)]
    code: Option<NumberOrString>,
    #[serde(default)]
    data: Option<Value>,
}

impl RawDiag {
    fn into_lsp(self, locale: Locale) -> Diagnostic {
        let severity = match self.severity.as_str() {
            "error" => DiagnosticSeverity::ERROR,
            "info" | "information" => DiagnosticSeverity::INFORMATION,
            "hint" => DiagnosticSeverity::HINT,
            _ => DiagnosticSeverity::WARNING,
        };
        let end = if self.end_col > self.col {
            self.end_col
        } else {
            self.col
        };
        let diagnostic = Diagnostic {
            range: Range {
                start: Position {
                    line: self.line,
                    character: self.col,
                },
                end: Position {
                    line: self.line,
                    character: end,
                },
            },
            severity: Some(severity),
            code: self.code,
            source: Some("vector-lsp/plugin".into()),
            message: self.message.clone().unwrap_or_default(),
            data: self.data,
            ..Default::default()
        };
        match self.message_key {
            Some(key) => i18n::localized_plugin_diagnostic(
                locale,
                &key,
                self.message_args.unwrap_or_default(),
                self.legacy_message.or(self.message),
                diagnostic,
            ),
            None => diagnostic,
        }
    }
}

// ---------------------------------------------------------------------------
// Wire type for hover()
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawHover {
    #[serde(default)]
    content: Option<String>,
    #[serde(rename = "contentKey", default)]
    content_key: Option<String>,
    #[serde(rename = "contentArgs", default)]
    content_args: Option<serde_json::Map<String, Value>>,
    #[serde(rename = "legacyContent", default)]
    legacy_content: Option<String>,
}

// ---------------------------------------------------------------------------
// Wire type for gotoDefinition()
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawGotoTarget {
    #[serde(rename = "targetFile")]
    target_file: String,
    #[serde(rename = "targetCol")]
    target_col: String,
    #[serde(rename = "targetValue")]
    target_value: String,
}

#[derive(Deserialize)]
struct RawPluginValidation {
    plugin: String,
    diagnostics: Value,
    error: Option<String>,
}

#[derive(Deserialize)]
struct RawPluginIssue {
    plugin: String,
    error: String,
}

#[derive(Deserialize)]
struct RawPluginValueResult {
    value: Value,
    #[serde(default)]
    errors: Vec<RawPluginIssue>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPluginApplicability {
    validate: Vec<Option<Vec<String>>>,
    hover: Vec<Option<Vec<String>>>,
    goto_definition: Vec<Option<Vec<String>>>,
}

#[derive(Clone, Debug)]
enum OperationApplicability {
    All,
    Files(HashSet<String>),
}

impl OperationApplicability {
    fn from_plugins(plugins: Vec<Option<Vec<String>>>) -> Self {
        if plugins.iter().any(Option::is_none) {
            return Self::All;
        }
        Self::Files(
            plugins
                .into_iter()
                .flatten()
                .flatten()
                .map(|file| file.trim().to_lowercase())
                .filter(|file| !file.is_empty())
                .collect(),
        )
    }

    fn applies_to(&self, file_stem: &str) -> bool {
        match self {
            Self::All => true,
            Self::Files(files) => files.contains(&file_stem.trim().to_lowercase()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct PluginOperationCounts {
    validate: usize,
    hover: usize,
    goto_definition: usize,
}

#[derive(Clone, Debug)]
struct PluginApplicability {
    validate: OperationApplicability,
    hover: OperationApplicability,
    goto_definition: OperationApplicability,
    counts: PluginOperationCounts,
}

impl From<RawPluginApplicability> for PluginApplicability {
    fn from(raw: RawPluginApplicability) -> Self {
        let counts = PluginOperationCounts {
            validate: raw.validate.len(),
            hover: raw.hover.len(),
            goto_definition: raw.goto_definition.len(),
        };
        Self {
            validate: OperationApplicability::from_plugins(raw.validate),
            hover: OperationApplicability::from_plugins(raw.hover),
            goto_definition: OperationApplicability::from_plugins(raw.goto_definition),
            counts,
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin host (owns the JS runtime on a dedicated non-Send thread)
// ---------------------------------------------------------------------------

enum PluginRequest {
    SetSchema {
        schema: Arc<Schema>,
        reply: oneshot::Sender<()>,
    },
    Validate {
        ctx: String,
        index: Arc<WorkspaceIndex>,
        snapshot: Arc<WorkspaceFileSnapshot>,
        locale: Locale,
        reply: oneshot::Sender<Vec<Diagnostic>>,
    },
    Hover {
        ctx: Value,
        index: Arc<WorkspaceIndex>,
        snapshot: Arc<WorkspaceFileSnapshot>,
        locale: Locale,
        reply: oneshot::Sender<Option<String>>,
    },
    GotoDefinition {
        ctx: Value,
        index: Arc<WorkspaceIndex>,
        snapshot: Arc<WorkspaceFileSnapshot>,
        reply: oneshot::Sender<Option<(String, String, String)>>,
    },
}

/// Cheap, cloneable handle to the plugin evaluation thread.
/// The underlying thread is shared across all clones.
#[derive(Clone)]
pub struct PluginHost {
    tx: mpsc::Sender<PluginRequest>,
    health: Arc<PluginHealth>,
    applicability: Arc<PluginApplicability>,
}

#[derive(Default)]
struct PluginHealth {
    reported: Mutex<HashSet<String>>,
}

impl PluginHealth {
    fn report_once(&self, key: impl Into<String>, message: impl AsRef<str>) {
        if self.reported.lock().unwrap().insert(key.into()) {
            eprintln!("vector-lsp: {}", message.as_ref());
        }
    }
}

impl PluginHost {
    /// Spawn the dedicated plugin thread, load all plugin files, and return a handle.
    pub fn new(paths: Vec<PathBuf>) -> anyhow::Result<Self> {
        let (tx, mut rx) = mpsc::channel::<PluginRequest>(32);
        let health = Arc::new(PluginHealth::default());
        let thread_health = Arc::clone(&health);
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
        let execution_budget = plugin_execution_budget();
        let startup_budget = plugin_startup_budget(execution_budget, paths.len());

        std::thread::spawn(move || {
            let mut rt = match ScriptRuntime::new() {
                Ok(r) => r,
                Err(e) => {
                    let _ = startup_tx.send(Err(format!("plugin runtime init failed: {e}")));
                    return;
                }
            };

            // Seed plugin registries and host-provided utility functions.
            if let Err(error) = rt.exec(
                "__seed__",
                "var __plugins = []; var __hovers = []; var __gotos = [];\
                 var __activePluginName=null;\
                 var __requestCtx=null;\
                 var __lookupCache={};\
                 var __colCache={};\
                 var __cvCache={};\
                 var __firstLineCache={};\
                 var __filteredCvCache={};\
                 function lookupKey(file,col,value){\
                     var k=file+'|'+col+'|'+value;\
                     if(!(k in __lookupCache)){__lookupCache[k]=Deno.core.ops.op_lookup_key(file,col,value);}\
                     return __lookupCache[k];\
                 }\
                 function lookupKeyFixed4(file,col,value){\
                     return Deno.core.ops.op_lookup_key_fixed4(file,col,value);\
                 }\
                 function getColumn(file,col){\
                     var k=file+'|'+col;\
                     if(!(k in __colCache)){__colCache[k]=Deno.core.ops.op_get_column(file,col)||undefined;}\
                     return __colCache[k];\
                 }\
                 function hasFile(stem){\
                     return Deno.core.ops.op_has_file(stem);\
                 }\
                 function getWorkspaceSource(stem){\
                     return Deno.core.ops.op_get_workspace_source(stem);\
                 }\
                 function hasLookupTarget(file,col){\
                     return Deno.core.ops.op_has_lookup_target(file,col);\
                 }\
                 function getColumnValues(stem,col){\
                     var k=stem+'|'+col;\
                     if(!(k in __cvCache)){__cvCache[k]=Deno.core.ops.op_get_column_values(stem,col);}\
                     return __cvCache[k];\
                 }\
                 function getFirstColumnValueLine(stem,col,value){\
                     var k=stem+'|'+col+'|'+value;\
                     if(!(k in __firstLineCache)){var result=Deno.core.ops.op_get_first_column_value_line(stem,col,value);__firstLineCache[k]=result===null?null:result.line;}\
                     return __firstLineCache[k];\
                 }\
                 function getFilteredColumnValues(stem,valueCol,filterCol,filterValue){\
                     var k=stem+'|'+valueCol+'|'+filterCol+'|'+filterValue;\
                     if(!(k in __filteredCvCache)){__filteredCvCache[k]=Deno.core.ops.op_get_filtered_column_values(stem,valueCol,filterCol,filterValue);}\
                     return __filteredCvCache[k];\
                 }\
                 function getEnumTable(file,col){\
                     return Deno.core.ops.op_get_enum_table(file,col)||null;\
                 }\
                 function __pluginFiles(metadata,key){\
                     try{\
                         var files=metadata&&metadata[key];\
                         if(!Array.isArray(files))return null;\
                         return files.map(function(file){return String(file).trim().toLowerCase();});\
                     }catch(error){return null;}\
                 }",
            ) {
                let _ = startup_tx.send(Err(format!("plugin runtime seed failed: {error}")));
                return;
            }

            for path in &paths {
                if let Err(error) = load_plugin(&mut rt, path, execution_budget) {
                    let _ = startup_tx.send(Err(format!(
                        "plugin '{}' failed to load: {error}",
                        path.display()
                    )));
                    return;
                }
            }
            let (applicability, metadata_timed_out) = eval_json_with_budget(
                &mut rt,
                "({validate:__plugins.map(function(plugin){return plugin.files;}),\
                   hover:__hovers.map(function(plugin){return plugin.files;}),\
                   gotoDefinition:__gotos.map(function(plugin){return plugin.files;})})",
                execution_budget,
            );
            let applicability = if metadata_timed_out {
                Err("plugin applicability metadata exceeded the execution budget".to_string())
            } else {
                applicability
                    .and_then(|value| Ok(serde_json::from_value::<RawPluginApplicability>(value)?))
                    .map(PluginApplicability::from)
                    .map_err(|error| format!("plugin applicability metadata failed: {error}"))
            };
            let operation_counts = applicability
                .as_ref()
                .map(|value| value.counts)
                .unwrap_or_default();
            if startup_tx.send(applicability).is_err() {
                return;
            }

            // Tracks the last snapshot seen so the column-value cache can be
            // invalidated exactly when the workspace data changes.
            let mut last_snapshot_ptr: usize = 0;

            while let Some(req) = rx.blocking_recv() {
                match req {
                    PluginRequest::SetSchema { schema, reply } => {
                        rt.set_schema(schema);
                        let _ = reply.send(());
                    }
                    PluginRequest::Validate {
                        ctx,
                        index,
                        snapshot,
                        locale,
                        reply,
                    } => {
                        install_workspace_view(&mut rt, &mut last_snapshot_ptr, index, snapshot);
                        let diags = run_validation_plugins(
                            &mut rt,
                            &thread_health,
                            ctx,
                            operation_counts.validate,
                            execution_budget,
                            locale,
                        );
                        let _ = reply.send(diags);
                    }
                    PluginRequest::Hover {
                        ctx,
                        index,
                        snapshot,
                        locale,
                        reply,
                    } => {
                        install_workspace_view(&mut rt, &mut last_snapshot_ptr, index, snapshot);
                        let ctx_json = ctx.to_string();
                        let debug = std::env::var("VLSP_DEBUG_LOGGING").is_ok();
                        if debug {
                            eprintln!("[hover-debug] ctx={ctx_json}");
                        }
                        let result = run_hover_plugins(
                            &mut rt,
                            &thread_health,
                            ctx_json,
                            operation_counts.hover,
                            execution_budget,
                            locale,
                        );
                        if debug {
                            eprintln!("[hover-debug] result={result:?}");
                        }
                        let _ = reply.send(result);
                    }
                    PluginRequest::GotoDefinition {
                        ctx,
                        index,
                        snapshot,
                        reply,
                    } => {
                        install_workspace_view(&mut rt, &mut last_snapshot_ptr, index, snapshot);
                        let result = run_goto_definition_plugins(
                            &mut rt,
                            &thread_health,
                            ctx.to_string(),
                            operation_counts.goto_definition,
                            execution_budget,
                        );
                        let _ = reply.send(result);
                    }
                }
            }
        });

        let applicability = match startup_rx.recv_timeout(startup_budget) {
            Ok(result) => result.map_err(anyhow::Error::msg)?,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                anyhow::bail!(
                    "plugin runtime startup exceeded its {:?} deadline",
                    startup_budget
                )
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                anyhow::bail!("plugin runtime startup channel closed")
            }
        };
        Ok(Self {
            tx,
            health,
            applicability: Arc::new(applicability),
        })
    }

    pub fn validates_file(&self, file_stem: &str) -> bool {
        self.applicability.validate.applies_to(file_stem)
    }

    pub fn hovers_file(&self, file_stem: &str) -> bool {
        self.applicability.hover.applies_to(file_stem)
    }

    pub fn defines_file(&self, file_stem: &str) -> bool {
        self.applicability.goto_definition.applies_to(file_stem)
    }

    /// Push the loaded schema to the plugin thread so `getEnumTable` has data.
    /// Return only after the worker has installed it, so callers can measure the
    /// completed schema phase rather than just the queue insertion.
    pub async fn set_schema(&self, schema: Arc<Schema>) {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .tx
            .send(PluginRequest::SetSchema {
                schema,
                reply: reply_tx,
            })
            .await
            .is_err()
        {
            self.health.report_once(
                "channel-closed",
                "plugin runtime channel closed while setting schema",
            );
            return;
        }
        if reply_rx.await.is_err() {
            self.health.report_once(
                "reply-closed",
                "plugin runtime closed the schema reply channel",
            );
        }
    }

    /// Run all `validate` plugin functions and return any diagnostics.
    #[cfg(test)]
    pub async fn run(
        &self,
        ctx: String,
        index: Arc<WorkspaceIndex>,
        snapshot: Arc<WorkspaceFileSnapshot>,
    ) -> Vec<Diagnostic> {
        self.run_localized(ctx, index, snapshot, Locale::EnUs).await
    }

    /// Run plugins and render their keyed product messages for one LSP
    /// session.  The locale travels with the request rather than mutating the
    /// dedicated runtime shared by cloned handles.
    pub async fn run_localized(
        &self,
        ctx: String,
        index: Arc<WorkspaceIndex>,
        snapshot: Arc<WorkspaceFileSnapshot>,
        locale: Locale,
    ) -> Vec<Diagnostic> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .tx
            .send(PluginRequest::Validate {
                ctx,
                index,
                snapshot,
                locale,
                reply: reply_tx,
            })
            .await
            .is_err()
        {
            self.health.report_once(
                "channel-closed",
                "plugin runtime channel closed before validate",
            );
            return vec![];
        }
        match reply_rx.await {
            Ok(diagnostics) => diagnostics,
            Err(_) => {
                self.health.report_once(
                    "reply-closed",
                    "plugin runtime closed the validate reply channel",
                );
                vec![]
            }
        }
    }

    /// Run all `hover` plugin functions and return the first non-null markdown content.
    #[cfg(test)]
    pub async fn hover(
        &self,
        ctx: Value,
        index: Arc<WorkspaceIndex>,
        snapshot: Arc<WorkspaceFileSnapshot>,
    ) -> Option<String> {
        self.hover_localized(ctx, index, snapshot, Locale::EnUs)
            .await
    }

    pub async fn hover_localized(
        &self,
        ctx: Value,
        index: Arc<WorkspaceIndex>,
        snapshot: Arc<WorkspaceFileSnapshot>,
        locale: Locale,
    ) -> Option<String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .tx
            .send(PluginRequest::Hover {
                ctx,
                index,
                snapshot,
                locale,
                reply: reply_tx,
            })
            .await
            .is_err()
        {
            self.health.report_once(
                "channel-closed",
                "plugin runtime channel closed before hover",
            );
            return None;
        }
        match reply_rx.await {
            Ok(result) => result,
            Err(_) => {
                self.health.report_once(
                    "reply-closed",
                    "plugin runtime closed the hover reply channel",
                );
                None
            }
        }
    }

    /// Run all `gotoDefinition` plugin functions and return the first non-null
    /// `(target_file, target_col, target_value)` triple.
    pub async fn goto_definition(
        &self,
        ctx: Value,
        index: Arc<WorkspaceIndex>,
        snapshot: Arc<WorkspaceFileSnapshot>,
    ) -> Option<(String, String, String)> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .tx
            .send(PluginRequest::GotoDefinition {
                ctx,
                index,
                snapshot,
                reply: reply_tx,
            })
            .await
            .is_err()
        {
            self.health.report_once(
                "channel-closed",
                "plugin runtime channel closed before gotoDefinition",
            );
            return None;
        }
        match reply_rx.await {
            Ok(result) => result,
            Err(_) => {
                self.health.report_once(
                    "reply-closed",
                    "plugin runtime closed the gotoDefinition reply channel",
                );
                None
            }
        }
    }
}

fn install_workspace_view(
    runtime: &mut ScriptRuntime,
    last_snapshot_ptr: &mut usize,
    index: Arc<WorkspaceIndex>,
    snapshot: Arc<WorkspaceFileSnapshot>,
) {
    let snapshot_ptr = Arc::as_ptr(&snapshot) as usize;
    if snapshot_ptr != *last_snapshot_ptr {
        let _ = runtime.exec(
            "__cache_reset__",
            "var __lookupCache={}; var __colCache={}; var __cvCache={}; var __firstLineCache={}; var __filteredCvCache={};",
        );
        *last_snapshot_ptr = snapshot_ptr;
    }
    runtime.set_workspace_index(index);
    runtime.set_workspace_snapshot(snapshot);
}

const VALIDATION_PLUGIN_EXPRESSION: &str = "(function(){\
    var plugin=__plugins[__INDEX__];__activePluginName=plugin.name;\
    if(plugin.files!==null&&plugin.files.indexOf(String(__requestCtx.file).toLowerCase())<0){__activePluginName=null;return [];}\
    try{var diagnostics=plugin.fn(__requestCtx);__activePluginName=null;return [{plugin:plugin.name,diagnostics:diagnostics==null?[]:diagnostics,error:null}];}\
    catch(error){__activePluginName=null;return [{plugin:plugin.name,diagnostics:[],error:String(error)}];}\
})()";

const PLUGIN_VALUE_EXPRESSION: &str = "(function(){\
    var plugin=__REGISTRY__[__INDEX__];__activePluginName=plugin.name;\
    if(plugin.files!==null&&plugin.files.indexOf(String(__requestCtx.file).toLowerCase())<0){__activePluginName=null;return {value:null,errors:[]};}\
    try{var value=plugin.fn(__requestCtx);__activePluginName=null;return {value:value==null?null:value,errors:[]};}\
    catch(error){__activePluginName=null;return {value:null,errors:[{plugin:plugin.name,error:String(error)}]};}\
})()";

fn prepare_plugin_context(
    runtime: &mut ScriptRuntime,
    health: &PluginHealth,
    operation: &str,
    ctx_json: String,
    budget: Duration,
) -> bool {
    runtime.set_ctx_json(ctx_json);
    let (result, timed_out) = eval_json_with_budget(
        runtime,
        "(function(){__requestCtx=JSON.parse(Deno.core.ops.op_get_ctx_json());return null;})()",
        budget,
    );
    if timed_out {
        health.report_once(
            format!("{operation}-context-timeout"),
            format!("plugin {operation} context preparation exceeded the execution budget"),
        );
        clear_plugin_request_state(runtime);
        return false;
    }
    if let Err(error) = result {
        health.report_once(
            format!("{operation}-context"),
            format!("plugin {operation} context preparation failed: {error}"),
        );
        clear_plugin_request_state(runtime);
        return false;
    }
    true
}

fn clear_plugin_request_state(runtime: &mut ScriptRuntime) {
    let _ = runtime.exec(
        "__plugin_request_cleanup__",
        "__activePluginName=null;__requestCtx=null;",
    );
}

fn validation_plugin_expression(plugin_index: usize) -> String {
    VALIDATION_PLUGIN_EXPRESSION.replace("__INDEX__", &plugin_index.to_string())
}

fn plugin_value_expression(registry: &str, plugin_index: usize) -> String {
    PLUGIN_VALUE_EXPRESSION
        .replace("__REGISTRY__", registry)
        .replace("__INDEX__", &plugin_index.to_string())
}

fn run_validation_plugins(
    runtime: &mut ScriptRuntime,
    health: &PluginHealth,
    ctx_json: String,
    plugin_count: usize,
    budget: Duration,
    locale: Locale,
) -> Vec<Diagnostic> {
    if !prepare_plugin_context(runtime, health, "validate", ctx_json, budget) {
        return vec![];
    }
    let mut diagnostics = Vec::new();
    for plugin_index in 0..plugin_count {
        let expression = validation_plugin_expression(plugin_index);
        let (raw, timed_out) = eval_json_with_budget(runtime, &expression, budget);
        if timed_out {
            report_plugin_timeout(runtime, health, "validate");
            continue;
        }
        diagnostics.extend(validation_results(raw, health, locale));
    }
    clear_plugin_request_state(runtime);
    diagnostics
}

fn run_hover_plugins(
    runtime: &mut ScriptRuntime,
    health: &PluginHealth,
    ctx_json: String,
    plugin_count: usize,
    budget: Duration,
    locale: Locale,
) -> Option<String> {
    if !prepare_plugin_context(runtime, health, "hover", ctx_json, budget) {
        return None;
    }
    let mut result = None;
    for plugin_index in 0..plugin_count {
        let expression = plugin_value_expression("__hovers", plugin_index);
        let (raw, timed_out) = eval_json_with_budget(runtime, &expression, budget);
        if timed_out {
            report_plugin_timeout(runtime, health, "hover");
            continue;
        }
        let Some(value) = plugin_value_result(raw, health, "hover") else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        if let Some(content) = hover_value(value, health, locale) {
            result = Some(content);
            break;
        }
    }
    clear_plugin_request_state(runtime);
    result
}

fn hover_value(value: Value, health: &PluginHealth, locale: Locale) -> Option<String> {
    match value {
        value @ Value::Object(_) => match serde_json::from_value::<RawHover>(value) {
            Ok(hover) => match hover.content_key {
                Some(key) => Some(
                    i18n::localized_plugin_diagnostic(
                        locale,
                        &key,
                        hover.content_args.unwrap_or_default(),
                        hover.legacy_content.or(hover.content),
                        Diagnostic::default(),
                    )
                    .message,
                ),
                None => hover.content,
            },
            Err(error) => {
                health.report_once(
                    "hover-shape",
                    format!("plugin hover result has invalid shape: {error}"),
                );
                None
            }
        },
        Value::String(content) => Some(content),
        _ => {
            health.report_once("hover-shape", "plugin hover result has invalid shape");
            None
        }
    }
}

fn run_goto_definition_plugins(
    runtime: &mut ScriptRuntime,
    health: &PluginHealth,
    ctx_json: String,
    plugin_count: usize,
    budget: Duration,
) -> Option<(String, String, String)> {
    if !prepare_plugin_context(runtime, health, "gotoDefinition", ctx_json, budget) {
        return None;
    }
    let mut result = None;
    for plugin_index in 0..plugin_count {
        let expression = plugin_value_expression("__gotos", plugin_index);
        let (raw, timed_out) = eval_json_with_budget(runtime, &expression, budget);
        if timed_out {
            report_plugin_timeout(runtime, health, "gotoDefinition");
            continue;
        }
        let Some(value) = plugin_value_result(raw, health, "gotoDefinition") else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        if let Some(target) = goto_definition_value(value, health) {
            result = Some(target);
            break;
        }
    }
    clear_plugin_request_state(runtime);
    result
}

fn goto_definition_value(value: Value, health: &PluginHealth) -> Option<(String, String, String)> {
    match value {
        value @ Value::Object(_) => match serde_json::from_value::<RawGotoTarget>(value) {
            Ok(target) => Some((target.target_file, target.target_col, target.target_value)),
            Err(error) => {
                health.report_once(
                    "goto-shape",
                    format!("plugin gotoDefinition result has invalid shape: {error}"),
                );
                None
            }
        },
        _ => {
            health.report_once(
                "goto-shape",
                "plugin gotoDefinition result has invalid shape",
            );
            None
        }
    }
}

fn plugin_execution_budget() -> Duration {
    let milliseconds = std::env::var("VLSP_PLUGIN_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(750)
        .clamp(10, 60_000);
    Duration::from_millis(milliseconds)
}

fn plugin_startup_budget(execution_budget: Duration, plugin_count: usize) -> Duration {
    let budgeted_steps = plugin_count.saturating_add(3).min(u32::MAX as usize) as u32;
    let aggregate_budget = execution_budget
        .checked_mul(budgeted_steps)
        .unwrap_or(Duration::MAX);
    aggregate_budget.max(Duration::from_secs(5))
}

fn exec_with_budget(
    runtime: &mut ScriptRuntime,
    name: &'static str,
    source: String,
    budget: Duration,
) -> (anyhow::Result<()>, bool) {
    let isolate = runtime.execution_handle();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let watchdog = std::thread::spawn(move || {
        if done_rx.recv_timeout(budget).is_err() {
            isolate.terminate_execution();
            true
        } else {
            false
        }
    });
    let result = runtime.exec(name, source);
    let _ = done_tx.send(());
    let timed_out = watchdog.join().unwrap_or(true);
    if timed_out {
        runtime.cancel_terminate_execution();
    }
    (result, timed_out)
}

fn eval_json_with_budget(
    runtime: &mut ScriptRuntime,
    expression: &str,
    budget: Duration,
) -> (anyhow::Result<Value>, bool) {
    let isolate = runtime.execution_handle();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let watchdog = std::thread::spawn(move || {
        if done_rx.recv_timeout(budget).is_err() {
            isolate.terminate_execution();
            true
        } else {
            false
        }
    });
    let result = runtime.eval_json(expression);
    let _ = done_tx.send(());
    let timed_out = watchdog.join().unwrap_or(true);
    if timed_out {
        runtime.cancel_terminate_execution();
    }
    (result, timed_out)
}

fn report_plugin_timeout(runtime: &mut ScriptRuntime, health: &PluginHealth, operation: &str) {
    let plugin = runtime
        .eval_json("__activePluginName")
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown plugin".to_string());
    health.report_once(
        format!("timeout:{operation}:{plugin}"),
        format!("plugin '{plugin}' {operation} exceeded the execution budget and was interrupted"),
    );
    let _ = runtime.exec("__plugin_timeout_cleanup__", "__activePluginName=null;");
}

fn validation_results(
    raw: anyhow::Result<Value>,
    health: &PluginHealth,
    locale: Locale,
) -> Vec<Diagnostic> {
    let runs = match raw.and_then(|value| {
        serde_json::from_value::<Vec<RawPluginValidation>>(value).map_err(Into::into)
    }) {
        Ok(runs) => runs,
        Err(error) => {
            health.report_once(
                "validate-host-result",
                format!("plugin validate host result failed: {error}"),
            );
            return vec![];
        }
    };
    let mut diagnostics = Vec::new();
    for run in runs {
        if let Some(error) = run.error {
            health.report_once(
                format!("validate-error:{}", run.plugin),
                format!("plugin '{}' validate failed: {error}", run.plugin),
            );
            continue;
        }
        match serde_json::from_value::<Vec<RawDiag>>(run.diagnostics) {
            Ok(raw_diagnostics) => diagnostics.extend(
                raw_diagnostics
                    .into_iter()
                    .map(|diagnostic| diagnostic.into_lsp(locale)),
            ),
            Err(error) => health.report_once(
                format!("validate-shape:{}", run.plugin),
                format!(
                    "plugin '{}' returned an invalid diagnostic shape: {error}",
                    run.plugin
                ),
            ),
        }
    }
    diagnostics
}

fn plugin_value_result(
    raw: anyhow::Result<Value>,
    health: &PluginHealth,
    operation: &str,
) -> Option<Value> {
    let result = match raw
        .and_then(|value| serde_json::from_value::<RawPluginValueResult>(value).map_err(Into::into))
    {
        Ok(result) => result,
        Err(error) => {
            health.report_once(
                format!("{operation}-host-result"),
                format!("plugin {operation} host result failed: {error}"),
            );
            return None;
        }
    };
    for issue in result.errors {
        health.report_once(
            format!("{operation}-error:{}", issue.plugin),
            format!(
                "plugin '{}' {operation} failed: {}",
                issue.plugin, issue.error
            ),
        );
    }
    Some(result.value)
}

fn load_plugin(rt: &mut ScriptRuntime, path: &Path, budget: Duration) -> anyhow::Result<()> {
    let src = std::fs::read_to_string(path)?;
    let src = strip_typescript(&src);
    let name = serde_json::to_string(&path.display().to_string())?;
    let wrapped = format!(
        "(function(){{\n{src}\n\
         var __pluginName={name};\
         var __pluginMetadata=typeof pluginMetadata==='object'&&pluginMetadata!==null?pluginMetadata:null;\
         if(typeof validate==='function'){{__plugins.push({{name:__pluginName,fn:validate,files:__pluginFiles(__pluginMetadata,'validateFiles')}});}}\
         if(typeof hover==='function'){{__hovers.push({{name:__pluginName,fn:hover,files:__pluginFiles(__pluginMetadata,'hoverFiles')}});}}\
         if(typeof gotoDefinition==='function'){{__gotos.push({{name:__pluginName,fn:gotoDefinition,files:__pluginFiles(__pluginMetadata,'gotoDefinitionFiles')}});}}\
         }})();"
    );
    let (result, timed_out) = exec_with_budget(rt, "<plugin>", wrapped, budget);
    if timed_out {
        anyhow::bail!("top-level/metadata evaluation exceeded the execution budget");
    }
    result?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Context construction
// ---------------------------------------------------------------------------

/// Build the JSON context string passed to plugin `validate` functions.
///
/// Shape: `{"file":"cubemain","headers":[...],"rows":[{"__line":1,"__colstarts":{...},...}]}`
///
/// Serialised directly to a `String` to avoid constructing an intermediate
/// `serde_json::Value` tree and then re-serialising it — for a 20k-row document
/// that would otherwise allocate millions of temporary strings.
pub fn build_context(file_stem: &str, doc: &DocumentData) -> String {
    let col_count = doc.headers.len();
    let row_count = doc.rows.len();
    // Rough capacity estimate: ~100 bytes overhead + ~60 bytes per (row × col).
    let capacity = 128 + row_count * col_count * 60;
    let mut s = String::with_capacity(capacity);

    s.push_str("{\"file\":");
    push_json_str(&mut s, file_stem);
    s.push_str(",\"headers\":[");
    for (i, h) in doc.headers.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        push_json_str(&mut s, h);
    }
    s.push_str("],\"rows\":[");
    let mut emitted_rows = 0usize;
    for row in &doc.rows {
        if row
            .cells
            .first()
            .map(|cell| cell.value.trim_start().starts_with('*'))
            .unwrap_or(false)
        {
            continue;
        }
        if emitted_rows > 0 {
            s.push(',');
        }
        emitted_rows += 1;
        s.push_str("{\"__line\":");
        s.push_str(&row.line.to_string());
        s.push_str(",\"__colstarts\":{");
        let mut first_cs = true;
        for (i, cell) in row.cells.iter().enumerate() {
            if let Some(h) = doc.headers.get(i) {
                if !h.is_empty() {
                    if !first_cs {
                        s.push(',');
                    }
                    first_cs = false;
                    push_json_str(&mut s, h);
                    s.push(':');
                    s.push_str(&cell.col_start.to_string());
                }
            }
        }
        s.push('}');
        for (i, cell) in row.cells.iter().enumerate() {
            if let Some(h) = doc.headers.get(i) {
                if !h.is_empty() {
                    s.push(',');
                    push_json_str(&mut s, h);
                    s.push(':');
                    push_json_str(&mut s, &cell.value);
                }
            }
        }
        s.push('}');
    }
    s.push_str("]}");
    s
}

/// Append a JSON-escaped string literal (with surrounding quotes) to `buf`.
fn push_json_str(buf: &mut String, v: &str) {
    buf.push('"');
    for ch in v.chars() {
        match ch {
            '"' => buf.push_str("\\\""),
            '\\' => buf.push_str("\\\\"),
            '\n' => buf.push_str("\\n"),
            '\r' => buf.push_str("\\r"),
            '\t' => buf.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write;
                let _ = write!(buf, "\\u{:04x}", c as u32);
            }
            c => buf.push(c),
        }
    }
    buf.push('"');
}

/// Build the context object passed to plugin `hover` functions.
///
/// Shape:
/// ```json
/// {
///   "file": "cubemain",
///   "col": "numinputs",
///   "value": "3",
///   "rowLine": 5,
///   "row": { "enabled": "1", "numinputs": "3", ... }
/// }
/// ```
/// Workspace data is NOT included; plugins access it via `getWorkspaceFile(stem)`.
pub fn build_hover_context(
    file_stem: &str,
    col_name: &str,
    cell_value: &str,
    row_line: u32,
    doc: &DocumentData,
) -> Value {
    let row = doc.rows.iter().find(|r| r.line == row_line);
    let row_obj: serde_json::Map<String, Value> = row
        .map(|r| {
            r.cells
                .iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    doc.headers
                        .get(i)
                        .filter(|h| !h.is_empty())
                        .map(|h| (h.clone(), json!(c.value)))
                })
                .collect()
        })
        .unwrap_or_default();

    json!({
        "file": file_stem,
        "col": col_name,
        "value": cell_value,
        "rowLine": row_line,
        "row": Value::Object(row_obj),
    })
}

/// Build a per-file snapshot for plugin ops.
/// Open documents shadow file-cache entries for the same stem.
/// No serialization happens here — ops serialize only the data they need on demand.
#[cfg(test)]
pub fn build_workspace_snapshot(
    open_docs: &HashMap<Url, Arc<DocumentData>>,
    file_cache: &HashMap<PathBuf, Arc<DocumentData>>,
) -> Arc<WorkspaceFileSnapshot> {
    let mut snap = WorkspaceFileSnapshot::new();
    for source in effective_workspace_sources(open_docs, file_cache) {
        let kind = match source.kind {
            SourceKind::Open => "open",
            SourceKind::Workspace => "workspace",
            SourceKind::Bundled => "bundled",
            SourceKind::Sibling => "sibling",
        };
        snap.sources.insert(
            source.stem.clone(),
            WorkspaceSourceInfo {
                kind: kind.to_string(),
                version: source.bundled_version.clone(),
            },
        );
        snap.files.insert(source.stem, source.document);
    }

    Arc::new(snap)
}

pub fn build_workspace_snapshot_with_fallback(
    open_docs: &HashMap<Url, Arc<DocumentData>>,
    file_cache: &HashMap<PathBuf, Arc<DocumentData>>,
    fallback_cache: &HashMap<String, Arc<DocumentData>>,
    workspace_present_stems: &HashSet<String>,
    fallback_version: Option<&str>,
) -> Arc<WorkspaceFileSnapshot> {
    let mut snapshot = WorkspaceFileSnapshot::new();
    for source in effective_workspace_sources_with_fallback(
        open_docs,
        file_cache,
        fallback_cache,
        workspace_present_stems,
        fallback_version,
    ) {
        let kind = match source.kind {
            SourceKind::Open => "open",
            SourceKind::Workspace => "workspace",
            SourceKind::Bundled => "bundled",
            SourceKind::Sibling => "sibling",
        };
        snapshot.sources.insert(
            source.stem.clone(),
            WorkspaceSourceInfo {
                kind: kind.to_string(),
                version: source.bundled_version.clone(),
            },
        );
        snapshot.files.insert(source.stem, source.document);
    }
    Arc::new(snapshot)
}

pub fn build_workspace_snapshot_from_sources(
    sources: &[EffectiveSource],
) -> Arc<WorkspaceFileSnapshot> {
    let mut snapshot = WorkspaceFileSnapshot::new();
    for source in sources {
        let kind = match source.kind {
            SourceKind::Open => "open",
            SourceKind::Workspace => "workspace",
            SourceKind::Sibling => "sibling",
            SourceKind::Bundled => "bundled",
        };
        snapshot.sources.insert(
            source.stem.clone(),
            WorkspaceSourceInfo {
                kind: kind.to_string(),
                version: source.bundled_version.clone(),
            },
        );
        snapshot
            .files
            .insert(source.stem.clone(), Arc::clone(&source.document));
    }
    Arc::new(snapshot)
}

// ---------------------------------------------------------------------------
// TypeScript preprocessor
// ---------------------------------------------------------------------------
// Two-pass approach:
//   Pass 1 — structural: removes `interface Foo { ... }` and `type Foo = ...`
//             declarations, preserving line numbers.
//   Pass 2 — inline:     strips `: Type` annotations and `as Type` casts from
//             executable code using a character-level scanner.
//
// Known remaining limitations (uncommon in plugin code):
//   - Generic type parameters on functions: `function f<T>()` — the `<T>` is
//     not stripped.  Avoid or pre-compile.
//   - Brace counting inside removed structural blocks ignores strings/comments.

/// Full TypeScript → JavaScript preprocessor.  Chains structural stripping then
/// inline annotation stripping.
fn strip_typescript(src: &str) -> String {
    strip_ts_inline(&strip_ts_declarations(src))
}
// --- Pass 2: inline annotation stripping ------------------------------------

/// Strip inline TypeScript annotations from already-structurally-cleaned source.
fn strip_ts_inline(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;

    let mut brace_depth: usize = 0;
    // Per-paren-depth ternary tracking: entry k is true if a `?` was seen at
    // paren depth k without a matching `:` yet.
    let mut ternary: Vec<bool> = vec![false]; // index 0 = global scope
    // Track parens opened since the most recent `{`. Resets to 0 when a `{`
    // opens (saved on a stack) and restores when `}` closes.  When this is > 0
    // we are in a function-param / arrow-param context above the current brace
    // level, so `:` annotations should be stripped even at deep brace nesting.
    let mut paren_above_brace: usize = 0;
    let mut brace_paren_stack: Vec<usize> = Vec::new();

    while i < n {
        let ch = chars[i];

        // ---- Verbatim regions (strings, template literals, comments) --------
        if ch == '"' || ch == '\'' {
            i = copy_str_lit(&chars, i, &mut out);
            continue;
        }
        if ch == '`' {
            i = copy_template_lit(&chars, i, &mut out);
            continue;
        }
        if i + 1 < n && ch == '/' && chars[i + 1] == '/' {
            while i < n && chars[i] != '\n' {
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }
        if i + 1 < n && ch == '/' && chars[i + 1] == '*' {
            out.push('/');
            out.push('*');
            i += 2;
            while i + 1 < n && !(chars[i] == '*' && chars[i + 1] == '/') {
                out.push(chars[i]);
                i += 1;
            }
            if i + 1 < n {
                out.push('*');
                out.push('/');
                i += 2;
            }
            continue;
        }

        // ---- Depth bookkeeping ----------------------------------------------
        if ch == '{' {
            brace_paren_stack.push(paren_above_brace);
            paren_above_brace = 0;
            brace_depth += 1;
            out.push(ch);
            i += 1;
            continue;
        }
        if ch == '}' && brace_depth > 0 {
            brace_depth -= 1;
            paren_above_brace = brace_paren_stack.pop().unwrap_or(0);
            out.push(ch);
            i += 1;
            continue;
        }
        if ch == '(' {
            paren_above_brace += 1;
            ternary.push(false);
            out.push(ch);
            i += 1;
            continue;
        }
        if ch == ')' {
            if paren_above_brace > 0 {
                paren_above_brace -= 1;
            }
            if ternary.len() > 1 {
                ternary.pop();
            }
            out.push(ch);
            i += 1;
            continue;
        }

        // ---- `?` — ternary, optional chaining, optional parameter, or `??` ----
        if ch == '?' {
            let next = chars.get(i + 1).copied();
            if next == Some(':') {
                // Optional parameter `x?:` — strip the `?`; `:` handled below
                i += 1;
                continue;
            }
            if next == Some('.') {
                // Optional chaining `?.` — keep as-is
                out.push(ch);
                i += 1;
                continue;
            }
            if next == Some('?') {
                // Nullish coalescing `??` (or `??=`) — not a ternary; consume both `?`s
                out.push(ch);
                out.push('?');
                i += 2;
                continue;
            }
            // Ternary `?`
            if let Some(top) = ternary.last_mut() {
                *top = true;
            }
            out.push(ch);
            i += 1;
            continue;
        }

        // ---- `:` — type annotation or ternary colon -------------------------
        if ch == ':' {
            // If a pending ternary `?` exists at this depth it's the ternary colon.
            if *ternary.last().unwrap_or(&false) {
                if let Some(top) = ternary.last_mut() {
                    *top = false;
                }
                out.push(ch);
                i += 1;
                continue;
            }

            // Use the OUTPUT for the previous-char check so that stripped tokens
            // (e.g. the `?` in `x?:`) don't confuse the context detection.
            let prev_out = out
                .chars()
                .rev()
                .find(|c| !matches!(*c, ' ' | '\t' | '\n' | '\r'));
            // Strip when at top level, inside parens opened since the last `{`
            // (function/arrow params), or immediately after `)` (return-type
            // annotation — `): Type` is never a valid JS object-literal colon).
            let depth_ok = brace_depth == 0 || paren_above_brace > 0 || prev_out == Some(')');
            let prev_ok = prev_out.map_or(false, |c| is_id(c) || matches!(c, ')' | ']' | '>'));
            // What follows `:` must look like a type start
            let next_ok = chars[i + 1..]
                .iter()
                .find(|&&c| c != ' ' && c != '\t')
                .map_or(false, |&c| {
                    c.is_alphabetic() || c == '_' || matches!(c, '(' | '[' | '{' | '"' | '\'')
                });

            // Also strip `const x: T`, `let x: T`, `var x: T` inside function
            // bodies where depth_ok would otherwise be false.
            let var_decl_ok = !depth_ok && prev_out.map_or(false, is_id) && {
                let trimmed = out.trim_end_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r'));
                let before_id = trimmed.trim_end_matches(|c: char| is_id(c));
                let before_id =
                    before_id.trim_end_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r'));
                let kw_start = before_id
                    .rfind(|c: char| !is_id(c))
                    .map(|i| i + 1)
                    .unwrap_or(0);
                matches!(&before_id[kw_start..], "const" | "let" | "var")
            };

            if (depth_ok || var_decl_ok) && prev_ok && next_ok {
                let is_return = prev_out == Some(')');
                i += 1; // skip `:`
                i = skip_ws(&chars, i);
                i = skip_type_expr(&chars, i, is_return);
                continue;
            }
        }

        // ---- `as Type` casts ------------------------------------------------
        if ch == 'a'
            && i + 2 < n
            && chars[i + 1] == 's'
            && !is_id(chars.get(i + 2).copied().unwrap_or(' '))
            && (i == 0 || !is_id(chars[i - 1]))
        {
            let after = skip_ws(&chars, i + 2);
            if after < n && (chars[after].is_alphabetic() || chars[after] == '_') {
                let prev_out = out.chars().rev().find(|c| !c.is_whitespace());
                let prev_expr = prev_out.map_or(false, |c| {
                    is_id(c) || matches!(c, ')' | ']' | '"' | '\'' | '`')
                });
                if prev_expr {
                    i = after; // jump past `as ` + whitespace
                    i = skip_type_expr(&chars, i, false);
                    // Keep at least one space so adjacent tokens don't merge.
                    if out.ends_with(|c: char| is_id(c)) {
                        out.push(' ');
                    }
                    continue;
                }
            }
        }

        out.push(ch);
        i += 1;
    }

    out
}

/// Skip a complete TypeScript type expression starting at `start`.
/// Returns the index of the first character NOT part of the type.
///
/// `stop_at_brace` — true when stripping a return-type annotation (the `{` that
/// follows is the function body, not an object-type literal).
fn skip_type_expr(chars: &[char], start: usize, stop_at_brace: bool) -> usize {
    let n = chars.len();
    let mut i = start;
    let mut d_angle: usize = 0;
    let mut d_paren: usize = 0;
    let mut d_bracket: usize = 0;
    let mut d_brace: usize = 0;

    while i < n {
        let all_zero = d_angle == 0 && d_paren == 0 && d_bracket == 0 && d_brace == 0;
        match chars[i] {
            // String literal types ("error" | "warning") — skip verbatim.
            '"' | '\'' => {
                let q = chars[i];
                i += 1;
                while i < n {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    if i < n {
                        let c = chars[i];
                        i += 1;
                        if c == q {
                            break;
                        }
                    }
                }
            }
            // Hard stop characters at outermost depth.
            ',' | ';' if all_zero => break,
            ')' if all_zero => break,
            '{' if stop_at_brace && all_zero => break,
            // `=>` — signals function body for return types; part of function
            // type for parameter/variable types.
            '=' if all_zero && i + 1 < n && chars[i + 1] == '>' => {
                if stop_at_brace {
                    break;
                } else {
                    i += 2;
                } // consume `=>`
            }
            // Plain `=` (default value) stops the type.
            '=' if all_zero => break,
            // Bare `>` at top level means we over-consumed a generic — stop.
            '>' if all_zero && d_angle == 0 => break,
            // Depth tracking.
            '<' => {
                d_angle += 1;
                i += 1;
            }
            '>' if d_angle > 0 => {
                d_angle -= 1;
                i += 1;
            }
            '(' => {
                d_paren += 1;
                i += 1;
            }
            ')' if d_paren > 0 => {
                d_paren -= 1;
                i += 1;
            }
            '[' => {
                d_bracket += 1;
                i += 1;
            }
            ']' if d_bracket > 0 => {
                d_bracket -= 1;
                i += 1;
            }
            '{' => {
                d_brace += 1;
                i += 1;
            }
            '}' if d_brace > 0 => {
                d_brace -= 1;
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    i
}

#[inline]
fn is_id(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn skip_ws(chars: &[char], start: usize) -> usize {
    let mut i = start;
    while i < chars.len() && matches!(chars[i], ' ' | '\t') {
        i += 1;
    }
    i
}

fn copy_str_lit(chars: &[char], start: usize, out: &mut String) -> usize {
    let q = chars[start];
    out.push(q);
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' => {
                out.push('\\');
                i += 1;
                if i < chars.len() {
                    out.push(chars[i]);
                    i += 1;
                }
            }
            c if c == q => {
                out.push(c);
                i += 1;
                break;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    i
}

fn copy_template_lit(chars: &[char], start: usize, out: &mut String) -> usize {
    out.push('`');
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' => {
                out.push('\\');
                i += 1;
                if i < chars.len() {
                    out.push(chars[i]);
                    i += 1;
                }
            }
            '`' => {
                out.push('`');
                i += 1;
                break;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    i
}

// --- Pass 1: structural declaration removal ---------------------------------

fn strip_ts_declarations(src: &str) -> String {
    let mut out: Vec<&str> = Vec::with_capacity(64);
    let mut in_block = false; // inside a removed { ... } body
    let mut after_decl = false; // saw keyword, waiting for opening { on next line
    let mut depth: usize = 0;

    for line in src.lines() {
        if in_block {
            for ch in line.chars() {
                match ch {
                    '{' => depth += 1,
                    '}' if depth > 0 => {
                        depth -= 1;
                        if depth == 0 {
                            in_block = false;
                        }
                    }
                    _ => {}
                }
            }
            out.push(""); // blank preserves line number
        } else if after_decl {
            // K&R style: opening brace on the line after the keyword.
            if line.contains('{') {
                in_block = true;
                depth = 0;
                for ch in line.chars() {
                    match ch {
                        '{' => depth += 1,
                        '}' if depth > 0 => {
                            depth -= 1;
                            if depth == 0 {
                                in_block = false;
                            }
                        }
                        _ => {}
                    }
                }
            } else if line.trim().is_empty() {
                // blank continuation line — keep waiting
            } else {
                // Non-brace content after a bare `interface Foo` — not actually
                // a block declaration; stop suppression and emit the line.
                after_decl = false;
                out.push(line);
                continue;
            }
            out.push("");
            if !in_block {
                after_decl = false;
            }
        } else {
            let trimmed = line.trim_start();
            let kw = trimmed.strip_prefix("export ").unwrap_or(trimmed);

            let is_decl = kw.starts_with("interface ")
                || kw.starts_with("declare ")
                || (kw.starts_with("type ") && kw.contains('=') && !kw.starts_with("typeof "));

            if is_decl {
                if line.contains('{') {
                    in_block = true;
                    depth = 0;
                    for ch in line.chars() {
                        match ch {
                            '{' => depth += 1,
                            '}' if depth > 0 => {
                                depth -= 1;
                                if depth == 0 {
                                    in_block = false;
                                }
                            }
                            _ => {}
                        }
                    }
                } else if !trimmed.ends_with(';') {
                    // No brace and no semicolon — opening brace may be on the next line.
                    after_decl = true;
                }
                // Lines ending with ';' are self-contained single-line declarations
                // (e.g. `type Foo = string | number;`) — just suppress the line.
                out.push("");
            } else {
                out.push(line);
            }
        }
    }

    let sep = if src.contains("\r\n") { "\r\n" } else { "\n" };
    let mut result = out.join(sep);
    if src.ends_with('\n') || src.ends_with("\r\n") {
        result.push('\n');
    }
    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::i18n::Locale;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use serde_json::json;
    use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Url};

    use super::{
        OperationApplicability, PluginApplicability, PluginHost, RawDiag, RawPluginApplicability,
        build_context, build_hover_context, build_workspace_snapshot, strip_ts_declarations,
        strip_typescript,
    };
    use crate::document::DocumentData;
    use crate::runtime::{WorkspaceFileSnapshot, WorkspaceSourceInfo, build_workspace_index};

    // --- Structural (pass 1) -------------------------------------------------

    #[test]
    fn applicability_is_operation_specific_normalized_and_custom_safe() {
        let applicability = PluginApplicability::from(RawPluginApplicability {
            validate: vec![Some(vec!["  CubeMain ".to_string()])],
            hover: vec![Some(vec!["Skills".to_string()])],
            goto_definition: vec![None],
        });
        assert!(applicability.validate.applies_to("cubemain"));
        assert!(!applicability.validate.applies_to("skills"));
        assert!(applicability.hover.applies_to("SKILLS"));
        assert!(!applicability.hover.applies_to("cubemain"));
        assert!(matches!(
            applicability.goto_definition,
            OperationApplicability::All
        ));
        assert!(applicability.goto_definition.applies_to("any-custom-file"));
    }

    #[test]
    fn strips_interface_block() {
        let src = "interface Foo {\n  bar: string;\n}\nconst x = 1;\n";
        let out = strip_ts_declarations(src);
        assert!(
            !out.contains("interface"),
            "interface block should be removed"
        );
        assert!(out.contains("const x = 1;"), "regular code preserved");
        assert_eq!(
            out.lines().count(),
            src.lines().count(),
            "line count preserved"
        );
    }

    #[test]
    fn strips_exported_interface() {
        let src = "export interface Foo {\n  x: number;\n}\nfunction f() {}\n";
        let out = strip_ts_declarations(src);
        assert!(!out.contains("interface"));
        assert!(out.contains("function f()"));
    }

    #[test]
    fn strips_type_alias() {
        let src = "type Severity = 'error' | 'warning';\nfunction validate() {}\n";
        let out = strip_ts_declarations(src);
        assert!(!out.contains("type Severity"), "type alias removed");
        assert!(out.contains("function validate"), "function preserved");
    }

    #[test]
    fn strips_multiline_type_object() {
        let src = "type Foo = {\n  bar: string;\n};\nvar x = 1;\n";
        let out = strip_ts_declarations(src);
        assert!(!out.contains("type Foo"));
        assert!(out.contains("var x = 1;"));
    }

    #[test]
    fn preserves_typeof() {
        let src = "const x = typeof foo;\n";
        let out = strip_ts_declarations(src);
        assert!(out.contains("typeof foo"), "typeof must not be stripped");
    }

    #[test]
    fn preserves_regular_code() {
        let src = "function validate(ctx) {\n  return [];\n}\n";
        let out = strip_ts_declarations(src);
        assert_eq!(out, src);
    }

    // --- Inline annotations (pass 2 via strip_typescript) --------------------

    #[test]
    fn strips_function_param_type() {
        let out = strip_typescript("function validate(ctx: PluginContext) {}");
        assert!(out.contains("function validate(ctx)"), "got: {out}");
    }

    #[test]
    fn strips_return_type() {
        let out = strip_typescript("function validate(ctx: Ctx): Diag[] {}");
        assert!(out.contains("function validate(ctx)"), "got: {out}");
        assert!(out.contains("{}"), "body preserved, got: {out}");
    }

    #[test]
    fn strips_union_return_type() {
        let out = strip_typescript("function hover(ctx: HoverCtx): HoverResult | null {}");
        assert!(out.contains("function hover(ctx)"), "got: {out}");
        assert!(out.contains("{}"), "body preserved, got: {out}");
    }

    #[test]
    fn strips_array_return_type() {
        let out = strip_typescript("function f(): string[] {}");
        assert!(out.contains("function f()"), "got: {out}");
        assert!(out.contains("{}"), "body preserved, got: {out}");
    }

    #[test]
    fn strips_top_level_variable_type() {
        let out = strip_typescript("const x: string = 'hello';");
        assert!(out.contains("const x"), "got: {out}");
        assert!(out.contains("= 'hello'"), "rhs preserved, got: {out}");
        assert!(!out.contains(": string"), "type stripped, got: {out}");
    }

    #[test]
    fn strips_as_cast() {
        let out = strip_typescript("const x = getValue() as string;");
        assert!(!out.contains(" as "), "got: {out}");
        assert!(out.contains("getValue()"), "expr preserved, got: {out}");
    }

    #[test]
    fn strips_optional_param() {
        let out = strip_typescript("function f(x?: string) {}");
        // `x?:` → `x`, then `: string` stripped
        assert!(out.contains("function f(x)"), "got: {out}");
    }

    #[test]
    fn preserves_object_literal_in_call() {
        // `{ key: value }` inside a function call must NOT be stripped
        let out = strip_typescript("return { content: msg };");
        assert!(
            out.contains("content: msg"),
            "object literal preserved, got: {out}"
        );
    }

    #[test]
    fn preserves_ternary_colon() {
        let out = strip_typescript("var x = a ? b : c;");
        assert!(out.contains("a ? b : c"), "ternary preserved, got: {out}");
    }

    #[test]
    fn strips_arrow_param_type_in_callback() {
        // Arrow function param inside a function body (brace_depth=1, paren_depth=2)
        let out = strip_typescript(
            "function f(ctx: Ctx): void {\n  ctx.rows.forEach((row: Row) => {});\n}",
        );
        assert!(
            out.contains("(row)"),
            "arrow param type stripped, got: {out}"
        );
        assert!(
            out.contains("(ctx)"),
            "outer param type stripped, got: {out}"
        );
    }

    #[test]
    fn nullish_coalescing_does_not_poison_ternary() {
        // `??` must not set the pending-ternary flag; a later `:` return-type
        // annotation must still be stripped.
        let src =
            "function f() { const x = a ?? 0; }\nfunction g(): string | null { return null; }";
        let out = strip_typescript(src);
        assert!(
            out.contains("function g()"),
            "return type stripped, got: {out}"
        );
        assert!(
            !out.contains(": string"),
            "return type stripped, got: {out}"
        );
        assert!(out.contains("return null"), "body preserved, got: {out}");
    }

    #[test]
    fn strips_const_type_in_function_body() {
        let out = strip_typescript(
            "function f() {\n    const tokens: string[] = [];\n    let n: number = 0;\n    var m: Record<string, number> = {};\n    return tokens;\n}",
        );
        assert!(
            out.contains("const tokens= []"),
            "const type stripped, got: {out}"
        );
        assert!(out.contains("let n= 0"), "let type stripped, got: {out}");
        assert!(out.contains("var m= {}"), "var type stripped, got: {out}");
        assert!(out.contains("return tokens"), "body preserved, got: {out}");
    }

    #[test]
    fn preserves_object_literal_in_var_decl() {
        // `const x = { key: value }` — the rename colon must NOT be stripped
        let out = strip_typescript("function f() { const x = { key: value }; }");
        assert!(
            out.contains("key: value"),
            "object literal preserved, got: {out}"
        );
    }

    #[test]
    fn preserves_string_in_string() {
        // Colon inside a string must not be treated as a type annotation.
        let out = strip_typescript("var x = \"key: value\";");
        assert!(
            out.contains("\"key: value\""),
            "string content preserved, got: {out}"
        );
    }

    #[test]
    fn full_plugin_snippet() {
        let src = r#"
interface PluginContext { file: string; headers: string[]; }
function validate(ctx: PluginContext): string[] {
    if (ctx.file !== "cubemain") return [];
    var diags: string[] = [];
    ctx.rows.forEach((row: Record<string, string>) => {
        var n = parseInt(row["numinputs"] || "0", 10);
        diags.push(n > 0 ? "ok" : "empty");
    });
    return diags;
}
"#;
        let out = strip_typescript(src);
        assert!(
            !out.contains("interface PluginContext"),
            "interface removed"
        );
        assert!(
            out.contains("function validate(ctx)"),
            "param type stripped, got: {out}"
        );
        assert!(out.contains("if (ctx.file"), "body preserved, got: {out}");
        assert!(
            out.contains("n > 0 ? \"ok\" : \"empty\""),
            "ternary preserved, got: {out}"
        );
        // Arrow param inside forEach should be stripped
        assert!(out.contains("(row)"), "arrow param stripped, got: {out}");
    }

    #[test]
    fn strips_arrow_return_type_inside_function_body() {
        // `): ReturnType =>` inside a function body — depth_ok was false before the fix
        let out = strip_typescript(
            "function hover(ctx: Ctx): HoverResult | null {\n    const fmt = (x: number): string => { return x.toFixed(2); };\n    return null;\n}",
        );
        assert!(
            out.contains("const fmt = (x)"),
            "arrow return type stripped, got: {out}"
        );
        assert!(
            !out.contains(": string"),
            "no type annotation left, got: {out}"
        );
    }

    #[test]
    fn strips_arrow_param_type_deep_nested() {
        // Arrow param types must be stripped even when paren_depth == brace_depth
        // (e.g. inside two if-blocks, inside .map((s: string) => ...))
        let out = strip_typescript(
            "function f() {\
                if (a) {\
                    x.map((s: string) => s.trim())\
                     .filter((s: string) => s.length > 0);\
                }\
            }",
        );
        assert!(
            out.contains("(s)"),
            "arrow param type stripped in deep nest, got: {out}"
        );
        assert!(
            !out.contains(": string"),
            "no type annotation left, got: {out}"
        );
    }

    struct PluginFixture {
        docs: HashMap<String, Arc<DocumentData>>,
        index: Arc<crate::runtime::WorkspaceIndex>,
        snapshot: Arc<WorkspaceFileSnapshot>,
    }

    fn plugin_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("contrib")
            .join("d2rdoc")
            .join("plugins")
            .join(name)
    }

    fn fixture(files: &[(&str, &str)]) -> PluginFixture {
        let mut docs = HashMap::new();
        let mut file_cache = HashMap::new();
        for (stem, text) in files {
            let doc = Arc::new(DocumentData::parse(text, '\t'));
            file_cache.insert(PathBuf::from(format!("{stem}.txt")), Arc::clone(&doc));
            docs.insert((*stem).to_string(), doc);
        }

        let open_docs = HashMap::new();
        let index = build_workspace_index(&open_docs, &file_cache);
        let snapshot = build_workspace_snapshot(&open_docs, &file_cache);

        PluginFixture {
            docs,
            index,
            snapshot,
        }
    }

    #[tokio::test]
    async fn hover_and_definition_reset_workspace_caches_without_validate_requests() {
        let path =
            std::env::temp_dir().join(format!("vector-lsp-hover-cache-{}.js", std::process::id()));
        std::fs::write(
            &path,
            "const pluginMetadata={hoverFiles:['target'],gotoDefinitionFiles:['target']};\n\
             function hover(){return {content:getColumnValues('source','code')[0]};}\n\
             function gotoDefinition(){return {targetFile:'source',targetCol:'code',targetValue:getColumnValues('source','code')[0]};}\n",
        )
        .unwrap();
        let host = PluginHost::new(vec![path.clone()]).unwrap();
        let old = fixture(&[("target", "id\n1"), ("source", "code\nOLD")]);
        let new = fixture(&[("target", "id\n1"), ("source", "code\nNEW")]);
        let newest = fixture(&[("target", "id\n1"), ("source", "code\nNEWEST")]);
        let ctx = json!({"file":"target"});

        assert_eq!(
            host.hover(ctx.clone(), old.index, old.snapshot)
                .await
                .as_deref(),
            Some("OLD")
        );
        assert_eq!(
            host.hover(ctx.clone(), new.index, new.snapshot)
                .await
                .as_deref(),
            Some("NEW")
        );
        assert_eq!(
            host.goto_definition(ctx, newest.index, newest.snapshot)
                .await,
            Some((
                "source".to_string(),
                "code".to_string(),
                "NEWEST".to_string()
            ))
        );
        let _ = std::fs::remove_file(path);
    }

    #[expect(
        clippy::type_complexity,
        reason = "test helper returns the coordinated live, disk, and document fixtures used by workspace precedence tests"
    )]
    fn open_shadow_sources() -> (
        HashMap<Url, Arc<DocumentData>>,
        HashMap<PathBuf, Arc<DocumentData>>,
        Arc<DocumentData>,
    ) {
        let disk = Arc::new(DocumentData::parse("code\tremoved\nOLD\tDISK_ONLY\n", '\t'));
        let live = Arc::new(DocumentData::parse("code\nNEW\n", '\t'));
        let file_cache = HashMap::from([(PathBuf::from("workspace/weapons.txt"), disk)]);
        let open_docs = HashMap::from([(
            Url::parse("file:///workspace/weapons.txt").expect("valid test URI"),
            Arc::clone(&live),
        )]);
        (open_docs, file_cache, live)
    }

    // V-VLSP-01: an open document is the sole authority for its stem.
    #[test]
    fn workspace_index_open_document_removes_shadowed_disk_values() {
        let (open_docs, file_cache, _) = open_shadow_sources();
        let index = build_workspace_index(&open_docs, &file_cache);

        assert!(index.lookup("weapons", "code", "NEW"));
        assert!(
            !index.lookup("weapons", "code", "OLD"),
            "disk OLD must not remain visible while the same-stem live document contains NEW"
        );
    }

    #[test]
    fn workspace_index_uses_ascii_only_case_folding_for_lookup_values() {
        let fx = fixture(&[("unicode", "Name\nÄName\n")]);

        assert!(fx.index.lookup("UNICODE", "name", "ÄName"));
        assert!(
            !fx.index.lookup("unicode", "Name", "äName"),
            "non-ASCII case variants must remain distinct"
        );
    }

    // V-VLSP-01: columns removed in the open document cannot survive from disk.
    #[test]
    fn workspace_index_open_document_removes_shadowed_disk_columns() {
        let (open_docs, file_cache, _) = open_shadow_sources();
        let index = build_workspace_index(&open_docs, &file_cache);

        assert!(index.has_lookup_target("weapons", "code"));
        assert!(
            !index.has_lookup_target("weapons", "removed"),
            "a disk-only column must disappear from the live workspace index"
        );
        assert!(
            !index.lookup("weapons", "removed", "DISK_ONLY"),
            "values from a removed disk-only column must not remain lookup-visible"
        );
    }

    // V-VLSP-02: the plugin snapshot and lookup index built for one request
    // must expose the same document generation.
    #[test]
    fn workspace_snapshot_and_index_observe_the_same_live_generation() {
        let (open_docs, file_cache, live) = open_shadow_sources();
        let snapshot = build_workspace_snapshot(&open_docs, &file_cache);
        let index = build_workspace_index(&open_docs, &file_cache);
        let snap_doc = snapshot
            .files
            .get("weapons")
            .expect("snapshot should contain the live weapons document");

        assert!(
            Arc::ptr_eq(snap_doc, &live),
            "snapshot should retain the exact live document generation"
        );
        for value in ["OLD", "NEW"] {
            let snapshot_contains = snap_doc.rows.iter().any(|row| {
                row.cells
                    .first()
                    .map(|cell| cell.value == value)
                    .unwrap_or(false)
            });
            assert_eq!(
                index.lookup("weapons", "code", value),
                snapshot_contains,
                "snapshot and index disagree for value {value}"
            );
        }
        assert_eq!(
            index.has_lookup_target("weapons", "removed"),
            snap_doc.headers.iter().any(|header| header == "removed"),
            "snapshot and index disagree about the removed column"
        );
    }

    #[test]
    fn workspace_shadow_normalizes_case_and_percent_encoded_file_stems() {
        let disk = Arc::new(DocumentData::parse("code\nOLD", '\t'));
        let live = Arc::new(DocumentData::parse("code\nNEW", '\t'));
        let file_cache = HashMap::from([(PathBuf::from("C:/workspace/Weapon Set.txt"), disk)]);
        let open_docs = HashMap::from([(
            Url::parse("file:///C:/workspace/weapon%20set.txt").unwrap(),
            Arc::clone(&live),
        )]);
        let index = build_workspace_index(&open_docs, &file_cache);
        let snapshot = build_workspace_snapshot(&open_docs, &file_cache);

        assert!(index.lookup("weapon set", "code", "NEW"));
        assert!(!index.lookup("weapon set", "code", "OLD"));
        assert!(Arc::ptr_eq(
            snapshot.files.get("weapon set").unwrap(),
            &live
        ));
    }

    async fn run_plugin(plugin_name: &str, file: &str, fx: &PluginFixture) -> Vec<Diagnostic> {
        let host = PluginHost::new(vec![plugin_path(plugin_name)]).unwrap();
        let doc = fx.docs.get(file).expect("test document should exist");
        host.run(
            build_context(file, doc),
            Arc::clone(&fx.index),
            Arc::clone(&fx.snapshot),
        )
        .await
    }

    async fn run_plugin_localized(
        plugin_name: &str,
        file: &str,
        fx: &PluginFixture,
        locale: Locale,
    ) -> Vec<Diagnostic> {
        let host = PluginHost::new(vec![plugin_path(plugin_name)]).unwrap();
        let doc = fx.docs.get(file).expect("test document should exist");
        host.run_localized(
            build_context(file, doc),
            Arc::clone(&fx.index),
            Arc::clone(&fx.snapshot),
            locale,
        )
        .await
    }

    fn range(diag: &Diagnostic) -> (u32, u32, u32) {
        (
            diag.range.start.line,
            diag.range.start.character,
            diag.range.end.character,
        )
    }

    fn first_range(diags: &[Diagnostic]) -> (u32, u32, u32) {
        assert_eq!(
            diags.len(),
            1,
            "expected exactly one diagnostic, got {diags:#?}"
        );
        range(&diags[0])
    }

    fn assert_code(diag: &Diagnostic, expected: &str) {
        assert_eq!(
            diag.code.as_ref(),
            Some(&NumberOrString::String(expected.to_string()))
        );
    }

    fn diag_data(diag: &Diagnostic) -> &serde_json::Value {
        diag.data
            .as_ref()
            .expect("diagnostic should carry structured data")
    }

    fn data_str<'a>(diag: &'a Diagnostic, key: &str) -> &'a str {
        diag_data(diag)
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("missing string data field {key}: {diag:#?}"))
    }

    fn data_u64(diag: &Diagnostic, key: &str) -> u64 {
        diag_data(diag)
            .get(key)
            .and_then(|v| v.as_u64())
            .unwrap_or_else(|| panic!("missing numeric data field {key}: {diag:#?}"))
    }

    fn base_lookup_files() -> Vec<(&'static str, &'static str)> {
        vec![
            (
                "weapons",
                "code\tname\nhpot\tHealing Potion\nabc \tPadded Code\n가xA\tMultibyte Item\néab\tFour-byte UTF-8 Item\n",
            ),
            ("armor", "code\tname\ncap\tCap\n"),
            ("misc", "code\tname\nkey\tKey\n"),
            (
                "itemtypes",
                "Code\tTreasureClass\tItemType\nweap\t1\tWeapon\narmo\t1\tArmor\n",
            ),
            ("uniqueitems", "index\nThe Gnasher\n"),
            ("setitems", "index\nHsarus' Iron Heel\n"),
        ]
    }

    fn with_base_files(primary: (&'static str, &'static str)) -> Vec<(&'static str, &'static str)> {
        let mut files = base_lookup_files();
        files.push(primary);
        files
    }

    fn with_cube_output_files(
        primary: (&'static str, &'static str),
    ) -> Vec<(&'static str, &'static str)> {
        let mut files = base_lookup_files();
        files.push(("properties", "code\nfire-damage\n"));
        files.push(("propertygroups", "code\nvalidgroup\n"));
        files.push(primary);
        files
    }

    #[test]
    fn raw_diag_preserves_optional_code_and_data() {
        let raw: RawDiag = serde_json::from_value(json!({
            "line": 3,
            "col": 10,
            "endCol": 10,
            "severity": "error",
            "message": "missing token",
            "code": "calc.expected-rparen.eof",
            "data": {
                "rule": "calcCheck",
                "kind": "missing-token",
                "expected": ")",
                "actual": "EOF",
                "parserPosition": 33,
                "insertionPoint": 33,
                "insertText": ")"
            }
        }))
        .expect("raw diagnostic should deserialize");

        let diag = raw.into_lsp(Locale::EnUs);
        assert_eq!(range(&diag), (3, 10, 10));
        assert_code(&diag, "calc.expected-rparen.eof");
        assert_eq!(
            diag.data,
            Some(json!({
                "rule": "calcCheck",
                "kind": "missing-token",
                "expected": ")",
                "actual": "EOF",
                "parserPosition": 33,
                "insertionPoint": 33,
                "insertText": ")"
            }))
        );
    }

    #[test]
    fn raw_diag_still_accepts_plugins_without_code_or_data() {
        let raw: RawDiag = serde_json::from_value(json!({
            "line": 1,
            "col": 2,
            "message": "old plugin shape"
        }))
        .expect("legacy raw diagnostic should deserialize");

        let diag = raw.into_lsp(Locale::EnUs);
        assert_eq!(range(&diag), (1, 2, 2));
        assert_eq!(diag.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(diag.code, None);
        assert_eq!(diag.data, None);
    }

    #[tokio::test]
    async fn calc_check_reports_narrow_parser_ranges() {
        let cases = [
            (
                "unexpected character at start",
                "code\tlen\nr\t\"bad\n",
                (1, 2, 3),
                "Unexpected character '\"'",
                "calc.unexpected-character",
                "unexpected-character",
            ),
            (
                "unterminated string literal",
                "code\tlen\nr\tskill('Fire\n",
                (1, 8, 9),
                "Unterminated string literal",
                "calc.unterminated-string",
                "unterminated-string",
            ),
            (
                "unexpected token",
                "code\tlen\nr\t1 + * 2\n",
                (1, 6, 7),
                "Unexpected token '*'",
                "calc.unexpected-token",
                "unexpected-token",
            ),
            (
                "expected quoted argument",
                "code\tlen\nr\tskill(1)\n",
                (1, 8, 9),
                "Expected quoted string",
                "calc.expected-quoted-argument",
                "invalid-argument",
            ),
            (
                "leading whitespace mapping",
                "code\tlen\nr\t  1 + * 2\n",
                (1, 8, 9),
                "Unexpected token '*'",
                "calc.unexpected-token",
                "unexpected-token",
            ),
            (
                "outer quote mapping",
                "code\tlen\nr\t\"1 + * 2\"\n",
                (1, 7, 8),
                "Unexpected token '*'",
                "calc.unexpected-token",
                "unexpected-token",
            ),
        ];

        for (label, misc, expected, message, code, kind) in cases {
            let fx = fixture(&[("misc", misc)]);
            let diags = run_plugin("calcCheck.ts", "misc", &fx).await;
            assert_eq!(first_range(&diags), expected, "{label}");
            assert!(
                diags[0].message.contains(message),
                "{label}: human-readable message missing, got {:?}",
                diags[0].message
            );
            assert_code(&diags[0], code);
            assert_eq!(data_str(&diags[0], "rule"), "calcCheck", "{label}");
            assert_eq!(data_str(&diags[0], "kind"), kind, "{label}");
            assert!(
                diags[0].range.end.character - diags[0].range.start.character < 3,
                "{label}: diagnostic should stay narrow, got {:?}",
                range(&diags[0])
            );
        }
    }

    #[tokio::test]
    async fn calc_check_reports_eof_missing_tokens_as_insertion_points() {
        let formula = "min(5,1+skill('Fire Ball'.blvl)/5";
        let misc = format!("code\tlen\nr\t{formula}\n");
        let fx = fixture(&[("misc", misc.as_str())]);
        let diags = run_plugin("calcCheck.ts", "misc", &fx).await;
        let insertion = 2 + formula.chars().count() as u32;

        assert_eq!(first_range(&diags), (1, insertion, insertion));
        assert!(
            diags[0]
                .message
                .contains("Missing ')' before end of formula"),
            "friendly EOF message missing: {:?}",
            diags[0].message
        );
        assert_code(&diags[0], "calc.expected-rparen.eof");
        assert_eq!(data_str(&diags[0], "kind"), "missing-token");
        assert_eq!(data_str(&diags[0], "expected"), ")");
        assert_eq!(data_str(&diags[0], "actual"), "EOF");
        assert_eq!(data_str(&diags[0], "insertText"), ")");
        assert_eq!(data_u64(&diags[0], "parserPosition"), formula.len() as u64);
        assert_eq!(data_u64(&diags[0], "insertionPoint"), formula.len() as u64);
    }

    #[tokio::test]
    async fn calc_check_reports_other_expected_token_eof_as_insertion_point() {
        let formula = "1 ? 2";
        let misc = format!("code\tlen\nr\t{formula}\n");
        let fx = fixture(&[("misc", misc.as_str())]);
        let diags = run_plugin("calcCheck.ts", "misc", &fx).await;
        let insertion = 2 + formula.chars().count() as u32;

        assert_eq!(first_range(&diags), (1, insertion, insertion));
        assert!(
            diags[0]
                .message
                .contains("Missing ':' before end of formula"),
            "friendly EOF message missing: {:?}",
            diags[0].message
        );
        assert_code(&diags[0], "calc.expected-colon.eof");
        assert_eq!(data_str(&diags[0], "kind"), "missing-token");
        assert_eq!(data_str(&diags[0], "expected"), ":");
        assert_eq!(data_str(&diags[0], "actual"), "EOF");
        assert_eq!(data_str(&diags[0], "insertText"), ":");
        assert_eq!(data_u64(&diags[0], "insertionPoint"), formula.len() as u64);
    }

    #[tokio::test]
    async fn calc_check_maps_missing_token_before_trailing_whitespace() {
        let formula = "min(1, 2   ";
        let trimmed_len = "min(1, 2".len();
        let misc = format!("code\tlen\nr\t{formula}\n");
        let fx = fixture(&[("misc", misc.as_str())]);
        let diags = run_plugin("calcCheck.ts", "misc", &fx).await;
        let insertion = 2 + trimmed_len as u32;

        assert_eq!(first_range(&diags), (1, insertion, insertion));
        assert_code(&diags[0], "calc.expected-rparen.eof");
        assert_eq!(data_str(&diags[0], "kind"), "missing-token");
        assert_eq!(data_u64(&diags[0], "parserPosition"), trimmed_len as u64);
        assert_eq!(data_u64(&diags[0], "insertionPoint"), trimmed_len as u64);
    }

    #[tokio::test]
    async fn calc_check_reports_unexpected_eof_as_insertion_point() {
        let formula = "1 +";
        let misc = format!("code\tlen\nr\t{formula}\n");
        let fx = fixture(&[("misc", misc.as_str())]);
        let diags = run_plugin("calcCheck.ts", "misc", &fx).await;
        let insertion = 2 + formula.chars().count() as u32;

        assert_eq!(first_range(&diags), (1, insertion, insertion));
        assert!(diags[0].message.contains("Unexpected end of formula"));
        assert_code(&diags[0], "calc.unexpected-eof");
        assert_eq!(data_str(&diags[0], "kind"), "unexpected-eof");
        assert_eq!(data_str(&diags[0], "expected"), "expression");
        assert_eq!(data_str(&diags[0], "actual"), "EOF");
        assert_eq!(data_u64(&diags[0], "insertionPoint"), formula.len() as u64);
    }

    #[tokio::test]
    async fn calc_check_reports_expected_quoted_argument_metadata() {
        let fx = fixture(&[("misc", "code\tlen\nr\tskill(1)\n")]);
        let diags = run_plugin("calcCheck.ts", "misc", &fx).await;

        assert_eq!(first_range(&diags), (1, 8, 9));
        assert_code(&diags[0], "calc.expected-quoted-argument");
        assert_eq!(data_str(&diags[0], "kind"), "invalid-argument");
        assert_eq!(data_str(&diags[0], "expected"), "quoted string");
        assert_eq!(data_str(&diags[0], "actual"), "1");
        assert_eq!(data_u64(&diags[0], "tokenStart"), 6);
        assert_eq!(data_u64(&diags[0], "tokenEnd"), 7);
    }

    #[tokio::test]
    async fn calc_check_maps_offsets_after_non_ascii_text() {
        let fx = fixture(&[("misc", "code\tlen\nr\tskill('한', 1 + * 2)\n")]);
        let diags = run_plugin("calcCheck.ts", "misc", &fx).await;
        assert_eq!(first_range(&diags), (1, 17, 18));
        assert!(diags[0].message.contains("Unexpected token '*'"));
        assert_code(&diags[0], "calc.unexpected-token");
        assert_eq!(data_str(&diags[0], "kind"), "unexpected-token");
        assert_eq!(data_u64(&diags[0], "tokenStart"), 15);
        assert_eq!(data_u64(&diags[0], "tokenEnd"), 16);
    }

    #[tokio::test]
    async fn calc_check_accepts_existing_valid_formula_without_diagnostics() {
        let fx = fixture(&[("misc", "code\tlen\nr\tmin(1, 2)\n")]);
        let diags = run_plugin("calcCheck.ts", "misc", &fx).await;
        assert!(
            diags.is_empty(),
            "valid formula should stay clean: {diags:#?}"
        );
    }

    #[tokio::test]
    async fn calc_check_accepts_stock_monpet_comparison_sum_formula() {
        let formula = "((100-hpct)/((elte==1+elte==4)?20:((elte==2)?10:5)))";
        let fx = fixture(&[
            ("moncalc", "code\nhpct\nelte\n"),
            ("monpet", &format!("id\tcalc1\nwolf\t{formula}\n")),
        ]);
        let diags = run_plugin("calcCheck.ts", "monpet", &fx).await;

        assert!(
            diags.is_empty(),
            "stock monpet formula should be valid: {diags:#?}"
        );
    }

    #[tokio::test]
    async fn calc_check_accepts_comparison_terms_in_arithmetic_expressions() {
        let fx = fixture(&[
            ("moncalc", "code\nelte\nhpct\n"),
            (
                "monpet",
                "id\tcalc1\tcalc2\tcalc3\n\
                 wolf\telte==1+elte==4\t100-(elte==1+elte==4)\t(elte==1+elte==4)?20:5\n",
            ),
        ]);
        let diags = run_plugin("calcCheck.ts", "monpet", &fx).await;

        assert!(
            diags.is_empty(),
            "comparison-plus-comparison formulas should be valid: {diags:#?}"
        );
    }

    #[tokio::test]
    async fn calc_check_preserves_pre_revalidation_monpet_scope_allowlist_and_argument_grammar() {
        let fx = fixture(&[
            ("moncalc", "code\nhpct\n"),
            (
                "monpet",
                "id\tcalc1\tcalc2\tcalc3\tcalc4\tcalc5\n\
                 wolf\tskill()\tcond('IsType',1)\tcond('IsType',1,2)\tcond('istype',1)\tunknown\n",
            ),
        ]);
        let diags = run_plugin("calcCheck.ts", "monpet", &fx).await;

        assert_eq!(diags.len(), 3, "{diags:#?}");
        assert_code(&diags[0], "calc.expected-rparen");
        assert_code(&diags[1], "unknownCondition");
        assert_code(&diags[2], "unknownIdentifier");
        assert!(
            diags
                .iter()
                .all(|diag| diag.severity == Some(DiagnosticSeverity::ERROR))
        );
    }

    #[tokio::test]
    async fn calc_check_defers_item_and_tc_bare_identifiers_without_losing_syntax_checks() {
        let fx = fixture(&[(
            "misc",
            "code\tlen\tDropConditionCalc\nrow\tbinaryresolved\totherresolved\n",
        )]);
        let diags = run_plugin("calcCheck.ts", "misc", &fx).await;
        assert!(
            diags.is_empty(),
            "Item/TC callbacks have no closed identifier dictionary: {diags:#?}"
        );

        let tc = fixture(&[(
            "treasureclassex",
            "Treasure Class\tConditionCalc\nAct 1\tbinaryresolved\n",
        )]);
        let diags = run_plugin("calcCheck.ts", "treasureclassex", &tc).await;
        assert!(
            diags.is_empty(),
            "TreasureClassEx ConditionCalc must use the TC fallback: {diags:#?}"
        );

        let syntax = fixture(&[("misc", "code\tlen\nrow\t1 + * 2\n")]);
        let diags = run_plugin("calcCheck.ts", "misc", &syntax).await;
        assert_eq!(first_range(&diags), (1, 8, 9));
        assert_code(&diags[0], "calc.unexpected-token");
    }

    #[tokio::test]
    async fn calc_check_keeps_closed_scope_identifier_rejection() {
        let fx = fixture(&[
            ("misscalc", "code\nknown\n"),
            ("missiles", "Missile\tSrvCalc1\nrow\tunknown\n"),
        ]);
        let diags = run_plugin("calcCheck.ts", "missiles", &fx).await;
        assert_eq!(first_range(&diags), (1, 4, 11));
        assert!(diags[0].message.contains("Unknown missile value 'unknown'"));
        assert!(diags[0].message.contains("game treats it as 0"));
        assert!(diags[0].message.contains("has no effect"));
        assert!(!diags[0].message.contains("Invalid calc formula"));
        assert_code(&diags[0], "unknownIdentifier");
    }

    #[tokio::test]
    async fn calc_check_reports_edmgsympercalc_ulvl_as_zero_substituted_misscalc_miss() {
        let formula = "(skill('Firestorm'.clc5)-skill('Molten Boulder'.blvl))*skill('Molten Boulder'.par8)+ulvl*skill('Molten Boulder'.par5)";
        let fx = fixture(&[
            ("misscalc", "code\nknown\n"),
            ("skillcalc", "code\nclc5\nblvl\npar8\npar5\n"),
            ("skills", "skill\nFirestorm\nMolten Boulder\n"),
            (
                "missiles",
                &format!("Missile\tEDmgSymPerCalc\nrow\t{formula}\n"),
            ),
        ]);
        let diags = run_plugin("calcCheck.ts", "missiles", &fx).await;

        let offset = formula.find("ulvl").expect("fixture contains ulvl") as u32 + 4;
        assert_eq!(diags.len(), 1, "{diags:#?}");
        assert_eq!(first_range(&diags), (1, offset, offset + 4));
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(
            diags[0].message,
            "Unknown missile value 'ulvl'. The game treats it as 0, so this part of the calculation has no effect."
        );
        assert!(!diags[0].message.contains("Invalid calc formula"));
        assert_code(&diags[0], "unknownIdentifier");
        assert_eq!(data_str(&diags[0], "scope"), "Missile scope BBE");
        assert_eq!(data_str(&diags[0], "identifier"), "ulvl");
        assert_eq!(data_str(&diags[0], "namespace"), "MissCalc.code");
        assert_eq!(
            data_str(&diags[0], "lookup"),
            "first-four-byte exact case-sensitive"
        );
        assert_eq!(
            diag_data(&diags[0])
                .get("resolverResult")
                .and_then(|value| value.as_i64()),
            Some(-1)
        );
        assert_eq!(data_str(&diags[0], "binaryFallback"), "integer constant 0");
        assert_eq!(
            data_str(&diags[0], "compileEffect"),
            "remaining formula continues"
        );
    }

    #[tokio::test]
    async fn calc_check_uses_missile_first_four_byte_case_sensitive_bare_lookup_only() {
        let fx = fixture(&[
            ("misscalc", "code\npar1\n"),
            (
                "missiles",
                "Missile\tSrvCalc1\tCltCalc1\nrow\tpar1suffix\tPAR1\n",
            ),
        ]);
        let diags = run_plugin("calcCheck.ts", "missiles", &fx).await;

        assert_eq!(diags.len(), 1, "{diags:#?}");
        assert_eq!(range(&diags[0]), (1, 15, 19));
        assert!(diags[0].message.contains("'PAR1'"));
        assert_eq!(
            data_str(&diags[0], "lookup"),
            "first-four-byte exact case-sensitive"
        );
    }

    #[tokio::test]
    async fn cube_output_check_accepts_binary_equivalent_forms_without_cubemod() {
        let files = with_cube_output_files((
            "cubemain",
            "description\tenabled\tinput 1\tinput 2\tinput 3\toutput\toutput b\toutput c\tb mod 1\tc mod 1\n\
             row\t1\thpot\tcap\tkey\thpot,pre,162\tuseitem,qty,3\tRED PORTAL\tFire-Damage\tFIRE-DAMAGE\n\
             named\t1\thpot\tcap\tkey\tthe gnasher\tusetype,pre=162\tPandemonium Finale Portal\t\t\n",
        ));
        let fx = fixture(&files);
        let diags = run_plugin("cubeOutputCheck.ts", "cubemain", &fx).await;
        assert!(
            diags.is_empty(),
            "comma/equal parameters, b/c ordinals, portal ASCII-CI, names/properties CI should pass: {diags:#?}"
        );
    }

    #[tokio::test]
    async fn cube_output_check_rejects_compact_portals_and_code_case_variants() {
        for (label, output) in [
            ("compact portal", "cowportal"),
            ("packed-code case", "HPOT"),
            ("over-four-byte raw code", "hpota"),
        ] {
            let cubemain = format!("description\tenabled\toutput\nrow\t1\t{output}\n");
            let mut files: Vec<(&str, &str)> = base_lookup_files();
            files.push(("properties", "code\nfire-damage\n"));
            files.push(("propertygroups", "code\nvalidgroup\n"));
            files.push(("cubemain", cubemain.as_str()));
            let fx = fixture(&files);
            let diags = run_plugin("cubeOutputCheck.ts", "cubemain", &fx).await;
            assert_eq!(diags.len(), 1, "{label}: {diags:#?}");
            assert_code(&diags[0], "cube-output.invalid-base");
        }
    }

    #[tokio::test]
    async fn cube_output_check_uses_lossless_utf8_fixed4_padding_for_short_raw_codes() {
        let files = with_cube_output_files((
            "cubemain",
            "description\tenabled\toutput\n\
             space padded\t1\tabc\n\
             multibyte\t1\t가x\n",
        ));
        let fx = fixture(&files);
        let diags = run_plugin("cubeOutputCheck.ts", "cubemain", &fx).await;
        assert!(
            diags.is_empty(),
            "abc -> 'abc ' and 가x -> 가xA share the exact padded/first-four UTF-8 bytes: {diags:#?}"
        );
    }

    #[tokio::test]
    async fn cube_output_check_does_not_accept_object_prototype_keys() {
        let files = with_cube_output_files((
            "cubemain",
            "description\tenabled\toutput\n\
             prototype base\t1\tconstructor\n\
             prototype flag\t1\thpot,toString\n\
             prototype parameter\t1\thpot,__proto__=3\n",
        ));
        let fx = fixture(&files);
        let diags = run_plugin("cubeOutputCheck.ts", "cubemain", &fx).await;
        assert_eq!(
            diags.len(),
            3,
            "prototype keys must not become grammar entries: {diags:#?}"
        );
        assert_code(&diags[0], "cube-output.invalid-base");
        assert_code(&diags[1], "cube-output.ignored-suffix");
        assert_code(&diags[2], "cube-output.ignored-suffix");
    }

    #[tokio::test]
    async fn cube_output_check_rejects_a_nonblank_cell_with_an_empty_base() {
        let files =
            with_cube_output_files(("cubemain", "description\tenabled\toutput\nrow\t1\t,mag\n"));
        let fx = fixture(&files);
        let diags = run_plugin("cubeOutputCheck.ts", "cubemain", &fx).await;
        assert_eq!(diags.len(), 1, "empty base must be explicit: {diags:#?}");
        assert_code(&diags[0], "cube-output.invalid-base");
        assert!(
            diags[0].message.contains("empty base for output"),
            "{diags:#?}"
        );
    }

    #[tokio::test]
    async fn cube_output_check_reports_ordinal_suffix_storage_and_property_semantics() {
        let cases = [
            (
                "ordinal",
                "description\tenabled\tinput 2\toutput b\nrow\t1\t\tuseitem\n",
                "cube-output.missing-ordinal-input",
                "has no matching 'input 2'",
            ),
            (
                "ignored suffix",
                "description\tenabled\toutput\nrow\t1\thpot,mag,noe,qty=2\n",
                "cube-output.ignored-suffix",
                "The game stops at 'noe'",
            ),
            (
                "u8 storage",
                "description\tenabled\toutput\nrow\t1\thpot,qty=256\n",
                "cube-output.u8-range",
                "outside 0..255",
            ),
            (
                "secondary property",
                "description\tenabled\tb mod 1\nrow\t1\tbad-property\n",
                "cube-output.invalid-property",
                "invalid property 'bad-property'",
            ),
        ];

        for (label, cubemain, code, message) in cases {
            let files = with_cube_output_files(("cubemain", cubemain));
            let fx = fixture(&files);
            let diags = run_plugin("cubeOutputCheck.ts", "cubemain", &fx).await;
            assert_eq!(diags.len(), 1, "{label}: {diags:#?}");
            assert_code(&diags[0], code);
            assert!(diags[0].message.contains(message), "{label}: {diags:#?}");
        }
    }

    #[tokio::test]
    async fn cube_output_check_reports_legacy_core_messages_on_whole_cells() {
        let cases = [
            (
                "invalid base",
                "description\tenabled\toutput\nrow\t1\tbad-base,mag\n",
                "cube-output.invalid-base",
                (1, 6, 18),
                "cubemain.txt, line 2: could not find 'bad-base' for output in recipe 'row'",
            ),
            (
                "empty base",
                "description\tenabled\toutput\nrow\t1\t,mag\n",
                "cube-output.invalid-base",
                (1, 6, 10),
                "cubemain.txt, line 2: empty base for output in recipe 'row'",
            ),
            (
                "ordinal",
                "description\tenabled\tinput 2\toutput b\nrow\t1\t\tuseitem\n",
                "cube-output.missing-ordinal-input",
                (1, 7, 14),
                "cubemain.txt, line 2: 'useitem' for 'output b' has no matching 'input 2' in recipe 'row'",
            ),
            (
                "secondary property",
                "description\tenabled\tb mod 1\nrow\t1\tbad-property\n",
                "cube-output.invalid-property",
                (1, 6, 18),
                "cubemain.txt, line 2: invalid property 'bad-property' for 'b mod 1' in recipe 'row'",
            ),
            (
                "inline byte storage",
                "description\tenabled\toutput\nrow\t1\thpot,qty=256\n",
                "cube-output.u8-range",
                (1, 6, 18),
                "cubemain.txt, line 2: 'qty=256' for 'output' is outside 0..255, so the game truncates it. Enter a value from 0 through 255 in recipe 'row'",
            ),
            (
                "ignored suffix",
                "description\tenabled\toutput\nrow\t1\thpot,mag,noe,qty=2\n",
                "cube-output.ignored-suffix",
                (1, 6, 24),
                "cubemain.txt, line 2: The game stops at 'noe' for 'output' in recipe 'row'. The base and modifiers before it still work; 'noe' and everything after it are ignored.",
            ),
            (
                "separate byte storage",
                "description\tenabled\toutput\tplvl\nrow\t1\thpot\t-1\n",
                "cube-output.u8-range",
                (1, 11, 13),
                "cubemain.txt, line 2: 'plvl' value '-1' for 'output' is outside 0..255, so the game truncates it. Enter a value from 0 through 255 in recipe 'row'",
            ),
        ];

        for (label, cubemain, code, expected_range, expected_message) in cases {
            let files = with_cube_output_files(("cubemain", cubemain));
            let fx = fixture(&files);
            let diags = run_plugin("cubeOutputCheck.ts", "cubemain", &fx).await;
            assert_eq!(diags.len(), 1, "{label}: {diags:#?}");
            assert_code(&diags[0], code);
            assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
            assert_eq!(range(&diags[0]), expected_range, "{label}");
            assert_eq!(diags[0].message, expected_message, "{label}");
        }
    }

    #[tokio::test]
    async fn cube_output_check_pins_all_separate_output_byte_columns() {
        for column in [
            "lvl", "plvl", "ilvl", "b lvl", "b plvl", "b ilvl", "c lvl", "c plvl", "c ilvl",
        ] {
            let output_column = if column.starts_with("b ") {
                "output b"
            } else if column.starts_with("c ") {
                "output c"
            } else {
                "output"
            };
            let cubemain =
                format!("description\tenabled\t{output_column}\t{column}\nrow\t1\thpot\t256\n");
            let mut files: Vec<(&str, &str)> = base_lookup_files();
            files.push(("properties", "code\nfire-damage\n"));
            files.push(("propertygroups", "code\nvalidgroup\n"));
            files.push(("cubemain", cubemain.as_str()));
            let fx = fixture(&files);
            let diags = run_plugin("cubeOutputCheck.ts", "cubemain", &fx).await;
            assert_eq!(diags.len(), 1, "{column}: {diags:#?}");
            assert_code(&diags[0], "cube-output.u8-range");
            assert!(diags[0].message.contains(column), "{column}: {diags:#?}");
            assert_eq!(range(&diags[0]), (1, 11, 14), "{column}");
        }
    }

    #[tokio::test]
    async fn cube_output_check_does_not_call_a_failed_base_preserved() {
        let files = with_cube_output_files((
            "cubemain",
            "description\tenabled\toutput\tilvl\nrow\t1\tbad-base,noe,qty=999\t256\n",
        ));
        let fx = fixture(&files);
        let diags = run_plugin("cubeOutputCheck.ts", "cubemain", &fx).await;
        assert_eq!(
            diags.len(),
            1,
            "failed base should stop cell semantics: {diags:#?}"
        );
        assert_code(&diags[0], "cube-output.invalid-base");
        assert!(!diags[0].message.contains("preserved"));
    }

    #[tokio::test]
    async fn cube_output_check_matches_legacy_storage_warnings_for_noncanonical_parameters() {
        let files = with_cube_output_files((
            "cubemain",
            "description\tenabled\toutput\toutput c\tc ilvl\nrow\t1\thpot,qty=+12,mag\thpot\t+12\n",
        ));
        let fx = fixture(&files);
        let diags = run_plugin("cubeOutputCheck.ts", "cubemain", &fx).await;
        assert_eq!(diags.len(), 2, "inline and separate plus forms: {diags:#?}");
        assert!(
            diags.iter().all(
                |diag| diag.code == Some(NumberOrString::String("cube-output.u8-range".into()))
            )
        );
        assert_eq!(range(&diags[0]), (1, 6, 22));
        assert_eq!(range(&diags[1]), (1, 28, 31));
        assert_eq!(
            diags[0].message,
            "cubemain.txt, line 2: 'qty=+12' for 'output' is outside 0..255, so the game truncates it. Enter a value from 0 through 255 in recipe 'row'"
        );
        assert_eq!(
            diags[1].message,
            "cubemain.txt, line 2: 'c ilvl' value '+12' for 'output c' is outside 0..255, so the game truncates it. Enter a value from 0 through 255 in recipe 'row'"
        );
        assert!(
            diags
                .iter()
                .all(|diag| !diag.message.contains("suffix is ignored")),
            "present noncanonical values must not be overclaimed as ignored: {diags:#?}"
        );
    }

    #[tokio::test]
    async fn cube_output_check_keeps_incomplete_lookup_guard_and_disabled_rows() {
        let incomplete = fixture(&[(
            "cubemain",
            "description\tenabled\toutput\tilvl\nrow\t1\tunknown-base,qty=999\t256\n",
        )]);
        let diags = run_plugin("cubeOutputCheck.ts", "cubemain", &incomplete).await;
        assert!(
            diags.is_empty(),
            "incomplete lookup must suppress inline and separate-byte claims equally: {diags:#?}"
        );

        let files = with_cube_output_files((
            "cubemain",
            "description\tenabled\toutput\nrow\t0\tunknown-base\n",
        ));
        let fx = fixture(&files);
        let diags = run_plugin("cubeOutputCheck.ts", "cubemain", &fx).await;
        assert!(
            diags.is_empty(),
            "disabled recipes should stay ignored: {diags:#?}"
        );
    }

    #[tokio::test]
    async fn cube_output_hover_explains_ordinal_and_preserved_prefix() {
        let files = with_cube_output_files((
            "cubemain",
            "description\tenabled\tinput 2\toutput b\nrow\t1\tcap\tuseitem,pre,162,noe\n",
        ));
        let fx = fixture(&files);
        let host = PluginHost::new(vec![plugin_path("cubeOutputCheck.ts")]).unwrap();
        let doc = fx.docs.get("cubemain").expect("cubemain fixture");
        let ctx = build_hover_context("cubemain", "output b", "useitem,pre,162,noe", 1, doc);

        let hover = host
            .hover(ctx, Arc::clone(&fx.index), Arc::clone(&fx.snapshot))
            .await
            .expect("cube output hover");
        assert!(
            hover.contains("input 2"),
            "ordinal mapping missing: {hover}"
        );
        assert!(
            hover.contains("Prefix ID: 162"),
            "applied prefix missing: {hover}"
        );
        assert!(
            hover.contains("Ignored text begins at"),
            "ignored suffix missing: {hover}"
        );
        assert!(
            hover.contains("base and modifiers before it still work"),
            "prefix preservation missing: {hover}"
        );
    }

    #[tokio::test]
    async fn cube_output_hover_omits_the_effective_reference_source() {
        let files =
            with_cube_output_files(("cubemain", "description\tenabled\toutput\nrow\t1\thpot\n"));
        let mut fx = fixture(&files);
        Arc::get_mut(&mut fx.snapshot)
            .expect("unshared fixture snapshot")
            .sources
            .insert(
                "weapons".into(),
                WorkspaceSourceInfo {
                    kind: "sibling".into(),
                    version: Some("3.2".into()),
                },
            );
        let host = PluginHost::new(vec![plugin_path("cubeOutputCheck.ts")]).unwrap();
        let doc = fx.docs.get("cubemain").expect("cubemain fixture");
        let ctx = build_hover_context("cubemain", "output", "hpot", 1, doc);

        let hover = host
            .hover(ctx, Arc::clone(&fx.index), Arc::clone(&fx.snapshot))
            .await
            .expect("cube output hover");
        assert!(
            hover.contains("four-character item code") && !hover.contains("Source:"),
            "{hover}"
        );
    }

    #[tokio::test]
    async fn cube_output_hover_does_not_claim_preservation_for_an_invalid_base() {
        let files = with_cube_output_files((
            "cubemain",
            "description\tenabled\toutput\nrow\t1\tbad-base,mag,noe\n",
        ));
        let fx = fixture(&files);
        let host = PluginHost::new(vec![plugin_path("cubeOutputCheck.ts")]).unwrap();
        let doc = fx.docs.get("cubemain").expect("cubemain fixture");
        let ctx = build_hover_context("cubemain", "output", "bad-base,mag,noe", 1, doc);

        let hover = host
            .hover(ctx, Arc::clone(&fx.index), Arc::clone(&fx.snapshot))
            .await
            .expect("cube output hover");
        assert!(
            hover.contains("Unknown output value"),
            "invalid base status missing: {hover}"
        );
        assert!(
            hover.contains("the game does not create this output"),
            "failed-base parser result missing: {hover}"
        );
        assert!(
            !hover.contains("Applied modifiers")
                && !hover.contains("Modifiers the game will use")
                && !hover.contains("stored u8")
                && !hover.contains("are preserved"),
            "failed base must not be described as an applied, stored, or preserved output: {hover}"
        );
    }

    #[tokio::test]
    async fn cube_input_check_reports_legacy_messages_on_whole_cell_ranges() {
        let cases = [
            (
                "invalid base",
                "desc\tinput 1\nr\tbadbase,qty=1\n",
                (1, 2, 15),
                "couldn't find 'badbase' for input 1",
            ),
            (
                "invalid modifier key",
                "desc\tinput 1\nr\thpot,foo=1\n",
                (1, 2, 12),
                "The game stops at 'foo=1'",
            ),
            (
                "invalid modifier value",
                "desc\tinput 1\nr\thpot,qty=abc\n",
                (1, 2, 14),
                "input quantity 'abc'",
            ),
            (
                "missing modifier value",
                "desc\tinput 1\nr\thpot,qty=\n",
                (1, 2, 11),
                "input quantity ''",
            ),
            (
                "quoted input",
                "desc\tinput 1\nr\t\"hpot,qty=abc\"\n",
                (1, 2, 16),
                "input quantity 'abc'",
            ),
        ];

        for (label, cubemain, expected, message) in cases {
            let files = with_base_files(("cubemain", cubemain));
            let fx = fixture(&files);
            let diags = run_plugin("cubeInputCheck.ts", "cubemain", &fx).await;
            assert_eq!(first_range(&diags), expected, "{label}");
            assert!(
                diags[0].message.contains(message),
                "{label}: message mismatch: {:?}",
                diags[0].message
            );
        }
    }

    #[tokio::test]
    async fn cube_input_check_keeps_incomplete_workspace_guard_and_disabled_rows() {
        let fx = fixture(&[("cubemain", "desc\tinput 1\nr\tbadbase\n")]);
        let diags = run_plugin("cubeInputCheck.ts", "cubemain", &fx).await;
        assert!(
            diags.is_empty(),
            "incomplete lookup evidence should not flag invalid bases: {diags:#?}"
        );

        let partial_itemtypes = vec![
            ("weapons", "code\nhpot\n"),
            ("armor", "code\ncap\n"),
            ("misc", "code\nkey\n"),
            ("itemtypes", "Code\nweap\n"),
            ("uniqueitems", "index\nThe Gnasher\n"),
            ("setitems", "index\nHsarus' Iron Heel\n"),
            (
                "treasureclassex",
                "Treasure Class\tItem1\tProb1\nAct 1\tbaditem\t1\n",
            ),
        ];
        let fx = fixture(&partial_itemtypes);
        let diags = run_plugin("tcItemCheck.ts", "treasureclassex", &fx).await;
        assert!(
            diags.is_empty(),
            "missing ItemTypes.TreasureClass evidence must defer invalid-base policy: {diags:#?}"
        );

        let files = with_base_files(("cubemain", "desc\tinput 1\n*disabled\tbadbase\n"));
        let fx = fixture(&files);
        let diags = run_plugin("cubeInputCheck.ts", "cubemain", &fx).await;
        assert!(
            diags.is_empty(),
            "disabled rows should stay ignored: {diags:#?}"
        );
    }

    #[tokio::test]
    async fn tc_item_check_reports_legacy_messages_on_whole_cell_ranges() {
        let cases = [
            (
                "invalid base",
                "Treasure Class\tPicks\tNoDrop\tItem1\tProb1\nAct 1\t1\t0\tbaditem,mul=1280\t1\n",
                (1, 10, 26),
                "can't find 'baditem' for 'item1'",
            ),
            (
                "invalid modifier key",
                "Treasure Class\tPicks\tNoDrop\tItem1\tProb1\nAct 1\t1\t0\thpot,zz=1\t1\n",
                (1, 10, 19),
                "The game stops at 'zz=1'",
            ),
            (
                "missing modifier value",
                "Treasure Class\tPicks\tNoDrop\tItem1\tProb1\nAct 1\t1\t0\thpot,mul=\t1\n",
                (1, 10, 19),
                "The game stops at 'mul='",
            ),
            (
                "quoted modifier",
                "Treasure Class\tPicks\tNoDrop\tItem1\tProb1\nAct 1\t1\t0\t\"hpot,zz=1\"\t1\n",
                (1, 10, 21),
                "The game stops at 'zz=1'",
            ),
            (
                "forward reference",
                "Treasure Class\tPicks\tNoDrop\tItem1\tProb1\nFirst\t1\t0\tLater\t1\nLater\t1\t0\thpot\t1\n",
                (1, 10, 15),
                "can't find 'Later' for 'item1'",
            ),
        ];

        for (label, treasureclassex, expected, message) in cases {
            let files = with_base_files(("treasureclassex", treasureclassex));
            let fx = fixture(&files);
            let diags = run_plugin("tcItemCheck.ts", "treasureclassex", &fx).await;
            assert_eq!(first_range(&diags), expected, "{label}");
            assert!(
                diags[0].message.contains(message),
                "{label}: message mismatch: {:?}",
                diags[0].message
            );
        }
    }

    #[tokio::test]
    async fn tc_item_check_keeps_incomplete_workspace_guard() {
        let fx = fixture(&[(
            "treasureclassex",
            "Treasure Class\tPicks\tNoDrop\tItem1\tProb1\nAct 1\t1\t0\tbaditem\t1\n",
        )]);
        let diags = run_plugin("tcItemCheck.ts", "treasureclassex", &fx).await;
        assert!(
            diags.is_empty(),
            "incomplete lookup evidence should not flag invalid bases: {diags:#?}"
        );
    }

    #[tokio::test]
    async fn tc_item_check_treats_a_missing_item_header_as_the_first_empty_slot() {
        let files = with_base_files((
            "treasureclassex",
            "Treasure Class\tItem1\tProb1\tItem3\tProb3\nHeader gap\thpot\t1\tbad-after-gap\t7\n",
        ));
        let fx = fixture(&files);
        let diags = run_plugin("tcItemCheck.ts", "treasureclassex", &fx).await;

        assert_eq!(
            diags.len(),
            2,
            "missing Item2 must terminate sequential slots: {diags:#?}"
        );
        assert_code(&diags[0], "tc-item.after-first-gap");
        assert_code(&diags[1], "tc-prob.after-first-gap");
        assert!(
            diags
                .iter()
                .all(|diag| diag.message.contains("first empty Item slot"))
        );
    }

    #[tokio::test]
    async fn tc_item_check_reports_probability_omissions_without_gap_or_orphan_duplicates() {
        let files = with_base_files((
            "treasureclassex",
            "Treasure Class\tItem1\tProb1\n\
             Blank\thpot\t\n\
             Zero\thpot\t0\n\
             Negative\thpot\t-1\n\
             Noncanonical\thpot\tabc\n\
             Quoted zero\thpot\t\"0\"\n\
             Valid\thpot\t1\n\
             Orphan\t\t7\n",
        ));
        let fx = fixture(&files);
        let diags = run_plugin("tcItemCheck.ts", "treasureclassex", &fx).await;

        assert_eq!(
            diags.len(),
            6,
            "four omission forms plus one quoted zero and one orphan: {diags:#?}"
        );
        let expected = [
            (
                "tc-prob.blank-omission",
                "treasureclassex.txt, line 2: This Treasure Class entry is skipped because 'prob1' is blank ('item1').",
            ),
            (
                "tc-prob.nonpositive-omission",
                "treasureclassex.txt, line 3: This Treasure Class entry is skipped because 'prob1' is 0 ('item1').",
            ),
            (
                "tc-prob.nonpositive-omission",
                "treasureclassex.txt, line 4: This Treasure Class entry is skipped because 'prob1' is -1 ('item1').",
            ),
            (
                "tc-prob.noncanonical",
                "treasureclassex.txt, line 5: 'prob1' is not a whole number and may cause 'item1' to be skipped.",
            ),
            (
                "tc-prob.nonpositive-omission",
                "treasureclassex.txt, line 6: This Treasure Class entry is skipped because 'prob1' is 0 ('item1').",
            ),
        ];
        for (diag, (code, message)) in diags.iter().zip(expected).take(5) {
            assert_code(diag, code);
            assert_eq!(diag.severity, Some(DiagnosticSeverity::WARNING));
            assert_eq!(diag.message, message);
        }
        assert_eq!(
            range(&diags[0]).1,
            range(&diags[0]).2,
            "blank Prob is an insertion range"
        );
        assert_code(&diags[5], "tc-prob.orphaned");
        assert_eq!(
            diags
                .iter()
                .filter(|diag| diag.range.start.line == 7)
                .count(),
            1,
            "an orphan probability must not also receive an omission diagnostic"
        );

        let missing_prob_files = with_base_files((
            "treasureclassex",
            "Treasure Class\tItem1\tItem2\tProb2\nNo Prob1\thpot\tcap\t1\n",
        ));
        let missing_prob = fixture(&missing_prob_files);
        assert!(
            run_plugin("tcItemCheck.ts", "treasureclassex", &missing_prob)
                .await
                .is_empty(),
            "a missing Prob header is not a blank probability cell"
        );
    }

    #[tokio::test]
    async fn cube_input_hover_and_definition_still_use_the_base_token() {
        let files = with_base_files(("cubemain", "desc\tinput 1\nr\thpot,qty=3\n"));
        let fx = fixture(&files);
        let host = PluginHost::new(vec![plugin_path("cubeInputCheck.ts")]).unwrap();
        let doc = fx.docs.get("cubemain").expect("cubemain fixture");
        let ctx = build_hover_context("cubemain", "input 1", "hpot,qty=3", 1, doc);

        let hover = host
            .hover(ctx.clone(), Arc::clone(&fx.index), Arc::clone(&fx.snapshot))
            .await
            .expect("hover content");
        assert!(
            hover.contains("hpot"),
            "hover should name the base: {hover}"
        );
        assert!(
            hover.contains("Quantity: 3"),
            "hover should include modifier text: {hover}"
        );
        assert!(
            !hover.contains("Source:"),
            "hover should omit reference provenance: {hover}"
        );

        let target = host
            .goto_definition(ctx, Arc::clone(&fx.index), Arc::clone(&fx.snapshot))
            .await
            .expect("definition target");
        assert_eq!(
            target,
            (
                "weapons".to_string(),
                "code".to_string(),
                "hpot".to_string()
            )
        );
    }

    #[tokio::test]
    async fn single_token_item_and_prop_checks_still_cover_the_cell() {
        let item_files = with_base_files(("uniqueitems", "index\tcode\nUnique Bad\tbad\n"));
        let fx = fixture(&item_files);
        let diags = run_plugin("itemCodeCheck.ts", "uniqueitems", &fx).await;
        assert_eq!(first_range(&diags), (1, 11, 14));
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
        assert_code(&diags[0], "item-code.unresolved-policy");

        let prop_files = vec![
            ("cubemain", "desc\tmod 1\nr\tbadprop\n"),
            ("properties", "code\nvalidprop\n"),
            ("propertygroups", "code\nvalidgroup\n"),
        ];
        let mut fx = fixture(&prop_files);
        Arc::get_mut(&mut fx.snapshot)
            .expect("unshared fixture snapshot")
            .sources
            .insert(
                "properties".into(),
                WorkspaceSourceInfo {
                    kind: "bundled".into(),
                    version: Some("3.2".into()),
                },
            );
        let diags = run_plugin("propCodeCheck.ts", "cubemain", &fx).await;
        assert_eq!(first_range(&diags), (1, 2, 9));
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
        assert_code(&diags[0], "property.unknown-code");

        let host = PluginHost::new(vec![plugin_path("propCodeCheck.ts")]).unwrap();
        let doc = fx.docs.get("cubemain").expect("cubemain fixture");
        let ctx = build_hover_context("cubemain", "mod 1", "validprop", 1, doc);
        let hover = host
            .hover(ctx, Arc::clone(&fx.index), Arc::clone(&fx.snapshot))
            .await
            .expect("property hover");
        assert!(
            hover.contains("properties.txt code") && !hover.contains("Source:"),
            "{hover}"
        );
    }

    #[tokio::test]
    async fn property_checks_follow_the_selected_versions_propertygroups_capability() {
        let mut files = base_lookup_files();
        files.push(("properties", "code\nknown\n"));
        files.push((
            "cubemain",
            "description\tenabled\tmod 1\tb mod 1\nrow\t1\tmissing\tmissing\n",
        ));

        let mut old = fixture(&files);
        Arc::get_mut(&mut old.snapshot)
            .expect("unshared old-version snapshot")
            .sources
            .get_mut("properties")
            .expect("properties source")
            .version = Some("1.13c".into());
        let prop_diags = run_plugin("propCodeCheck.ts", "cubemain", &old).await;
        let output_diags = run_plugin("cubeOutputCheck.ts", "cubemain", &old).await;
        assert_eq!(
            prop_diags.len(),
            1,
            "1.13c properties-only lookup: {prop_diags:#?}"
        );
        assert_eq!(
            output_diags.len(),
            1,
            "1.13c output property lookup: {output_diags:#?}"
        );

        let mut modern = fixture(&files);
        Arc::get_mut(&mut modern.snapshot)
            .expect("unshared modern-version snapshot")
            .sources
            .get_mut("properties")
            .expect("properties source")
            .version = Some("3.2".into());
        assert!(
            run_plugin("propCodeCheck.ts", "cubemain", &modern)
                .await
                .is_empty(),
            "3.2 must wait for propertygroups before proving a missing property"
        );
        assert!(
            run_plugin("cubeOutputCheck.ts", "cubemain", &modern)
                .await
                .is_empty(),
            "3.2 cube output must wait for propertygroups before proving a missing property"
        );
    }

    #[tokio::test]
    async fn calc_check_models_revalidated_function_case_arity_and_prefix_policy() {
        let valid = [
            "MIN(1,2)",
            "mAx(1,2)",
            "RAND(1,2)",
            "SKILL('x'.lvl)",
            "Miss('x'.lvl)",
            "STAT('x'.base)",
            "Sklvl('x'.lvl.blvl)",
            "SkSrC('x'.lvl)",
            "CoNd('DESECRATED')",
            "COND('difficulty',hell)",
        ];
        for formula in valid {
            let misc = format!("code\tlen\nrow\t{formula}\n");
            let fx = fixture(&[("misc", misc.as_str())]);
            let diags = run_plugin("calcCheck.ts", "misc", &fx).await;
            assert!(diags.is_empty(), "valid {formula}: {diags:#?}");
        }

        let wrong_arity = [
            "min(1)",
            "max(1,2,3)",
            "rand()",
            "skill('x')",
            "miss('x'.lvl.extra)",
            "stat('x')",
            "sklvl('x'.lvl)",
            "sksrc('x'.lvl.extra)",
            "cond('difficulty')",
        ];
        for formula in wrong_arity {
            let misc = format!("code\tlen\nrow\t{formula}\n");
            let fx = fixture(&[("misc", misc.as_str())]);
            let diags = run_plugin("calcCheck.ts", "misc", &fx).await;
            assert_eq!(diags.len(), 1, "wrong arity {formula}: {diags:#?}");
            assert_code(&diags[0], "calc.wrong-arity");
            assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
        }

        let fx = fixture(&[("misc", "code\tlen\nrow\tcond('MonsterHasMod',1)\n")]);
        let diags = run_plugin("calcCheck.ts", "misc", &fx).await;
        assert_eq!(diags.len(), 1, "unverified condition must not be accepted");
        assert_code(&diags[0], "unknownCondition");

        for (formula, code) in [
            ("min(1,2", "calc.expected-rparen.eof"),
            ("1%2", "calc.prefix-stop"),
            ("1.5+2", "calc.decimal-policy"),
            ("1+2.5", "calc.decimal-policy"),
        ] {
            let misc = format!("code\tlen\nrow\t{formula}\n");
            let fx = fixture(&[("misc", misc.as_str())]);
            let diags = run_plugin("calcCheck.ts", "misc", &fx).await;
            assert_eq!(diags.len(), 1, "policy case {formula}: {diags:#?}");
            assert_code(&diags[0], code);
            assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
        }

        for (formula, code) in [
            ("%2", "calc.unexpected-character"),
            ("@ignored", "calc.unexpected-character"),
            ("1+@ignored", "calc.unexpected-character"),
            ("1.5+", "calc.unexpected-eof"),
        ] {
            let misc = format!("code\tlen\nrow\t{formula}\n");
            let fx = fixture(&[("misc", misc.as_str())]);
            let diags = run_plugin("calcCheck.ts", "misc", &fx).await;
            assert_eq!(diags.len(), 1, "hard syntax case {formula}: {diags:#?}");
            assert_code(&diags[0], code);
            assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
        }

        let monpet = fixture(&[("monpet", "Id\tcalc1\nrow\t1.5\n")]);
        let diags = run_plugin("calcCheck.ts", "monpet", &monpet).await;
        assert!(
            diags.is_empty(),
            "decimal policy must remain excluded from MonPet: {diags:#?}"
        );
    }

    #[tokio::test]
    async fn skill_param_aliases_are_explained_for_confirmed_skill_scope_versions() {
        fn versioned_fixture(file: &str, text: &str, version: &str) -> PluginFixture {
            let skillcalc = "code\npar1\npar2\npar3\npa10\npa11\npa12\npa13\npa14\npa15\npa16\npa17\npa18\npa19\npa20\n";
            let mut fx = fixture(&[("skillcalc", skillcalc), (file, text)]);
            Arc::get_mut(&mut fx.snapshot)
                .expect("unshared fixture snapshot")
                .sources
                .get_mut(file)
                .expect("versioned source")
                .version = Some(version.into());
            fx
        }

        for version in ["3.1", "3.2"] {
            for parameter in 10..=20 {
                let identifier = format!("par{parameter}");
                let text = format!("skill\tcalc1\nrow\t{identifier}\n");
                let fx = versioned_fixture("skills", &text, version);
                let diags = run_plugin("calcCheck.ts", "skills", &fx).await;
                assert_eq!(diags.len(), 1, "{version} {identifier}: {diags:#?}");
                let diag = &diags[0];
                let interpreted_as = if parameter == 20 { "par2" } else { "par1" };
                assert_eq!(diag.severity, Some(DiagnosticSeverity::WARNING));
                assert_code(diag, "calc.skill-param-alias");
                assert_eq!(
                    diag.message,
                    format!(
                        "{identifier} is interpreted as {interpreted_as} because SkillCalc identifiers use only the first four characters."
                    )
                );
                assert_eq!(range(diag), (1, 4, 4 + identifier.len() as u32));
                assert_eq!(data_str(diag, "kind"), "identifier-alias");
                assert_eq!(data_str(diag, "scope"), "Skill scope BBE");
                assert_eq!(data_str(diag, "identifier"), identifier);
                assert_eq!(data_str(diag, "interpretedAs"), interpreted_as);
                assert_eq!(data_str(diag, "suggestion"), format!("pa{parameter}"));
                assert_eq!(data_str(diag, "parameter"), format!("Param{parameter}"));
            }
        }

        let skilldesc =
            versioned_fixture("skilldesc", "skilldesc\tdsc3calca1\nrow\tpar20\n", "3.2");
        let skilldesc_diags = run_plugin("calcCheck.ts", "skilldesc", &skilldesc).await;
        assert_code(&skilldesc_diags[0], "calc.skill-param-alias");
        assert_eq!(data_str(&skilldesc_diags[0], "interpretedAs"), "par2");

        let older = versioned_fixture("skills", "skill\tcalc1\nrow\tpar10\n", "2.4");
        let older_diags = run_plugin("calcCheck.ts", "skills", &older).await;
        assert_eq!(older_diags.len(), 1, "{older_diags:#?}");
        assert_eq!(older_diags[0].severity, Some(DiagnosticSeverity::ERROR));
        assert_code(&older_diags[0], "unknownIdentifier");
        assert_eq!(
            older_diags[0].message,
            "Invalid calculation: Unknown identifier 'par10' for this BBE scope"
        );

        let uppercase = versioned_fixture("skills", "skill\tcalc1\nrow\tPAR10\n", "3.2");
        let uppercase_diags = run_plugin("calcCheck.ts", "skills", &uppercase).await;
        assert_code(&uppercase_diags[0], "unknownIdentifier");

        let legacy_113c = versioned_fixture("skills", "skill\tcalc1\nrow\tpar34\n", "1.13c");
        let legacy_113c_diags = run_plugin("calcCheck.ts", "skills", &legacy_113c).await;
        assert_eq!(legacy_113c_diags.len(), 1, "{legacy_113c_diags:#?}");
        let legacy_113c_diag = &legacy_113c_diags[0];
        assert_eq!(legacy_113c_diag.severity, Some(DiagnosticSeverity::WARNING));
        assert_code(legacy_113c_diag, "calc.skill-param-alias");
        assert_eq!(data_str(legacy_113c_diag, "kind"), "identifier-alias");
        assert_eq!(data_str(legacy_113c_diag, "scope"), "Skill scope BBE");
        assert_eq!(data_str(legacy_113c_diag, "identifier"), "par34");
        assert_eq!(data_str(legacy_113c_diag, "interpretedAs"), "par3");
        assert_eq!(
            data_str(legacy_113c_diag, "lookup"),
            "first-four-byte exact case-sensitive"
        );

        let legacy_113c_korean =
            run_plugin_localized("calcCheck.ts", "skills", &legacy_113c, Locale::KoKr).await;
        assert_eq!(legacy_113c_korean.len(), 1, "{legacy_113c_korean:#?}");
        let korean_diag = &legacy_113c_korean[0];
        assert!(
            korean_diag.message.contains("par34"),
            "{}",
            korean_diag.message
        );
        assert!(
            korean_diag.message.contains("par3"),
            "{}",
            korean_diag.message
        );
        assert!(
            !korean_diag.message.contains("{\"identifier\""),
            "{}",
            korean_diag.message
        );
        assert!(
            !korean_diag.message.contains("올바른 식별자: par34"),
            "{}",
            korean_diag.message
        );
        assert_eq!(data_str(korean_diag, "identifier"), "par34");
        assert_eq!(data_str(korean_diag, "interpretedAs"), "par3");
        assert_eq!(
            korean_diag.data.as_ref().unwrap()["messageArgs"]["alias"],
            "par34"
        );
        assert_eq!(
            korean_diag.data.as_ref().unwrap()["messageArgs"]["identifier"],
            "par3"
        );
        let guidance = korean_diag.data.as_ref().unwrap()["localizedGuidance"]
            .as_str()
            .expect("localized guidance");
        assert!(guidance.contains("`par34`"), "{guidance}");
        assert!(guidance.contains("`par3`"), "{guidance}");
        assert!(!guidance.contains("`par3` 참조에는 `par3`"), "{guidance}");

        let unknown_113c = versioned_fixture("skills", "skill\tcalc1\nrow\tnope5\n", "1.13c");
        let unknown_113c_diags = run_plugin("calcCheck.ts", "skills", &unknown_113c).await;
        assert_eq!(unknown_113c_diags.len(), 1, "{unknown_113c_diags:#?}");
        assert_eq!(
            unknown_113c_diags[0].severity,
            Some(DiagnosticSeverity::ERROR)
        );
        assert_code(&unknown_113c_diags[0], "unknownIdentifier");

        let uppercase_113c = versioned_fixture("skills", "skill\tcalc1\nrow\tPAR34\n", "1.13c");
        let uppercase_113c_diags = run_plugin("calcCheck.ts", "skills", &uppercase_113c).await;
        assert_eq!(uppercase_113c_diags.len(), 1, "{uppercase_113c_diags:#?}");
        assert_eq!(
            uppercase_113c_diags[0].severity,
            Some(DiagnosticSeverity::ERROR)
        );
        assert_code(&uppercase_113c_diags[0], "unknownIdentifier");

        let missile = fixture(&[
            ("misscalc", "code\npar1\n"),
            ("missiles", "Missile\tSrvCalc1\nrow\tpar10\n"),
        ]);
        assert!(
            run_plugin("calcCheck.ts", "missiles", &missile)
                .await
                .is_empty(),
            "Missile first-four lookup remains valid without a Skill-param warning"
        );
    }

    #[tokio::test]
    async fn skilldesc_3_2_decimal_warning_reports_used_prefix_and_ignored_suffix() {
        async fn run(formula: &str) -> Vec<Diagnostic> {
            let text = format!("skilldesc\tdsc3calca1\tdsc3calca2\nrow\t\t{formula}\n");
            let mut fx = fixture(&[("skilldesc", text.as_str())]);
            Arc::get_mut(&mut fx.snapshot)
                .expect("unshared fixture snapshot")
                .sources
                .get_mut("skilldesc")
                .expect("skilldesc source")
                .version = Some("3.2".into());
            run_plugin("calcCheck.ts", "skilldesc", &fx).await
        }

        assert!(run("-6").await.is_empty());
        for (formula, consumed, ignored) in [
            ("6.25", "6", ".25"),
            ("-6.25", "-6", ".25"),
            ("6.", "6", "."),
            ("6.25+1", "6", ".25+1"),
            ("-6.25+1", "-6", ".25+1"),
        ] {
            let diagnostics = run(formula).await;
            assert_eq!(diagnostics.len(), 1, "{formula}: {diagnostics:#?}");
            let diagnostic = &diagnostics[0];
            assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::WARNING));
            assert_code(diagnostic, "calc.skilldesc-decimal-prefix");
            assert_eq!(
                diagnostic.message,
                format!(
                    "Decimal values are not supported here. The game reads '{formula}' as '{consumed}' and ignores '{ignored}'."
                )
            );
            assert_eq!(data_str(diagnostic, "consumedPrefix"), consumed);
            assert_eq!(data_str(diagnostic, "ignoredSuffix"), ignored);
            assert!(!diagnostic.message.contains("compiler"));
            assert!(!diagnostic.message.contains("bytecode"));
        }

        let invalid_start = run(".25").await;
        assert_eq!(invalid_start.len(), 1, "{invalid_start:#?}");
        assert_eq!(invalid_start[0].severity, Some(DiagnosticSeverity::ERROR));
        assert_ne!(
            invalid_start[0].code,
            Some(NumberOrString::String(
                "calc.skilldesc-decimal-prefix".into()
            ))
        );

        let mut older = fixture(&[(
            "skilldesc",
            "skilldesc\tdsc3calca1\tdsc3calca2\nrow\t\t-6.25\n",
        )]);
        Arc::get_mut(&mut older.snapshot)
            .expect("unshared fixture snapshot")
            .sources
            .get_mut("skilldesc")
            .expect("skilldesc source")
            .version = Some("3.1".into());
        let older_diagnostics = run_plugin("calcCheck.ts", "skilldesc", &older).await;
        assert_code(&older_diagnostics[0], "calc.decimal-policy");
        assert!(
            !older_diagnostics[0]
                .message
                .contains("game reads '-6.25' as '-6'")
        );
    }

    #[tokio::test]
    async fn cube_input_check_matches_parameter_and_ignored_suffix_grammar() {
        let files = with_base_files((
            "cubemain",
            "desc\tinput 1\tinput 2\tinput 3\tinput 4\tinput 5\n\
             valid\thpot,qty,3\thpot,qty=3\thpot,sock\tqty=3,hpot,id\tqty,3,hpot,id\n",
        ));
        let fx = fixture(&files);
        let diags = run_plugin("cubeInputCheck.ts", "cubemain", &fx).await;
        assert!(
            diags.is_empty(),
            "binary-equivalent input forms: {diags:#?}"
        );

        let files = with_base_files((
            "cubemain",
            "desc\tinput 1\tinput 2\tinput 3\tinput 4\tinput 5\tinput 6\tinput 7\n\
             bad\thpot,brk\thpot,MAG\thpot, mag\thpot,,mag\thpot,sock=3\thpot,sock,3\thpot,qty,256\n",
        ));
        let fx = fixture(&files);
        let diags = run_plugin("cubeInputCheck.ts", "cubemain", &fx).await;
        assert_eq!(
            diags.len(),
            7,
            "each first ignored suffix/range: {diags:#?}"
        );
        assert!(
            diags
                .iter()
                .all(|d| d.severity == Some(DiagnosticSeverity::WARNING))
        );
        assert_eq!(
            diags
                .iter()
                .filter(
                    |d| d.code == Some(NumberOrString::String("cube-input.ignored-suffix".into()))
                )
                .count(),
            6
        );
        assert!(
            diags
                .iter()
                .any(|d| d.code == Some(NumberOrString::String("cube-input.u8-range".into())))
        );
        assert!(
            diags
                .iter()
                .any(|d| { d.message.contains("The game stops at 'sock=3'") })
        );
        assert!(diags.iter().any(|d| d.message.contains("outside 0..255")));
    }

    #[tokio::test]
    async fn cube_input_check_matches_legacy_numinputs_relationship() {
        let files = with_base_files((
            "cubemain",
            "description\tenabled\tnuminputs\tinput 1\tinput 2\n\
             match\t1\t3\thpot,qty,3\t\n\
             mismatch\t1\t2\thpot\t\n\
             zero\t1\t0\tbadbase\t\n\
             invalid\t1\tabc\tbadbase\t\n\
             disabled\t0\t99\tbadbase\t\n",
        ));
        let fx = fixture(&files);
        let diags = run_plugin("cubeInputCheck.ts", "cubemain", &fx).await;

        assert_eq!(diags.len(), 3, "numinputs parity diagnostics: {diags:#?}");
        assert!(
            diags
                .iter()
                .all(|diag| diag.severity == Some(DiagnosticSeverity::WARNING))
        );

        let mismatch = &diags[0];
        assert_code(mismatch, "cube-input.numinputs-mismatch");
        assert_eq!(range(mismatch), (2, 11, 12));
        assert_eq!(
            mismatch.message,
            "cubemain.txt, line 3: wrong numinputs. expected 1, found 2 in recipe 'mismatch'"
        );

        let zero = &diags[1];
        assert_code(zero, "cube-input.no-inputs");
        assert_eq!(range(zero), (3, 7, 8));
        assert_eq!(
            zero.message,
            "cubemain.txt, line 4: no inputs for recipe 'zero'"
        );

        let invalid = &diags[2];
        assert_code(invalid, "cube-input.invalid-numinputs");
        assert_eq!(range(invalid), (4, 10, 13));
        assert_eq!(
            invalid.message,
            "cubemain.txt, line 5: invalid value for 'numinputs' for recipe 'invalid'"
        );
    }

    #[tokio::test]
    async fn cube_input_and_rune_item_codes_use_fixed_four_byte_lookup() {
        let files = vec![
            (
                "weapons",
                "code\tname\nstaf\tStaff\n가xA\tMultibyte\nabcé\tBoundary A\n",
            ),
            ("armor", "code\tname\ncap\tCap\n"),
            ("misc", "code\tname\nhpot\tHealing Potion\n"),
            (
                "itemtypes",
                "Code\tTreasureClass\tItemType\nweap\t1\tWeapon\n",
            ),
            ("uniqueitems", "index\nThe Gnasher\n"),
            ("setitems", "index\nHsarus' Iron Heel\n"),
            (
                "cubemain",
                "desc\tinput 1\tinput 2\tinput 3\tinput 4\nrow\tstaff\tSTAF\t가x\tabc€\n",
            ),
        ];
        let fx = fixture(&files);
        let diags = run_plugin("cubeInputCheck.ts", "cubemain", &fx).await;
        assert_eq!(
            diags.len(),
            2,
            "staff -> staf and multibyte first-four bytes resolve, while case and truncated UTF-8 prefixes stay exact: {diags:#?}"
        );
        assert_eq!(range(&diags[0]), (1, 10, 14));
        assert_code(&diags[1], "cube-input.invalid-base");

        let host = PluginHost::new(vec![plugin_path("cubeInputCheck.ts")]).unwrap();
        let doc = fx.docs.get("cubemain").expect("cube fixture");
        let multibyte_ctx = build_hover_context("cubemain", "input 3", "가x", 1, doc);
        let hover = host
            .hover(
                multibyte_ctx.clone(),
                Arc::clone(&fx.index),
                Arc::clone(&fx.snapshot),
            )
            .await
            .expect("multibyte cube-input hover");
        assert!(hover.contains("Multibyte"), "{hover}");
        assert_eq!(
            host.goto_definition(
                multibyte_ctx,
                Arc::clone(&fx.index),
                Arc::clone(&fx.snapshot),
            )
            .await,
            Some(("weapons".into(), "code".into(), "가xA".into()))
        );

        let rune_files = vec![
            (
                "weapons",
                "code\tname\nstaf\tStaff\n가xA\tMultibyte\nabcé\tBoundary A\n",
            ),
            ("armor", "code\tname\ncap\tCap\n"),
            ("misc", "code\tname\nhpot\tHealing Potion\n"),
            (
                "runes",
                "Name\tRune1\tRune2\tRune3\tRune4\tRune5\tRune6\nrow\tstaff\tHPOT\t가x\tabc€\t\t\n",
            ),
        ];
        let mut fx = fixture(&rune_files);
        Arc::get_mut(&mut fx.snapshot)
            .expect("unshared fixture snapshot")
            .sources
            .insert(
                "weapons".into(),
                WorkspaceSourceInfo {
                    kind: "sibling".into(),
                    version: Some("3.2".into()),
                },
            );
        let diags = run_plugin("itemCodeCheck.ts", "runes", &fx).await;
        assert_eq!(
            diags.len(),
            2,
            "actual Rune# headers and lossless first-four byte identity must be enforced: {diags:#?}"
        );
        assert_eq!(range(&diags[0]), (1, 10, 14));
        assert_code(&diags[0], "item-code.unresolved");
        assert_code(&diags[1], "item-code.unresolved");

        let host = PluginHost::new(vec![plugin_path("itemCodeCheck.ts")]).unwrap();
        let doc = fx.docs.get("runes").expect("runes fixture");
        let ctx = build_hover_context("runes", "Rune1", "staff", 1, doc);
        let hover = host
            .hover(ctx.clone(), Arc::clone(&fx.index), Arc::clone(&fx.snapshot))
            .await
            .expect("Rune hover");
        assert!(
            hover.contains("Staff") && !hover.contains("Source:"),
            "{hover}"
        );
        let target = host
            .goto_definition(ctx, Arc::clone(&fx.index), Arc::clone(&fx.snapshot))
            .await
            .expect("Rune fixed4 definition");
        assert_eq!(target, ("weapons".into(), "code".into(), "staf".into()));

        let multibyte_ctx = build_hover_context("runes", "Rune3", "가x", 1, doc);
        let hover = host
            .hover(
                multibyte_ctx.clone(),
                Arc::clone(&fx.index),
                Arc::clone(&fx.snapshot),
            )
            .await
            .expect("Rune multibyte fixed4 hover");
        assert!(hover.contains("Multibyte"), "{hover}");
        assert_eq!(
            host.goto_definition(
                multibyte_ctx,
                Arc::clone(&fx.index),
                Arc::clone(&fx.snapshot),
            )
            .await,
            Some(("weapons".into(), "code".into(), "가xA".into()))
        );
    }

    #[tokio::test]
    async fn item_code_check_separates_engine_lookup_from_unproven_policy_fields() {
        let files = vec![
            ("weapons", "code\tname\nstaf\tStaff\n"),
            ("armor", "code\tname\ncap\tCap\n"),
            ("misc", "code\tname\nhpot\tHealing Potion\n"),
            ("uniqueitems", "index\tcode\nPolicy Sample\tmissing\n"),
        ];
        let fx = fixture(&files);
        let diags = run_plugin("itemCodeCheck.ts", "uniqueitems", &fx).await;

        assert_eq!(diags.len(), 1, "raw type-9 policy result: {diags:#?}");
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
        assert_code(&diags[0], "item-code.unresolved-policy");
        assert!(diags[0].message.contains("No matching item was found"));
        assert!(
            diags[0]
                .message
                .contains("may keep the text without resolving it to an item")
        );
    }

    #[tokio::test]
    async fn prop_code_check_covers_gapped_headers_and_reports_sentinel_policy() {
        let targets = [
            ("properties", "code\nvalid\n"),
            ("propertygroups", "code\nvalidgroup\n"),
        ];
        for (file, content, expected_code) in [
            (
                "monprop",
                "id\tprop1\tprop1 (H)\tprop1 (N)\nrow\tvalid\tvalid\tmissing-monprop\n",
                "property.unknown-code",
            ),
            (
                "setitems",
                "index\taprop1a\nrow\tmissing-aprop\n",
                "property.unknown-code",
            ),
            (
                "sets",
                "index\tPCode2a\nrow\tmissing-pcode\n",
                "property.unknown-code",
            ),
            (
                "uniqueitems",
                "index\tprop1\nrow\t*enr\n",
                "property.unknown-marker",
            ),
        ] {
            let files = vec![(file, content), targets[0], targets[1]];
            let fx = fixture(&files);
            let diags = run_plugin("propCodeCheck.ts", file, &fx).await;
            assert_eq!(diags.len(), 1, "coverage {file}: {diags:#?}");
            assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
            assert_code(&diags[0], expected_code);
        }
    }

    #[tokio::test]
    async fn tc_item_check_reports_u16_conversion_ignored_suffix_and_fixed_width() {
        let width_value = format!("hpot,mul={}", "0".repeat(55));
        assert_eq!(width_value.len(), 64);
        let treasure = format!(
            "Treasure Class\tItem1\tProb1\n\
             Storage\thpot,mul=-1,cu=abc\t1\n\
             Width\t{width_value}\t1\n\
             Suffix\thpot,mul=1,wat=2,cu=abc\t1\n"
        );
        let mut files: Vec<(&str, &str)> = base_lookup_files();
        files.push(("treasureclassex", treasure.as_str()));
        let fx = fixture(&files);
        let diags = run_plugin("tcItemCheck.ts", "treasureclassex", &fx).await;

        assert_eq!(
            diags.len(),
            4,
            "independent TC storage policies: {diags:#?}"
        );
        assert!(
            diags
                .iter()
                .all(|diag| diag.severity == Some(DiagnosticSeverity::WARNING))
        );
        assert!(
            diags
                .iter()
                .any(|diag| diag.message.contains("game converts it to 65535"))
        );
        assert!(
            diags
                .iter()
                .any(|diag| diag.message.contains("game converts it to 0"))
        );
        assert!(diags.iter().any(|diag| diag.code
            == Some(NumberOrString::String("tc-item.field-width".into()))));
        assert!(
            diags
                .iter()
                .any(|diag| diag.message.contains("The game stops at 'wat=2'")
                    && diag.message.contains("everything after it are ignored"))
        );
    }

    #[tokio::test]
    async fn tc_item_check_matches_sequential_lookup_gap_and_modifier_policy() {
        let files = with_base_files((
            "treasureclassex",
            "Treasure Class\tPicks\tNoDrop\tItem1\tProb1\tItem2\tProb2\n\
             Self\t1\t0\tSelf\t1\t\t\n\
             Gap\t1\t0\t\t1\tbad-after-gap\t1\n\
             Case\t1\t0\tHPOT\t1\t\t\n\
             GeneratedFinite\t1\t0\tweap3\t1\t\t\n\
             GeneratedInvalid\t1\t0\tweap5\t1\t\t\n\
             BareItemType\t1\t0\tweap\t1\t\t\n\
             UnknownMod\t1\t0\thpot,ZZ=1\t1\t\t\n\
             Range\t1\t0\thpot,mul=65536\t1\t\t\n\
             Forward\t1\t0\tLater\t1\t\t\n\
             Later\t1\t0\thpot\t1\t\t\n\
             Multibyte\t1\t0\t가x\t1\t\t\n\
             Utf8TooLong\t1\t0\téabc\t1\t\t\n",
        ));
        let fx = fixture(&files);
        let diags = run_plugin("tcItemCheck.ts", "treasureclassex", &fx).await;
        assert_eq!(
            diags.len(),
            10,
            "finite generated name clean; broad ItemTypes/gap/exact/forward/modifiers diagnosed: {diags:#?}"
        );
        assert!(
            diags
                .iter()
                .all(|d| d.severity == Some(DiagnosticSeverity::WARNING)),
            "Legacy parity uses warnings for these TC diagnostics: {diags:#?}"
        );
        assert!(diags.iter().any(|d| d.code == Some(NumberOrString::String("tc-item.forward-reference".into()))));
        assert!(
            diags
                .iter()
                .any(|d| d.code == Some(NumberOrString::String("tc-item.ignored-suffix".into())))
        );
        assert!(
            diags
                .iter()
                .any(|d| d.code == Some(NumberOrString::String("tc-item.modifier-range".into())))
        );
        let modifier_range = diags
            .iter()
            .find(|d| d.code == Some(NumberOrString::String("tc-item.modifier-range".into())))
            .expect("modifier range warning");
        assert_eq!(
            modifier_range.message,
            "treasureclassex.txt, line 9: Modifier 'mul=65536' for 'item1' in TC 'Range' is outside 0..65535. The game converts it to 0. Replace it with the number you actually want."
        );
        assert!(
            diags
                .iter()
                .any(|d| d.code == Some(NumberOrString::String("tc-item.after-first-gap".into())))
        );
        assert!(
            diags
                .iter()
                .any(|d| d.code == Some(NumberOrString::String("tc-prob.orphaned".into())))
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("can't find 'weap5' for 'item1'")),
            "only the finite equipment-TC name set may resolve: {diags:#?}"
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("can't find 'weap' for 'item1'")),
            "bare ItemTypes codes are not Item# parser lookups: {diags:#?}"
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("can't find 'éabc' for 'item1'")),
            "a five-byte UTF-8 base must not enter the raw four-byte item-code path: {diags:#?}"
        );
        let orphan = diags
            .iter()
            .find(|diag| diag.code == Some(NumberOrString::String("tc-prob.orphaned".into())))
            .expect("orphaned probability warning");
        assert_eq!(range(orphan), (2, 9, 10));
        assert_eq!(
            orphan.message,
            "prob1 is orphaned and ignored because item1 is empty."
        );
        let ignored_item = diags
            .iter()
            .find(|diag| {
                diag.code == Some(NumberOrString::String("tc-item.after-first-gap".into()))
            })
            .expect("post-gap item warning");
        assert_eq!(range(ignored_item), (2, 11, 24));
        assert_eq!(
            ignored_item.message,
            "item2 is ignored because the first empty Item slot already ended this treasure class."
        );
        let ignored_prob = diags
            .iter()
            .find(|diag| {
                diag.code == Some(NumberOrString::String("tc-prob.after-first-gap".into()))
            })
            .expect("post-gap probability warning");
        assert_eq!(range(ignored_prob), (2, 25, 26));
        assert_eq!(
            ignored_prob.message,
            "prob2 is ignored because the first empty Item slot already ended this treasure class."
        );

        let host = PluginHost::new(vec![plugin_path("tcItemCheck.ts")]).unwrap();
        let doc = fx.docs.get("treasureclassex").expect("TC fixture");
        let ctx = build_hover_context("treasureclassex", "Item1", "hpot,ZZ=1", 7, doc);
        let hover = host
            .hover(ctx.clone(), Arc::clone(&fx.index), Arc::clone(&fx.snapshot))
            .await
            .expect("TC hover");
        assert!(
            hover.contains("Healing Potion") && !hover.contains("Source:"),
            "{hover}"
        );
        assert!(
            hover.contains("Ignored text begins at: `ZZ=1`")
                && hover.contains("base and modifiers before it still work"),
            "hover should share TC parser suffix semantics: {hover}"
        );
        let target = host
            .goto_definition(ctx, Arc::clone(&fx.index), Arc::clone(&fx.snapshot))
            .await
            .expect("TC definition from parsed base");
        assert_eq!(target, ("weapons".into(), "code".into(), "hpot".into()));

        let generated_ctx = build_hover_context("treasureclassex", "Item1", "weap3", 4, doc);
        let generated_hover = host
            .hover(
                generated_ctx.clone(),
                Arc::clone(&fx.index),
                Arc::clone(&fx.snapshot),
            )
            .await
            .expect("generated TC hover");
        assert!(
            generated_hover.contains("Generated Treasure Class")
                && !generated_hover.contains("Source:"),
            "generated named-TC hover: {generated_hover}"
        );
        let generated_target = host
            .goto_definition(
                generated_ctx,
                Arc::clone(&fx.index),
                Arc::clone(&fx.snapshot),
            )
            .await
            .expect("generated TC definition");
        assert_eq!(
            generated_target,
            ("itemtypes".into(), "Code".into(), "weap".into())
        );

        let invalid_generated_ctx =
            build_hover_context("treasureclassex", "Item1", "weap5", 5, doc);
        let invalid_generated_hover = host
            .hover(
                invalid_generated_ctx.clone(),
                Arc::clone(&fx.index),
                Arc::clone(&fx.snapshot),
            )
            .await
            .expect("unresolved TC item still exposes probability context");
        assert!(
            !invalid_generated_hover.contains("Generated Treasure Class")
                && !invalid_generated_hover.contains("Source:"),
            "arbitrary suffix must not inherit ItemTypes provenance: {invalid_generated_hover}"
        );
        assert!(
            host.goto_definition(
                invalid_generated_ctx,
                Arc::clone(&fx.index),
                Arc::clone(&fx.snapshot)
            )
            .await
            .is_none(),
            "arbitrary generated-TC suffix must not define into ItemTypes"
        );

        let forward_ctx = build_hover_context("treasureclassex", "Item1", "Later", 9, doc);
        let forward_hover = host
            .hover(
                forward_ctx.clone(),
                Arc::clone(&fx.index),
                Arc::clone(&fx.snapshot),
            )
            .await
            .expect("forward TC hover keeps probability context");
        assert!(
            !forward_hover.contains("Treasure Class") && !forward_hover.contains("Source:"),
            "a later TC row must not resolve in hover: {forward_hover}"
        );
        assert!(
            host.goto_definition(forward_ctx, Arc::clone(&fx.index), Arc::clone(&fx.snapshot),)
                .await
                .is_none(),
            "a later TC row must not resolve in goto-definition"
        );

        let self_ctx = build_hover_context("treasureclassex", "Item1", "Self", 1, doc);
        let self_hover = host
            .hover(
                self_ctx.clone(),
                Arc::clone(&fx.index),
                Arc::clone(&fx.snapshot),
            )
            .await
            .expect("self TC hover");
        assert!(
            self_hover.contains("Treasure Class") && !self_hover.contains("Source:"),
            "the current row is already in the sequential TC map: {self_hover}"
        );
        assert_eq!(
            host.goto_definition(self_ctx, Arc::clone(&fx.index), Arc::clone(&fx.snapshot),)
                .await,
            Some((
                "treasureclassex".into(),
                "treasure class".into(),
                "Self".into(),
            ))
        );

        let multibyte_ctx = build_hover_context("treasureclassex", "Item1", "가x", 11, doc);
        let multibyte_hover = host
            .hover(
                multibyte_ctx.clone(),
                Arc::clone(&fx.index),
                Arc::clone(&fx.snapshot),
            )
            .await
            .expect("multibyte fixed4 TC hover");
        assert!(
            multibyte_hover.contains("Multibyte Item"),
            "{multibyte_hover}"
        );
        assert_eq!(
            host.goto_definition(
                multibyte_ctx,
                Arc::clone(&fx.index),
                Arc::clone(&fx.snapshot),
            )
            .await,
            Some(("weapons".into(), "code".into(), "가xA".into()))
        );
    }
}
