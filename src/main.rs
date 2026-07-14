mod backend;
mod cli;
mod contrib;
mod diagnostics;
mod document;
#[cfg(test)]
mod performance_measurement_tests;
mod plugin;
mod reference_data;
mod runtime;
mod scan;
mod schema;
mod settings;
mod source_selection;
mod workspace;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use config::{Config, Environment, File};
use tokio::sync::RwLock;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity};
use tower_lsp::{LspService, Server};

use cli::CliArgs;
use document::DocumentData;
use runtime::build_workspace_index_with_fallback;
use schema::find_loader;
use settings::{IoType, VectorLspSettings};
use source_selection::{SourceKind, effective_workspace_sources_with_fallback};
use workspace::{SymbolIndex, Workspace};

/// Append sorted .ts/.js plugin files from `dir` to `out`, skipping `_patches.js`.
fn scan_plugin_dir(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut found: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && matches!(
                    p.extension().and_then(|e| e.to_str()),
                    Some("ts") | Some("js")
                )
                && p.file_name().map_or(true, |n| n != "_patches.js")
        })
        .collect();
    found.sort();
    out.extend(found);
}

/// Preserve tier order while collapsing path aliases of the same plugin file.
fn deduplicate_plugin_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| {
            let identity = path.canonicalize().unwrap_or_else(|_| path.clone());
            seen.insert(identity)
        })
        .collect()
}

/// Collect plugin file paths in tier order: base → variant → explicit override.
fn collect_plugin_paths(settings: &VectorLspSettings) -> Vec<std::path::PathBuf> {
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    // Ask the loader for its default plugin directories (base + variant).
    if let Ok(loader) = find_loader(
        &settings.schema_loader,
        settings.schema_variant.clone(),
        settings.plugin_path.clone(),
    ) {
        for dir in loader.default_plugin_dirs() {
            scan_plugin_dir(&dir, &mut paths);
        }
    }
    // Explicit plugin_path is additive on top of the defaults.
    if let Some(ref dir) = settings.plugin_path {
        scan_plugin_dir(dir, &mut paths);
    }
    deduplicate_plugin_paths(paths)
}

fn diagnostic_severity_name(diag: &Diagnostic) -> &'static str {
    match diag.severity {
        Some(DiagnosticSeverity::ERROR) => "error",
        Some(DiagnosticSeverity::WARNING) => "warning",
        Some(DiagnosticSeverity::INFORMATION) => "info",
        Some(DiagnosticSeverity::HINT) => "hint",
        _ => "hint",
    }
}

fn count_diagnostic(diag: &Diagnostic, counts: &mut (usize, usize, usize, usize)) {
    match diag.severity {
        Some(DiagnosticSeverity::ERROR) => counts.0 += 1,
        Some(DiagnosticSeverity::WARNING) => counts.1 += 1,
        Some(DiagnosticSeverity::INFORMATION) => counts.2 += 1,
        Some(DiagnosticSeverity::HINT) => counts.3 += 1,
        _ => counts.3 += 1,
    }
}

