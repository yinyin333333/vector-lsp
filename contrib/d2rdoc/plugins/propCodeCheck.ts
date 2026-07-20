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
    hoverFiles: [
        "automagic", "magicprefix", "magicsuffix", "gems", "qualityitems", "cubemain",
        "monprop", "setitems", "uniqueitems", "runes", "sets",
    ],
};

// Validates fields that must contain a valid property code (properties.txt or
// propertygroups.txt).
//
// Keep the binary-backed descriptor columns explicit.  Several table families
// intentionally have header gaps (MonProp difficulty suffixes, SetItems
// aprop#a/b, Sets PCode#a/b); stop-at-first-gap expansion silently skipped
// 30 columns / 280 populated stock cells.

type FieldSpec = string[];

const PROP_CODE_FIELDS: Record<string, FieldSpec> = {
    automagic:    ["mod1code", "mod2code", "mod3code"],
    magicprefix:  ["mod1code", "mod2code", "mod3code"],
    magicsuffix:  ["mod1code", "mod2code", "mod3code"],
    gems: [
        "weaponMod1Code", "weaponMod2Code", "weaponMod3Code",
        "helmMod1Code", "helmMod2Code", "helmMod3Code",
        "shieldMod1Code", "shieldMod2Code", "shieldMod3Code",
    ],
    qualityitems: ["mod1code", "mod2code"],
    cubemain:     ["mod 1", "mod 2", "mod 3", "mod 4", "mod 5"],
    monprop: [
        "prop1", "prop1 (H)", "prop1 (N)",
        "prop2", "prop2 (H)", "prop2 (N)",
        "prop3", "prop3 (H)", "prop3 (N)",
        "prop4", "prop4 (H)", "prop4 (N)",
        "prop5", "prop5 (H)", "prop5 (N)",
        "prop6", "prop6 (H)", "prop6 (N)",
    ],
    setitems: [
        "prop1", "prop2", "prop3", "prop4", "prop5", "prop6", "prop7", "prop8", "prop9",
        "aprop1a", "aprop1b", "aprop2a", "aprop2b", "aprop3a", "aprop3b",
        "aprop4a", "aprop4b", "aprop5a", "aprop5b",
    ],
    uniqueitems: [
        "prop1", "prop2", "prop3", "prop4", "prop5", "prop6",
        "prop7", "prop8", "prop9", "prop10", "prop11", "prop12",
    ],
    runes: ["T1Code1", "T1Code2", "T1Code3", "T1Code4", "T1Code5", "T1Code6", "T1Code7"],
    sets: [
        "FCode1", "FCode2", "FCode3", "FCode4", "FCode5", "FCode6", "FCode7", "FCode8",
        "PCode2a", "PCode2b", "PCode3a", "PCode3b",
        "PCode4a", "PCode4b", "PCode5a", "PCode5b",
    ],
};

function actualHeader(headers: string[], requested: string): string | null {
    const lower = requested.toLowerCase();
    for (const header of headers) {
        if (header.toLowerCase() === lower) return header;
    }
    return null;
}

function isPropCodeCol(file: string, col: string): boolean {
    const fields = PROP_CODE_FIELDS[file];
    if (!fields) return false;
    const lower = col.toLowerCase();
    return fields.some((field) => field.toLowerCase() === lower);
}

function propTargetsAvailable(): boolean {
    return hasLookupTarget("properties", "code")
        && (!propertyGroupsEnabled() || hasLookupTarget("propertygroups", "code"));
}

function propertyGroupsEnabled(): boolean {
    const source = getWorkspaceSource("properties") ?? getWorkspaceSource("propertygroups");
    const version = source?.version;
    return version !== "1.13" && version !== "1.13c" && version !== "2.4";
}

function validate(ctx: PluginContext): PluginDiagnostic[] {
    const fields = PROP_CODE_FIELDS[ctx.file];
    if (!fields) return [];
    if (!propTargetsAvailable()) return [];

    const diags: PluginDiagnostic[] = [];

    ctx.rows.forEach((row) => {
        for (const field of fields) {
            const col = actualHeader(ctx.headers, field);
            if (col) checkPropCode(ctx.file, col, row, diags);
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

    const valid = lookupKey("properties", "code", val)
               || (propertyGroupsEnabled() && lookupKey("propertygroups", "code", val));
    if (!valid) {
        const c = row.__colstarts[col];
        const marker = val.startsWith("*");
        diags.push({
            line:     row.__line,
            col:      c,
            endCol:   c + val.length,
            severity: "warning",
            code: marker ? "property.unknown-marker" : "property.unknown-code",
            messageKey: marker
                ? "plugin.property.unknown-marker"
                : "plugin.property.unknown-code",
            messageArgs: { value: val },
            legacyMessage: marker
                ? `This value starts with '*', but it is not a known property code. Keep it only if it is intentionally used as a marker.`
                : `Unknown property code '${val}'. Choose a code from properties.txt or propertygroups.txt.`,
        });
    }
}

function gotoDefinition(ctx: GotoDefinitionContext): GotoDefinitionTarget | null {
    const fields = PROP_CODE_FIELDS[ctx.file];
    if (!fields) return null;
    if (!isPropCodeCol(ctx.file, ctx.col)) return null;

    if (lookupKey("properties", "code", ctx.value))
        return { targetFile: "properties", targetCol: "code", targetValue: ctx.value };
    if (propertyGroupsEnabled() && lookupKey("propertygroups", "code", ctx.value))
        return { targetFile: "propertygroups", targetCol: "code", targetValue: ctx.value };

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

function hover(ctx: HoverContext): HoverResult | null {
    if (!isPropCodeCol(ctx.file, ctx.col) || !ctx.value) return null;
    let stem: string | null = null;
    if (lookupKey("properties", "code", ctx.value)) stem = "properties";
    else if (propertyGroupsEnabled() && lookupKey("propertygroups", "code", ctx.value)) stem = "propertygroups";
    if (!stem) {
        if (!propTargetsAvailable()) return null;
        return {
            contentKey: "plugin.property.unknown-hover",
            contentArgs: { value: ctx.value },
            legacyContent: `Unknown property code: \`${ctx.value}\`. Choose a code from properties.txt or propertygroups.txt.`,
        };
    }
    const source = getWorkspaceSource(stem);
    return {
        contentKey: "plugin.property.hover",
        contentArgs: {
            value: ctx.value,
            sourceFile: stem,
            sourceKind: source?.kind || "",
            sourceVersion: source?.version || "",
        },
        legacyContent: `Property: \`${ctx.value}\` (${stem}.txt code)`
            + (sourceDescription(stem) ? "\n\nSource: " + sourceDescription(stem) : ""),
    };
}
