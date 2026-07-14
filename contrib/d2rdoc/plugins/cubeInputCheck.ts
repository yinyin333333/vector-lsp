/// <reference path="../../vector-lsp-plugin.d.ts" />

const pluginMetadata: PluginMetadata = {
    validateFiles: ["cubemain"],
    hoverFiles: ["cubemain"],
    gotoDefinitionFiles: ["cubemain"],
};

// Validates "input #" fields in cubemain.txt.
//
// Each cell has the form:   ["][QTY,]BASE[,MOD[,MOD...]]["]
//
// Surrounding double-quotes are optional and are stripped before parsing.
// After stripping, the value is comma-split without trimming or case-folding.
// qty=N and qty,N are accepted before or after BASE.  Once an unrecognised
// suffix is reached, the successfully parsed base/modifier prefix is kept.
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
    upg:  "Upgraded",
    bas:  "Basic (un-upgraded)",
    exc:  "Exceptional",
    eli:  "Elite",
    nru:  "No Runeword",
    id:   "Identified",
};

// The binary parser accepts both qty=# and qty,#.  Input sock is only a bare
// flag; sock=# / sock,# do not encode a socket count (that is output syntax).
const PARAM_MODS: Record<string, string> = {
    qty:  "Quantity",
};

// ─── Helpers ─────────────────────────────────────────────────────────────────

function isInputCol(col: string): boolean {
    return /^input \d+$/.test(col.toLowerCase());
}

interface TextSpan {
    text: string;
    start: number;
    end: number;
}

interface QualifierSpan {
    text: string;
    start: number;
    end: number;
}

interface ParsedInputWithSpans {
    raw: string;
    base: TextSpan;
    qualifiers: QualifierSpan[];
    qty: string | null;
    ignoredSuffix: TextSpan | null;
}

interface ParsedQty {
    raw: QualifierSpan;
    value: string;
    consumed: number;
}

function rawTextSpan(raw: string, start: number, end: number): TextSpan {
    return { text: raw.slice(start, end), start, end };
}

function normalizeInputSpan(raw: string): TextSpan {
    let span = rawTextSpan(raw, 0, raw.length);
    if (span.text.length >= 2 && span.text[0] === '"' && span.text[span.text.length - 1] === '"') {
        span = rawTextSpan(raw, span.start + 1, span.end - 1);
    }
    return span;
}

function splitCommaSpans(raw: string, span: TextSpan): TextSpan[] {
    const parts: TextSpan[] = [];
    let start = span.start;
    for (let i = span.start; i < span.end; i++) {
        if (raw[i] === ",") {
            parts.push(rawTextSpan(raw, start, i));
            start = i + 1;
        }
    }
    parts.push(rawTextSpan(raw, start, span.end));
    return parts;
}

function parseQty(parts: TextSpan[], index: number): ParsedQty | null {
    const token = parts[index];
    if (!token) return null;
    if (token.text.indexOf("qty=") === 0) {
        return {
            raw: token,
            value: token.text.slice(4),
            consumed: 1,
        };
    }
    if (token.text === "qty" && parts[index + 1] !== undefined) {
        const parameter = parts[index + 1];
        return {
            raw: {
                text: token.text + "," + parameter.text,
                start: token.start,
                end: parameter.end,
            },
            value: parameter.text,
            consumed: 2,
        };
    }
    return null;
}

function parseInputWithSpans(raw: string): ParsedInputWithSpans {
    const normalized = normalizeInputSpan(raw);
    const parts = splitCommaSpans(raw, normalized);
    const qualifiers: QualifierSpan[] = [];
    let index = 0;
    let qty: string | null = null;
    let ignoredSuffix: TextSpan | null = null;

    const leadingQty = parseQty(parts, index);
    if (leadingQty) {
        qty = leadingQty.value;
        qualifiers.push(leadingQty.raw);
        index += leadingQty.consumed;
    }

    const base = parts[index] ?? { text: "", start: normalized.end, end: normalized.end };
    index++;

    for (; index < parts.length; index++) {
        const token = parts[index];
        const parsedQty = parseQty(parts, index);
        if (parsedQty) {
            qty = parsedQty.value;
            qualifiers.push(parsedQty.raw);
            index += parsedQty.consumed - 1;
            continue;
        }
        if (SIMPLE_MODS[token.text]) {
            qualifiers.push(token);
            continue;
        }
        // The input decoder sets the socket flag before rejecting a token
        // whose exact bytes merely begin with "sock" (for example sock=3).
        if (token.text.indexOf("sock") === 0 && token.text !== "sock") {
            qualifiers.push({ text: "sock", start: token.start, end: token.end });
        }
        ignoredSuffix = {
            text: raw.slice(token.start, normalized.end),
            start: token.start,
            end: normalized.end,
        };
        break;
    }

    return { raw, base, qualifiers, qty, ignoredSuffix };
}

