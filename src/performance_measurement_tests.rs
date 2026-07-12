use std::collections::HashMap;
use std::hint::black_box;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use serde_json::json;
use tower_lsp::lsp_types::{Diagnostic, Url};

use crate::document::DocumentData;
use crate::plugin::{PluginHost, build_context, build_workspace_snapshot};
use crate::runtime::build_workspace_index;
use crate::workspace::Workspace;

fn document(file: usize, rows: usize, columns: usize) -> Arc<DocumentData> {
    let headers = (0..columns)
        .map(|column| format!("c{column}"))
        .collect::<Vec<_>>()
        .join("\t");
    let mut text = String::with_capacity(rows * columns * 12);
    text.push_str(&headers);
    text.push('\n');
    for row in 0..rows {
        for column in 0..columns {
            if column > 0 {
                text.push('\t');
            }
            text.push_str(&format!("v{file:03}_{row:03}_{column:02}"));
        }
        text.push('\n');
    }
    Arc::new(DocumentData::parse(&text, '\t'))
}

fn synthetic_cache(
    files: usize,
    rows: usize,
    columns: usize,
) -> HashMap<PathBuf, Arc<DocumentData>> {
    (0..files)
        .map(|file| {
            (
                PathBuf::from(format!("/workspace/file{file:03}.txt")),
                document(file, rows, columns),
            )
        })
        .collect()
}

fn diagnostic_fingerprint(uri: &str, diagnostics: &[Diagnostic]) -> String {
    let mut canonical = diagnostics
        .iter()
        .map(|diagnostic| {
            serde_json::to_string(&json!({
                "uri": uri,
                "range": diagnostic.range,
                "severity": diagnostic.severity,
                "message": diagnostic.message,
                "code": diagnostic.code,
                "data": diagnostic.data,
            }))
            .unwrap()
        })
        .collect::<Vec<_>>();
    canonical.sort();
    canonical.join("\n")
}

fn bundled_plugin_paths() -> Vec<PathBuf> {
    let mut paths =
        std::fs::read_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("contrib/d2rdoc/plugins"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                matches!(
                    path.extension().and_then(|extension| extension.to_str()),
                    Some("ts") | Some("js")
                )
            })
            .collect::<Vec<_>>();
    paths.sort();
    paths
}

// Measurement harness for V-VLSP-15. This is ignored because it intentionally
// performs 200 whole-workspace rebuilds and reports timing rather than enforcing
// a machine-dependent duration threshold.
#[test]
#[ignore = "explicit performance measurement"]
fn measure_v_vlsp_15_same_revision_rebuilds_against_arc_reuse() {
    const FILES: usize = 48;
    const ROWS: usize = 96;
    const COLUMNS: usize = 12;
    const REQUESTS: usize = 200; // 100 hover + 100 definition

    let file_cache = synthetic_cache(FILES, ROWS, COLUMNS);
    let open_documents = HashMap::new();

    let rebuild_start = Instant::now();
    for _ in 0..REQUESTS {
        black_box(build_workspace_index(&open_documents, &file_cache));
        black_box(build_workspace_snapshot(&open_documents, &file_cache));
    }
    let rebuild_elapsed = rebuild_start.elapsed();

    let mut workspace = Workspace::new();
    workspace.begin_initialization(41);
    workspace.file_cache = file_cache;
    let reuse_start = Instant::now();
    let first = workspace.plugin_workspace_view();
    for _ in 0..REQUESTS {
        let cached = black_box(workspace.plugin_workspace_view());
        assert!(Arc::ptr_eq(&first.index, &cached.index));
        assert!(Arc::ptr_eq(&first.snapshot, &cached.snapshot));
    }
    let reuse_elapsed = reuse_start.elapsed();

    assert!(first.index.lookup("file000", "c0", "v000_000_00"));
    assert_eq!(first.snapshot.files["file000"].rows.len(), ROWS);
    eprintln!(
        "V-VLSP-15 files={FILES} cells={} requests={REQUESTS} baseline_index_builds={REQUESTS} baseline_snapshot_builds={REQUESTS} baseline_ms={:.3} cached_index_builds=1 cached_snapshot_builds=1 cached_ms={:.3}",
        FILES * ROWS * COLUMNS,
        rebuild_elapsed.as_secs_f64() * 1000.0,
        reuse_elapsed.as_secs_f64() * 1000.0,
    );
}

