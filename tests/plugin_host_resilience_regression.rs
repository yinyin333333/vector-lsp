use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

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

    fn write(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.0.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_config(tree: &TempTree, plugin_path: Option<&Path>, bundled: bool) -> PathBuf {
    let workspace = tree.0.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let mut config = serde_json::json!({
        "workspace_path": workspace.to_string_lossy(),
        "single_shot": true,
        "extension": "txt",
        "schema_loader": if bundled { "d2rdoc" } else { "no-test-loader" }
    });
    if let Some(path) = plugin_path {
        config["plugin_path"] = serde_json::json!(path.to_string_lossy());
    }
    let config_path = tree.0.join("config.json");
    fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    config_path
}

fn run(tree: &TempTree, plugin_path: Option<&Path>, bundled: bool) -> std::process::Output {
    let config = write_config(tree, plugin_path, bundled);
    Command::new(env!("CARGO_BIN_EXE_vector-lsp"))
        .args(["--config-file", config.to_str().unwrap()])
        .output()
        .expect("run vector-lsp single-shot")
}

struct TimedOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

fn run_with_timeout(
    tree: &TempTree,
    plugin_path: &Path,
    timeout: Duration,
) -> Result<TimedOutput, String> {
    let config = write_config(tree, Some(plugin_path), false);
    let mut child = Command::new(env!("CARGO_BIN_EXE_vector-lsp"))
        .args(["--config-file", config.to_str().unwrap()])
        .env("VLSP_PLUGIN_TIMEOUT_MS", "100")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn vector-lsp single-shot");
    let deadline = Instant::now() + timeout;

    let status = loop {
        if let Some(status) = child.try_wait().expect("poll vector-lsp child") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "plugin request exceeded the {:?} execution budget and was killed",
                timeout
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    Ok(TimedOutput {
        status,
        stdout,
        stderr,
    })
}

// V-VLSP-11: exercise the actual sorted bundled set in one PluginHost.
#[test]
fn all_bundled_plugins_load_together_for_a_representative_fixture() {
    let tree = TempTree::new("bundled-plugins");
    tree.write(
        "workspace/cubemain.txt",
        "desc\tinput 1\nr\tbadbase,qty=1\n",
    );
    tree.write(
        "workspace/weapons.txt",
        "code\tname\nhpot\tHealing Potion\n",
    );
    tree.write("workspace/armor.txt", "code\tname\ncap\tCap\n");
    tree.write("workspace/misc.txt", "code\tname\nkey\tKey\n");
    tree.write(
        "workspace/itemtypes.txt",
        "Code\tTreasureClass\tItemType\nweap\t1\tWeapon\narmo\t1\tArmor\n",
    );
    tree.write("workspace/uniqueitems.txt", "index\nThe Gnasher\n");
    tree.write("workspace/setitems.txt", "index\nHsarus' Iron Heel\n");

    let output = run(&tree, None, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(stderr.contains("Loaded 8 plugin file(s)."), "{stderr}");
    assert!(
        !stderr.contains("vector-lsp: plugin '"),
        "bundled plugins must load without errors:\n{stderr}"
    );
    assert!(
        stdout.contains("couldn't find 'badbase' for input 1"),
        "combined host must preserve the representative cube rule result:\n{stdout}\n{stderr}"
    );
}

// V-VLSP-12: each plugin needs an independent lexical scope. The expected
// messages also prove that each registered function keeps its own helper.
#[test]
fn plugins_with_colliding_globals_keep_independent_bindings() {
    let tree = TempTree::new("plugin-collision");
    let plugins = tree.0.join("plugins");
    tree.write(
        "plugins/a.js",
        "const SHARED='A'; function helper(){return SHARED;}\n\
         function validate(){return [{line:0,col:0,message:'PLUGIN_'+helper()}];}\n",
    );
    tree.write(
        "plugins/b.js",
        "const SHARED='B'; function helper(){return SHARED;}\n\
         function validate(){return [{line:0,col:0,message:'PLUGIN_'+helper()}];}\n",
    );
    tree.write("workspace/sample.txt", "code\nvalue\n");

    let output = run(&tree, Some(&plugins), false);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("PLUGIN_A") && stdout.contains("PLUGIN_B"),
        "both colliding plugins must load and retain their helper binding:\n{stdout}\n{stderr}"
    );
}

#[test]
fn plugin_applicability_skips_only_declared_non_targets_and_keeps_custom_fallback() {
    let tree = TempTree::new("plugin-applicability");
    let plugins = tree.0.join("plugins");
    tree.write(
        "plugins/scoped.js",
        "const pluginMetadata={validateFiles:['TARGET']};\n\
         function validate(ctx){return [{line:0,col:0,message:'SCOPED_'+ctx.file}];}\n",
    );
    tree.write(
        "plugins/custom.js",
        "function validate(ctx){return [{line:0,col:0,message:'CUSTOM_'+ctx.file}];}\n",
    );
    tree.write("workspace/target.txt", "code\nvalue\n");
    tree.write("workspace/other.txt", "code\nvalue\n");

    let output = run(&tree, Some(&plugins), false);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("SCOPED_target"), "{stdout}\n{stderr}");
    assert!(!stdout.contains("SCOPED_other"), "{stdout}\n{stderr}");
    assert!(stdout.contains("CUSTOM_target"), "{stdout}\n{stderr}");
    assert!(stdout.contains("CUSTOM_other"), "{stdout}\n{stderr}");
}

#[test]
fn single_shot_duplicate_stems_use_one_lexical_source_for_every_consumer() {
    let tree = TempTree::new("single-shot-duplicate-stem");
    let workspace = tree.0.join("workspace");
    let schema = tree.0.join("schema");
    let plugins = tree.0.join("plugins");

    tree.write(
        "schema/items.js",
        "files['items']={fields:[{name:'id',type:{type:'text'}}]};\n",
    );
    tree.write(
        "schema/source.js",
        "files['source']={fields:[{name:'target',type:{type:'reference',file:'items',field:'id'}}]};\n",
    );
    tree.write(
        "plugins/winner.js",
        "const pluginMetadata={validateFiles:['items']};\n\
         function validate(ctx){\n\
           const values=getColumnValues('items','id').join(',');\n\
           const winner=lookupKey('items','id','WIN_Z');\n\
           const loser=lookupKey('items','id','LOSE_A');\n\
           return [{line:1,col:0,message:'WINNER_CTX='+ctx.rows[0].id+';INDEX='+winner+'/'+loser+';SNAP='+values}];\n\
         }\n",
    );
    tree.write("workspace/Z/items.txt", "id\nWIN_Z\n");
    tree.write("workspace/a/ITEMS.txt", "id\nLOSE_A\n");
    tree.write("workspace/source.txt", "target\nLOSE_A\n");

    let config = tree.0.join("duplicate-config.json");
    fs::write(
        &config,
        serde_json::to_vec_pretty(&serde_json::json!({
            "workspace_path": workspace.to_string_lossy(),
            "single_shot": true,
            "extension": "txt",
            "schema_loader": "d2rdoc",
            "schema_path": schema.to_string_lossy(),
            "plugin_path": plugins.to_string_lossy()
        }))
        .unwrap(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_vector-lsp"))
        .args(["--config-file", config.to_str().unwrap()])
        .output()
        .expect("run duplicate-stem single-shot regression");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        stdout.matches("WINNER_CTX=").count(),
        1,
        "only the lexical duplicate-stem winner may be validated:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("WINNER_CTX=WIN_Z;INDEX=true/false;SNAP=WIN_Z"),
        "validation context, WorkspaceIndex, and plugin snapshot must observe the same winner:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("Reference value 'LOSE_A' not found in items.id"),
        "the SymbolIndex must exclude the duplicate-stem loser:\n{stdout}\n{stderr}"
    );
    assert!(
        !stdout.contains("WINNER_CTX=LOSE_A"),
        "the duplicate-stem loser must not be validated:\n{stdout}\n{stderr}"
    );
}

// V-VLSP-13: repeated plugin exceptions must be visible once, not converted
// into indistinguishable clean results or repeated once per workspace file.
#[test]
fn thrown_plugin_failure_is_identifiable_and_rate_limited() {
    let tree = TempTree::new("plugin-throw");
    let plugins = tree.0.join("plugins");
    tree.write(
        "plugins/throwing.js",
        "function validate(){throw new Error('boom-from-plugin');}\n",
    );
    tree.write("workspace/one.txt", "code\none\n");
    tree.write("workspace/two.txt", "code\ntwo\n");

    let output = run(&tree, Some(&plugins), false);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("throwing.js") && stderr.contains("boom-from-plugin"),
        "plugin exception must be an identifiable health/log error:\n{stderr}"
    );
    assert_eq!(
        stderr.matches("boom-from-plugin").count(),
        1,
        "the same plugin failure must be rate-limited:\n{stderr}"
    );
}

// V-VLSP-13: a malformed result is a plugin health failure, not a clean file.
#[test]
fn malformed_plugin_diagnostic_shape_is_observable() {
    let tree = TempTree::new("plugin-shape");
    let plugins = tree.0.join("plugins");
    tree.write(
        "plugins/malformed.js",
        "function validate(){return [{line:'bad',col:0,message:'bad shape'}];}\n",
    );
    tree.write("workspace/sample.txt", "code\nvalue\n");

    let output = run(&tree, Some(&plugins), false);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lower = stderr.to_lowercase();
    assert!(
        stderr.contains("malformed.js")
            && (lower.contains("diagnostic")
                || lower.contains("shape")
                || lower.contains("deserial")),
        "malformed plugin output must be reported as a health/log error:\n{stderr}"
    );
}

// V-VLSP-14: the loop runs only in a child process; this test always kills it
// at the outer deadline so a missing runtime interrupt cannot hang cargo test.
#[test]
fn plugin_execution_budget_interrupts_a_loop_and_keeps_the_queue_usable() {
    let tree = TempTree::new("plugin-budget");
    let plugins = tree.0.join("plugins");
    tree.write(
        "plugins/budget.js",
        "function validate(ctx){\n\
           if(ctx.file==='a_hang'){while(true){}}\n\
           if(ctx.file==='b_ok'){return [{line:0,col:0,message:'AFTER_TIMEOUT'}];}\n\
           return [];\n\
         }\n",
    );
    tree.write("workspace/a_hang.txt", "code\nhang\n");
    tree.write("workspace/b_ok.txt", "code\nok\n");

    let output = run_with_timeout(&tree, &plugins, Duration::from_secs(3))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        output.stdout.contains("AFTER_TIMEOUT"),
        "a normal queued request must run after interrupting the loop:\n{}\n{}",
        output.stdout,
        output.stderr
    );
    assert!(
        output.status.success(),
        "timeout recovery should not turn a warning-only run into a process failure"
    );
}

