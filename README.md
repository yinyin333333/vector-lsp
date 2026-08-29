# vector-lsp

A Language Server Protocol (LSP) server for structured, delimited text files — tab-delimited `.txt`, CSV, and similar formats. Turns flat data files into an IDE-supported editing experience with real-time diagnostics, hover documentation, go-to-definition, and an extensible plugin system.

The primary motivation is tooling for game data files such as Diablo II and Diablo II: Resurrected `.txt` data tables, but the server is intentionally generic and works with any tab- or delimiter-separated workspace.

---

## Features

| Capability | Status |
|---|---|
| **Diagnostics** — schema-declared value/reference/uniqueness rules, plugins, and opt-in localization JSON checks | Implemented |
| **Hover** — schema field description on hover over any cell | Implemented |
| **Go-to-definition** — jump from a reference value to its row in the target file | Implemented |
| **Plugins** — custom diagnostic checks, hover content, and go-to-definition targets written in TypeScript/JavaScript | Implemented |
| **Single-shot mode** — validate a workspace from the command line and exit (CI-friendly) | Implemented |
| **Completions** — autocomplete valid values for enum-typed columns | Planned |
| **Document/workspace symbols** | Planned |

### Built-in diagnostics

The core schema validator reports duplicate values in schema fields marked
`unique`, invalid `int`/`float`/`boolean` values, unresolved cross-file
references, and a small set of version-aware game semantics. An unresolved
reference uses its schema policy (`warning` by default, or `error`/`ignore`).
Plugins and the opt-in localization JSON rules can add their own diagnostics.

Unknown column headers do **not** currently produce a standalone diagnostic.
`ignoreFields` is parsed for d2rdoc schema compatibility, but it does not alter
that behavior. Without a loaded schema, the core schema validator has no field
rules to apply; plugin and enabled JSON diagnostics can still run.

---

## Supported schemas

The `d2rdoc` loader supports the following generated schema layouts under
`contrib/d2rdoc/`:

| `schema_variant` | Game version |
|---|---|
| `3.3` | Diablo II: Resurrected 3.3 |
| `3.2` | Diablo II: Resurrected 3.2 |
| `3.1` | Diablo II: Resurrected 3.1 |
| `2.4` | Diablo II: Resurrected 2.4 |
| `1.13` | Diablo II: Lord of Destruction 1.13 (classic) |

Each generated variant includes schemas for dozens of data files (armor,
weapons, skills, monsters, cube recipes, item types, etc.) and a shared set of
base plugins for cross-file validation.

To use a bundled schema, set `schema_variant` in your `config.json`:

```json
{ "schema_variant": "3.3" }
```