// Measurement harness for V-VLSP-16. It records the current conservative
// invalidation fan-out and the corresponding repeated workspace builder count.
#[test]
#[ignore = "explicit performance measurement"]
fn measure_v_vlsp_16_single_change_invalidation_fanout() {
    const OPEN_DOCUMENTS: usize = 32;
    const DISK_DOCUMENTS: usize = 128;

    let mut workspace = Workspace::new();
    workspace.begin_initialization(31);
    let scan_generation = workspace.begin_scan();
    assert!(workspace.begin_reconciliation(scan_generation));
    workspace.file_cache = synthetic_cache(DISK_DOCUMENTS, 24, 8);

    let mut uris = Vec::new();
    for file in 0..OPEN_DOCUMENTS {
        let uri = Url::parse(&format!("file:///workspace/open{file:03}.txt")).unwrap();
        workspace.accept_open(uri.clone(), 1, document(1_000 + file, 24, 8));
        uris.push(uri);
    }
    for ticket in workspace.open_tickets() {
        assert!(workspace.mark_published(&ticket));
    }
    assert!(workspace.pending_open_tickets().is_empty());

    let changed_uri = &uris[0];
    workspace
        .accept_change(changed_uri, 2, document(9_999, 24, 8))
        .unwrap();
    let pending = workspace.pending_open_tickets().len();
    assert_eq!(pending, OPEN_DOCUMENTS);

    let build_pairs = 1 + pending; // one shared disk pass + one per pending open ticket
    let rebuild_start = Instant::now();
    let mut last_index = None;
    let mut last_snapshot = None;
    for _ in 0..build_pairs {
        last_index = Some(build_workspace_index(
            &workspace.open_documents,
            &workspace.file_cache,
        ));
        last_snapshot = Some(build_workspace_snapshot(
            &workspace.open_documents,
            &workspace.file_cache,
        ));
    }
    let rebuild_elapsed = rebuild_start.elapsed();
    let index = last_index.unwrap();
    let snapshot = last_snapshot.unwrap();

    let cached_start = Instant::now();
    let first_cached = workspace.plugin_workspace_view();
    for _ in 0..build_pairs {
        let cached = workspace.plugin_workspace_view();
        assert!(Arc::ptr_eq(&first_cached.index, &cached.index));
        assert!(Arc::ptr_eq(&first_cached.snapshot, &cached.snapshot));
    }
    let cached_elapsed = cached_start.elapsed();

    assert!(index.lookup("open000", "c0", "v9999_000_00"));
    assert!(!index.lookup("open000", "c0", "v1000_000_00"));
    assert_eq!(
        snapshot.files["open000"].rows[0].cells[0].value,
        "v9999_000_00"
    );
    eprintln!(
        "V-VLSP-16 changed_stems=1 open_documents={OPEN_DOCUMENTS} pending_open={pending} disk_documents={DISK_DOCUMENTS} baseline_index_builds={build_pairs} baseline_snapshot_builds={build_pairs} baseline_ms={:.3} cached_index_builds=1 cached_snapshot_builds=1 cached_ms={:.3} oracle=new-live-generation",
        rebuild_elapsed.as_secs_f64() * 1000.0,
        cached_elapsed.as_secs_f64() * 1000.0,
    );
}

