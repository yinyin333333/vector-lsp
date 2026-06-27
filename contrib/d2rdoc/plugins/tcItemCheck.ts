/// <reference path="../../vector-lsp-plugin.d.ts" />

// Validates Item# fields in TreasureClassEx.txt.
//
// Each Item# cell has the form:   BASE[,KEY=VALUE[,KEY=VALUE...]]
//
// BASE must be one of:
//   1. An item code (weapons/armor/misc "code" column).
//   2. A Treasure Class name from another TreasureClassEx row, but ONLY if
//      that row appears ABOVE the current row in the file.
//   3. An auto-generated TC: any itemtypes "Code" where TreasureClass=1,
//      optionally followed by a positive integer level suffix
//      (e.g. "weap", "weap3", "armo6").
//   4. A uniqueitems "index" value.
//   5. A setitems "index" value.
//
// Valid modifier keys after the comma: mul cu cs cr cm ce cg ma mg

// ─── Constants ────────────────────────────────────────────────────────────────

const VALID_MOD_KEYS: Record<string, true> = {
    mul: true, cu: true, cs: true, cr: true,
    cm: true, ce: true, cg: true, ma: true, mg: true,
};

interface TextSpan {
    text: string;
    start: number;
    end: number;
}

interface ModifierSpan {
    text: string;
    start: number;
    end: number;
    key?: TextSpan;
    value?: TextSpan;
    equals?: TextSpan;
}

interface ParsedItemWithSpans {
    base: TextSpan;
    modifiers: ModifierSpan[];
}

