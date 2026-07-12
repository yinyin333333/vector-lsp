use std::path::Path;
use std::process::Command;

fn main() {
    // V8 (via deno_core) uses ETW and registry APIs that live in advapi32.
    // When cargo builds with an explicit --target triple (as cargo-dist does),
    // the MSVC default-lib resolution doesn't always pull this in automatically.
    #[cfg(windows)]
    println!("cargo:rustc-link-lib=advapi32");

    println!("cargo:rerun-if-changed=contrib/d2rdoc/sync-schemas.ps1");
    println!("cargo:rerun-if-changed=contrib/");

    if std::env::var("CARGO_FEATURE_D2RDOC").is_err() {
        return;
    }

    if !schemas_present() {
        println!("cargo:warning=Schema files missing — running sync-schemas.ps1...");

        let status = Command::new("powershell")
            .args([
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                "contrib\\d2rdoc\\sync-schemas.ps1",
            ])
            .status();

        match status {
            Ok(s) if s.success() => {}
            Ok(s) => println!(
                "cargo:warning=sync-schemas.ps1 exited with {s}; \
                 schema files may be incomplete"
            ),
            Err(e) => println!(
                "cargo:warning=Could not run sync-schemas.ps1: {e}; \
                 schema files may be missing at runtime"
            ),
        }
    }

    copy_contrib_to_target();
}

/// Returns true if at least one contrib/d2rdoc/<version>/schema/ directory
/// contains files, indicating schemas have been synced.
fn schemas_present() -> bool {
    let contrib = Path::new("contrib/d2rdoc");
    let Ok(entries) = std::fs::read_dir(contrib) else {
        return false;
    };
    for entry in entries.flatten() {
        let schema_dir = entry.path().join("schema");
        if schema_dir.is_dir() {
            if let Ok(mut inner) = std::fs::read_dir(&schema_dir) {
                if inner.next().is_some() {
                    return true;
                }
            }
        }
    }
    false
}

/// Copy the contrib/ tree into the profile output directory (e.g.
/// target/release/) so the binary and its runtime assets are co-located.
///
/// OUT_DIR is target/{profile}/build/vector-lsp-{hash}/out — four levels up
/// is target/{profile}/.
fn copy_contrib_to_target() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let target_profile_dir = Path::new(&out_dir)
        .ancestors()
        .nth(3)
        .expect("unexpected OUT_DIR depth")
        .to_owned();

    copy_dir_recursive(Path::new("contrib"), &target_profile_dir.join("contrib"));
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    let Ok(entries) = std::fs::read_dir(src) else {
        return;
    };
    let entries: Vec<_> = entries.flatten().collect();
    std::fs::create_dir_all(dst).expect("failed to create output dir");
    let source_names: std::collections::HashSet<_> =
        entries.iter().map(std::fs::DirEntry::file_name).collect();
    if let Ok(destination_entries) = std::fs::read_dir(dst) {
        for entry in destination_entries.flatten() {
            if source_names.contains(&entry.file_name()) {
                continue;
            }
            let stale_path = entry.path();
            let removal = if stale_path.is_dir() {
                std::fs::remove_dir_all(&stale_path)
            } else {
                std::fs::remove_file(&stale_path)
            };
            if let Err(error) = removal {
                panic!(
                    "failed to prune stale contrib asset '{}': {error}",
                    stale_path.display()
                );
            }
        }
    }
    for entry in entries {
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path);
        } else {
            std::fs::copy(&src_path, &dst_path).unwrap_or_else(|e| {
                println!("cargo:warning=Failed to copy {}: {e}", src_path.display());
                0
            });
        }
    }
}
