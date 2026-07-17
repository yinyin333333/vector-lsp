use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[allow(dead_code)]
mod build_script_under_test {
    include!("../build.rs");

    pub fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
        copy_dir_recursive(src, dst);
    }
}

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

struct TempTree(PathBuf);

impl TempTree {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "vector-lsp-asset-sync-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create temporary test directory");
        Self(path)
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

// V-VLSP-18: rebuilding from a changed source manifest must prune assets that
// were deleted or renamed in the source tree.
#[test]
fn contrib_copy_prunes_stale_destination_files() {
    let tree = TempTree::new();
    let source = tree.0.join("source");
    let destination = tree.0.join("destination");

    write(&source.join("plugins/old.ts"), "old");
    build_script_under_test::copy_tree(&source, &destination);
    assert!(destination.join("plugins/old.ts").is_file());

    fs::remove_file(source.join("plugins/old.ts")).unwrap();
    write(&source.join("plugins/new.ts"), "new");
    build_script_under_test::copy_tree(&source, &destination);

    assert!(destination.join("plugins/new.ts").is_file());
    assert!(
        !destination.join("plugins/old.ts").exists(),
        "a deleted source asset must not survive the next destination sync"
    );
}

#[test]
fn reference_sync_requires_an_explicit_source_root_without_a_machine_path_default() {
    let script = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("contrib/d2rdoc/sync-reference-data.ps1"),
    )
    .expect("read bundled reference sync script");

    assert!(script.contains("REFERENCE_TXT_SOURCE_ROOT"));
    assert!(script.contains("Pass -ReferenceSourceRoot explicitly"));
    assert!(!script.contains(&["E:", "\\"].concat()));
    assert!(!script.contains(&["C:", "\\Users\\"].concat()));
}
