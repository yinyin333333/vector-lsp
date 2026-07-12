use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tower_lsp::lsp_types::Url;

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

struct TempTree(PathBuf);

impl TempTree {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "vector-lsp-tcp-isolation-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
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

struct ServerChild(Child);

impl Drop for ServerChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct LspClient {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
}

impl LspClient {
    fn connect(port: u16) -> Self {
        let stream = (0..100)
            .find_map(|_| match TcpStream::connect(("127.0.0.1", port)) {
                Ok(stream) => Some(stream),
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(20));
                    None
                }
            })
            .expect("connect to vector-lsp TCP listener");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let writer = stream.try_clone().unwrap();
        Self {
            reader: BufReader::new(stream),
            writer,
            next_id: 1,
        }
    }

    fn send(&mut self, message: &Value) {
        let body = message.to_string();
        write!(
            self.writer,
            "Content-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
        self.writer.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc":"2.0","method":method,"params":params}));
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        loop {
            let message = self.read_message();
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                assert!(message.get("error").is_none(), "{message}");
                return message.get("result").cloned().unwrap_or(Value::Null);
            }
        }
    }

    fn read_message(&mut self) -> Value {
        let mut content_length = None;
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).expect("read LSP header");
            assert!(!line.is_empty(), "LSP TCP stream closed");
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                content_length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
        let mut body = vec![0; content_length.expect("Content-Length header")];
        self.reader.read_exact(&mut body).expect("read LSP body");
        serde_json::from_slice(&body).unwrap()
    }

    fn read_message_before(&mut self, deadline: Instant) -> Option<Value> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        self.reader
            .get_mut()
            .set_read_timeout(Some(remaining))
            .unwrap();

        let mut content_length = None;
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => return None,
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return None;
                }
                Err(error) => panic!("read LSP header: {error}"),
            }
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                content_length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
        let mut body = vec![0; content_length.expect("Content-Length header")];
        self.reader.read_exact(&mut body).expect("read LSP body");
        Some(serde_json::from_slice(&body).unwrap())
    }

    fn wait_for_method(&mut self, method: &str) -> Value {
        loop {
            let message = self.read_message();
            if message.get("method").and_then(Value::as_str) == Some(method) {
                return message;
            }
        }
    }

    fn initialize(&mut self, root: &Path, generation: u64) {
        self.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": file_uri(root),
                "initializationOptions": {"sessionGeneration":generation},
                "capabilities": {"textDocument":{"publishDiagnostics":{}}}
            }),
        );
        self.notify("initialized", json!({}));
        let ready = self.wait_for_method("vectorLsp/ready");
        assert_eq!(ready["params"]["sessionGeneration"], generation);
    }

    fn open(&mut self, path: &Path, version: i32) {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument":{
                "uri":file_uri(path),
                "languageId":"plaintext",
                "version":version,
                "text":fs::read_to_string(path).unwrap()
            }}),
        );
        let uri = file_uri(path);
        loop {
            let message = self.wait_for_method("textDocument/publishDiagnostics");
            if message["params"]["uri"] == uri && message["params"]["version"] == version {
                break;
            }
        }
    }

    fn definition_uri(&mut self, path: &Path, character: u32) -> Option<String> {
        let result = self.request(
            "textDocument/definition",
            json!({"textDocument":{"uri":file_uri(path)},"position":{"line":1,"character":character}}),
        );
        match result {
            Value::Null => None,
            Value::Object(map) => map.get("uri").and_then(Value::as_str).map(str::to_string),
            Value::Array(items) => items
                .first()
                .and_then(|item| item.get("uri"))
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        }
    }

    fn shutdown(self) {}
}

fn file_uri(path: &Path) -> String {
    Url::from_file_path(path).unwrap().to_string()
}

fn write_workspace(root: &Path, key: &str) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("skilldesc.txt"), format!("skilldesc\n{key}\n")).unwrap();
    fs::write(
        root.join("skills.txt"),
        format!("skill\tskilldesc\tsrvstfunc\nskill-{key}\t{key}\t0\n"),
    )
    .unwrap();
}

fn skilldesc_character(key: &str) -> u32 {
    format!("skill-{key}\t").encode_utf16().count() as u32 + 1
}

