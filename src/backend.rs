use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, jsonrpc::Result as LspResult};

use crate::diagnostics;
use crate::document::DocumentData;
use crate::plugin;
use crate::runtime;
use crate::schema::{FieldTypeName, find_loader, format_description};
use crate::settings::VectorLspSettings;
use crate::workspace::Workspace;

pub struct Backend {
    pub client: Client,
    // Arc so the same settings can be shared across TCP connections cheaply.
    pub settings: Arc<VectorLspSettings>,
    pub workspace: Arc<RwLock<Workspace>>,
    /// None when no plugins are configured.
    pub plugin_host: Option<plugin::PluginHost>,
}

impl Backend {
    /// Extract the lowercase file stem from a URI (e.g. `"armor"` from `.../armor.txt`).
    fn file_stem(uri: &Url) -> String {
        let name = uri.path_segments().and_then(|s| s.last()).unwrap_or("");
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

    fn collect_workspace_files(
        root: &std::path::Path,
        ext: &str,
    ) -> std::io::Result<Vec<std::path::PathBuf>> {
        let mut files = Vec::new();
        let mut stack = vec![root.to_path_buf()];

        while let Some(dir) = stack.pop() {
            let mut entries: Vec<_> = std::fs::read_dir(&dir)?.filter_map(|e| e.ok()).collect();
            entries.sort_by_key(|e| e.path());

            for entry in entries {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
                    files.push(path);
                }
            }
        }

        files.sort();
        Ok(files)
    }

