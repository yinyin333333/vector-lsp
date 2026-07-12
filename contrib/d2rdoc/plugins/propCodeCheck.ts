/// <reference path="../../vector-lsp-plugin.d.ts" />

const pluginMetadata: PluginMetadata = {
    validateFiles: [
        "automagic", "magicprefix", "magicsuffix", "gems", "qualityitems", "cubemain",
        "monprop", "setitems", "uniqueitems", "runes", "sets",
    ],
    gotoDefinitionFiles: [
        "automagic", "magicprefix", "magicsuffix", "gems", "qualityitems", "cubemain",
        "monprop", "setitems", "uniqueitems", "runes", "sets",
    ],
};

// Validates fields that must contain a valid property code (properties.txt or
// propertygroups.txt).
//
// Fields containing "#" are treated as numbered patterns where # is replaced
// by the counter (e.g. "mod#code" → "mod1code", "mod 2code", …).  The plugin
// expands from 1 upward, stopping as soon as getColumn() returns null.  This
// handles both suffix patterns ("prop#" → "prop1") and mid-name patterns
// ("mod#code" → "mod1code").

type FieldSpec = string[];

const PROP_CODE_FIELDS: Record<string, FieldSpec> = {
    automagic:    ["mod#code"],
    magicprefix:  ["mod#code"],
    magicsuffix:  ["mod#code"],
    gems:         ["weaponMod#Code", "helmMod#Code", "shieldMod#Code"],
    qualityitems: ["mod#code"],
    cubemain:     ["mod #"],
    monprop:      ["prop#"],
    setitems:     ["prop#", "aprop#"],
    uniqueitems:  ["prop#"],
    runes:        ["T1Code#"],
    sets:         ["PCode#", "FCode#"],
};

function propTargetsAvailable(): boolean {
    return hasLookupTarget("properties", "code")
        && hasLookupTarget("propertygroups", "code");
}

function validate(ctx: PluginContext): PluginDiagnostic[] {
    const fields = PROP_CODE_FIELDS[ctx.file];
    if (!fields) return [];
    if (!propTargetsAvailable()) return [];

    const diags: PluginDiagnostic[] = [];

    ctx.rows.forEach((row) => {
        for (const field of fields) {
            if (field.includes("#")) {
                for (let i = 1; ; i++) {
                    const col = field.replace("#", String(i));
                    if (!getColumn(ctx.file, col)) break;
                    checkPropCode(ctx.file, col, row, diags);
                }
            } else {
                checkPropCode(ctx.file, field, row, diags);
            }
        }
    });

    return diags;
}

function checkPropCode(
    file: string,
    col: string,
    row: WorkspaceRow,
    diags: PluginDiagnostic[],
): void {
    const val = row[col] as string;
    if (!val) return;

    const valid = lookupKey("properties",     "code", val)
               || lookupKey("propertygroups", "code", val);
    if (!valid) {
        const c = row.__colstarts[col];
        diags.push({
            line:     row.__line,
            col:      c,
            endCol:   c + val.length,
            severity: "error",
            message:  `propCodeCheck: '${val}' is not a valid property code (not found in properties or propertygroups)`,
        });
    }
}

function gotoDefinition(ctx: GotoDefinitionContext): GotoDefinitionTarget | null {
    const fields = PROP_CODE_FIELDS[ctx.file];
    if (!fields) return null;

    if (lookupKey("properties", "code", ctx.value))
        return { targetFile: "properties", targetCol: "code", targetValue: ctx.value };
    if (lookupKey("propertygroups", "code", ctx.value))
        return { targetFile: "propertygroups", targetCol: "code", targetValue: ctx.value };

    return null;
}
