/// <reference path="../../vector-lsp-plugin.d.ts" />

const pluginMetadata: PluginMetadata = {
    validateFiles: ["uniqueitems", "books", "charstats", "gamble", "gems", "monequip", "setitems", "runes"],
    hoverFiles: ["uniqueitems", "books", "charstats", "gamble", "gems", "monequip", "setitems", "runes"],
    gotoDefinitionFiles: ["uniqueitems", "books", "charstats", "gamble", "gems", "monequip", "setitems", "runes"],
};

// Validates fields that must contain a valid item code (weapons/armor/misc)
// but are typed as plain text in the schema because they resolve against
// multiple source files simultaneously.
//
// Fields ending with "#" are treated as numbered patterns: the plugin
// expands "item#" to "item1", "item2", … stopping as soon as getColumn()
// returns null for that file.  This keeps the plugin compatible with
// workspace versions that have fewer numbered columns.

type FieldSpec = string[];

function asciiLower(value: string): string {
    return value.replace(/[A-Z]/g, (ch: string) => String.fromCharCode(ch.charCodeAt(0) + 32));
}

const ITEM_CODE_FIELDS: Record<string, FieldSpec> = {
    uniqueitems: ["code"],
    books:       ["ScrollSpellCode", "BookSpellCode"],
    charstats:   ["item#"],
    gamble:      ["code"],
    gems:        ["code"],
    monequip:    ["item#"],
    setitems:    ["item"],
    runes:       ["rune#"]
};

function isItemCodeCol(file: string, col: string): boolean {
    const fields = ITEM_CODE_FIELDS[file];
    if (!fields) return false;
    const colLower = col.toLowerCase();
    for (const field of fields) {
        const fieldLower = field.toLowerCase();
        if (field.endsWith("#")) {
            const base = fieldLower.slice(0, -1);
            if (colLower.startsWith(base) && /^\d+$/.test(colLower.slice(base.length))) return true;
        } else if (colLower === fieldLower) {
            return true;
        }
    }
    return false;
}

function actualHeader(headers: string[], requested: string): string | null {
    const lower = requested.toLowerCase();
    for (const header of headers) {
        if (header.toLowerCase() === lower) return header;
    }
    return null;
}

function fixed4ByteKey(value: string): string {
    const bytes: number[] = [];
    for (let index = 0; index < value.length && bytes.length < 4; index++) {
        let code = value.charCodeAt(index);
        if (code >= 0xd800 && code <= 0xdbff
            && index + 1 < value.length
            && value.charCodeAt(index + 1) >= 0xdc00
            && value.charCodeAt(index + 1) <= 0xdfff) {
            code = 0x10000 + ((code - 0xd800) << 10) + (value.charCodeAt(++index) - 0xdc00);
        }
        if (code <= 0x7f) bytes.push(code);
        else if (code <= 0x7ff) bytes.push(0xc0 | (code >> 6), 0x80 | (code & 0x3f));
        else if (code <= 0xffff) bytes.push(0xe0 | (code >> 12), 0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f));
        else bytes.push(0xf0 | (code >> 18), 0x80 | ((code >> 12) & 0x3f), 0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f));
    }
    while (bytes.length < 4) bytes.push(0x20);
    return bytes.slice(0, 4).map((byte: number) => byte.toString(16).padStart(2, "0")).join("");
}

function findPackedItemTarget(value: string): [string, string] | null {
    const packed = fixed4ByteKey(value);
    for (const file of ["weapons", "armor", "misc"]) {
        if (!lookupKeyFixed4(file, "code", value)) continue;
        for (const candidate of getColumnValues(file, "code")) {
            if (fixed4ByteKey(candidate) === packed) return [file, candidate];
        }
    }
    return null;
}

function findCiItemTarget(value: string): [string, string] | null {
    const lower = asciiLower(value);
    for (const file of ["weapons", "armor", "misc"]) {
        for (const candidate of getColumnValues(file, "code")) {
            if (asciiLower(candidate) === lower) return [file, candidate];
        }
    }
    return null;
}

function sourceDescription(stem: string): string | null {
    const source = getWorkspaceSource(stem);
    if (!source) return null;
    if (source.kind === "bundled") {
        return "Built-in reference data (game version " + (source.version ?? "unknown") + ")";
    }
    const version = source.version ? " (game version " + source.version + ")" : "";
    if (source.kind === "open") return "Open document" + version;
    if (source.kind === "sibling") return "TXT file in the same folder" + version;
    return "TXT file in the current workspace" + version;
}