interface ItemValidationError {
    message: string;
    span: TextSpan;
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

// Split "BASE,key=val,key=val" → [base, "key=val,key=val"].
// Only splits when what follows the first comma looks like a modifier.
function isWhitespace(ch: string): boolean {
    return /\s/.test(ch);
}

function trimTextSpan(raw: string, start: number, end: number): TextSpan {
    while (start < end && isWhitespace(raw[start])) start++;
    while (end > start && isWhitespace(raw[end - 1])) end--;
    return { text: raw.slice(start, end), start, end };
}

function normalizeItemSpan(raw: string): TextSpan {
    let span = trimTextSpan(raw, 0, raw.length);
    if (span.text.length >= 2 && span.text[0] === '"' && span.text[span.text.length - 1] === '"') {
        span = trimTextSpan(raw, span.start + 1, span.end - 1);
    }
    return span;
}

function splitCommaSpans(raw: string, span: TextSpan): TextSpan[] {
    const parts: TextSpan[] = [];
    let start = span.start;
    for (let i = span.start; i < span.end; i++) {
        if (raw[i] === ",") {
            parts.push(trimTextSpan(raw, start, i));
            start = i + 1;
        }
    }
    parts.push(trimTextSpan(raw, start, span.end));
    return parts;
}

function parseModifierSpan(raw: string, span: TextSpan): ModifierSpan {
    const eq = span.text.indexOf("=");
    if (eq === -1) {
        return { text: span.text, start: span.start, end: span.end };
    }
    const eqStart = span.start + eq;
    return {
        text: span.text,
        start: span.start,
        end: span.end,
        key: trimTextSpan(raw, span.start, eqStart),
        value: trimTextSpan(raw, eqStart + 1, span.end),
        equals: { text: "=", start: eqStart, end: eqStart + 1 },
    };
}

function nonEmptySpan(span: TextSpan): TextSpan {
    if (span.end > span.start) return span;
    if (span.start > 0) return { text: "", start: span.start - 1, end: span.start };
    return { text: "", start: span.start, end: span.end };
}

function splitModifiers(raw: string): [string, string] {
    const idx = raw.indexOf(",");
    if (idx !== -1) {
        const after = raw.slice(idx + 1);
        // After comma must start with a known modifier key followed by "="
        if (/^[a-z]+=/.test(after)) {
            return [raw.slice(0, idx).trim(), after];
        }
    }
    return [raw.trim(), ""];
}

function parseItemWithSpans(raw: string): ParsedItemWithSpans {
    const normalized = normalizeItemSpan(raw);
    const comma = normalized.text.indexOf(",");
    if (comma !== -1) {
        const commaStart = normalized.start + comma;
        const after = raw.slice(commaStart + 1, normalized.end);
        if (/^[a-z]+=/.test(after)) {
            const base = trimTextSpan(raw, normalized.start, commaStart);
            const modsSpan = { text: after, start: commaStart + 1, end: normalized.end };
            return {
                base,
                modifiers: splitCommaSpans(raw, modsSpan).map((m) => parseModifierSpan(raw, m)),
            };
        }
    }
    return { base: normalized, modifiers: [] };
}

function validateModifiers(modifiers: ModifierSpan[]): ItemValidationError | null {
    for (const mod of modifiers) {
        if (!mod.key || !mod.value || !mod.equals) {
            return {
                message: `Invalid modifier '${mod.text}' (expected key=value)`,
                span: nonEmptySpan({ text: mod.text, start: mod.start, end: mod.end }),
            };
        }
        const key = mod.key.text;
        const val = mod.value.text;
        if (!VALID_MOD_KEYS[key]) {
            return {
                message: `Unknown modifier key '${key}' (valid: mul, cu, cs, cr, cm, ce, cg, ma, mg)`,
                span: nonEmptySpan(mod.key),
            };
        }
        if (!val) {
            return {
                message: `Missing value for modifier '${key}'`,
                span: mod.equals,
            };
        }
    }
    return null;
}

// ─── validate ─────────────────────────────────────────────────────────────────

function tcItemTargetsAvailable(): boolean {
    return hasLookupTarget("weapons", "code")
        && hasLookupTarget("armor", "code")
        && hasLookupTarget("misc", "code")
        && hasLookupTarget("itemtypes", "Code")
        && hasLookupTarget("itemtypes", "TreasureClass")
        && hasLookupTarget("uniqueitems", "index")
        && hasLookupTarget("setitems", "index");
}

function validate(ctx: PluginContext): PluginDiagnostic[] {
    if (ctx.file !== "treasureclassex") return [];

    // ── Build TC-name → first-line-number map (all rows, case-insensitive key).
    // Store only the first occurrence so forward-reference detection is stable.
    const tcLineMap: Record<string, number> = {};
    for (const row of ctx.rows) {
        const name = (row["Treasure Class"] as string | undefined);
        if (name && name.trim()) {
            const key = name.trim().toLowerCase();
            if (!(key in tcLineMap)) {
                tcLineMap[key] = row.__line;
            }
        }
    }

    // ── Build set of auto-TC base codes from itemtypes where TreasureClass=1.
    const canProveExternalItemInvalid = tcItemTargetsAvailable();
    const autoTcCodes = new Set(
        canProveExternalItemInvalid
            ? getFilteredColumnValues("itemtypes", "Code", "TreasureClass", "1")
                .map((c: string) => c.toLowerCase())
            : []
    );

    // ── Determine which external workspace files are present (guard against
    // false positives when running on an incomplete workspace).
    const hasWeapons    = hasLookupTarget("weapons", "code");
    const hasArmor      = hasLookupTarget("armor", "code");
    const hasMisc       = hasLookupTarget("misc", "code");
    const hasUnique     = hasLookupTarget("uniqueitems", "index");
    const hasSetitems   = hasLookupTarget("setitems", "index");

    // ── Expand Item# columns until the workspace says a column is absent.
    const cols: string[] = [];
    for (let n = 1; ; n++) {
        const col = "Item" + n;
        if (!getColumn("treasureclassex", col)) break;
        cols.push(col);
    }

    const diags: PluginDiagnostic[] = [];

    for (const row of ctx.rows) {
        for (const col of cols) {
            const raw = row[col] as string | undefined;
            if (!raw || !raw.trim()) continue;

            const err = validateItem(
                raw, row.__line,
                tcLineMap, autoTcCodes,
                hasWeapons, hasArmor, hasMisc, hasUnique, hasSetitems, canProveExternalItemInvalid,
            );
            if (err) {
                const c = row.__colstarts[col] ?? 0;
                diags.push({
                    line:     row.__line,
                    col:      c + err.span.start,
                    endCol:   c + Math.max(err.span.end, err.span.start + 1),
                    severity: "error",
                    message:  err.message,
                });
            }
        }
    }

    return diags;
}

function validateItem(
    raw: string,
    currentLine: number,
    tcLineMap: Record<string, number>,
    autoTcCodes: Set<string>,
    hasWeapons: boolean,
    hasArmor: boolean,
    hasMisc: boolean,
    hasUnique: boolean,
    hasSetitems: boolean,
    canProveExternalItemInvalid: boolean,
): ItemValidationError | null {
    // Strip surrounding double-quotes — the game engine requires quotes around
    // entries that contain modifiers (e.g. `"gld,mul=1280"`).
    // parseItemWithSpans keeps quote/trim offsets mapped to the original cell.

    // ── Split BASE from optional modifiers.
    const parsed = parseItemWithSpans(raw);
    const base = parsed.base.text;

    if (!base) return { message: "Empty item value", span: nonEmptySpan(parsed.base) };

    // ── Validate modifiers (always, regardless of base validity).
    if (parsed.modifiers.length > 0) {
        const modErr = validateModifiers(parsed.modifiers);
        if (modErr) return modErr;
    }

    const baseLower = base.toLowerCase();

    // ── 1. Item code (weapons / armor / misc).
    if ((hasWeapons && lookupKey("weapons", "code", base))
     || (hasArmor   && lookupKey("armor",   "code", base))
     || (hasMisc    && lookupKey("misc",    "code", base))) {
        return null;
    }

    // ── 2. Treasure Class name from another row ABOVE the current row.
    if (baseLower in tcLineMap) {
        const defLine = tcLineMap[baseLower];
        if (defLine >= currentLine) {
            return {
                message: `'${base}' is a treasure class defined at or below the current row`
                    + ` (line ${defLine + 1}); TC references must point upward`,
                span: parsed.base,
            };
        }
        return null;
    }

    // ── 3. Auto-generated TC: itemtype code (with TreasureClass=1), optionally
    //        followed by a positive integer level suffix (e.g. "weap", "weap3").
    if (autoTcCodes.size > 0) {
        if (autoTcCodes.has(baseLower)) return null;
        // Concatenated suffix: "weap3" → code="weap", level="3"
        const concatMatch = /^([a-z]+)([1-9][0-9]*)$/.exec(baseLower);
        if (concatMatch && autoTcCodes.has(concatMatch[1])) return null;
    }

    // ── 4. uniqueitems index.
    if (hasUnique && lookupKey("uniqueitems", "index", base)) return null;

    // ── 5. setitems index.
    if (hasSetitems && lookupKey("setitems", "index", base)) return null;

    // ── If no external item-source files are loaded at all we cannot determine
    // whether this is a valid item code or unique/set index, so skip rather than
    // emit a false positive.
    if (!canProveExternalItemInvalid) return null;

    return {
        message: `'${base}' is not a valid item code, treasure class, auto-TC, unique index, or set item index`,
        span: parsed.base,
    };
}

// ─── hover helpers ──────────────────────────────────────────────────────────

// Case-insensitive row value lookup. ctx.row keys come from raw file headers
// whose casing may differ from what we construct (e.g. "Item1" vs "item1").
function rowGet(row: Record<string, string>, key: string): string {
    const lower = key.toLowerCase();
    const found = Object.keys(row).find(function(k) { return k.toLowerCase() === lower; });
    return found ? (row[found] || "") : "";
}

// Resolve an item base token to "token\n\nHuman Name" if a name is found in
// the workspace, or null if the token is an unrecognised TC/unique/set reference.
function resolveItemName(base: string): string | null {
    const names = getFilteredColumnValues("weapons", "name", "code", base).concat(
        getFilteredColumnValues("armor",   "name", "code", base),
        getFilteredColumnValues("misc",    "name", "code", base),
    );
    if (names.length > 0) return base + "\n\n" + names[0];

    const typeNames = getFilteredColumnValues("itemtypes", "ItemType", "Code", base);
    if (typeNames.length > 0) return base + "\n\n" + typeNames[0] + " (Item Type)";

    const concatMatch = /^([a-zA-Z]+)([1-9][0-9]*)$/.exec(base);
    if (concatMatch) {
        const autoTypeNames = getFilteredColumnValues("itemtypes", "ItemType", "Code", concatMatch[1]);
        if (autoTypeNames.length > 0) {
            return base + "\n\n" + autoTypeNames[0] + " (Level " + concatMatch[2] + " TC)";
        }
    }

    return null;
}

// ─── hover ──────────────────────────────────────────────────────────────────
// Handles both Item# and Prob# columns in TreasureClassEx.txt.
//
// Item# — shows the resolved item name plus the per-slot drop chance.
// Prob# — shows the paired Item#'s resolved name plus the drop chance.
//
// Chance formula (only when Picks > 0):
//   total        = NoDrop + Prob1 + … + Prob10
//   per_roll     = ProbX / total
//   at_least_one = 1 − (1 − per_roll)^Picks   (shown only when Picks > 1)
function hover(ctx: HoverContext): HoverResult | null {
    if (ctx.file !== "treasureclassex") return null;

    const colLower = ctx.col.toLowerCase();
    const itemMatch = /^item(\d+)$/.exec(colLower);
    const probMatch = /^prob(\d+)$/.exec(colLower);
    if (!itemMatch && !probMatch) return null;

    const idx = (itemMatch ?? probMatch)[1];

    // Locate the raw item token for this slot.
    // ItemX: the hovered cell itself.  ProbX: the paired Item{idx} cell.
    // rowGet is used for cross-column lookups to tolerate header casing differences.
    let rawItem = itemMatch
        ? (ctx.value || "").trim()
        : rowGet(ctx.row, "Item" + idx).trim();
    if (rawItem.startsWith('"') && rawItem.endsWith('"')) {
        rawItem = rawItem.slice(1, -1).trim();
    }
    const [base] = splitModifiers(rawItem);
    if (!base) return null;

    // Attempt to resolve the token to a human-readable name.
    // Fall back to the raw token so chance info always has a header.
    const nameContent = resolveItemName(base) ?? base;

    // ── Chance calculation ─────────────────────────────────────────────────
    let chanceContent: string | null = null;

    const picksRaw = rowGet(ctx.row, "Picks");
    const picks = picksRaw.trim() ? parseInt(picksRaw.trim(), 10) : 1;

    if (!isNaN(picks) && picks > 0) {
        const probRaw = probMatch
            ? (ctx.value || "").trim()
            : rowGet(ctx.row, "Prob" + idx).trim();
        const probVal = parseInt(probRaw, 10);

        if (!isNaN(probVal) && probVal > 0) {
            let total = 0;
            for (let i = 1; ; i++) {
                if (!getColumn("treasureclassex", "Prob" + i)) break;
                const v = rowGet(ctx.row, "Prob" + i);
                if (v && v.trim()) {
                    const p = parseInt(v.trim(), 10);
                    if (!isNaN(p)) total += p;
                }
            }
            const noDropRaw = rowGet(ctx.row, "NoDrop");
            if (noDropRaw.trim()) {
                const nd = parseInt(noDropRaw.trim(), 10);
                if (!isNaN(nd)) total += nd;
            }

            if (total > 0) {
                const perRoll = probVal / total;
                const atLeastOnce = 1 - Math.pow(1 - perRoll, picks);

                const fmt = (x: number) => {
                    let s = (x * 100).toFixed(4);
                    s = s.replace(/\.?0+$/, "");
                    return s + "%";
                };

                if (picks > 1) {
                    chanceContent = "Per-roll chance: " + fmt(perRoll) + " (" + probVal + " / " + total + ")"
                        + "\nAt least once in " + picks + " picks: **" + fmt(atLeastOnce) + "**";
                } else {
                    chanceContent = "Chance: **" + fmt(perRoll) + "** (" + probVal + " / " + total + ")";
                }
            }
        }
    }

    const parts: string[] = [nameContent];
    if (chanceContent) { parts.push(""); parts.push(chanceContent); }
    return { content: parts.join("\n") };
}

// ─── gotoDefinition ───────────────────────────────────────────────────────────
function gotoDefinition(ctx: GotoDefinitionContext) : GotoDefinitionTarget | null {
    if (ctx.file !== "treasureclassex") return null;
    if (!ctx.col.toLocaleLowerCase().startsWith("item")) return null;

    if (lookupKey("weapons", "code", ctx.value))
        return { targetFile: "weapons", targetCol: "code", targetValue: ctx.value };
    if (lookupKey("armor", "code", ctx.value))
        return { targetFile: "armor", targetCol: "code", targetValue: ctx.value };
    if (lookupKey("misc", "code", ctx.value))
        return { targetFile: "misc", targetCol: "code", targetValue: ctx.value };
    if (lookupKey("uniqueitems", "index", ctx.value))
        return { targetFile: "uniqueitems", targetCol: "index", targetValue: ctx.value };
    if (lookupKey("setitems", "index", ctx.value))
        return { targetFile: "setitems", targetCol: "index", targetValue: ctx.value };
    if (lookupKey("treasureclassex", "treasure class", ctx.value))
        return { targetFile: "treasureclassex", targetCol: "treasure class", targetValue: ctx.value };

    const autoTcCodes = new Set(
        getFilteredColumnValues("itemtypes", "Code", "TreasureClass", "1")
            .map((c: string) => c.toLowerCase())
    );
    const concatMatch = /^([a-z]+)([1-9][0-9]*)$/.exec(ctx.value.toLocaleLowerCase());
    if (concatMatch && autoTcCodes.has(concatMatch[1])) {
        return { targetFile: "itemtypes", targetCol: "Code", targetValue: concatMatch[1] };
    }

    return null;
}