    /// Scan all data files in the workspace root, parse and index them.
    /// Called after the schema (and thus ref_targets) is ready.
    async fn scan_and_index_workspace(&self) {
        let (root_uri, delimiter, ext) = {
            let ws = self.workspace.read().await;
            (
                ws.root_uri.clone(),
                self.settings.delimiter_char(),
                self.settings.extension.clone(),
            )
        };

        let Some(root_uri) = root_uri else {
            return;
        };
        let Ok(root_path) = root_uri.to_file_path() else {
            return;
        };

        let paths = match Self::collect_workspace_files(&root_path, &ext) {
            Ok(paths) => paths,
            Err(e) => {
                self.client
                    .log_message(MessageType::WARNING, format!("Workspace scan failed: {e}"))
                    .await;
                return;
            }
        };

        // Collect directory entries before spawning so we can log errors on the main task.
        let mut entries: Vec<(Url, std::path::PathBuf, String)> = Vec::new();
        for path in paths {
            let Ok(uri) = Url::from_file_path(&path) else {
                continue;
            };
            let stem = Self::file_stem(&uri);
            entries.push((uri, path, stem));
        }

        // Read and parse all files in parallel. Each file gets its own task so I/O
        // and CPU-intensive parsing don't serialize behind each other.
        let settings = Arc::clone(&self.settings);
        let mut join_set: tokio::task::JoinSet<
            Option<(Url, std::path::PathBuf, String, Arc<DocumentData>)>,
        > = tokio::task::JoinSet::new();
        for (uri, path, stem) in entries {
            let settings = Arc::clone(&settings);
            join_set.spawn(async move {
                let bytes = tokio::fs::read(&path).await.ok()?;
                let src = settings.encoding.decode(&bytes).ok()?;
                // Parse is synchronous and CPU-intensive; run it off the async executor.
                let doc = tokio::task::spawn_blocking(move || {
                    Arc::new(DocumentData::parse(&src, delimiter))
                })
                .await
                .ok()?;
                Some((uri, path, stem, doc))
            });
        }

        let mut parsed: Vec<(Url, std::path::PathBuf, String, Arc<DocumentData>)> = Vec::new();
        while let Some(result) = join_set.join_next().await {
            if let Ok(Some(item)) = result {
                parsed.push(item);
            }
        }

        // Retain (uri, stem) pairs before consuming the vec for indexing.
        let uri_stems: Vec<(Url, String)> = parsed
            .iter()
            .map(|(uri, _, stem, _)| (uri.clone(), stem.clone()))
            .collect();

        let count = parsed.len();
        {
            let mut ws = self.workspace.write().await;
            let ref_targets = ws.ref_targets.clone();
            for (uri, path, stem, doc) in parsed {
                ws.symbols.index_document(&uri, &stem, &doc, &ref_targets);
                ws.file_cache.insert(path, doc);
            }
        }

        let t_index = Instant::now();
        self.client
            .log_message(
                MessageType::INFO,
                format!("Indexed {count} workspace files."),
            )
            .await;

        // Build workspace snapshot + index once for plugins; shared via Arc.
        let t_snapshot_start = Instant::now();
        let shared = if self.plugin_host.is_some() {
            let ws = self.workspace.read().await;
            let snapshot = plugin::build_workspace_snapshot(&ws.open_documents, &ws.file_cache);
            let idx = runtime::build_workspace_index(&ws.open_documents, &ws.file_cache);
            Some((snapshot, idx))
        } else {
            None
        };
        let t_snapshot = t_snapshot_start.elapsed();

        // Snapshot schema + symbol index once under a single read lock, then release it.
        // Cloning SymbolIndex (one HashMap copy) lets all spawn_blocking tasks validate
        // in parallel without any of them holding the workspace lock.
        let (schema_arc, symbols_arc, file_pairs) = {
            let ws = self.workspace.read().await;
            let schema = ws.schema.clone();
            let symbols = Arc::new(ws.symbols.clone());
            let pairs: Vec<(Url, String, Option<Arc<DocumentData>>)> = uri_stems
                .iter()
                .map(|(uri, stem)| {
                    let doc = uri
                        .to_file_path()
                        .ok()
                        .and_then(|p| ws.file_cache.get(&p))
                        .cloned();
                    (uri.clone(), stem.clone(), doc)
                })
                .collect();
            (schema, symbols, pairs)
        };

        // Validate all files in parallel on the blocking thread pool.
        let t_schema_start = Instant::now();
        let mut diag_set: tokio::task::JoinSet<(
            Url,
            String,
            Option<Arc<DocumentData>>,
            Vec<Diagnostic>,
        )> = tokio::task::JoinSet::new();
        for (uri, stem, doc) in file_pairs {
            let schema = schema_arc.clone();
            let symbols = Arc::clone(&symbols_arc);
            diag_set.spawn_blocking(move || {
                let diags = doc
                    .as_ref()
                    .map(|d| diagnostics::validate_document(&stem, d, schema.as_deref(), &*symbols))
                    .unwrap_or_default();
                (uri, stem, doc, diags)
            });
        }
        let mut schema_results: Vec<(Url, String, Option<Arc<DocumentData>>, Vec<Diagnostic>)> =
            Vec::new();
        while let Some(result) = diag_set.join_next().await {
            if let Ok(item) = result {
                schema_results.push(item);
            }
        }
        let schema_total = t_schema_start.elapsed();

        // Plugin diagnostics are async and must remain sequential.
        let mut plugin_total = std::time::Duration::ZERO;
        let mut pending_publish: Vec<(Url, Vec<Diagnostic>)> = Vec::new();
        for (uri, stem, doc, schema_diags) in schema_results {
            let t = Instant::now();
            let plugin_diags = match (&shared, doc.as_ref(), &self.plugin_host) {
                (Some((snap, idx)), Some(d), Some(ph)) => {
                    let ctx = plugin::build_context(&stem, d);
                    ph.run(ctx, idx.clone(), snap.clone()).await
                }
                _ => vec![],
            };
            plugin_total += t.elapsed();
            let mut diags = schema_diags;
            diags.extend(plugin_diags);
            pending_publish.push((uri, diags));
        }

        // Publish all diagnostics concurrently so they arrive at the client in a
        // burst rather than trickling in one sequential await at a time.
        let mut publish_set: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();
        for (uri, diags) in pending_publish {
            let client = self.client.clone();
            publish_set.spawn(async move {
                client.publish_diagnostics(uri, diags, None).await;
            });
        }
        while publish_set.join_next().await.is_some() {}

        let total = t_index.elapsed();
        self.client
            .log_message(
                MessageType::LOG,
                format!(
                    "vlsp perf [{count} files]: snapshot={t_snapshot:.0?} \
                     schema={schema_total:.0?}(parallel) plugins={plugin_total:.0?} total={total:.0?}"
                ),
            )
            .await;
    }
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
    let sc = range.start.character as usize;
    let el = range.end.line as usize;
    let ec = range.end.character as usize;

    let prefix: String = lines
        .get(sl)
        .map(|l| l.chars().take(sc).collect())
        .unwrap_or_default();
    let suffix: String = lines
        .get(el)
        .map(|l| l.chars().skip(ec.min(l.chars().count())).collect())
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
        let root_uri = params
            .workspace_folders
            .as_deref()
            .and_then(|f| f.first())
            .map(|f| f.uri.clone())
            .or(params.root_uri);

        if let Some(uri) = root_uri {
            self.workspace.write().await.root_uri = Some(uri);
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
        let has_schema =
            self.settings.schema_path.is_some() || !self.settings.schema_variant.is_empty();
        if has_schema {
            let loader = match find_loader(
                &self.settings.schema_loader,
                self.settings.schema_variant.clone(),
                self.settings.plugin_path.clone(),
            ) {
                Ok(l) => l,
                Err(e) => {
                    self.client
                        .log_message(MessageType::ERROR, format!("{e}"))
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
                    self.scan_and_index_workspace().await;
                }
                Ok(Err(e)) => {
                    self.client
                        .log_message(MessageType::ERROR, format!("Schema load failed: {e:#}"))
                        .await;
                }
                Err(e) => {
                    self.client
                        .log_message(MessageType::ERROR, format!("Schema task panicked: {e}"))
                        .await;
                }
            }
        }
    }

