use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

struct TempTree(PathBuf);

impl TempTree {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "vector-lsp-{label}-{}-{}",
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

// VLSP-07: one invalid field must fail with its config context instead of
// discarding every otherwise-valid setting and continuing with defaults.
#[test]
fn invalid_config_field_is_reported_instead_of_defaulting_the_whole_config() {
    let tree = TempTree::new("invalid-config");
    let workspace = tree.0.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let config_path = tree.0.join("config.json");
    fs::write(
        &config_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "workspace_path": workspace.to_string_lossy(),
            "extension": "dat",
            "schema_loader": "deliberately-invalid-loader",
            "encoding": { "invalid": true }
        }))
        .unwrap(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_vector-lsp"))
        .args([
            "--config-file",
            config_path.to_str().unwrap(),
            "--single-shot",
        ])
        .output()
        .expect("run vector-lsp");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "invalid config must fail startup");
    assert!(
        stderr.to_lowercase().contains("encoding"),
        "startup error must identify the invalid encoding field; stderr was:\n{stderr}"
    );
}

#[test]
fn editor_mode_ignores_workspace_transport_and_lifecycle_config() {
    let tree = TempTree::new("editor-mode-config");
    fs::write(
        tree.0.join("config.json"),
        r#"{
          "single_shot": true,
          "io_type": { "type": "tcp", "host": "127.0.0.1", "port": 9 },
          "encoding": { "invalid": true },
          "schema_variant": "workspace-must-not-win"
        }"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_vector-lsp"))
        .current_dir(&tree.0)
        .args(["--editor-mode", "--single-shot"])
        .env_remove("VLSP_IO_TYPE")
        .env_remove("VLSP_SINGLE_SHOT")
        .env_remove("VLSP_SCHEMA_VARIANT")
        .output()
        .expect("run vector-lsp editor mode");

    assert!(
        output.status.success(),
        "editor mode must start stdio and exit cleanly on EOF; stderr was:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "single-shot output leaked into LSP stdout"
    );
}
