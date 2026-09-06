use std::collections::{HashMap, HashSet};
#[cfg(feature = "d2rdoc")]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "d2rdoc")]
use anyhow::Context as _;
use anyhow::Result;
use deno_core::{FastString, JsRuntime, OpState, RuntimeOptions, extension, op2};
use serde_json::Value;
use tower_lsp::lsp_types::Url;

use crate::document::DocumentData;
use crate::schema::{Schema, format_description};
#[cfg(test)]
use crate::source_selection::effective_workspace_sources;
use crate::source_selection::{EffectiveSource, effective_workspace_sources_with_fallback};
use crate::workspace::fixed4_key;

// ---------------------------------------------------------------------------
// WorkspaceFileSnapshot — per-file DocumentData references for plugin ops
// ---------------------------------------------------------------------------

/// Holds references to parsed documents so plugin ops can access them without
/// pre-serialising the whole workspace.  Only the columns a plugin actually
/// requests are serialised, when the op is called.
pub struct WorkspaceFileSnapshot {
    /// Lowercase stem → parsed document.
    pub files: HashMap<String, Arc<DocumentData>>,
    pub sources: HashMap<String, WorkspaceSourceInfo>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSourceInfo {
    pub kind: String,
    pub version: Option<String>,
}

impl WorkspaceFileSnapshot {
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
            sources: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// WorkspaceIndex — O(1) lookup table for plugin ops
// ---------------------------------------------------------------------------

/// Pre-built lookup table keyed by `(file_stem_lowercase, column_name)`.
/// Stored in the JS runtime's `OpState` so plugin ops can reach it.
pub struct WorkspaceIndex {
    /// Value existence index: `(file_stem_lowercase, col_name)` → set of values.
    data: HashMap<(String, String), HashSet<String>>,
    fixed4_data: HashMap<(String, String), HashSet<String>>,
    /// Ordered header list per file (open-doc entries shadow cache entries).
    columns: HashMap<String, Vec<String>>,
}

impl WorkspaceIndex {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
            fixed4_data: HashMap::new(),
            columns: HashMap::new(),
        }
    }

    fn insert(&mut self, file: &str, col: &str, value: String) {
        let key = (file.to_ascii_lowercase(), col.to_ascii_lowercase());
        self.data
            .entry(key.clone())
            .or_default()
            .insert(value.to_ascii_lowercase());
        if col.eq_ignore_ascii_case("code") {
            self.fixed4_data
                .entry(key)
                .or_default()
                .insert(fixed4_key(&value));
        }
    }

    pub fn lookup(&self, file: &str, col: &str, value: &str) -> bool {
        self.data
            .get(&(file.to_ascii_lowercase(), col.to_ascii_lowercase()))
            .map(|s| s.contains(&value.to_ascii_lowercase()))
            .unwrap_or(false)
    }

    pub fn lookup_fixed4(&self, file: &str, col: &str, value: &str) -> bool {
        self.fixed4_data
            .get(&(file.to_ascii_lowercase(), col.to_ascii_lowercase()))
            .map(|values| values.contains(&fixed4_key(value)))
            .unwrap_or(false)
    }

    /// Return the 0-based header position of `col` in `file`, or `None`.
    /// The comparison is case-insensitive.
    pub fn column_index(&self, file: &str, col: &str) -> Option<usize> {
        let col_lower = col.to_ascii_lowercase();
        self.columns
            .get(&file.to_ascii_lowercase())?
            .iter()
            .position(|h| h.to_ascii_lowercase() == col_lower)
    }

    pub fn has_lookup_target(&self, file: &str, col: &str) -> bool {
        self.column_index(file, col).is_some()
    }
}

/// Build a `WorkspaceIndex` from open documents and the file cache.
/// Open documents shadow file-cache entries for the same stem.
/// Returns an `Arc` so the index can be shared cheaply across multiple plugin calls.
#[cfg(test)]
pub fn build_workspace_index(
    open_docs: &HashMap<Url, Arc<DocumentData>>,
    file_cache: &HashMap<PathBuf, Arc<DocumentData>>,
) -> Arc<WorkspaceIndex> {
    let mut idx = WorkspaceIndex::new();
    for source in effective_workspace_sources(open_docs, file_cache) {
        index_doc(&mut idx, &source.stem, &source.document);
    }

    Arc::new(idx)
}

