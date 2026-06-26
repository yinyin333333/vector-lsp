/**
 * Type declarations for vector-lsp plugins.
 *
 * Reference this file in your plugin for IDE type checking:
 *
 *   // @ts-check
 *   /// <reference path="./vector-lsp-plugin.d.ts" />
 *
 * Plugin files define one or both of the global functions below.
 * The server auto-registers them at startup; no import/export needed.
 *
 * NOTE: Inline type annotations on function parameters (e.g. `x: string`)
 * require pre-compiling your .ts file to .js.  Structural declarations
 * (interface / type aliases) are stripped at load time and are safe to use.
 */

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

/** A data row with positional metadata included. */
interface WorkspaceRow {
    [column: string]: string | number | Record<string, number>;
    /** 0-based source line number for this row. */
    __line: number;
    /** Character offset (UTF-16) of each column's value start within its line. */
    __colstarts: Record<string, number>;
}

// ---------------------------------------------------------------------------
// validate(ctx) — custom diagnostic checks
// ---------------------------------------------------------------------------

interface PluginContext {
    /** Stem of the file being validated (e.g. `"cubemain"` for `cubemain.txt`). */
    file: string;
    /** Ordered column names for the current file. */
    headers: string[];
    /** Data rows for the current file, each with `__line` and `__colstarts`. */
    rows: WorkspaceRow[];
}

interface PluginDiagnostic {
    /** 0-based line number. */
    line: number;
    /** 0-based start character (UTF-16 offset). Use `row.__colstarts[col]`. */
    col: number;
    /** 0-based end character (exclusive). Defaults to `col` when omitted. */
    endCol?: number;
    /** Defaults to `"warning"`. */
    severity?: "error" | "warning" | "info" | "hint";
    message: string;
}

// ---------------------------------------------------------------------------
// Host-provided utilities (always available in plugin scope)
// ---------------------------------------------------------------------------

/**
 * Returns true if `value` exists as a cell in column `col` of the file whose
 * stem is `file` (case-insensitive).  Uses the Rust symbol index for O(1)
 * lookup — prefer this over scanning `ctx.workspace[file].rows` manually.
 *
 * @example
 * if (!lookupKey("properties", "code", row["prop1"])) { ... }
 */
declare function lookupKey(file: string, col: string, value: string): boolean;

/** Metadata returned by `getColumn` when a column exists. */
interface ColumnInfo {
    /** 0-based position of this column in the file's header row. */
    index: number;
}

/**
 * Return `true` if the workspace contains a file with the given stem
 * (case-insensitive).  O(1) — no serialization.
 *
 * @example
 * if (hasFile("weapons")) { ... }
 */
declare function hasFile(stem: string): boolean;

/**
 * Return true only when the workspace has file `file` and column `col`.
 * Use this before value-level diagnostics that depend on lookup evidence.
 */
declare function hasLookupTarget(file: string, col: string): boolean;

/**
 * Return all non-empty values from column `col` in file `stem`
 * (both case-insensitive).  Only that column is serialized.
 * Returns an empty array when the file or column is absent.
 *
 * @example
 * const codes = new Set(getColumnValues("skillcalc", "code"));
 */
declare function getColumnValues(stem: string, col: string): string[];

/**
 * Return non-empty values from `valueCol` in file `stem` where
 * `filterCol === filterValue` (exact string match).
 * Only the two columns are read; no full-file serialization.
 * Returns an empty array when the file or either column is absent.
 *
 * @example
 * const autoTcCodes = getFilteredColumnValues("itemtypes", "Code", "TreasureClass", "1");
 */
declare function getFilteredColumnValues(stem: string, valueCol: string, filterCol: string, filterValue: string): string[];

/**
 * Returns metadata for column `col` in file `file`, or `null` if the column
 * does not exist in the workspace data for that file.
 *
 * Useful for writing version-agnostic plugins: iterate `col#` patterns (e.g.
 * `"item1"`, `"item2"`, …) and stop as soon as `getColumn` returns `null`.
 *
 * @example
 * for (let i = 1; ; i++) {
 *   const col = `item${i}`;
 *   if (!getColumn(ctx.file, col)) break;
 *   const val = row[col];
 *   // validate val...
 * }
 */
declare function getColumn(file: string, col: string): ColumnInfo | null;