type ItemCodeSemantics = "resolved-fixed4" | "raw-fixed4-policy" | "ci-policy";

function itemCodeSemantics(file: string, col: string): ItemCodeSemantics {
    if (file === "runes" && /^rune[1-6]$/i.test(col)) return "resolved-fixed4";
    if (file === "uniqueitems" && col.toLowerCase() === "code") return "raw-fixed4-policy";
    return "ci-policy";
}

function usesPackedItemCode(file: string, col: string): boolean {
    return itemCodeSemantics(file, col) !== "ci-policy";
}

function itemCodeTargetsAvailable(): boolean {
    return hasLookupTarget("weapons", "code")
        && hasLookupTarget("armor", "code")
        && hasLookupTarget("misc", "code");
}

function hover(ctx: HoverContext): HoverResult | null {
    if (!ctx.value) return null;
    if (!isItemCodeCol(ctx.file, ctx.col)) return null;

    const target = usesPackedItemCode(ctx.file, ctx.col)
        ? findPackedItemTarget(ctx.value)
        : findCiItemTarget(ctx.value);
    if (!target) return null;
    const names = getFilteredColumnValues(target[0], "name", "code", target[1]);
    const name = names[0];
    if (!name) return null;

    const source = sourceDescription(target[0]);
    return { content: ctx.value + "\n\n" + name + (source ? "\n\nSource: " + source : "") };
}

function validate(ctx: PluginContext): PluginDiagnostic[] {
    const fields = ITEM_CODE_FIELDS[ctx.file];
    if (!fields) return [];
    if (!itemCodeTargetsAvailable()) return [];

    const diags: PluginDiagnostic[] = [];

    ctx.rows.forEach((row) => {
        for (const field of fields) {
            if (field.endsWith("#")) {
                // Numbered pattern: expand from 1 until the column is absent.
                const base = field.slice(0, -1);
                for (let i = 1; ; i++) {
                    const requested = base + i;
                    if (!getColumn(ctx.file, requested)) break;
                    const col = actualHeader(ctx.headers, requested);
                    if (col) checkItemCode(ctx.file, col, row, diags);
                }
            } else {
                const col = actualHeader(ctx.headers, field);
                if (col) checkItemCode(ctx.file, col, row, diags);
            }
        }
    });

    return diags;
}

function checkItemCode(
    file: string,
    col: string,
    row: WorkspaceRow,
    diags: PluginDiagnostic[],
): void {
    const val = row[col] as string;
    if (!val) return;
    if (val === "0" || val === "xxx") return;

    const semantics = itemCodeSemantics(file, col);
    const valid = semantics !== "ci-policy"
        ? lookupKeyFixed4("weapons", "code", val)
            || lookupKeyFixed4("armor", "code", val)
            || lookupKeyFixed4("misc", "code", val)
        : lookupKey("weapons", "code", val)
            || lookupKey("armor",   "code", val)
            || lookupKey("misc",    "code", val);
    if (!valid) {
        const c = row.__colstarts[col];
        const engineResolved = semantics === "resolved-fixed4";
        const rawPacked = semantics === "raw-fixed4-policy";
        diags.push({
            line:     row.__line,
            col:      c,
            endCol:   c + val.length,
            severity: engineResolved ? "error" : "warning",
            code:     engineResolved ? "item-code.unresolved" : "item-code.unresolved-policy",
            message: engineResolved
                ? `Unknown item code '${val}'. Check the four-character code and letter case.`
                : rawPacked
                    ? `No matching item was found. This field may keep the text without resolving it to an item; check whether that is intentional.`
                    : `Item code '${val}' is not listed in weapons, armor, or misc. Verify that the code is intentional.`,
        });
    }
}

function gotoDefinition(ctx: GotoDefinitionContext): GotoDefinitionTarget | null {
    const fields = ITEM_CODE_FIELDS[ctx.file];
    if (!fields) return null;
    if (!isItemCodeCol(ctx.file, ctx.col)) return null;

    if (usesPackedItemCode(ctx.file, ctx.col)) {
        const target = findPackedItemTarget(ctx.value);
        if (!target) return null;
        return { targetFile: target[0], targetCol: "code", targetValue: target[1] };
    }

    if (lookupKey("weapons", "code", ctx.value))
        return { targetFile: "weapons", targetCol: "code", targetValue: ctx.value };
    if (lookupKey("armor", "code", ctx.value))
        return { targetFile: "armor", targetCol: "code", targetValue: ctx.value };
    if (lookupKey("misc", "code", ctx.value))
        return { targetFile: "misc", targetCol: "code", targetValue: ctx.value };

    return null;
}
