use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Url};

use crate::document::DocumentData;
use crate::runtime::{ScriptRuntime, WorkspaceFileSnapshot, WorkspaceIndex};
use crate::schema::Schema;

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
    message: String,
    #[serde(default)]
    code: Option<NumberOrString>,
    #[serde(default)]
    data: Option<Value>,
}

impl RawDiag {
    fn into_lsp(self) -> Diagnostic {
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
        Diagnostic {
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
            message: self.message,
            data: self.data,
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Wire type for hover()
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawHover {
    content: String,
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

// ---------------------------------------------------------------------------
// Plugin host (owns the JS runtime on a dedicated non-Send thread)
// ---------------------------------------------------------------------------

enum PluginRequest {
    SetSchema {
        schema: Arc<Schema>,
    },
    Validate {
        ctx: String,
        index: Arc<WorkspaceIndex>,
        snapshot: Arc<WorkspaceFileSnapshot>,
        reply: oneshot::Sender<Vec<Diagnostic>>,
    },
    Hover {
        ctx: Value,
        index: Arc<WorkspaceIndex>,
        snapshot: Arc<WorkspaceFileSnapshot>,
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
}

impl PluginHost {
    /// Spawn the dedicated plugin thread, load all plugin files, and return a handle.
    pub fn new(paths: Vec<PathBuf>) -> Self {
        let (tx, mut rx) = mpsc::channel::<PluginRequest>(32);

        std::thread::spawn(move || {
            let mut rt = match ScriptRuntime::new() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("vector-lsp: plugin runtime init failed: {e}");
                    return;
                }
            };

            // Seed plugin registries and host-provided utility functions.
            let _ = rt.exec(
                "__seed__",
                "var __plugins = []; var __hovers = []; var __gotos = [];\
                 var validate; var hover; var gotoDefinition;\
                 var __lookupCache={};\
                 var __colCache={};\
                 var __cvCache={};\
                 var __filteredCvCache={};\
                 function lookupKey(file,col,value){\
                     var k=file+'|'+col+'|'+value;\
                     if(!(k in __lookupCache)){__lookupCache[k]=Deno.core.ops.op_lookup_key(file,col,value);}\
                     return __lookupCache[k];\
                 }\
                 function getColumn(file,col){\
                     var k=file+'|'+col;\
                     if(!(k in __colCache)){__colCache[k]=Deno.core.ops.op_get_column(file,col)||undefined;}\
                     return __colCache[k];\
                 }\
                 function hasFile(stem){\
                     return Deno.core.ops.op_has_file(stem);\
                 }\
                 function hasLookupTarget(file,col){\
                     return Deno.core.ops.op_has_lookup_target(file,col);\
                 }\
                 function getColumnValues(stem,col){\
                     var k=stem+'|'+col;\
                     if(!(k in __cvCache)){__cvCache[k]=Deno.core.ops.op_get_column_values(stem,col);}\
                     return __cvCache[k];\
                 }\
                 function getFilteredColumnValues(stem,valueCol,filterCol,filterValue){\
                     var k=stem+'|'+valueCol+'|'+filterCol+'|'+filterValue;\
                     if(!(k in __filteredCvCache)){__filteredCvCache[k]=Deno.core.ops.op_get_filtered_column_values(stem,valueCol,filterCol,filterValue);}\
                     return __filteredCvCache[k];\
                 }\
                 function getEnumTable(file,col){\
                     return Deno.core.ops.op_get_enum_table(file,col)||null;\
                 }",
            );

            for path in &paths {
                if let Err(e) = load_plugin(&mut rt, path) {
                    eprintln!("vector-lsp: plugin '{}': {e}", path.display());
                }
            }

            // Tracks the last snapshot seen so the column-value cache can be
            // invalidated exactly when the workspace data changes.
            let mut last_snapshot_ptr: usize = 0;

            while let Some(req) = rx.blocking_recv() {
                match req {
                    PluginRequest::SetSchema { schema } => {
                        rt.set_schema(schema);
                    }
                    PluginRequest::Validate {
                        ctx,
                        index,
                        snapshot,
                        reply,
                    } => {
                        let ptr = Arc::as_ptr(&snapshot) as usize;
                        if ptr != last_snapshot_ptr {
                            let _ = rt.exec("__cache_reset__", "var __lookupCache={}; var __colCache={}; var __cvCache={}; var __filteredCvCache={};");
                            last_snapshot_ptr = ptr;
                        }
                        rt.set_workspace_index(index);
                        rt.set_workspace_snapshot(snapshot);
                        rt.set_ctx_json(ctx);
                        let debug = std::env::var("VLSP_DEBUG_LOGGING").is_ok();
                        let expr = if debug {
                            "(function(){\
                             var __c__=JSON.parse(Deno.core.ops.op_get_ctx_json());\
                             return \
                             __plugins.flatMap(function(fn,i){\
                                 try{return fn(__c__)||[];}catch(e){Deno.core.print('[validate-err-'+i+'] '+String(e)+'\\n',true);return []}\
                             });\
                             })()"
                        } else {
                            "(function(){\
                             var __c__=JSON.parse(Deno.core.ops.op_get_ctx_json());\
                             return \
                             __plugins.flatMap(function(fn){\
                                 try{return fn(__c__)||[];}catch(e){return []}\
                             });\
                             })()"
                        };
                        let diags = rt
                            .eval_json(expr)
                            .ok()
                            .and_then(|v| serde_json::from_value::<Vec<RawDiag>>(v).ok())
                            .into_iter()
                            .flatten()
                            .map(RawDiag::into_lsp)
                            .collect();
                        let _ = reply.send(diags);
                    }
                    PluginRequest::Hover {
                        ctx,
                        index,
                        snapshot,
                        reply,
                    } => {
                        rt.set_workspace_index(index);
                        rt.set_workspace_snapshot(snapshot);
                        let ctx_json = ctx.to_string();
                        // Call each hover function in turn; return the first non-null result.
                        let debug = std::env::var("VLSP_DEBUG_LOGGING").is_ok();
                        if debug {
                            eprintln!("[hover-debug] ctx={ctx_json}");
                            let len = rt
                                .eval_json("__hovers.length")
                                .ok()
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            eprintln!("[hover-debug] __hovers.length={len}");
                        }
                        let expr = if debug {
                            format!(
                                "(function(ctx){{\
                                    for(var i=0;i<__hovers.length;i++){{\
                                        try{{var r=__hovers[i](ctx);if(r!=null)return r;}}\
                                        catch(e){{Deno.core.print('[hover-err-'+i+'] '+String(e)+'\\n',true);}}\
                                    }}\
                                    return null;\
                                }})({ctx_json})"
                            )
                        } else {
                            format!(
                                "(function(ctx){{\
                                    for(var i=0;i<__hovers.length;i++){{\
                                        try{{var r=__hovers[i](ctx);if(r!=null)return r;}}catch(e){{}}\
                                    }}\
                                    return null;\
                                }})({ctx_json})"
                            )
                        };
                        let raw = rt.eval_json(&expr);
                        if debug {
                            eprintln!("[hover-debug] raw={raw:?}");
                        }
                        let result = raw.ok().and_then(|v| match v {
                            Value::Null => None,
                            // Plugin returned { content: "..." }
                            Value::Object(_) => serde_json::from_value::<RawHover>(v)
                                .ok()
                                .map(|h| h.content),
                            // Plugin returned a plain string
                            Value::String(s) => Some(s),
                            _ => None,
                        });
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
                        rt.set_workspace_index(index);
                        rt.set_workspace_snapshot(snapshot);
                        let ctx_json = ctx.to_string();
                        // Call each gotoDefinition function in turn; return the first non-null result.
                        let expr = format!(
                            "(function(ctx){{\
                                for(var i=0;i<__gotos.length;i++){{\
                                    try{{var r=__gotos[i](ctx);if(r!=null)return r;}}catch(e){{}}\
                                }}\
                                return null;\
                            }})({ctx_json})"
                        );
                        let result = rt.eval_json(&expr).ok().and_then(|v| match v {
                            Value::Null => None,
                            Value::Object(_) => serde_json::from_value::<RawGotoTarget>(v)
                                .ok()
                                .map(|t| (t.target_file, t.target_col, t.target_value)),
                            _ => None,
                        });
                        let _ = reply.send(result);
                    }
                }
            }
        });

        Self { tx }
    }

    /// Push the loaded schema to the plugin thread so `getEnumTable` has data.
    /// Fire-and-forget: the schema is immutable after loading so ordering with
    /// subsequent validate/hover calls (which are queued after this) is fine.
    pub async fn set_schema(&self, schema: Arc<Schema>) {
        let _ = self.tx.send(PluginRequest::SetSchema { schema }).await;
    }

    /// Run all `validate` plugin functions and return any diagnostics.
    pub async fn run(
        &self,
        ctx: String,
        index: Arc<WorkspaceIndex>,
        snapshot: Arc<WorkspaceFileSnapshot>,
    ) -> Vec<Diagnostic> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .tx
            .send(PluginRequest::Validate {
                ctx,
                index,
                snapshot,
                reply: reply_tx,
            })
            .await
            .is_err()
        {
            return vec![];
        }
        reply_rx.await.unwrap_or_default()
    }

    /// Run all `hover` plugin functions and return the first non-null markdown content.
    pub async fn hover(
        &self,
        ctx: Value,
        index: Arc<WorkspaceIndex>,
        snapshot: Arc<WorkspaceFileSnapshot>,
    ) -> Option<String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(PluginRequest::Hover {
                ctx,
                index,
                snapshot,
                reply: reply_tx,
            })
            .await
            .ok()?;
        reply_rx.await.ok().flatten()
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
        self.tx
            .send(PluginRequest::GotoDefinition {
                ctx,
                index,
                snapshot,
                reply: reply_tx,
            })
            .await
            .ok()?;
        reply_rx.await.ok().flatten()
    }
}

fn load_plugin(rt: &mut ScriptRuntime, path: &Path) -> anyhow::Result<()> {
    let src = std::fs::read_to_string(path)?;
    let src = strip_typescript(&src);
    rt.exec("<plugin>", src)?;
    // Auto-register top-level `validate` and/or `hover` functions if defined.
    rt.exec(
        "__register__",
        "if(typeof validate==='function'){__plugins.push(validate);validate=undefined;}\
         if(typeof hover==='function'){__hovers.push(hover);hover=undefined;}\
         if(typeof gotoDefinition==='function'){__gotos.push(gotoDefinition);gotoDefinition=undefined;}",
    )?;
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
pub fn build_workspace_snapshot(
    open_docs: &HashMap<Url, Arc<DocumentData>>,
    file_cache: &HashMap<PathBuf, Arc<DocumentData>>,
) -> Arc<WorkspaceFileSnapshot> {
    let mut snap = WorkspaceFileSnapshot::new();

    for (path, doc) in file_cache {
        if let Some(stem) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase())
        {
            snap.files.entry(stem).or_insert_with(|| Arc::clone(doc));
        }
    }

    for (uri, doc) in open_docs {
        let stem = uri
            .path_segments()
            .and_then(|s| s.last())
            .and_then(|n| n.rfind('.').map(|i| n[..i].to_lowercase()))
            .unwrap_or_default();
        if !stem.is_empty() {
            snap.files.insert(stem, Arc::clone(doc));
        }
    }

    Arc::new(snap)
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
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use serde_json::json;
    use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};