/**
 * Define this function to add custom diagnostic checks.
 * Return an empty array for files this plugin does not handle.
 *
 * @example
 * function validate(ctx) {
 *   if (ctx.file !== "cubemain") return [];
 *   var diags = [];
 *   var nIdx = ctx.headers.indexOf("numinputs");
 *   ctx.rows.forEach(function(row) {
 *     var n = parseInt(row["numinputs"] || "0", 10);
 *     for (var i = 1; i <= 7; i++) {
 *       var filled = (row["input " + i] || "") !== "";
 *       if (i <= n && !filled) {
 *         diags.push({
 *           line: row.__line,
 *           col: row.__colstarts["input " + i] || 0,
 *           severity: "warning",
 *           message: "input " + i + " is required when numinputs=" + n,
 *         });
 *       }
 *     }
 *   });
 *   return diags;
 * }
 */
declare function validate(ctx: PluginContext): PluginDiagnostic[];

// ---------------------------------------------------------------------------
// hover(ctx) — custom hover content
// ---------------------------------------------------------------------------

interface HoverContext {
    /** Stem of the file containing the hovered cell. */
    file: string;
    /** Name of the column being hovered. */
    col: string;
    /** Raw string value of the hovered cell. */
    value: string;
    /** 0-based line number of the row containing the hovered cell. */
    rowLine: number;
    /** All cell values in the hovered row, keyed by column name. */
    row: Record<string, string>;
}

interface HoverResult {
    /** Markdown content appended after the schema description (if any). */
    content: string;
}

/**
 * Return the enum table for column `col` in file `file`, or `null` if the
 * column has no table defined in the schema.
 *
 * `headers` is the header row; `rows` are the data rows.  Column 0 is always
 * the code/ID that matches the cell value.  All cells are pre-formatted:
 * `$!file#field!$` cross-references are resolved and basic HTML is converted
 * to Markdown.
 *
 * @example
 * const t = getEnumTable("misc", "pSpell");
 * if (t) {
 *   const row = t.rows.find(r => r[0] === ctx.value);
 * }
 */
declare function getEnumTable(file: string, col: string): { headers: string[]; rows: string[][] } | null;

/**
 * Define this function to supply custom hover content.
 * Return `null` for columns this plugin does not handle.
 * The returned content is appended after the schema description with a divider.
 *
 * @example
 * function hover(ctx) {
 *   if (ctx.file !== "cubemain" || ctx.col !== "op") return null;
 *   var ops = { "1": "Add value", "2": "Multiply", "3": "Set value" };
 *   var label = ops[ctx.value];
 *   if (!label) return null;
 *   return { content: "Op **" + ctx.value + "**: " + label };
 * }
 */
declare function hover(ctx: HoverContext): HoverResult | null;

// ---------------------------------------------------------------------------
// gotoDefinition(ctx) — custom Go-to-Definition targets
// ---------------------------------------------------------------------------

interface GotoDefinitionContext {
    /** Stem of the file containing the active cell. */
    file: string;
    /** Name of the column being activated. */
    col: string;
    /** Raw string value of the active cell. */
    value: string;
    /** 0-based line number of the row containing the active cell. */
    rowLine: number;
    /** All cell values in the active row, keyed by column name. */
    row: Record<string, string>;
}

interface GotoDefinitionTarget {
    /**
     * Stem of the file to navigate to (e.g. `"weapons"`).
     * The server performs `SymbolIndex::lookup(targetFile, targetCol, targetValue)`.
     */
    targetFile: string;
    /** Column in `targetFile` to match against (e.g. `"code"`). */
    targetCol: string;
    /** Value to find in `targetCol` (e.g. `"hax"`). */
    targetValue: string;
}

/**
 * Return a go-to-definition target for the given cell, or `null` if this
 * plugin does not handle the column.
 *
 * The server resolves the returned `{ targetFile, targetCol, targetValue }`
 * via the symbol index and navigates to the matching row.
 *
 * @example
 * function gotoDefinition(ctx) {
 *   if (ctx.file !== "cubemain") return null;
 *   if (!/^input \d+$/.test(ctx.col)) return null;
 *   var base = ctx.value.replace(/^"|"$/g, "").split(",")[0].trim();
 *   if (!base || base === "any") return null;
 *   if (lookupKey("weapons", "code", base))
 *     return { targetFile: "weapons", targetCol: "code", targetValue: base };
 *   return null;
 * }
 */
declare function gotoDefinition(ctx: GotoDefinitionContext): GotoDefinitionTarget | null;