// Stage 3 feasibility harness. The local `routed` branch represents optional
// metadata saying no validator applies; it does not change product routing.
#[tokio::test]
#[ignore = "explicit performance measurement"]
async fn measure_stage_3_applicability_with_diagnostic_fingerprints() {
    let all_paths = bundled_plugin_paths();
    let all_host = PluginHost::new(all_paths).unwrap();
    let cube_host = PluginHost::new(vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("contrib/d2rdoc/plugins/cubeInputCheck.ts"),
    ])
    .unwrap();

    let mut file_cache = HashMap::new();
    for (stem, text) in [
        ("weapons", "code\tname\nhpot\tHealing Potion\n"),
        ("armor", "code\tname\ncap\tCap\n"),
        ("misc", "code\tname\nkey\tKey\n"),
        (
            "itemtypes",
            "Code\tTreasureClass\tItemType\nweap\t1\tWeapon\narmo\t1\tArmor\n",
        ),
        ("uniqueitems", "index\nThe Gnasher\n"),
        ("setitems", "index\nHsarus' Iron Heel\n"),
        ("cubemain", "desc\tinput 1\nr\tbadbase,qty=1\n"),
    ] {
        file_cache.insert(
            PathBuf::from(format!("/workspace/{stem}.txt")),
            Arc::new(DocumentData::parse(text, '\t')),
        );
    }
    let open_documents = HashMap::new();
    let index = build_workspace_index(&open_documents, &file_cache);
    let snapshot = build_workspace_snapshot(&open_documents, &file_cache);
    let cube = &file_cache[&PathBuf::from("/workspace/cubemain.txt")];
    let context = build_context("cubemain", cube);
    let all_diagnostics = all_host
        .run(context.clone(), Arc::clone(&index), Arc::clone(&snapshot))
        .await;
    let routed_diagnostics = cube_host
        .run(context, Arc::clone(&index), Arc::clone(&snapshot))
        .await;
    assert_eq!(
        diagnostic_fingerprint("file:///workspace/cubemain.txt", &all_diagnostics),
        diagnostic_fingerprint("file:///workspace/cubemain.txt", &routed_diagnostics),
        "routing an applicable bundled validator must preserve its complete diagnostic fingerprint"
    );

    let sounds = document(7_777, 400, 64);
    assert!(!all_host.validates_file("sounds"));
    let non_target_context_bytes = build_context("sounds", &sounds).len();
    let iterations = 12;
    let full_start = Instant::now();
    let mut current = Vec::new();
    for _ in 0..iterations {
        let non_target_context = build_context("sounds", &sounds);
        current = all_host
            .run(
                non_target_context,
                Arc::clone(&index),
                Arc::clone(&snapshot),
            )
            .await;
    }
    let full_elapsed = full_start.elapsed();

    let skip_start = Instant::now();
    let mut routed = Vec::<Diagnostic>::new();
    for _ in 0..iterations {
        if all_host.validates_file("sounds") {
            routed = all_host
                .run(
                    build_context("sounds", &sounds),
                    Arc::clone(&index),
                    Arc::clone(&snapshot),
                )
                .await;
        }
        black_box(&routed);
    }
    let skip_elapsed = skip_start.elapsed();
    assert_eq!(
        diagnostic_fingerprint("file:///workspace/sounds.txt", &current),
        diagnostic_fingerprint("file:///workspace/sounds.txt", &routed),
        "skipping a proven non-target bundled validator set must preserve the empty fingerprint"
    );
    eprintln!(
        "STAGE-3 non_target=sounds rows=400 columns=64 context_bytes={} iterations={iterations} full_context_plugin_ms={:.3} metadata_skip_ms={:.6} non_target_diagnostics={} fingerprint=identical-empty applicable_cube_diagnostics={} applicable_cube_fingerprint=identical",
        non_target_context_bytes,
        full_elapsed.as_secs_f64() * 1000.0,
        skip_elapsed.as_secs_f64() * 1000.0,
        current.len(),
        all_diagnostics.len(),
    );
}