#[test]
fn tcp_clients_and_reconnects_have_independent_workspace_state() {
    let tree = TempTree::new();
    let root_a = tree.0.join("root-a");
    let root_b = tree.0.join("root-b");
    write_workspace(&root_a, "A-DESC");
    write_workspace(&root_b, "B-DESC");

    let port = TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = tree.0.join("config.json");
    fs::write(
        &config,
        serde_json::to_vec_pretty(&json!({
            "io_type":{"type":"tcp","host":"127.0.0.1","port":port},
            "schema_variant":"3.2",
            "encoding":"auto"
        }))
        .unwrap(),
    )
    .unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_vector-lsp"))
        .args(["--config-file", config.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _server = ServerChild(child);

    let mut client_a = LspClient::connect(port);
    client_a.initialize(&root_a, 1);
    let mut client_b = LspClient::connect(port);
    client_b.initialize(&root_b, 2);
    let skills_a = root_a.join("skills.txt");
    let skills_b = root_b.join("skills.txt");
    client_a.open(&skills_a, 1);
    client_b.open(&skills_b, 1);

    assert_eq!(
        client_a.definition_uri(&skills_a, skilldesc_character("A-DESC")),
        Some(file_uri(&root_a.join("skilldesc.txt")))
    );
    assert_eq!(
        client_b.definition_uri(&skills_b, skilldesc_character("B-DESC")),
        Some(file_uri(&root_b.join("skilldesc.txt")))
    );

    let skilldesc_a = root_a.join("skilldesc.txt");
    fs::write(&skilldesc_a, "skilldesc\nCHANGED\n").unwrap();
    client_a.open(&skilldesc_a, 1);
    assert_eq!(
        client_a.definition_uri(&skills_a, skilldesc_character("A-DESC")),
        None
    );
    assert_eq!(
        client_b.definition_uri(&skills_b, skilldesc_character("B-DESC")),
        Some(file_uri(&root_b.join("skilldesc.txt")))
    );

    client_a.shutdown();
    fs::remove_file(&skilldesc_a).unwrap();
    let mut reconnected_a = LspClient::connect(port);
    reconnected_a.initialize(&root_a, 3);
    reconnected_a.open(&skills_a, 1);
    assert_eq!(
        reconnected_a.definition_uri(&skills_a, skilldesc_character("A-DESC")),
        None
    );
    reconnected_a.shutdown();
    client_b.shutdown();
}

#[test]
fn same_stem_open_clears_disk_winner_and_close_revalidates_it() {
    let tree = TempTree::new();
    let root = tree.0.join("shadow-root");
    let disk_items = tree.write("shadow-root/A/items.txt", "id\nINVALID\n");
    let open_items = root.join("B").join("items.txt");
    let plugins = tree.0.join("shadow-plugins");
    tree.write(
        "shadow-plugins/shadow.js",
        "const pluginMetadata={validateFiles:['items']};\n\
         function validate(ctx){\n\
           return ctx.rows.some(row=>row.id==='INVALID')\n\
             ? [{line:1,col:0,message:'DISK_INVALID'}]\n\
             : [];\n\
         }\n",
    );

    let port = TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = tree.0.join("shadow-config.json");
    fs::write(
        &config,
        serde_json::to_vec_pretty(&json!({
            "io_type":{"type":"tcp","host":"127.0.0.1","port":port},
            "plugin_path": plugins.to_string_lossy(),
            "schema_loader":"no-test-loader",
            "encoding":"auto"
        }))
        .unwrap(),
    )
    .unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_vector-lsp"))
        .args(["--config-file", config.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _server = ServerChild(child);

    let mut client = LspClient::connect(port);
    client.request(
        "initialize",
        json!({
            "processId":std::process::id(),
            "rootUri":file_uri(&root),
            "initializationOptions":{"sessionGeneration":91},
            "capabilities":{"textDocument":{"publishDiagnostics":{}}}
        }),
    );
    client.notify("initialized", json!({}));

    let disk_uri = file_uri(&disk_items);
    let open_uri = file_uri(&open_items);
    let startup_deadline = Instant::now() + Duration::from_secs(5);
    let mut startup_disk_diagnostics = None;
    let mut ready = false;
    while !ready {
        let message = client
            .read_message_before(startup_deadline)
            .expect("startup must publish disk diagnostics and become ready");
        if message["method"] == "textDocument/publishDiagnostics"
            && message["params"]["uri"] == disk_uri
        {
            startup_disk_diagnostics = Some(message["params"]["diagnostics"].clone());
        }
        ready = message["method"] == "vectorLsp/ready";
    }
    let startup_disk_diagnostics =
        startup_disk_diagnostics.expect("startup must publish the invalid disk winner");
    assert_eq!(startup_disk_diagnostics.as_array().unwrap().len(), 1);
    assert_eq!(startup_disk_diagnostics[0]["message"], "DISK_INVALID");

    client.notify(
        "textDocument/didOpen",
        json!({"textDocument":{
            "uri":open_uri,
            "languageId":"plaintext",
            "version":1,
            "text":"id\nVALID\n"
        }}),
    );
    let open_deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_disk_clear = false;
    let mut saw_valid_open_publish = false;
    while !(saw_disk_clear && saw_valid_open_publish) {
        let message = client
            .read_message_before(open_deadline)
            .expect("same-stem didOpen must clear the old disk URI and publish the live document");
        if message["method"] != "textDocument/publishDiagnostics" {
            continue;
        }
        let params = &message["params"];
        if params["uri"] == disk_uri && params["diagnostics"].as_array().unwrap().is_empty() {
            saw_disk_clear = true;
        }
        if params["uri"] == open_uri
            && params["version"] == 1
            && params["diagnostics"].as_array().unwrap().is_empty()
        {
            saw_valid_open_publish = true;
        }
    }

    client.notify(
        "textDocument/didClose",
        json!({"textDocument":{"uri":open_uri}}),
    );
    let close_deadline = Instant::now() + Duration::from_secs(5);
    let restored = loop {
        let message = client
            .read_message_before(close_deadline)
            .expect("didClose must revalidate the restored disk winner");
        if message["method"] == "textDocument/publishDiagnostics"
            && message["params"]["uri"] == disk_uri
            && !message["params"]["diagnostics"]
                .as_array()
                .unwrap()
                .is_empty()
        {
            break message["params"]["diagnostics"].clone();
        }
    };
    assert_eq!(restored.as_array().unwrap().len(), 1);
    assert_eq!(restored[0]["message"], "DISK_INVALID");
}