    use super::{
        PluginHost, RawDiag, build_context, build_hover_context, strip_ts_declarations,
        strip_typescript,
    };
    use crate::document::DocumentData;
    use crate::runtime::{WorkspaceFileSnapshot, build_workspace_index};

    // --- Structural (pass 1) -------------------------------------------------

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
        let mut snapshot = WorkspaceFileSnapshot::new();
        for (path, doc) in &file_cache {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                snapshot.files.insert(stem.to_lowercase(), Arc::clone(doc));
            }
        }

        PluginFixture {
            docs,
            index,
            snapshot: Arc::new(snapshot),
        }
    }

    async fn run_plugin(plugin_name: &str, file: &str, fx: &PluginFixture) -> Vec<Diagnostic> {
        let host = PluginHost::new(vec![plugin_path(plugin_name)]);
        let doc = fx.docs.get(file).expect("test document should exist");
        host.run(
            build_context(file, doc),
            Arc::clone(&fx.index),
            Arc::clone(&fx.snapshot),
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
            ("weapons", "code\tname\nhpot\tHealing Potion\n"),
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

        let diag = raw.into_lsp();
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

        let diag = raw.into_lsp();
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
    async fn cube_input_check_reports_token_ranges() {
        let cases = [
            (
                "invalid base",
                "desc\tinput 1\nr\tbadbase,qty=1\n",
                (1, 2, 9),
                "not a valid cubemain input",
            ),
            (
                "invalid modifier key",
                "desc\tinput 1\nr\thpot,foo=1\n",
                (1, 7, 10),
                "Unknown parameterized modifier",
            ),
            (
                "invalid modifier value",
                "desc\tinput 1\nr\thpot,qty=abc\n",
                (1, 11, 14),
                "requires a non-negative integer",
            ),
            (
                "missing modifier value",
                "desc\tinput 1\nr\thpot,qty=\n",
                (1, 10, 11),
                "requires a non-negative integer",
            ),
            (
                "quoted input",
                "desc\tinput 1\nr\t\"hpot,qty=abc\"\n",
                (1, 12, 15),
                "requires a non-negative integer",
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

        let files = with_base_files(("cubemain", "desc\tinput 1\n*disabled\tbadbase\n"));
        let fx = fixture(&files);
        let diags = run_plugin("cubeInputCheck.ts", "cubemain", &fx).await;
        assert!(
            diags.is_empty(),
            "disabled rows should stay ignored: {diags:#?}"
        );
    }

    #[tokio::test]
    async fn tc_item_check_reports_token_ranges() {
        let cases = [
            (
                "invalid base",
                "Treasure Class\tPicks\tNoDrop\tItem1\tProb1\nAct 1\t1\t0\tbaditem,mul=1280\t1\n",
                (1, 10, 17),
                "not a valid item code",
            ),
            (
                "invalid modifier key",
                "Treasure Class\tPicks\tNoDrop\tItem1\tProb1\nAct 1\t1\t0\thpot,zz=1\t1\n",
                (1, 15, 17),
                "Unknown modifier key",
            ),
            (
                "missing modifier value",
                "Treasure Class\tPicks\tNoDrop\tItem1\tProb1\nAct 1\t1\t0\thpot,mul=\t1\n",
                (1, 18, 19),
                "Missing value",
            ),
            (
                "quoted modifier",
                "Treasure Class\tPicks\tNoDrop\tItem1\tProb1\nAct 1\t1\t0\t\"hpot,zz=1\"\t1\n",
                (1, 16, 18),
                "Unknown modifier key",
            ),
            (
                "forward reference",
                "Treasure Class\tPicks\tNoDrop\tItem1\tProb1\nFirst\t1\t0\tLater\t1\nLater\t1\t0\thpot\t1\n",
                (1, 10, 15),
                "defined at or below",
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
    async fn cube_input_hover_and_definition_still_use_the_base_token() {
        let files = with_base_files(("cubemain", "desc\tinput 1\nr\thpot,qty=3\n"));
        let fx = fixture(&files);
        let host = PluginHost::new(vec![plugin_path("cubeInputCheck.ts")]);
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
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));

        let prop_files = vec![
            ("cubemain", "desc\tmod 1\nr\tbadprop\n"),
            ("properties", "code\nvalidprop\n"),
            ("propertygroups", "code\nvalidgroup\n"),
        ];
        let fx = fixture(&prop_files);
        let diags = run_plugin("propCodeCheck.ts", "cubemain", &fx).await;
        assert_eq!(first_range(&diags), (1, 2, 9));
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
    }
}