#[test]
fn timed_out_validate_plugin_does_not_discard_or_block_healthy_plugins_for_the_same_file() {
    let tree = TempTree::new("plugin-budget-isolation");
    let plugins = tree.0.join("plugins");
    tree.write(
        "plugins/a_before.js",
        "function validate(){return [{line:0,col:0,message:'BEFORE_TIMEOUT'}];}\n",
    );
    tree.write(
        "plugins/b_timeout.js",
        "function validate(){while(true){}}\n",
    );
    tree.write(
        "plugins/c_after.js",
        "function validate(){return [{line:0,col:0,message:'AFTER_TIMEOUT'}];}\n",
    );
    tree.write("workspace/sample.txt", "code\nvalue\n");

    let output = run_with_timeout(&tree, &plugins, Duration::from_secs(3))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        output.stdout.contains("BEFORE_TIMEOUT") && output.stdout.contains("AFTER_TIMEOUT"),
        "a timed-out plugin must not discard earlier results or block later plugins:\n{}\n{}",
        output.stdout,
        output.stderr
    );
    assert!(
        output.stderr.contains("b_timeout.js")
            && output
                .stderr
                .contains("validate exceeded the execution budget"),
        "the timed-out plugin must remain identifiable:\n{}\n{}",
        output.stdout,
        output.stderr
    );
    assert!(
        output.status.success(),
        "timeout isolation should preserve a warning-only successful run"
    );
}