You can also point at a custom schema directory with `schema_path` — see [Configuration](#configuration).

---

## Building

**Requirements:** Rust 1.85 or newer (edition 2024) and Cargo. Generating the
d2rdoc schema assets also requires Git 2.25+ and PowerShell.

```powershell
git clone https://github.com/eezstreet/vector-lsp
cd vector-lsp
powershell -ExecutionPolicy Bypass -File .\contrib\d2rdoc\sync-schemas.ps1 -Branch master
cargo build --release --locked
```

The schema directories are generated assets and are intentionally ignored by
Git. `sync-schemas.ps1` clones `https://github.com/eezstreet/d2rdoc.git`, maps
upstream `data/files` to variant `3.3`, and maps `data/old/<version>` to the
older variants. The script defaults to the moving `master` branch; use a
reviewed immutable upstream tag instead when producing a release.

With the default `d2rdoc` feature enabled, `build.rs` checks that all five
advertised variants contain JavaScript assets. If they do not, the current
compatibility fallback invokes `sync-schemas.ps1` automatically, which can
therefore access the network and follows `master`. Pre-populate the assets as
an explicit release step when network access or moving inputs are unacceptable.
`cargo build --no-default-features` skips d2rdoc support and schema syncing.
If `powershell` is unavailable, synchronization fails, or a required variant
remains incomplete, the fallback currently emits a warning and continues;
release automation must verify the resulting asset set. The checked-in
cargo-dist workflow does not yet provide a pinned pre-sync step.

There is currently no tracked commit pin, schema checksum/provenance manifest,
or upstream schema license/notice bundled with these ignored JavaScript assets.
The tracked `reference-manifest.json` authenticates the separately bundled
reference TXT datasets only; it does not attest the schema JavaScript. Release
packagers must record the reviewed d2rdoc revision and confirm its redistribution
requirements until a schema packaging policy is adopted.

The binary is placed at `target/release/vector-lsp` (`vector-lsp.exe` on
Windows). The generated `contrib/` directory next to the binary contains the
runtime schemas, reference tables, and plugins and must be distributed alongside
the binary.

To run tests:

```bash
cargo test
```

---

## Configuration

Configuration is loaded from a JSON file (default: `config.json` in the working directory) and can be overridden with environment variables prefixed `VLSP_`.

| Key | Type | Default | Description |
|---|---|---|---|
| `io_type` | `"stdio"` \| `{"type":"tcp","host":"…","port":…}` | `"stdio"` | Transport — `stdio` for editor integration, `tcp` for debugging |
| `delimiter` | string | `"\t"` | Column delimiter character |
| `extension` | string | `"txt"` | File extension to treat as workspace data files (without leading dot) |
| `encoding` | `"auto"` \| `"utf8"` \| `"utf-16-le"` \| `"utf-16-be"` \| `"latin-1"` | `"utf8"` | File encoding. `auto` detects UTF BOMs, then valid UTF-8, then falls back to Windows-1252 |
| `schema_loader` | string | `"d2rdoc"` | Schema driver to use (currently only `"d2rdoc"` is built in) |
| `schema_variant` | string | `""` | Bundled schema set to use (e.g. `"3.3"`) |
| `schema_path` | path | _(none)_ | Explicit path to a schema directory; overrides `schema_variant` auto-discovery |
| `plugin_path` | path | _(none)_ | Directory of additional plugin files (`.ts`/`.js`); loaded on top of any bundled plugins |
| `reference_variant` | string | `""` | Bundled reference TXT fallback (`1.13`/`1.13c`, `2.4`, `3.1`, `3.2`, or `3.3`); empty infers a supported `schema_variant`, otherwise disables fallback |
| `workspace_path` | path | _(none)_ | Root directory of the data file workspace; required for single-shot mode |
| `single_shot` | bool | `false` | Validate the workspace and exit instead of starting the LSP server |
| `json_diagnostics` | bool | `false` | In LSP mode, enable d2rlint-compatible diagnostics for physical top-level `local/lng/strings/*.json` files beside the primary mod's `global/excel` directory; reference and bundled data are never substituted |
| `locale` | string | `"enUS"` | Product-message locale for CLI/single-shot runs; an LSP session instead uses its initialize locale |
| `json_duplicate_ids_action` | `"ignore"` \| `"warn"` | `"warn"` | Action for `Json/DuplicateIds` |
| `json_string_format_action` | `"ignore"` \| `"warn"` | `"warn"` | Action for `Json/StringFormat` |
| `json_key_usage_action` | `"ignore"` \| `"warn"` | `"ignore"` | Action for `Json/KeyUsage` |
| `json_key_usage_id_start` | finite number | `40000` | Report unused keys only when their JavaScript-coerced ID is strictly greater than this value |

**CLI flags** (override their config equivalents):

```
vector-lsp [--config-file <path>] [--single-shot] [--schema-path <path>] [--editor-mode] [--locale <locale>]
```

`--editor-mode` is the deterministic TXTeditor launch mode: it skips the JSON
config file, forces stdio LSP, disables single-shot mode, and clears
`workspace_path`. `VLSP_` environment settings are still loaded. `--schema-path`
and `--locale` override their environment/config values. Supported locale IDs
are `enUS`, `zhTW`, `deDE`, `esES`, `frFR`, `itIT`, `koKR`, `plPL`, `esMX`,
`jaJP`, `ptBR`, `ruRU`, and `zhCN`; matching is case-insensitive and also accepts
dash/underscore separators. Invalid or absent locale values fall back to
`enUS`.

Each LSP client negotiates its own locale through the top-level `locale` string
in `initializationOptions`. That value, rather than `locale` from config or
`VLSP_LOCALE`, controls diagnostics and messages after initialization.

**Example `config.json` for D2R 3.3:**

```json
{
  "delimiter": "\t",
  "extension": "txt",
  "encoding": "auto",
  "schema_variant": "3.3",
  "reference_variant": "3.3",
  "workspace_path": "/path/to/d2r/data/global/excel"
}
```

**Example with a custom schema and extra plugins:**

```json
{
  "delimiter": "\t",
  "extension": "txt",
  "schema_path": "/path/to/my-schema",
  "plugin_path": "/path/to/my-plugins"
}
```

**Environment variable override:**

```bash
VLSP_SCHEMA_PATH=/alt/schema vector-lsp
VLSP_ENCODING=auto VLSP_REFERENCE_VARIANT=3.3 vector-lsp --editor-mode
VLSP_JSON_DIAGNOSTICS=true vector-lsp --editor-mode
VLSP_JSON_KEY_USAGE_ACTION=warn VLSP_JSON_KEY_USAGE_ID_START=50000 vector-lsp --editor-mode
```

When localization JSON diagnostics are enabled, vector-lsp dynamically registers
standard `workspace/didChangeWatchedFiles` patterns with capable LSP clients for
the physical top-level string JSON and layout JSON inputs in the active mod
scope. External saves, creation and deletion trigger a debounced refresh without
continuous polling; reference and bundled JSON inputs remain excluded. The same
event path refreshes unopened workspace and sibling TXT snapshots.

---

## Single-shot mode (CI / command-line linting)

Single-shot mode validates an entire workspace without starting an LSP server, then exits. This is useful for CI pipelines and pre-commit hooks.

**Requirements:** `workspace_path` must be set in `config.json` (or via environment variable).

**Run:**

```bash
vector-lsp --single-shot --config-file config.json
```

Or set `"single_shot": true` in `config.json` and run normally.

**Output format** — each diagnostic is printed to stdout as:

```
/path/to/file.txt:42:7: warning: Reference value 'hax' not found in weapons.code.
```

A summary including every severity and the parsed-file count is written to
stderr (localized when a non-English CLI locale is selected):

```
3 error(s), 1 warning(s), 0 info, 0 hint diagnostic(s) across 2 file(s); 12 parsed file(s).
```

**Exit codes:**

| Code | Meaning |
|---|---|
| `0` | No error-severity diagnostics (warnings/info/hints do not fail the run) |
| `1` | One or more error-severity diagnostics, or an error returned before the single-shot runner starts (for example invalid configuration) |
| `2` | A runtime setup or I/O failure handled by the single-shot runner (workspace unreadable, schema/plugin load failed, etc.) |

---

## Editor integration

`vector-lsp` communicates over the standard [Language Server Protocol](https://microsoft.github.io/language-server-protocol/) (JSON-RPC over stdio or TCP). Any LSP-capable editor can connect to it.

### VS Code

VS Code does not natively launch arbitrary language servers without an extension. The simplest approach is to write a small extension using [`vscode-languageclient`](https://www.npmjs.com/package/vscode-languageclient):

```ts
import * as path from 'path';
import { ExtensionContext } from 'vscode';
import { LanguageClient, ServerOptions, TransportKind } from 'vscode-languageclient/node';

export function activate(context: ExtensionContext) {
    const serverOptions: ServerOptions = {
        command: '/path/to/vector-lsp',
        args: ['--config-file', path.join(vscode.workspace.rootPath!, 'config.json')],
        transport: TransportKind.stdio,
    };
    const client = new LanguageClient('vector-lsp', 'vector-lsp', serverOptions, {
        documentSelector: [{ scheme: 'file', language: 'plaintext' }],
    });
    context.subscriptions.push(client.start());
}
```

Adjust `documentSelector` to match the file extension configured in `config.json`.

### Neovim (via `nvim-lspconfig`)

```lua
local lspconfig = require('lspconfig')
local configs = require('lspconfig.configs')

if not configs.vector_lsp then
  configs.vector_lsp = {
    default_config = {
      cmd = { '/path/to/vector-lsp', '--config-file', vim.fn.getcwd() .. '/config.json' },
      filetypes = { 'text' },
      root_dir = lspconfig.util.root_pattern('config.json', '.git'),
      single_file_support = true,
    },
  }
end

lspconfig.vector_lsp.setup {}
```

### Helix

In `~/.config/helix/languages.toml`:

```toml
[[language]]
name = "text"
language-servers = ["vector-lsp"]

[language-server.vector-lsp]
command = "/path/to/vector-lsp"
args = ["--config-file", "./config.json"]
```

### TCP transport (debugging / remote)

Set `io_type` to TCP in `config.json`:

```json
{
  "io_type": { "type": "tcp", "host": "127.0.0.1", "port": 7777 }
}
```

The server will listen on that address and accept multiple simultaneous connections. Connect any LSP client that supports TCP, or use a raw TCP connection to inspect the JSON-RPC traffic.

### LSP capabilities

`vector-lsp` advertises and implements the following LSP features:

| LSP method | Feature |
|---|---|
| `textDocument/publishDiagnostics` | Type errors, broken references, unknown columns |
| `textDocument/hover` | Column description and cell value info |
| `textDocument/definition` | Jump to the referenced row in the target file |

The server sends diagnostics for all workspace files at startup and refreshes them whenever a document is opened or changed.

---

## Writing schemas

The `d2rdoc` loader reads a directory of JavaScript files. Each file assigns entries into a global `files` object. You can point the server at your own schema directory with the `schema_path` config key.

**Basic structure:**

```js
// schema/armor.js
files["armor"] = {
  title: "armor.txt",
  overview: "Defines all armour base types.",
  appendFiles: ["shareditems"],          // merge field list from another schema entry
  ignoreFields: ["2handed", "wclass"],   // compatibility metadata; see below
  fields: [
    {
      name: "name",
      description: "Internal item code referenced by other files.",
      type: { type: "string" }
    },
    {
      name: "minac",
      description: "Minimum defence value.",
      type: { type: "int" }
    },
    {
      name: "code",
      description: "Short item code. References $!itemtypes#Code!$ for category lookups.",
      type: {
        type: "reference",
        file: "itemtypes",   // target file stem
        field: "Code"        // target column name
      }
    }
  ]
};
```

### Schema file properties

| Property | Type | Description |
|---|---|---|
| `title` | string | Human-readable file name shown in hover |
| `overview` | string | Human-readable summary of what this file does |
| `fields` | array | Ordered column definitions (see below) |
| `appendFiles` | string[] | Schema entries whose fields are prepended to this file's list |
| `ignoreFields` | string[] | Retained d2rdoc compatibility metadata; currently does not change diagnostics because unknown headers are not reported |
| `guideOnly` | bool | If true, this entry is a reference table with no corresponding data file |
| `referenceFiles` | string[] | Additional schema entries whose fields are merged for reference resolution |

### Field properties

| Property | Type | Description |
|---|---|---|
| `name` | string | Column header name (must match exactly, case-insensitively) |
| `description` | string | Markdown text shown in hover |
| `type` | object | Type descriptor (see below) |
| `altNames` | string[] | Alternative column names this field may appear under |
| `appendField` | object | `{ file, field }` — draw valid values from an enum in another file |
| `table` | array | Inline enum table for `guideOnly` entries; first column is the code |

### Field types

| `type` | Behaviour |
|---|---|
| `int` | Warns if the cell value cannot be parsed as an integer |
| `float` | Warns if the cell value cannot be parsed as a floating-point number |
| `string` / `text` | No type validation |
| `boolean` | Warns unless a non-empty value is exactly `0` or `1` (a few known consumer fields use specialized rules) |
| `reference` | Resolves against `file`/`field`; an unknown value follows `unknownPolicy` (`warning` by default) when the target table is available |
| `parse` | Calc-expression field — no generic core type check; bundled plugins may add checks |
| `comment` | Documentation-only; not a real column, not validated |

For `reference` fields, set `file` (target file stem) and `field` (target column name):

```js
type: { type: "reference", file: "weapons", field: "code" }
```

### Cross-reference syntax in descriptions

Field descriptions may use `$!file#field!$` to link to related columns. The server renders these as readable hover text:

```js
description: "References $!itemtypes#Code!$ for item category lookups."
// Rendered as: References `Code` (in *itemtypes*) for item category lookups.
```

### Patching a bundled schema

Place a `_patches.js` file in your `plugin_path` directory (or in the `contrib/d2rdoc/` directory next to the binary). It is executed last and can override or extend any entry:

```js
// _patches.js
files["armor"].fields.push({
  name: "myCustomCol",
  description: "Added by my mod.",
  type: { type: "int" }
});
```

---

## Plugin development

Plugins are `.ts` or `.js` files in the directory specified by `plugin_path`. TypeScript type annotations are stripped at load time — no compile step needed for most plugin code.

**Setup:**

1. Copy `contrib/vector-lsp-plugin.d.ts` into your plugin directory for IDE type checking.
2. Create plugin files in that directory.
3. Set `plugin_path` in `config.json` to the directory containing your plugins.

### Plugin functions

Each plugin file may define any combination of these three global functions:

```ts
// Called for every file on open/change/startup. Return [] for files you don't handle.
function validate(ctx: PluginContext): PluginDiagnostic[] { ... }

// Called on every hover request. Return null for columns you don't handle.
function hover(ctx: HoverContext): HoverResult | null { ... }

// Called on every go-to-definition request. Return null for columns you don't handle.
function gotoDefinition(ctx: GotoDefinitionContext): GotoDefinitionTarget | null { ... }
```

Multiple plugin files are loaded in alphabetical order. All `validate` functions run and their results are merged. For `hover` and `gotoDefinition`, the first non-null result wins.

### Context shapes

**`validate(ctx)`**

```ts
interface PluginContext {
  file: string;          // file stem, e.g. "cubemain"
  headers: string[];     // ordered column names
  rows: WorkspaceRow[];  // data rows (see below)
}

interface WorkspaceRow {
  [column: string]: string;
  __line: number;                      // 0-based source line number
  __colstarts: Record<string, number>; // UTF-16 char offset of each cell's value
}
```

**`hover(ctx)`**

```ts
interface HoverContext {
  file: string;     // file stem
  col: string;      // column name being hovered
  value: string;    // raw cell value
  rowLine: number;  // 0-based source line number
  row: Record<string, string>; // all cell values in this row
}
```

**`gotoDefinition(ctx)`**

```ts
interface GotoDefinitionContext {
  file: string;     // file stem
  col: string;      // column name activated
  value: string;    // raw cell value
  rowLine: number;  // 0-based source line number
  row: Record<string, string>; // all cell values in this row
}
```

### Diagnostic severity levels

| `severity` | LSP level |
|---|---|
| `"error"` | Error |
| `"warning"` _(default)_ | Warning |
| `"info"` / `"information"` | Information |
| `"hint"` | Hint |

### Host-provided utility functions

These functions are always available inside plugin code:

| Function | Description |
|---|---|
| `lookupKey(file, col, value)` | Returns `true` if `value` exists in `col` of `file` (O(1) symbol index lookup) |
| `hasFile(stem)` | Returns `true` if `stem` is present in the workspace |
| `getColumn(file, col)` | Returns `{ index }` for the column, or `null` if not present |
| `getColumnValues(stem, col)` | Returns all non-empty values from a single column |
| `getFilteredColumnValues(stem, valueCol, filterCol, filterValue)` | Column values filtered by another column's value |
| `getEnumTable(file, col)` | Returns `{ headers, rows }` for a schema enum table, or `null` |

### TypeScript support

The following TypeScript constructs are stripped automatically at load time:

- `interface Foo { ... }` and `type Foo = ...` declarations
- `: TypeAnnotation` on function parameters and return types
- `as TypeName` type-assertion expressions
- `x?:` optional parameter markers

**Known limitations** — these constructs are not stripped and will cause a runtime error; avoid them or pre-compile to plain JS:

- Generic type parameters on function declarations: `function f<T>()`
- Variable type annotations inside brace bodies: `const n: number = ...`

### Example — validate `numinputs` in cubemain.txt

```ts
/// <reference path="./vector-lsp-plugin.d.ts" />

function validate(ctx) {
    if (ctx.file !== "cubemain") return [];

    var diags = [];

    ctx.rows.forEach(function(row) {
        var n = parseInt(row["numinputs"] || "0", 10);
        for (var i = 1; i <= 7; i++) {
            var key = "input " + i;
            var filled = (row[key] || "").trim() !== "";
            if (i <= n && !filled) {
                diags.push({
                    line:     row.__line,
                    col:      row.__colstarts[key] || 0,
                    severity: "warning",
                    message:  key + " must be filled when numinputs = " + n,
                });
            }
            if (i > n && filled) {
                diags.push({
                    line:     row.__line,
                    col:      row.__colstarts[key] || 0,
                    severity: "information",
                    message:  key + " is non-empty but numinputs = " + n,
                });
            }
        }
    });

    return diags;
}
```

### Example — custom hover for an opcode column

```ts
/// <reference path="./vector-lsp-plugin.d.ts" />

var OP_NAMES = {
    "1": "Add value",
    "2": "Multiply by value",
    "3": "Set value",
    "11": "Percentage increase",
};

function hover(ctx) {
    if (ctx.file !== "properties" || ctx.col !== "op1") return null;
    var label = OP_NAMES[ctx.value];
    if (!label) return null;
    return { content: "Op **" + ctx.value + "**: " + label };
}
```

### Example — custom go-to-definition

```ts
/// <reference path="./vector-lsp-plugin.d.ts" />

function gotoDefinition(ctx) {
    if (ctx.file !== "cubemain") return null;
    if (!/^input \d+$/.test(ctx.col)) return null;

    var base = ctx.value.replace(/^"|"$/g, "").split(",")[0].trim();
    if (!base || base === "any") return null;

    if (lookupKey("weapons", "code", base))
        return { targetFile: "weapons", targetCol: "code", targetValue: base };
    if (lookupKey("armor", "code", base))
        return { targetFile: "armor", targetCol: "code", targetValue: base };
    return null;
}
```

---

## Project structure

```
src/
  main.rs          Entry point — config loading, single-shot mode, transport selection
  backend.rs       LSP Backend struct and all request/notification handlers
  document.rs      Row/cell parser for delimited files
  diagnostics.rs   Built-in diagnostic validation logic
  plugin.rs        Plugin host, TypeScript preprocessor, JS context builders
  runtime/         Thin wrapper around the deno_core V8 runtime
  schema/          Schema types (SchemaLoader trait, field types, reference resolution)
  workspace.rs     Workspace state — open documents, file cache, symbol index
  settings.rs      Configuration types
  cli/             CLI argument parsing
  contrib/
    d2rdoc/        D2rDoc schema loader (JS-based, self-registers via `inventory`)

contrib/           Runtime assets shipped alongside the binary
  d2rdoc/
    plugins/       Base plugins loaded for all variants
    3.3/schema/    D2R 3.3 schema files
    3.2/schema/    D2R 3.2 schema files
    3.1/schema/    D2R 3.1 schema files
    2.4/schema/    D2R 2.4 schema files
    1.13/schema/   Diablo II 1.13 schema files
  vector-lsp-plugin.d.ts   TypeScript type declarations for plugin authors
```

---

## License

MIT