/// Run a one-shot workspace check: scan all data files, validate them, print diagnostics, and
/// return an exit code (0 = clean, 1 = errors found, 2 = configuration/IO error).
async fn run_check(settings: &VectorLspSettings) -> i32 {
    let workspace_path = match &settings.workspace_path {
        Some(p) => p.clone(),
        None => {
            eprintln!("error: single_shot mode requires `workspace_path` in config");
            return 2;
        }
    };

    let plugin_paths = collect_plugin_paths(settings);
    let plugin_host = if plugin_paths.is_empty() {
        None
    } else {
        match plugin::PluginHost::new(plugin_paths.clone()) {
            Ok(host) => Some(host),
            Err(error) => {
                eprintln!("error: {error}");
                return 2;
            }
        }
    };

    // Load schema if a path or variant is configured.
    let schema_result = if settings.schema_path.is_some() || !settings.schema_variant.is_empty() {
        let loader = match find_loader(
            &settings.schema_loader,
            settings.schema_variant.clone(),
            settings.plugin_path.clone(),
        ) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        let schema_path = settings.schema_path.clone();
        match tokio::task::spawn_blocking(move || loader.load(schema_path.as_deref())).await {
            Ok(Ok(s)) => {
                eprintln!("Schema loaded.");
                Some(Arc::new(s))
            }
            Ok(Err(e)) => {
                eprintln!("error: schema load failed: {e:#}");
                return 2;
            }
            Err(e) => {
                eprintln!("error: schema task panicked: {e}");
                return 2;
            }
        }
    } else {
        None
    };

    if let (Some(ph), Some(schema)) = (&plugin_host, &schema_result) {
        ph.set_schema(Arc::clone(schema)).await;
    }
    eprintln!("Loaded {} plugin file(s).", plugin_paths.len());

    let reference_dataset = match reference_data::load_selected_reference_dataset(settings) {
        Ok(dataset) => dataset,
        Err(error) => {
            eprintln!("warning: bundled reference fallback disabled for this run: {error:#}");
            None
        }
    };
    if let Some(dataset) = &reference_dataset {
        eprintln!(
            "Loaded {} hidden reference tables for game version {} ({}).",
            dataset.documents.len(),
            dataset.game_version,
            dataset.canonical_sha256
        );
    } else {
        eprintln!("Bundled reference fallback is disabled for this run.");
    }

    let ref_targets: HashSet<(String, String)> = schema_result
        .as_ref()
        .map(|s| s.reference_targets())
        .unwrap_or_default();

    // Scan and parse workspace files.
    let ext = settings.extension.as_str();
    let delimiter = settings.delimiter_char();
    let discovery =
        match scan::collect_data_files(&workspace_path, &scan::ScanPolicy::standalone(ext)) {
            Ok(discovery) => discovery,
            Err(e) => {
                eprintln!(
                    "error: cannot read workspace directory '{}': {e}",
                    workspace_path.display()
                );
                return 2;
            }
        };
    for failure in &discovery.failures {
        eprintln!(
            "warning: skipping '{}': {}",
            failure.path.display(),
            failure.reason
        );
    }

    let mut parsed: Vec<(std::path::PathBuf, String, Arc<DocumentData>)> = Vec::new();
    let workspace_present_stems = discovery
        .paths
        .iter()
        .filter_map(|path| path.file_stem().and_then(|stem| stem.to_str()))
        .map(str::to_ascii_lowercase)
        .collect::<HashSet<_>>();
    for path in discovery.paths {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        match std::fs::read(&path).and_then(|b| Ok(settings.encoding.decode(&b))) {
            Ok(Ok(src)) => {
                parsed.push((path, stem, Arc::new(DocumentData::parse(&src, delimiter))))
            }
            Ok(Err(e)) => eprintln!("warning: skipping '{}': {e}", path.display()),
            Err(e) => eprintln!("warning: skipping '{}': {e}", path.display()),
        }
    }
    parsed.sort_by(|a, b| a.0.cmp(&b.0));

    let mut file_cache: HashMap<PathBuf, Arc<DocumentData>> = HashMap::new();
    for (path, _, doc) in &parsed {
        file_cache.insert(path.clone(), Arc::clone(doc));
    }
    let open_documents = HashMap::new();
    let empty_fallback = HashMap::new();
    let fallback_cache = reference_dataset
        .as_ref()
        .map(|dataset| &dataset.documents)
        .unwrap_or(&empty_fallback);
    let fallback_version = reference_dataset
        .as_ref()
        .map(|dataset| dataset.game_version.as_str());
    let effective_sources = effective_workspace_sources_with_fallback(
        &open_documents,
        &file_cache,
        fallback_cache,
        &workspace_present_stems,
        fallback_version,
    );
    let mut symbols = SymbolIndex::new();
    for source in &effective_sources {
        symbols.index_effective_document(
            source.uri.as_ref(),
            &source.stem,
            &source.document,
            &ref_targets,
            source.kind,
            source.bundled_version.as_deref(),
        );
    }
    let workspace_index = build_workspace_index_with_fallback(
        &open_documents,
        &file_cache,
        fallback_cache,
        &workspace_present_stems,
        fallback_version,
    );
    let snapshot = plugin::build_workspace_snapshot_with_fallback(
        &open_documents,
        &file_cache,
        fallback_cache,
        &workspace_present_stems,
        fallback_version,
    );

    // Validate and collect diagnostics.
    let mut counts = (0usize, 0usize, 0usize, 0usize);
    let mut file_count = 0usize;

    for source in &effective_sources {
        if source.kind == SourceKind::Bundled {
            continue;
        }
        let Some(path) = &source.path else { continue };
        let stem = &source.stem;
        let doc = &source.document;
        let mut diags = diagnostics::validate_document_for_version(
            stem,
            doc,
            schema_result.as_deref(),
            &symbols,
            fallback_version,
        );
        if let Some(ph) = &plugin_host
            && ph.validates_file(stem)
        {
            let ctx = plugin::build_context(stem, doc);
            diags.extend(
                ph.run(ctx, Arc::clone(&workspace_index), Arc::clone(&snapshot))
                    .await,
            );
        }
        if diags.is_empty() {
            continue;
        }
        file_count += 1;
        let display = path.display();
        for d in &diags {
            let line = d.range.start.line + 1;
            let col = d.range.start.character + 1;
            count_diagnostic(d, &mut counts);
            let severity = diagnostic_severity_name(d);
            println!("{display}:{line}:{col}: {severity}: {}", d.message);
        }
    }

    eprintln!(
        "{} error(s), {} warning(s), {} info, {} hint diagnostic(s) across {file_count} file(s); {} parsed file(s).",
        counts.0,
        counts.1,
        counts.2,
        counts.3,
        parsed.len()
    );
    if counts.0 > 0 { 1 } else { 0 }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = CliArgs::parse();

    let mut config_builder = Config::builder();
    if !args.editor_mode {
        config_builder =
            config_builder.add_source(File::with_name(&args.config_file).required(false));
    }
    let raw = config_builder
        .add_source(Environment::with_prefix("VLSP"))
        .build()?;

    let mut settings = raw
        .try_deserialize::<VectorLspSettings>()
        .map_err(|error| {
            anyhow::anyhow!(
                "invalid vector-lsp configuration from '{}': {error}",
                args.config_file
            )
        })?;
    if let Some(schema_path) = args.schema_path {
        settings.schema_path = Some(schema_path);
    }
    if args.editor_mode {
        settings.apply_editor_mode();
    }
    settings.validate()?;
    let settings = Arc::new(settings);

    if !settings.editor_mode && (settings.single_shot || args.single_shot) {
        let code = run_check(&settings).await;
        std::process::exit(code);
    }

    let plugin_paths = collect_plugin_paths(&settings);

    match settings.io_type.clone() {
        IoType::Stdio => {
            let workspace = Arc::new(RwLock::new(Workspace::new()));
            let publish_gates = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
            let plugin_host = if plugin_paths.is_empty() {
                None
            } else {
                Some(plugin::PluginHost::new(plugin_paths)?)
            };
            let stdin = tokio::io::stdin();
            let stdout = tokio::io::stdout();
            let (service, socket) = LspService::new(move |client| backend::Backend {
                client,
                settings: Arc::clone(&settings),
                workspace: Arc::clone(&workspace),
                plugin_host: plugin_host.clone(),
                publish_gates: Arc::clone(&publish_gates),
            });
            Server::new(stdin, stdout, socket).serve(service).await;
        }
        IoType::Tcp(tcp) => {
            let addr = format!("{}:{}", tcp.host, tcp.port);
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            let plugin_paths = Arc::new(plugin_paths);
            loop {
                let (stream, _) = listener.accept().await?;
                let (read, write) = tokio::io::split(stream);
                let settings = Arc::clone(&settings);
                let workspace = Arc::new(RwLock::new(Workspace::new()));
                let publish_gates = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
                let plugin_host = if plugin_paths.is_empty() {
                    None
                } else {
                    match plugin::PluginHost::new(plugin_paths.as_ref().clone()) {
                        Ok(host) => Some(host),
                        Err(error) => {
                            eprintln!("vector-lsp: TCP plugin runtime startup failed: {error}");
                            continue;
                        }
                    }
                };
                let (service, socket) = LspService::new(move |client| backend::Backend {
                    client,
                    settings: Arc::clone(&settings),
                    workspace: Arc::clone(&workspace),
                    plugin_host: plugin_host.clone(),
                    publish_gates: Arc::clone(&publish_gates),
                });
                tokio::spawn(async move {
                    Server::new(read, write, socket).serve(service).await;
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod plugin_discovery_tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_plugin_dir(test_name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "vector-lsp-{test_name}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn plugin_scan_keeps_only_regular_javascript_and_typescript_files() {
        let dir = temp_plugin_dir("plugin-scan");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("b.js"), "function validate() { return []; }").unwrap();
        fs::write(
            dir.join("a.ts"),
            "function validate(): unknown[] { return []; }",
        )
        .unwrap();
        fs::write(dir.join("_patches.js"), "// schema patch").unwrap();
        fs::write(dir.join("ignored.txt"), "not a plugin").unwrap();
        fs::create_dir(dir.join("directory.js")).unwrap();

        let mut paths = Vec::new();
        scan_plugin_dir(&dir, &mut paths);
        let names = paths
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["a.ts", "b.js"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn plugin_deduplication_preserves_the_first_tier_and_order() {
        let dir = temp_plugin_dir("plugin-dedup");
        fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first.js");
        let second = dir.join("second.js");
        fs::write(&first, "function validate() { return []; }").unwrap();
        fs::write(&second, "function validate() { return []; }").unwrap();
        let alias = dir.join(".").join("first.js");

        assert_eq!(
            deduplicate_plugin_paths(vec![first.clone(), second.clone(), alias]),
            vec![first, second]
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