#[test]
fn top_level_plugin_loop_fails_startup_with_an_identifiable_timeout() {
    let tree = TempTree::new("plugin-top-level-budget");
    let plugins = tree.0.join("plugins");
    tree.write("plugins/hanging.js", "while(true){}\n");
    tree.write("workspace/sample.txt", "code\nvalue\n");

    let output = run_with_timeout(&tree, &plugins, Duration::from_secs(3))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        !output.status.success(),
        "a plugin that cannot finish loading must fail startup"
    );
    assert!(
        output.stderr.contains("hanging.js")
            && output.stderr.contains("top-level/metadata evaluation")
            && output.stderr.contains("execution budget"),
        "the startup failure must identify the plugin and timeout phase:\n{}\n{}",
        output.stdout,
        output.stderr
    );
}

#[test]
fn applicability_metadata_loop_fails_startup_with_an_identifiable_timeout() {
    let tree = TempTree::new("plugin-metadata-budget");
    let plugins = tree.0.join("plugins");
    tree.write(
        "plugins/metadata.js",
        "Array.prototype.map=function(){while(true){}};\n\
         function validate(){return [];}\n",
    );
    tree.write("workspace/sample.txt", "code\nvalue\n");

    let output = run_with_timeout(&tree, &plugins, Duration::from_secs(3))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        !output.status.success(),
        "metadata aggregation that cannot finish must fail startup"
    );
    assert!(
        output
            .stderr
            .contains("plugin applicability metadata exceeded the execution budget"),
        "the startup failure must identify the applicability metadata phase:\n{}\n{}",
        output.stdout,
        output.stderr
    );
}
