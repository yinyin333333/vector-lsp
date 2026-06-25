/// <reference path="../../vector-lsp-plugin.d.ts" />

// Validates fields that must contain a valid item code (weapons/armor/misc)
// but are typed as plain text in the schema because they resolve against
// multiple source files simultaneously.
//
// Fields ending with "#" are treated as numbered patterns: the plugin
// expands "item#" to "item1", "item2", … stopping as soon as getColumn()
// returns null for that file.  This keeps the plugin compatible with
// workspace versions that have fewer numbered columns.

type FieldSpec = string[];

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
    for (const field of fields) {
        if (field.endsWith("#")) {
            const base = field.slice(0, -1);
            if (col.startsWith(base) && /^\d+$/.test(col.slice(base.length))) return true;
        } else if (col === field) {
            return true;
        }
    }
    return false;
}

function itemCodeTargetsAvailable(): boolean {
    return hasLookupTarget("weapons", "code")
        && hasLookupTarget("armor", "code")
        && hasLookupTarget("misc", "code");
}

function hover(ctx: HoverContext): HoverResult | null {
    if (!ctx.value) return null;
    if (!isItemCodeCol(ctx.file, ctx.col)) return null;

    const names = getFilteredColumnValues("weapons", "name", "code", ctx.value).concat(
                  getFilteredColumnValues("armor",   "name", "code", ctx.value),
                  getFilteredColumnValues("misc",    "name", "code", ctx.value));
    const name = names[0];
    if (!name) return null;

    return { content: ctx.value + "\n\n" + name };
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
                    const col = base + i;
                    if (!getColumn(ctx.file, col)) break;
                    checkItemCode(ctx.file, col, row, diags);
                }
            } else {
                checkItemCode(ctx.file, field, row, diags);
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

    const valid = lookupKey("weapons", "code", val)
               || lookupKey("armor",   "code", val)
               || lookupKey("misc",    "code", val);
    if (!valid) {
        const c = row.__colstarts[col];
        diags.push({
            line:     row.__line,
            col:      c,
            endCol:   c + val.length,
            severity: "error",
            message:  `'${val}' is not a valid item code (not found in weapons, armor, or misc)`,
        });
    }
}

function gotoDefinition(ctx: GotoDefinitionContext): GotoDefinitionTarget | null {
    const fields = ITEM_CODE_FIELDS[ctx.file];
    if (!fields) return null;

    if (lookupKey("weapons", "code", ctx.value))
        return { targetFile: "weapons", targetCol: "code", targetValue: ctx.value };
    if (lookupKey("armor", "code", ctx.value))
        return { targetFile: "armor", targetCol: "code", targetValue: ctx.value };
    if (lookupKey("misc", "code", ctx.value))
        return { targetFile: "misc", targetCol: "code", targetValue: ctx.value };

    return null;
}
