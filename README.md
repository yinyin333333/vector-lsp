# vector-lsp

A Language Server Protocol (LSP) server for structured, delimited text files — tab-delimited `.txt`, CSV, and similar formats. Turns flat data files into an IDE-supported editing experience with real-time diagnostics, hover documentation, go-to-definition, and an extensible plugin system.

The primary motivation is tooling for game data files such as Diablo II and Diablo II: Resurrected `.txt` data tables, but the server is intentionally generic and works with any tab- or delimiter-separated workspace.

---

## Features

| Capability | Status |
|---|---|
| **Diagnostics** — schema type violations, broken cross-file references, unknown columns | Implemented |
| **Hover** — schema field description on hover over any cell | Implemented |
| **Go-to-definition** — jump from a reference value to its row in the target file | Implemented |
| **Plugins** — custom diagnostic checks, hover content, and go-to-definition targets written in TypeScript/JavaScript | Implemented |
| **Single-shot mode** — validate a workspace from the command line and exit (CI-friendly) | Implemented |
| **Completions** — autocomplete valid values for enum-typed columns | Planned |
| **Document/workspace symbols** | Planned |

### Built-in diagnostics

Three categories of diagnostics are produced without any schema:

- **Error** — a cell value in a `reference`-typed column does not match any row in the target file (broken cross-file reference)
- **Warning** — a cell value cannot be parsed as the column's declared `int` or `float` type
- **Information** — a column header is not defined in the schema and not listed in `ignoreFields`

---

## Supported schemas

The bundled `d2rdoc` schema loader ships schemas for the following game versions under `contrib/d2rdoc/`:

| `schema_variant` | Game version |
|---|---|
| `3.3` | Diablo II: Resurrected 3.3 |
| `3.2` | Diablo II: Resurrected 3.2 |
| `3.1` | Diablo II: Resurrected 3.1 |
| `2.4` | Diablo II: Resurrected 2.4 |
| `1.13` | Diablo II: Lord of Destruction 1.13 (classic) |

Each variant includes schemas for 65–80+ data files (armor, weapons, skills, monsters, cube recipes, item types, etc.) and a shared set of base plugins for cross-file validation.

To use a bundled schema, set `schema_variant` in your `config.json`:

```json
{ "schema_variant": "3.3" }
```

You can also point at a custom schema directory with `schema_path` — see [Configuration](#configuration).

---

## Building

**Requirements:** Rust 1.85 or newer (edition 2024), Cargo.

```bash
git clone https://github.com/eezstreet/vector-lsp
cd vector-lsp
cargo build --release
```

The binary is placed at `target/release/vector-lsp` (`vector-lsp.exe` on Windows). The `contrib/` directory next to the binary contains the bundled schema files and plugins and must be distributed alongside the binary.

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
| `encoding` | `"utf8"` \| `"utf-16-le"` \| `"utf-16-be"` \| `"latin-1"` | `"utf8"` | File encoding |
| `schema_loader` | string | `"d2rdoc"` | Schema driver to use (currently only `"d2rdoc"` is built in) |
| `schema_variant` | string | `""` | Bundled schema set to use (e.g. `"3.3"`) |
| `schema_path` | path | _(none)_ | Explicit path to a schema directory; overrides `schema_variant` auto-discovery |
| `plugin_path` | path | _(none)_ | Directory of additional plugin files (`.ts`/`.js`); loaded on top of any bundled plugins |
| `workspace_path` | path | _(none)_ | Root directory of the data file workspace; required for single-shot mode |
| `single_shot` | bool | `false` | Validate the workspace and exit instead of starting the LSP server |
| `json_diagnostics` | bool | `false` | In LSP mode, enable d2rlint-compatible diagnostics for physical top-level `local/lng/strings/*.json` files beside the primary mod's `global/excel` directory; reference and bundled data are never substituted |
| `json_duplicate_ids_action` | `"ignore"` \| `"warn"` | `"warn"` | Action for `Json/DuplicateIds` |
| `json_string_format_action` | `"ignore"` \| `"warn"` | `"warn"` | Action for `Json/StringFormat` |
| `json_key_usage_action` | `"ignore"` \| `"warn"` | `"ignore"` | Action for `Json/KeyUsage` |
| `json_key_usage_id_start` | finite number | `40000` | Report unused keys only when their JavaScript-coerced ID is strictly greater than this value |

**CLI flags** (override their config equivalents):

```
vector-lsp [--config-file <path>] [--single-shot] [--schema-path <path>]
```

**Example `config.json` for D2R 3.3:**

```json
{
  "delimiter": "\t",
  "extension": "txt",
  "encoding": "utf-16-le",
  "schema_variant": "3.3",
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
/path/to/file.txt:42:7: error: 'hax' not found in weapons#code
```

A summary is written to stderr:

```
3 error(s), 1 warning(s) across 2 file(s).
```

**Exit codes:**

| Code | Meaning |
|---|---|
| `0` | No errors or warnings found |
| `1` | One or more errors found |
| `2` | Configuration or I/O error (workspace unreadable, schema load failed, etc.) |

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
  ignoreFields: ["2handed", "wclass"],   // columns present in the file but intentionally undocumented
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
| `ignoreFields` | string[] | Columns that exist in the data but are intentionally not validated |
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
| `boolean` | No type validation |
| `reference` | Errors if `field.value` is not found in `file`'s `field` column across the workspace |
| `parse` | Calc-expression field — no type validation yet |
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
