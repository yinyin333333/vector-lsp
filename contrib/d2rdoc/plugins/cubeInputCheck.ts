/// <reference path="../../vector-lsp-plugin.d.ts" />

// Validates "input #" fields in cubemain.txt.
//
// Each cell has the form:   ["]BASE[,MOD[,MOD...]]["]
//
// Surrounding double-quotes are optional and are stripped before parsing.
// After stripping, the value is comma-split: the first token is BASE and
// each subsequent token is a modifier.
//
// BASE must be one of:
//   1. The special keyword "any"
//   2. An item code from weapons / armor / misc  (matched by "code" column)
//   3. An item type code from itemtypes          (matched by "Code" column)
//   4. A unique item name from uniqueitems       (matched by "index" column)
//   5. A set item name from setitems             (matched by "index" column)
//
// Modifiers are either simple keywords or parameterized "key=#" pairs.

// ─── Modifier tables ─────────────────────────────────────────────────────────

const SIMPLE_MODS: Record<string, string> = {
    low:  "Low Quality",
    nor:  "Normal Quality",
    hiq:  "Superior",
    mag:  "Magic",
    rar:  "Rare",
    set:  "Set",
    uni:  "Unique",
    crf:  "Crafted",
    tmp:  "Tempered",
    eth:  "Ethereal",
    noe:  "Non-Ethereal",
    sock: "Socketed",
    nos:  "Non-Socketed",
    brk:  "Broken",
    upg:  "Upgraded",
    bas:  "Basic (un-upgraded)",
    exc:  "Exceptional",
    eli:  "Elite",
    nru:  "No Runeword",
    id:   "Identified",
};

// Keys that accept a "key=#" form (value must be a non-negative integer).
// sock also appears in SIMPLE_MODS (bare "sock" = has sockets; "sock=2" = exactly 2).
const PARAM_MODS: Record<string, string> = {
    qty:  "Quantity",
    sock: "Socket Count",
};

// ─── Helpers ─────────────────────────────────────────────────────────────────

function isInputCol(col: string): boolean {
    return /^input \d+$/.test(col);
}

interface ParsedInput {
    base: string;
    modifiers: string[];
}

function parseInput(raw: string): ParsedInput {
    let s = raw.trim();
    if (s.length >= 2 && s.startsWith('"') && s.endsWith('"')) {
        s = s.slice(1, -1).trim();
    }
    const parts = s.split(",");
    return {
        base:      parts[0].trim(),
        modifiers: parts.slice(1).map((m) => m.trim()).filter((m) => m.length > 0),
    };
}

// Returns null if the modifier is valid, or an error string if not.
function validateModifier(mod: string): string | null {
    const eq = mod.indexOf("=");
    if (eq !== -1) {
        const key = mod.slice(0, eq).trim().toLowerCase();
        const val = mod.slice(eq + 1).trim();
        if (!PARAM_MODS[key]) {
            const validParams = Object.keys(PARAM_MODS).join(", ");
            return `Unknown parameterized modifier '${key}' (valid parameterized: ${validParams})`;
        }
        if (!/^\d+$/.test(val)) {
            return `Modifier '${key}' requires a non-negative integer value, got '${val}'`;
        }
        return null;
    }
    if (!SIMPLE_MODS[mod.toLowerCase()]) {
        const valid = Object.keys(SIMPLE_MODS).concat(Object.keys(PARAM_MODS).map((k) => k + "=#")).join(", ");
        return `Unknown input modifier '${mod}' (valid: ${valid})`;
    }
    return null;
}

function modifierDescription(mod: string): string {
    const eq = mod.indexOf("=");
    if (eq !== -1) {
        const key = mod.slice(0, eq).trim().toLowerCase();
        const val = mod.slice(eq + 1).trim();
        const label = PARAM_MODS[key] || key;
        return label + ": " + val;
    }
    return SIMPLE_MODS[mod.toLowerCase()] || mod;
}

type InputSource = "any" | "item" | "itemtype" | "unique" | "set";

function resolveSource(base: string): InputSource | null {
    if (base.toLowerCase() === "any") return "any";
    if (lookupKey("weapons",     "code",  base)
     || lookupKey("armor",       "code",  base)
     || lookupKey("misc",        "code",  base)) return "item";
    if (lookupKey("itemtypes",   "Code",  base)) return "itemtype";
    if (lookupKey("uniqueitems", "index", base)) return "unique";
    if (lookupKey("setitems",    "index", base)) return "set";
    return null;
}