function isUnsignedByte(value: string): boolean {
    if (!/^[0-9]+$/.test(value)) return false;
    const canonical = value.replace(/^0+/, "") || "0";
    return canonical.length < 3 || (canonical.length === 3 && canonical <= "255");
}

function unsignedByteValue(value: string | null): number {
    if (!/^[0-9]+$/.test(value ?? "")) return 0;
    let stored = 0;
    for (const digit of value ?? "") {
        stored = (stored * 10 + Number(digit)) % 256;
    }
    return stored;
}

function modifierDescription(mod: string): string {
    const eq = mod.indexOf("=");
    if (eq !== -1) {
        const key = mod.slice(0, eq);
        const val = mod.slice(eq + 1);
        const label = PARAM_MODS[key] || key;
        return label + ": " + val;
    }
    const comma = mod.indexOf(",");
    if (comma !== -1 && mod.slice(0, comma) === "qty") {
        return PARAM_MODS.qty + ": " + mod.slice(comma + 1);
    }
    return SIMPLE_MODS[mod] || mod;
}

type InputSource = "any" | "item" | "itemtype" | "unique" | "set";

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

function findPackedTarget(file: string, col: string, value: string): string | null {
    if (!lookupKeyFixed4(file, col, value)) return null;
    const packed = fixed4ByteKey(value);
    const values = getColumnValues(file, col);
    for (const candidate of values) {
        if (fixed4ByteKey(candidate) === packed) return candidate;
    }
    return null;
}