    async fn shutdown(&self) -> LspResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let doc = Arc::new(DocumentData::parse(
            &params.text_document.text,
            self.settings.delimiter_char(),
        ));
        let stem = Self::file_stem(&uri);

        let (schema_diags, plugin_data) = {
            let mut ws = self.workspace.write().await;
            let ref_targets = ws.ref_targets.clone();
            ws.symbols.remove_file(&stem);
            ws.symbols.index_document(&uri, &stem, &doc, &ref_targets);
            let schema_diags =
                diagnostics::validate_document(&stem, &doc, ws.schema.as_deref(), &ws.symbols);
            ws.open_documents.insert(uri.clone(), Arc::clone(&doc));
            let plugin_data = self.plugin_host.as_ref().map(|_| {
                let ctx = plugin::build_context(&stem, &doc);
                let idx = runtime::build_workspace_index(&ws.open_documents, &ws.file_cache);
                let snap = plugin::build_workspace_snapshot(&ws.open_documents, &ws.file_cache);
                (ctx, idx, snap)
            });
            (schema_diags, plugin_data)
        };

        let plugin_diags = match (plugin_data, &self.plugin_host) {
            (Some((ctx, idx, snap)), Some(ph)) => ph.run(ctx, idx, snap).await,
            _ => vec![],
        };
        let mut diags = schema_diags;
        diags.extend(plugin_diags);
        self.client.publish_diagnostics(uri, diags, None).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let delimiter = self.settings.delimiter_char();
        let stem = Self::file_stem(&uri);

        // Reconstruct current text from the stored document, apply each incremental
        // change in order, then re-parse. Avoids receiving the full document over IPC.
        let doc = {
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
            let ref_targets = ws.ref_targets.clone();
            ws.symbols.remove_file(&stem);
            ws.symbols.index_document(&uri, &stem, &doc, &ref_targets);
            let schema_diags =
                diagnostics::validate_document(&stem, &doc, ws.schema.as_deref(), &ws.symbols);
            ws.open_documents.insert(uri.clone(), Arc::clone(&doc));
            let plugin_data = self.plugin_host.as_ref().map(|_| {
                let ctx = plugin::build_context(&stem, &doc);
                let idx = runtime::build_workspace_index(&ws.open_documents, &ws.file_cache);
                let snap = plugin::build_workspace_snapshot(&ws.open_documents, &ws.file_cache);
                (ctx, idx, snap)
            });
            (schema_diags, plugin_data, doc)
        };

        let (schema_diags, plugin_data, _doc) = doc;
        let plugin_diags = match (plugin_data, &self.plugin_host) {
            (Some((ctx, idx, snap)), Some(ph)) => ph.run(ctx, idx, snap).await,
            _ => vec![],
        };
        let mut diags = schema_diags;
        diags.extend(plugin_diags);
        self.client.publish_diagnostics(uri, diags, None).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.workspace.write().await.open_documents.remove(&uri);
        // Clear editor diagnostics; the file-cache copy remains for workspace validation.
        self.client.publish_diagnostics(uri, vec![], None).await;
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
                self.plugin_host.as_ref().map(|_| {
                    let ctx = plugin::build_hover_context(
                        &file_stem,
                        &col_name,
                        &cell_value,
                        pos.line,
                        doc,
                    );
                    let idx = runtime::build_workspace_index(&ws.open_documents, &ws.file_cache);
                    let snap = plugin::build_workspace_snapshot(&ws.open_documents, &ws.file_cache);
                    (ctx, idx, snap)
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
        let plugin_target = match (plugin_data, &self.plugin_host) {
            (Some((ctx, idx, snap)), Some(ph)) => ph.goto_definition(ctx, idx, snap).await,
            _ => return Ok(None),
        };

        let Some((target_file, target_col, target_value)) = plugin_target else {
            return Ok(None);
        };

        // Phase 3: resolve the plugin-provided target via the symbol index.
        let ws = self.workspace.read().await;
        Ok(ws
            .symbols
            .lookup(&target_file, &target_col, &target_value)
            .cloned()
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

            // Compute col_start for the range by summing preceding header lengths.
            let col_start = doc.headers[..col_index]
                .iter()
                .map(|h| h.chars().count() as u32 + 1)
                .sum::<u32>();
            let col_len = col_name.chars().count() as u32;

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
                        character: col_start + col_len,
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
            let cell_len = cell.value.chars().count() as u32;

            let plugin_hover_data = self.plugin_host.as_ref().map(|_| {
                let ctx =
                    plugin::build_hover_context(&file_stem, &col_name, &cell_value, pos.line, doc);
                let idx = runtime::build_workspace_index(&ws.open_documents, &ws.file_cache);
                let snap = plugin::build_workspace_snapshot(&ws.open_documents, &ws.file_cache);
                (ctx, idx, snap)
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
            (Some((ctx, idx, snap)), Some(ph)) => ph.hover(ctx, idx, snap).await,
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