pub fn build_workspace_index_with_fallback(
    open_docs: &HashMap<Url, Arc<DocumentData>>,
    file_cache: &HashMap<PathBuf, Arc<DocumentData>>,
    fallback_cache: &HashMap<String, Arc<DocumentData>>,
    workspace_present_stems: &HashSet<String>,
    fallback_version: Option<&str>,
) -> Arc<WorkspaceIndex> {
    let mut idx = WorkspaceIndex::new();
    for source in effective_workspace_sources_with_fallback(
        open_docs,
        file_cache,
        fallback_cache,
        workspace_present_stems,
        fallback_version,
    ) {
        index_doc(&mut idx, &source.stem, &source.document);
    }
    Arc::new(idx)
}

pub fn build_workspace_index_from_sources(sources: &[EffectiveSource]) -> Arc<WorkspaceIndex> {
    let mut index = WorkspaceIndex::new();
    for source in sources {
        index_doc(&mut index, &source.stem, &source.document);
    }
    Arc::new(index)
}

fn index_doc(idx: &mut WorkspaceIndex, stem: &str, doc: &DocumentData) {
    // Record ordered header list (last write wins, so open docs beat cache).
    idx.columns
        .insert(stem.to_ascii_lowercase(), doc.headers.clone());

    for row in &doc.rows {
        if row
            .cells
            .first()
            .map(|cell| cell.value.trim_start().starts_with('*'))
            .unwrap_or(false)
        {
            continue;
        }
        for (col_i, cell) in row.cells.iter().enumerate() {
            if cell.value.trim().is_empty() {
                continue;
            }
            if let Some(h) = doc.headers.get(col_i).filter(|h| !h.is_empty()) {
                idx.insert(stem, h, cell.value.clone());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Deno op
// ---------------------------------------------------------------------------

/// Look up whether `value` exists in column `col` of file `file`.
/// Callable from JS as `Deno.core.ops.op_lookup_key(file, col, value)`.
#[op2(nofast)]
pub fn op_lookup_key(
    state: &OpState,
    #[string] file: &str,
    #[string] col: &str,
    #[string] value: &str,
) -> bool {
    state
        .try_borrow::<Arc<WorkspaceIndex>>()
        .map(|idx| idx.lookup(file, col, value))
        .unwrap_or(false)
}

#[op2(nofast)]
pub fn op_lookup_key_fixed4(
    state: &OpState,
    #[string] file: &str,
    #[string] col: &str,
    #[string] value: &str,
) -> bool {
    state
        .try_borrow::<Arc<WorkspaceIndex>>()
        .map(|idx| idx.lookup_fixed4(file, col, value))
        .unwrap_or(false)
}

#[derive(serde::Serialize)]
struct ColumnInfo {
    /// 0-based position of this column in the file's header row.
    index: usize,
}

#[op2]
#[serde]
pub fn op_get_column(
    state: &OpState,
    #[string] file: &str,
    #[string] col: &str,
) -> Option<ColumnInfo> {
    state.try_borrow::<Arc<WorkspaceIndex>>().and_then(|idx| {
        idx.column_index(file, col)
            .map(|index| ColumnInfo { index })
    })
}

/// Return `true` if `stem` is present in the workspace snapshot.
/// Callable from JS as `Deno.core.ops.op_has_file(stem)`.
#[op2(fast)]
pub fn op_has_file(state: &OpState, #[string] stem: &str) -> bool {
    state
        .try_borrow::<Arc<WorkspaceFileSnapshot>>()
        .map(|snap| snap.files.contains_key(&stem.to_ascii_lowercase()))
        .unwrap_or(false)
}

#[op2]
#[serde]
pub fn op_get_workspace_source(
    state: &OpState,
    #[string] stem: &str,
) -> Option<WorkspaceSourceInfo> {
    state
        .try_borrow::<Arc<WorkspaceFileSnapshot>>()
        .and_then(|snapshot| snapshot.sources.get(&stem.to_ascii_lowercase()).cloned())
}

/// Return true when the workspace contains `file` with column `col`.
/// Callable from JS as `Deno.core.ops.op_has_lookup_target(file, col)`.
#[op2(fast)]
pub fn op_has_lookup_target(state: &OpState, #[string] file: &str, #[string] col: &str) -> bool {
    state
        .try_borrow::<Arc<WorkspaceIndex>>()
        .map(|idx| idx.has_lookup_target(file, col))
        .unwrap_or(false)
}

/// Newtype wrapper so `CtxJson` has a unique type key in `OpState`.
pub struct CtxJson(pub String);

/// Return the current validation context JSON string.
/// Called from JS as `Deno.core.ops.op_get_ctx_json()` so plugins receive their
/// context via `JSON.parse(...)` rather than as an inline JS object literal —
/// V8's JSON parser is faster than its object-literal parser for large payloads,
/// and this avoids a large `format!` allocation on the Rust side.
#[op2]
#[string]
pub fn op_get_ctx_json(state: &OpState) -> String {
    state
        .try_borrow::<CtxJson>()
        .map(|c| c.0.clone())
        .unwrap_or_default()
}

/// Return all non-empty values in column `col` of file `stem`.
/// Callable from JS as `Deno.core.ops.op_get_column_values(stem, col)`.
#[op2]
#[serde]
pub fn op_get_column_values(
    state: &OpState,
    #[string] stem: &str,
    #[string] col: &str,
) -> Vec<String> {
    let Some(snap) = state.try_borrow::<Arc<WorkspaceFileSnapshot>>() else {
        return vec![];
    };
    let Some(doc) = snap.files.get(&stem.to_ascii_lowercase()) else {
        return vec![];
    };
    let col_lower = col.to_ascii_lowercase();
    let Some(col_idx) = doc
        .headers
        .iter()
        .position(|h| h.to_ascii_lowercase() == col_lower)
    else {
        return vec![];
    };
    doc.rows
        .iter()
        .filter(|row| {
            !row.cells
                .first()
                .map(|cell| cell.value.trim_start().starts_with('*'))
                .unwrap_or(false)
        })
        .filter_map(|row| row.cells.get(col_idx))
        .filter(|cell| !cell.value.trim().is_empty())
        .map(|cell| cell.value.clone())
        .collect()
}

/// Return the first physical line whose column value matches using the same
/// ASCII case-insensitive name-map semantics as `lookupKey`.
#[derive(serde::Serialize)]
struct FirstColumnValueLine {
    line: u32,
}

#[op2]
#[serde]
pub fn op_get_first_column_value_line(
    state: &OpState,
    #[string] stem: &str,
    #[string] col: &str,
    #[string] value: &str,
) -> Option<FirstColumnValueLine> {
    let snapshot = state.try_borrow::<Arc<WorkspaceFileSnapshot>>()?;
    let doc = snapshot.files.get(&stem.to_ascii_lowercase())?;
    let col_idx = doc
        .headers
        .iter()
        .position(|header| header.eq_ignore_ascii_case(col))?;
    doc.rows
        .iter()
        .filter(|row| {
            !row.cells
                .first()
                .map(|cell| cell.value.trim_start().starts_with('*'))
                .unwrap_or(false)
        })
        .find(|row| {
            row.cells
                .get(col_idx)
                .map(|cell| cell.value.eq_ignore_ascii_case(value))
                .unwrap_or(false)
        })
        .map(|row| FirstColumnValueLine { line: row.line })
}

/// Return non-empty values from `value_col` in file `stem` where `filter_col == filter_value`.
/// Callable from JS as `Deno.core.ops.op_get_filtered_column_values(stem, valueCol, filterCol, filterValue)`.
#[op2]
#[serde]
pub fn op_get_filtered_column_values(
    state: &OpState,
    #[string] stem: &str,
    #[string] value_col: &str,
    #[string] filter_col: &str,
    #[string] filter_value: &str,
) -> Vec<String> {
    let Some(snap) = state.try_borrow::<Arc<WorkspaceFileSnapshot>>() else {
        return vec![];
    };
    let Some(doc) = snap.files.get(&stem.to_ascii_lowercase()) else {
        return vec![];
    };
    let Some(vi) = doc
        .headers
        .iter()
        .position(|h| h.eq_ignore_ascii_case(value_col))
    else {
        return vec![];
    };
    let Some(fi) = doc
        .headers
        .iter()
        .position(|h| h.eq_ignore_ascii_case(filter_col))
    else {
        return vec![];
    };
    doc.rows
        .iter()
        .filter(|row| {
            !row.cells
                .first()
                .map(|cell| cell.value.trim_start().starts_with('*'))
                .unwrap_or(false)
        })
        .filter(|row| {
            row.cells
                .get(fi)
                .map(|c| c.value == filter_value)
                .unwrap_or(false)
        })
        .filter_map(|row| row.cells.get(vi))
        .filter(|cell| !cell.value.trim().is_empty())
        .map(|cell| cell.value.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Enum table op
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct EnumTableResult {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

/// Return the enum table for a field, or `null` if none exists.
/// Cells are normalised: object values use their `text` property; descriptions
/// have `$!...!$` cross-refs resolved and HTML simplified to Markdown.
/// Callable from JS as `Deno.core.ops.op_get_enum_table(file, col)`.
#[op2]
#[serde]
pub fn op_get_enum_table(
    state: &OpState,
    #[string] file: &str,
    #[string] col: &str,
) -> Option<EnumTableResult> {
    let debug = std::env::var("VLSP_DEBUG_LOGGING").is_ok();
    let Some(schema) = state.try_borrow::<Arc<Schema>>() else {
        if debug {
            eprintln!("[enum-debug] no schema in OpState for file={file} col={col}");
        }
        return None;
    };
    let Some(field) = schema.find_field(file, col) else {
        if debug {
            eprintln!("[enum-debug] find_field returned None for file={file} col={col}");
        }
        return None;
    };
    let Some(table) = field.table.as_ref() else {
        if debug {
            eprintln!("[enum-debug] field has no table for file={file} col={col}");
        }
        return None;
    };
    let header_row = table.first()?;
    let headers = header_row.iter().map(cell_raw).collect();
    let rows = table
        .iter()
        .skip(1)
        .map(|row| row.iter().map(cell_formatted).collect())
        .collect();
    if debug {
        eprintln!(
            "[enum-debug] returning table with {} rows for file={file} col={col}",
            table.len() - 1
        );
    }
    Some(EnumTableResult { headers, rows })
}

/// Extract the string value from a table cell (string literal or `{text: "..."}` object).
fn cell_raw(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(o) => o
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Like `cell_raw` but additionally formats `$!...!$` cross-refs and strips HTML
/// tags (converting `<li>` to a bullet prefix).
fn cell_formatted(v: &Value) -> String {
    let raw = cell_raw(v);
    let formatted = format_description(&raw);
    html_to_markdown(&formatted)
}

/// Convert a string that may contain basic HTML into Markdown.
/// - `<li>` → `\n- `
/// - `</li>`, `<ol>`, `</ol>`, `<ul>`, `</ul>` → stripped
/// - All other tags stripped as well.
fn html_to_markdown(s: &str) -> String {
    let s = s
        .replace("<li>", "\n- ")
        .replace("</li>", "")
        .replace("<ol>", "")
        .replace("</ol>", "")
        .replace("<ul>", "")
        .replace("</ul>", "");
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

extension!(
    vlsp_ops,
    ops = [
        op_lookup_key,
        op_lookup_key_fixed4,
        op_get_column,
        op_has_file,
        op_get_workspace_source,
        op_get_column_values,
        op_get_first_column_value_line,
        op_get_filtered_column_values,
        op_get_enum_table,
        op_get_ctx_json,
        op_has_lookup_target,
    ],
);

// ---------------------------------------------------------------------------
// ScriptRuntime
// ---------------------------------------------------------------------------

/// A lightweight wrapper around a Deno/V8 JavaScript runtime.
///
/// Used for schema loading (evaluating the JS schema folder) and serves as
/// the host for JS plugins. Keeping this abstraction isolated means the rest
/// of the codebase never touches deno_core directly.
pub struct ScriptRuntime {
    inner: JsRuntime,
}

/// Initialize the V8 platform. Call from `main` before any thread builds a
/// `ScriptRuntime`.
///
/// Otherwise deno_core initializes V8 on whichever thread builds the first
/// isolate. Where the CPU has protection keys (`pku`/`ospke`) only that thread
/// gets V8's key in its PKRU, and a second thread building an isolate faults
/// with `SEGV_PKUERR` on its first JIT code. Idempotent.
pub fn init_platform() {
    JsRuntime::init_platform(None, false);
}

impl ScriptRuntime {
    pub fn new() -> Result<Self> {
        let inner = JsRuntime::new(RuntimeOptions {
            extensions: vec![vlsp_ops::init_ops_and_esm()],
            ..Default::default()
        });
        Ok(Self { inner })
    }

    /// Replace the schema stored in `OpState`.
    /// Call this once after schema loading so `op_get_enum_table` has data.
    pub fn set_schema(&mut self, schema: Arc<Schema>) {
        self.inner.op_state().borrow_mut().put(schema);
    }

    /// Replace the workspace index stored in `OpState`.
    /// Call this before each plugin validation/hover run so `lookupKey` sees
    /// up-to-date data.
    pub fn set_workspace_index(&mut self, index: Arc<WorkspaceIndex>) {
        self.inner.op_state().borrow_mut().put(index);
    }

    /// Store the current file's context JSON so the `op_get_ctx_json` op can
    /// return it to JS without requiring a large `format!` embedding.
    pub fn set_ctx_json(&mut self, json: String) {
        self.inner.op_state().borrow_mut().put(CtxJson(json));
    }

    /// Replace the per-file workspace snapshot stored in `OpState`.
    /// Call this before each plugin run so `getWorkspaceFile` sees up-to-date data.
    pub fn set_workspace_snapshot(&mut self, snapshot: Arc<WorkspaceFileSnapshot>) {
        self.inner.op_state().borrow_mut().put(snapshot);
    }

    pub fn execution_handle(&mut self) -> deno_core::v8::IsolateHandle {
        self.inner.v8_isolate().thread_safe_handle()
    }

    pub fn cancel_terminate_execution(&mut self) {
        self.inner.v8_isolate().cancel_terminate_execution();
    }

    /// Execute a JavaScript snippet, discarding the return value.
    pub fn exec(&mut self, name: &'static str, src: impl Into<String>) -> Result<()> {
        self.inner
            .execute_script(name, FastString::from(src.into()))?;
        Ok(())
    }

    /// Load and execute a JavaScript file from disk.
    /// The script name shown in error messages will be the filename.
    #[cfg(feature = "d2rdoc")]
    pub fn exec_file(&mut self, path: &Path) -> Result<()> {
        let src = std::fs::read_to_string(path)?;
        self.inner
            .execute_script("<file>", FastString::from(src))
            .with_context(|| format!("in {}", path.display()))?;
        Ok(())
    }

    /// Evaluate a JavaScript expression and return its value as parsed JSON.
    /// The expression is wrapped in `JSON.stringify(...)` before evaluation.
    pub fn eval_json(&mut self, expr: &str) -> Result<serde_json::Value> {
        let src = format!("JSON.stringify({expr})");
        let global = self.inner.execute_script("<eval>", FastString::from(src))?;
        let scope = &mut self.inner.handle_scope();
        let local = deno_core::v8::Local::new(scope, global);
        let json_str = local.to_rust_string_lossy(scope);
        Ok(serde_json::from_str(&json_str)?)
    }
}

/// libtest gives each test its own thread and offers no pre-run hook, so the
/// platform is initialized at load time instead.
#[cfg(test)]
#[ctor::ctor]
fn init_platform_for_tests() {
    init_platform();
}

#[cfg(test)]
mod platform_tests {
    use super::*;

    /// The plugin-host/schema-loader shape. Without `init_platform` this
    /// aborts the test process rather than failing an assertion.
    #[test]
    fn isolates_can_be_built_on_more_than_one_thread() {
        init_platform();

        for tag in ["first", "second"] {
            std::thread::spawn(move || {
                let mut rt = ScriptRuntime::new().expect("runtime");
                rt.exec(
                    tag,
                    "var a = [1, 2, 3]; a.forEach(function (x) { return x; });",
                )
                .expect("exec");
            })
            .join()
            .expect("thread");
        }
    }
}