function resolveSource(base: string): InputSource | null {
    if (base === "any") return "any";
    if (lookupKeyFixed4("weapons", "code", base)
     || lookupKeyFixed4("armor",   "code", base)
     || lookupKeyFixed4("misc",    "code", base)) return "item";
    if (lookupKeyFixed4("itemtypes", "Code", base)) return "itemtype";
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

function rowGet(row: Record<string, string>, key: string): string {
    const lower = key.toLowerCase();
    const found = Object.keys(row).find((column) => column.toLowerCase() === lower);
    return found ? String(row[found] ?? "") : "";
}

function activeCubeRow(row: Record<string, string>, enabledCol: string | null): boolean {
    if (!enabledCol) return true;
    const enabled = String(row[enabledCol] ?? "").trim();
    return enabled !== "" && enabled !== "0";
}

function pushNumInputsDiagnostic(
    diags: PluginDiagnostic[],
    row: WorkspaceRow,
    col: string,
    raw: string,
    code: string,
    message: string,
): void {
    const start = row.__colstarts[col] ?? 0;
    diags.push({
        line: row.__line,
        col: start,
        endCol: start + raw.length,
        severity: "warning",
        code,
        message,
    });
}

function validate(ctx: PluginContext): PluginDiagnostic[] {
    if (ctx.file !== "cubemain") return [];

    const canProveBaseInvalid = cubeInputTargetsAvailable();
    const enabledCol = ctx.headers.find((header) => header.toLowerCase() === "enabled") ?? null;
    const numInputsCol = ctx.headers.find((header) => header.toLowerCase() === "numinputs") ?? null;

    const cols: string[] = [];
    for (let i = 1; i <= 7; i++) {
        const wanted = "input " + i;
        if (!getColumn("cubemain", wanted)) continue;
        const actual = ctx.headers.find((header) => header.toLowerCase() === wanted);
        if (actual) cols.push(actual);
    }

    const diags: PluginDiagnostic[] = [];

    for (const row of ctx.rows) {
        if (!activeCubeRow(row as Record<string, string>, enabledCol)) continue;

        const description = rowGet(row as Record<string, string>, "description");
        if (numInputsCol) {
            const declaredRaw = String(row[numInputsCol] ?? "");
            const declared = declaredRaw.trim();
            if (!declared || declared === "0") {
                pushNumInputsDiagnostic(
                    diags, row, numInputsCol, declaredRaw, "cube-input.no-inputs",
                    `cubemain.txt, line ${row.__line + 1}: no inputs for recipe '${description}'`,
                );
                continue;
            }
            if (!/^[0-9]+$/.test(declared)) {
                pushNumInputsDiagnostic(
                    diags, row, numInputsCol, declaredRaw, "cube-input.invalid-numinputs",
                    `cubemain.txt, line ${row.__line + 1}: invalid value for 'numinputs'`
                        + ` for recipe '${description}'`,
                );
                continue;
            }

            let actual = 0;
            for (const inputCol of cols) {
                const inputRaw = String(row[inputCol] ?? "");
                if (!inputRaw) continue;
                const parsed = parseInputWithSpans(inputRaw);
                actual += unsignedByteValue(parsed.qty) || 1;
            }
            const declaredNumber = Number(declared);
            if (declaredNumber !== actual) {
                pushNumInputsDiagnostic(
                    diags, row, numInputsCol, declaredRaw, "cube-input.numinputs-mismatch",
                    `cubemain.txt, line ${row.__line + 1}: wrong numinputs.`
                        + ` expected ${actual}, found ${declaredNumber} in recipe '${description}'`,
                );
            }
        }

        for (const col of cols) {
            const raw = String(row[col] ?? "");
            if (!raw) continue;

            const parsed = parseInputWithSpans(raw);
            const canonicalCol = col.toLowerCase();
            const c = row.__colstarts[col] ?? 0;
            const endCol = c + raw.length;
            if (!parsed.base.text) {
                diags.push({
                    line: row.__line,
                    col: c,
                    endCol,
                    severity: "warning",
                    code: "cube-input.invalid-base",
                    message: `cubemain.txt, line ${row.__line + 1}: empty base for ${canonicalCol}`
                        + ` in recipe '${description}'`,
                });
                continue;
            }

            if (canProveBaseInvalid && resolveSource(parsed.base.text) === null) {
                diags.push({
                    line:     row.__line,
                    col:      c,
                    endCol,
                    severity: "warning",
                    code: "cube-input.invalid-base",
                    message:  `cubemain.txt, line ${row.__line + 1}: couldn't find '${parsed.base.text}'`
                        + ` for ${canonicalCol} in recipe '${description}'`,
                });
                continue;
            }

            if (parsed.ignoredSuffix) {
                const stoppedAt = parsed.ignoredSuffix.text.split(",")[0] || "(empty modifier)";
                diags.push({
                    line: row.__line,
                    col: c,
                    endCol,
                    severity: "warning",
                    code: "cube-input.ignored-suffix",
                    message: `cubemain.txt, line ${row.__line + 1}: The game stops at '${stoppedAt}'`
                        + ` for '${canonicalCol}' in recipe '${description}'. The base and modifiers before it still work;`
                        + ` '${stoppedAt}' and everything after it are ignored.`,
                });
            }

            if (parsed.qty !== null && !isUnsignedByte(parsed.qty)) {
                const storedQty = unsignedByteValue(parsed.qty);
                const effectiveQty = storedQty || 1;
                diags.push({
                    line: row.__line,
                    col: c,
                    endCol,
                    severity: "warning",
                    code: "cube-input.u8-range",
                    message: `cubemain.txt, line ${row.__line + 1}: input quantity '${parsed.qty}'`
                        + ` for '${canonicalCol}' is outside 0..255. The game reads it as ${storedQty || 0}`
                        + ` and uses ${effectiveQty} item(s) in recipe '${description}'. Enter a value from 0 through 255.`,
                });
            }
        }
    }

    return diags;
}

// ─── hover ────────────────────────────────────────────────────────────────────

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
    if (ctx.file !== "cubemain") return null;
    if (!isInputCol(ctx.col)) return null;
    if (!ctx.value) return null;

    const parsed = parseInputWithSpans(ctx.value);
    const base = parsed.base.text;
    if (!base) return null;

    const parts: string[] = [];
    let resolvedStem: string | null = null;

    if (base === "any") {
        parts.push("**any** — Accepts any item");
    } else {
        const itemMatches: [string, string | null][] = [
            ["weapons", findPackedTarget("weapons", "code", base)],
            ["armor", findPackedTarget("armor", "code", base)],
            ["misc", findPackedTarget("misc", "code", base)],
        ];
        const itemMatch = itemMatches.find((entry) => entry[1] !== null);
        const itemCode = itemMatch ? itemMatch[1] : null;
        const itemType = findPackedTarget("itemtypes", "Code", base);
        const itemNames = itemCode
            ? getFilteredColumnValues(itemMatch ? itemMatch[0] : "", "name", "code", itemCode)
            : (itemType ? getFilteredColumnValues("itemtypes", "ItemType", "Code", itemType) : []);

        if (itemNames.length > 0) {
            parts.push("**" + base + "** — " + itemNames[0]);
            resolvedStem = itemCode && itemMatch ? itemMatch[0] : "itemtypes";
        } else if (lookupKey("uniqueitems", "index", base)) {
            parts.push("**" + base + "** (Unique Item)");
            resolvedStem = "uniqueitems";
        } else if (lookupKey("setitems", "index", base)) {
            parts.push("**" + base + "** (Set Item)");
            resolvedStem = "setitems";
        } else {
            return null;
        }
    }

    if (resolvedStem) {
        const source = sourceDescription(resolvedStem);
        if (source) parts.push("", "Source: " + source);
    }

    if (parsed.qualifiers.length > 0) {
        parts.push("");
        parts.push("*Modifiers the game will use:*");
        for (const qualifier of parsed.qualifiers) {
            parts.push("- " + modifierDescription(qualifier.text));
        }
    }

    if (parsed.ignoredSuffix) {
        const stoppedAt = parsed.ignoredSuffix.text.split(",")[0] || "(empty modifier)";
        parts.push("", "Ignored text begins at: `" + stoppedAt + "`");
        parts.push("The base and modifiers before it still work; `" + stoppedAt
            + "` and everything after it are ignored.");
    }

    if (parsed.qty !== null && !isUnsignedByte(parsed.qty)) {
        const storedQty = unsignedByteValue(parsed.qty);
        parts.push("", "Quantity `" + parsed.qty + "` is read as `" + storedQty
            + "`; the game uses `" + (storedQty || 1) + "` item(s). Use 0 through 255.");
    }

    return { content: parts.join("\n") };
}

// ─── gotoDefinition ───────────────────────────────────────────────────────────

function gotoDefinition(ctx: GotoDefinitionContext): GotoDefinitionTarget | null {
    if (ctx.file !== "cubemain") return null;
    if (!isInputCol(ctx.col)) return null;
    if (!ctx.value) return null;

    const base = parseInputWithSpans(ctx.value).base.text;
    if (!base || base === "any") return null;

    const weapon = findPackedTarget("weapons", "code", base);
    if (weapon) return { targetFile: "weapons", targetCol: "code", targetValue: weapon };
    const armor = findPackedTarget("armor", "code", base);
    if (armor) return { targetFile: "armor", targetCol: "code", targetValue: armor };
    const misc = findPackedTarget("misc", "code", base);
    if (misc) return { targetFile: "misc", targetCol: "code", targetValue: misc };
    const itemType = findPackedTarget("itemtypes", "Code", base);
    if (itemType) return { targetFile: "itemtypes", targetCol: "Code", targetValue: itemType };
    if (lookupKey("uniqueitems", "index", base))
        return { targetFile: "uniqueitems", targetCol: "index", targetValue: base };
    if (lookupKey("setitems", "index", base))
        return { targetFile: "setitems", targetCol: "index", targetValue: base };

    return null;
}