// ─── validate ─────────────────────────────────────────────────────────────────

function cubeInputTargetsAvailable(): boolean {
    return hasLookupTarget("weapons", "code")
        && hasLookupTarget("armor", "code")
        && hasLookupTarget("misc", "code")
        && hasLookupTarget("itemtypes", "Code")
        && hasLookupTarget("uniqueitems", "index")
        && hasLookupTarget("setitems", "index");
}

function validate(ctx: PluginContext): PluginDiagnostic[] {
    if (ctx.file !== "cubemain") return [];

    const canProveBaseInvalid = cubeInputTargetsAvailable();

    const cols: string[] = [];
    for (let i = 1; ; i++) {
        const col = "input " + i;
        if (!getColumn("cubemain", col)) break;
        cols.push(col);
    }

    const diags: PluginDiagnostic[] = [];

    for (const row of ctx.rows) {
        for (const col of cols) {
            const raw = row[col] as string | undefined;
            if (!raw || !raw.trim()) continue;

            const { base, modifiers } = parseInput(raw.trim());
            if (!base) continue;

            const c = row.__colstarts[col] ?? 0;

            if (base.toLowerCase() !== "any" && canProveBaseInvalid && resolveSource(base) === null) {
                diags.push({
                    line:     row.__line,
                    col:      c,
                    endCol:   c + raw.length,
                    severity: "error",
                    message:  `'${base}' is not a valid cubemain input`
                        + ` (not found in weapons/armor/misc codes, itemtypes codes,`
                        + ` uniqueitems index, or setitems index)`,
                });
                continue;
            }

            for (const mod of modifiers) {
                const err = validateModifier(mod);
                if (err) {
                    diags.push({
                        line:     row.__line,
                        col:      c,
                        endCol:   c + raw.length,
                        severity: "warning",
                        message:  err,
                    });
                }
            }
        }
    }

    return diags;
}

// ─── hover ────────────────────────────────────────────────────────────────────

function hover(ctx: HoverContext): HoverResult | null {
    if (ctx.file !== "cubemain") return null;
    if (!isInputCol(ctx.col)) return null;
    if (!ctx.value) return null;

    const { base, modifiers } = parseInput(ctx.value);
    if (!base) return null;

    const parts: string[] = [];

    if (base.toLowerCase() === "any") {
        parts.push("**any** — Accepts any item");
    } else {
        const itemNames = getFilteredColumnValues("weapons", "name", "code", base)
            .concat(getFilteredColumnValues("armor",      "name",     "code", base))
            .concat(getFilteredColumnValues("misc",       "name",     "code", base))
            .concat(getFilteredColumnValues("itemtypes",  "ItemType", "Code", base));

        if (itemNames.length > 0) {
            parts.push("**" + base + "** — " + itemNames[0]);
        } else if (lookupKey("uniqueitems", "index", base)) {
            parts.push("**" + base + "** (Unique Item)");
        } else if (lookupKey("setitems", "index", base)) {
            parts.push("**" + base + "** (Set Item)");
        } else {
            return null;
        }
    }

    if (modifiers.length > 0) {
        parts.push("");
        parts.push("*Modifiers:*");
        for (const mod of modifiers) {
            parts.push("- " + modifierDescription(mod));
        }
    }

    return { content: parts.join("\n") };
}

// ─── gotoDefinition ───────────────────────────────────────────────────────────

function gotoDefinition(ctx: GotoDefinitionContext): GotoDefinitionTarget | null {
    if (ctx.file !== "cubemain") return null;
    if (!isInputCol(ctx.col)) return null;
    if (!ctx.value) return null;

    const { base } = parseInput(ctx.value);
    if (!base || base.toLowerCase() === "any") return null;

    if (lookupKey("weapons", "code", base))
        return { targetFile: "weapons", targetCol: "code", targetValue: base };
    if (lookupKey("armor", "code", base))
        return { targetFile: "armor", targetCol: "code", targetValue: base };
    if (lookupKey("misc", "code", base))
        return { targetFile: "misc", targetCol: "code", targetValue: base };
    if (lookupKey("itemtypes", "Code", base))
        return { targetFile: "itemtypes", targetCol: "Code", targetValue: base };
    if (lookupKey("uniqueitems", "index", base))
        return { targetFile: "uniqueitems", targetCol: "index", targetValue: base };
    if (lookupKey("setitems", "index", base))
        return { targetFile: "setitems", targetCol: "index", targetValue: base };

    return null;
}
