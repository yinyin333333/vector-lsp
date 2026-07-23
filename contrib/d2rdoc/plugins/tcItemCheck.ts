/// <reference path="../../vector-lsp-plugin.d.ts" />

const pluginMetadata: PluginMetadata = {
    validateFiles: ["treasureclassex"],
    hoverFiles: ["treasureclassex"],
    gotoDefinitionFiles: ["treasureclassex"],
};

// Validates Item# fields in TreasureClassEx.txt.
//
// Each Item# cell has the form:   BASE[,KEY=VALUE[,KEY=VALUE...]]
//
// BASE must be one of:
//   1. An item code (weapons/armor/misc "code" column).
//   2. A Treasure Class name already inserted by the sequential loader.  The
//      current row is inserted before Item# parsing, so self-reference works
//      while a class first defined below the current row does not.
//      This named map is pre-populated by an earlier equipment-TC generation
//      stage for ItemTypes rows whose TreasureClass value is nonzero.  It adds
//      the finite level suffixes 3, 6, ... 96; the Item# parser itself does not
//      perform a broad ItemTypes lookup.
//   3. A uniqueitems "index" value.
//   4. A setitems "index" value.
//
// Valid modifier keys after the comma: mul cu cs cr cm ce cg ma mg

// ─── Constants ────────────────────────────────────────────────────────────────

const VALID_MOD_KEYS: Record<string, true> = {
    mul: true, cu: true, cs: true, cr: true,
    cm: true, ce: true, cg: true, ma: true, mg: true,
};

function asciiLower(value: string): string {
    return value.replace(/[A-Z]/g, (ch: string) => String.fromCharCode(ch.charCodeAt(0) + 32));
}

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
    raw: string;
    base: TextSpan;
    modifiers: ModifierSpan[];
    ignoredSuffix: TextSpan | null;
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

// Parse exact comma-separated bytes. Recognized lower-case key=value tokens
// form a prefix; the first non-matching token begins the ignored suffix.
function rawTextSpan(raw: string, start: number, end: number): TextSpan {
    return { text: raw.slice(start, end), start, end };
}

function normalizeItemSpan(raw: string): TextSpan {
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
        key: rawTextSpan(raw, span.start, eqStart),
        value: rawTextSpan(raw, eqStart + 1, span.end),
        equals: { text: "=", start: eqStart, end: eqStart + 1 },
    };
}

function parseItemWithSpans(raw: string): ParsedItemWithSpans {
    const normalized = normalizeItemSpan(raw);
    const parts = splitCommaSpans(raw, normalized);
    const base = parts[0] ?? { text: "", start: normalized.start, end: normalized.start };
    const modifiers: ModifierSpan[] = [];
    let ignoredSuffix: TextSpan | null = null;

    for (let index = 1; index < parts.length; index++) {
        const token = parseModifierSpan(raw, parts[index]);
        if (!token.key || !token.value || !token.equals
            || !VALID_MOD_KEYS[token.key.text] || token.value.text === "") {
            ignoredSuffix = {
                text: raw.slice(parts[index].start, normalized.end),
                start: parts[index].start,
                end: normalized.end,
            };
            break;
        }
        modifiers.push(token);
    }

    return { raw, base, modifiers, ignoredSuffix };
}

function isUnsigned16(value: string): boolean {
    if (!/^[0-9]+$/.test(value)) return false;
    const canonical = value.replace(/^0+/, "") || "0";
    return canonical.length < 5 || (canonical.length === 5 && canonical <= "65535");
}

function storedUnsigned16(value: string): number {
    if (!/^-?\d+$/.test(value)) return 0;
    const negative = value[0] === "-";
    const digits = negative ? value.slice(1) : value;
    let stored = 0;
    for (const digit of digits) {
        stored = (stored * 10 + Number(digit)) % 65536;
    }
    return negative && stored !== 0 ? 65536 - stored : stored;
}

function utf8ByteLength(value: string): number {
    let bytes = 0;
    for (let index = 0; index < value.length; index++) {
        const code = value.charCodeAt(index);
        if (code <= 0x7f) {
            bytes += 1;
        } else if (code <= 0x7ff) {
            bytes += 2;
        } else if (code >= 0xd800 && code <= 0xdbff
            && index + 1 < value.length
            && value.charCodeAt(index + 1) >= 0xdc00
            && value.charCodeAt(index + 1) <= 0xdfff) {
            bytes += 4;
            index++;
        } else {
            bytes += 3;
        }
    }
    return bytes;
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
    // This parser only attempts the raw item-code path for bases of at most
    // four characters.  Longer strings proceed to the CI name maps.
    if (utf8ByteLength(value) > 4) return null;
    const packed = fixed4ByteKey(value);
    for (const file of ["weapons", "armor", "misc"]) {
        if (!lookupKeyFixed4(file, "code", value)) continue;
        for (const candidate of getColumnValues(file, "code")) {
            if (fixed4ByteKey(candidate) === packed) return [file, candidate];
        }
    }
    return null;
}

type ItemBaseState = "valid" | "forward" | "invalid" | "unknown";

function nonzeroTreasureClassFlag(value: string): boolean {
    if (!/^-?[0-9]+$/.test(value)) return false;
    return Number(value) !== 0;
}

function generatedTcBaseCodes(): string[] {
    if (!hasLookupTarget("itemtypes", "Code")
        || !hasLookupTarget("itemtypes", "TreasureClass")) return [];

    const values = getColumnValues("itemtypes", "TreasureClass");
    const seenValues = new Set();
    const codes: string[] = [];
    for (const value of values) {
        if (seenValues.has(value) || !nonzeroTreasureClassFlag(value)) continue;
        seenValues.add(value);
        codes.push(...getFilteredColumnValues("itemtypes", "Code", "TreasureClass", value));
    }
    return codes;
}

function generatedTcNames(): Set<string> {
    const names = new Set();
    for (const code of generatedTcBaseCodes()) {
        for (let level = 3; level <= 96; level += 3) {
            names.add(asciiLower(code + level));
        }
    }
    return names;
}

function itemBaseState(
    base: string,
    currentLine: number,
    tcLineMap: Record<string, number>,
    generatedNames: Set<string>,
    hasWeapons: boolean,
    hasArmor: boolean,
    hasMisc: boolean,
    hasUnique: boolean,
    hasSetitems: boolean,
    canProveExternalItemInvalid: boolean,
): ItemBaseState {
    const baseLower = asciiLower(base);

    // Raw item codes take the exact fixed-4CC path only for short bases.
    // Longer values proceed to the case-insensitive named namespaces.
    if (utf8ByteLength(base) <= 4 && (
        (hasWeapons && lookupKeyFixed4("weapons", "code", base))
        || (hasArmor && lookupKeyFixed4("armor", "code", base))
        || (hasMisc && lookupKeyFixed4("misc", "code", base))
    )) return "valid";

    if (Object.prototype.hasOwnProperty.call(tcLineMap, baseLower)) {
        return tcLineMap[baseLower] > currentLine ? "forward" : "valid";
    }

    if (generatedNames.has(baseLower)) return "valid";
    if (hasUnique && lookupKey("uniqueitems", "index", base)) return "valid";
    if (hasSetitems && lookupKey("setitems", "index", base)) return "valid";
    return canProveExternalItemInvalid ? "invalid" : "unknown";
}

function actualHeader(headers: string[], wanted: string): string | null {
    const lower = asciiLower(wanted);
    return headers.find((header) => asciiLower(header) === lower) ?? null;
}

function cleanProbability(value: string): string {
    const trimmed = value.trim();
    if (trimmed.length >= 2 && trimmed[0] === '"' && trimmed[trimmed.length - 1] === '"') {
        return trimmed.slice(1, -1).trim();
    }
    return trimmed;
}

function pushWholeCell(
    diags: PluginDiagnostic[],
    row: WorkspaceRow,
    col: string,
    raw: string,
    code: string,
    message: string,
    extraArgs: Record<string, string | number | boolean> = {},
): void {
    const start = row.__colstarts[col] ?? 0;
    diags.push({
        line: row.__line,
        col: start,
        endCol: start + raw.length,
        severity: "warning",
        code,
        messageKey: "plugin." + code,
        messageArgs: {
            line: row.__line + 1,
            column: col,
            value: raw,
            treasureClass: rowGet(row as Record<string, string>, "Treasure Class"),
            ...extraArgs,
        },
        legacyMessage: message,
    });
}

function validate(ctx: PluginContext): PluginDiagnostic[] {
    if (ctx.file !== "treasureclassex") return [];

    // Store only the first TC occurrence.  The current row is already inserted
    // when Item# is parsed; a first definition below remains a forward miss.
    const tcLineMap: Record<string, number> = {};
    for (const row of ctx.rows) {
        const name = rowGet(row as Record<string, string>, "Treasure Class");
        if (!name) continue;
        const key = asciiLower(name);
        if (!Object.prototype.hasOwnProperty.call(tcLineMap, key)) tcLineMap[key] = row.__line;
    }

    const canProveExternalItemInvalid = tcItemTargetsAvailable();
    const generatedNames = canProveExternalItemInvalid ? generatedTcNames() : new Set();
    const hasWeapons = hasLookupTarget("weapons", "code");
    const hasArmor = hasLookupTarget("armor", "code");
    const hasMisc = hasLookupTarget("misc", "code");
    const hasUnique = hasLookupTarget("uniqueitems", "index");
    const hasSetitems = hasLookupTarget("setitems", "index");

    const slots: { item: string | null; prob: string | null; itemLabel: string; probLabel: string }[] = [];
    for (let n = 1; n <= 10; n++) {
        const item = actualHeader(ctx.headers, "Item" + n);
        slots.push({
            item,
            prob: actualHeader(ctx.headers, "Prob" + n),
            itemLabel: "item" + n,
            probLabel: "prob" + n,
        });
    }

    const diags: PluginDiagnostic[] = [];
    for (const row of ctx.rows) {
        const className = rowGet(row as Record<string, string>, "Treasure Class");
        let terminated = false;

        for (const slot of slots) {
            if (!slot.item) {
                terminated = true;
                continue;
            }
            const raw = rowGet(row as Record<string, string>, slot.item);
            const parsed = parseItemWithSpans(raw);
            const itemPresent = normalizeItemSpan(raw).text !== "";
            const probability = slot.prob
                ? cleanProbability(rowGet(row as Record<string, string>, slot.prob))
                : "";

            if (terminated) {
                if (itemPresent) {
                    pushWholeCell(
                        diags, row, slot.item, raw, "tc-item.after-first-gap",
                        `${slot.itemLabel} is ignored because the first empty Item slot`
                            + " already ended this treasure class.",
                    );
                }
                if (slot.prob && probability) {
                    const probRaw = rowGet(row as Record<string, string>, slot.prob);
                    pushWholeCell(
                        diags, row, slot.prob, probRaw, "tc-prob.after-first-gap",
                        `${slot.probLabel} is ignored because the first empty Item slot`
                            + " already ended this treasure class.",
                    );
                }
                continue;
            }

            if (!itemPresent || !parsed.base.text) {
                terminated = true;
                if (slot.prob && probability) {
                    const probRaw = rowGet(row as Record<string, string>, slot.prob);
                    pushWholeCell(
                        diags, row, slot.prob, probRaw, "tc-prob.orphaned",
                        `${slot.probLabel} is orphaned and ignored because`
                            + ` ${slot.itemLabel} is empty.`,
                    );
                }
                continue;
            }

            const state = itemBaseState(
                parsed.base.text, row.__line, tcLineMap, generatedNames,
                hasWeapons, hasArmor, hasMisc, hasUnique, hasSetitems,
                canProveExternalItemInvalid,
            );
            if (state === "forward" || state === "invalid") {
                pushWholeCell(
                    diags, row, slot.item, raw,
                    state === "forward" ? "tc-item.forward-reference" : "tc-item.unresolved-base",
                    `treasureclassex.txt, line ${row.__line + 1}: can't find '${parsed.base.text}'`
                        + ` for '${slot.itemLabel}' in TC '${className}'`,
                );
                continue;
            }
            if (state === "unknown") continue;

            if (slot.prob) {
                const probRaw = rowGet(row as Record<string, string>, slot.prob);
                if (!probability) {
                    pushWholeCell(
                        diags, row, slot.prob, probRaw, "tc-prob.blank-omission",
                        `treasureclassex.txt, line ${row.__line + 1}: This Treasure Class entry is skipped`
                            + ` because '${slot.probLabel}' is blank ('${slot.itemLabel}').`,
                    );
                } else if (!/^-?\d+$/.test(probability)) {
                    pushWholeCell(
                        diags, row, slot.prob, probRaw, "tc-prob.noncanonical",
                        `treasureclassex.txt, line ${row.__line + 1}: '${slot.probLabel}'`
                            + ` is not a whole number and may cause '${slot.itemLabel}' to be skipped.`,
                        { itemColumn: slot.itemLabel },
                    );
                } else if (Number(probability) <= 0) {
                    pushWholeCell(
                        diags, row, slot.prob, probRaw, "tc-prob.nonpositive-omission",
                        `treasureclassex.txt, line ${row.__line + 1}: This Treasure Class entry is skipped`
                            + ` because '${slot.probLabel}' is ${probability} ('${slot.itemLabel}').`,
                    );
                }
            }

            for (const modifier of parsed.modifiers) {
                const parameter = modifier.value?.text ?? "";
                if (isUnsigned16(parameter)) continue;
                const stored = storedUnsigned16(parameter);
                pushWholeCell(
                    diags, row, slot.item, raw, "tc-item.modifier-range",
                    `treasureclassex.txt, line ${row.__line + 1}: Modifier '${modifier.text}'`
                        + ` for '${slot.itemLabel}' in TC '${className}'`
                        + ` is outside 0..65535. The game converts it to ${stored}. Replace it with the number you actually want.`,
                    { itemColumn: slot.itemLabel, modifier: modifier.text, stored },
                );
            }

            if (parsed.ignoredSuffix) {
                const stoppedAt = parsed.ignoredSuffix.text.split(",")[0] || "(empty modifier)";
                pushWholeCell(
                    diags, row, slot.item, raw, "tc-item.ignored-suffix",
                    `treasureclassex.txt, line ${row.__line + 1}: The game stops at '${stoppedAt}'`
                        + ` for '${slot.itemLabel}' in TC '${className}'. The base and modifiers before it still work;`
                        + ` '${stoppedAt}' and everything after it are ignored.`,
                    { itemColumn: slot.itemLabel, stoppedAt },
                );
            }

            if (utf8ByteLength(parsed.raw) >= 64) {
                pushWholeCell(
                    diags, row, slot.item, raw, "tc-item.field-width",
                    `treasureclassex.txt, line ${row.__line + 1}: '${slot.itemLabel}'`
                        + ` in TC '${className}' is too long. Keep the value under 64 UTF-8 bytes.`,
                    { itemColumn: slot.itemLabel, utf8ByteLength: utf8ByteLength(parsed.raw) },
                );
            }
        }
    }

    return diags;
}

// Case-insensitive row value lookup. ctx.row keys come from raw file headers
// whose casing may differ from what we construct (e.g. "Item1" vs "item1").
function rowGet(row: Record<string, string>, key: string): string {
    const lower = asciiLower(key);
    const found = Object.keys(row).find(function(k) { return asciiLower(k) === lower; });
    return found ? (row[found] || "") : "";
}

// Resolve an item base token to "token\n\nHuman Name" if a name is found in
// the workspace, or null if the token is an unrecognised TC/unique/set reference.
function generatedTcCode(base: string): string | null {
    const lower = asciiLower(base);
    for (const code of generatedTcBaseCodes()) {
        for (let level = 3; level <= 96; level += 3) {
            if (asciiLower(code + level) === lower) return code;
        }
    }
    return null;
}

function resolvesSequentialTc(base: string, rowLine: number): boolean {
    const firstLine = getFirstColumnValueLine("treasureclassex", "Treasure Class", base);
    return firstLine !== null && firstLine <= rowLine;
}

function resolveItemName(base: string, rowLine: number): string | null {
    const packedTarget = findPackedItemTarget(base);
    const targetValue = packedTarget ? packedTarget[1] : "";
    const names = targetValue
        ? getFilteredColumnValues("weapons", "name", "code", targetValue).concat(
            getFilteredColumnValues("armor", "name", "code", targetValue),
            getFilteredColumnValues("misc", "name", "code", targetValue),
        )
        : [];
    if (names.length > 0) return base + "\n\n" + names[0];

    const generatedCode = generatedTcCode(base);
    if (generatedCode) {
        const typeNames = getFilteredColumnValues("itemtypes", "ItemType", "Code", generatedCode);
        if (typeNames.length > 0) {
            return base + "\n\n" + typeNames[0] + " (Generated Treasure Class)";
        }
        return base + "\n\nGenerated Treasure Class from " + generatedCode;
    }

    if (lookupKey("uniqueitems", "index", base)) return base + "\n\nUnique item name";
    if (lookupKey("setitems", "index", base)) return base + "\n\nSet item name";
    if (resolvesSequentialTc(base, rowLine)) return base + "\n\nTreasure Class";

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

    const colLower = asciiLower(ctx.col);
    const itemMatch = /^item(\d+)$/.exec(colLower);
    const probMatch = /^prob(\d+)$/.exec(colLower);
    if (!itemMatch && !probMatch) return null;

    const idx = (itemMatch ?? probMatch)[1];

    // Locate the raw item token for this slot.
    // ItemX: the hovered cell itself.  ProbX: the paired Item{idx} cell.
    // rowGet is used for cross-column lookups to tolerate header casing differences.
    const rawItem = itemMatch
        ? (ctx.value || "")
        : rowGet(ctx.row, "Item" + idx);
    const parsedItem = parseItemWithSpans(rawItem);
    const base = parsedItem.base.text;
    if (!base) return null;

    // Attempt to resolve the token to a human-readable name.
    // Fall back to the raw token so chance info always has a header.
    const nameContent = resolveItemName(base, ctx.rowLine) ?? base;

    for (let i = 1; i < Number(idx); i++) {
        if (!rowGet(ctx.row, "Item" + i)) {
            return {
                contentKey: "plugin.treasure-class.hover-after-gap",
                contentArgs: { base, slot: idx, firstEmptySlot: i },
                legacyContent: nameContent + "\n\nThe game ignores this entry because Item" + i
                    + " is the first empty Item# slot.",
            };
        }
    }

    // ── Chance calculation ─────────────────────────────────────────────────
    let chanceContent: string | null = null;
    let perRollChance = "—";
    let atLeastOnceChance = "—";

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
                if (!rowGet(ctx.row, "Item" + i)) break;
                const v = rowGet(ctx.row, "Prob" + i);
                if (v && v.trim()) {
                    const p = parseInt(v.trim(), 10);
                    if (!isNaN(p) && p > 0) total += p;
                }
            }
            const noDropRaw = rowGet(ctx.row, "NoDrop");
            if (noDropRaw.trim()) {
                const nd = parseInt(noDropRaw.trim(), 10);
                if (!isNaN(nd) && nd > 0) total += nd;
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
                    perRollChance = fmt(perRoll);
                    atLeastOnceChance = fmt(atLeastOnce);
                    chanceContent = "Per-roll chance: " + fmt(perRoll) + " (" + probVal + " / " + total + ")"
                        + "\nAt least once in " + picks + " picks: **" + fmt(atLeastOnce) + "**";
                } else {
                    perRollChance = fmt(perRoll);
                    chanceContent = "Chance: **" + fmt(perRoll) + "** (" + probVal + " / " + total + ")";
                }
            }
        }
    }

    const parts: string[] = [nameContent];
    if (parsedItem.modifiers.length > 0) {
        parts.push("", "*Modifiers the game will use:*");
        for (const modifier of parsedItem.modifiers) {
            const parameter = modifier.value?.text ?? "";
            const conversion = isUnsigned16(parameter)
                ? ""
                : " (outside 0..65535; the game converts it to " + storedUnsigned16(parameter)
                    + ". Replace it with the number you actually want)";
            parts.push("- " + modifier.text + conversion);
        }
    }
    if (parsedItem.ignoredSuffix) {
        const stoppedAt = parsedItem.ignoredSuffix.text.split(",")[0] || "(empty modifier)";
        parts.push("", "Ignored text begins at: `" + stoppedAt + "`");
        parts.push("The base and modifiers before it still work; `" + stoppedAt
            + "` and everything after it are ignored.");
    }
    if (utf8ByteLength(parsedItem.raw) >= 64) {
        parts.push("", "Warning: this Item# value uses 64 or more UTF-8 bytes.");
    }
    if (chanceContent) { parts.push(""); parts.push(chanceContent); }
    return {
        contentKey: "plugin.treasure-class.hover",
        contentArgs: {
            base,
            slot: idx,
            name: resolveItemName(base, ctx.rowLine) || "",
            modifiers: JSON.stringify(parsedItem.modifiers.map((modifier) => modifier.text)),
            modifierStorage: JSON.stringify(parsedItem.modifiers.map((modifier) => {
                const parameter = modifier.value?.text ?? "";
                return isUnsigned16(parameter)
                    ? modifier.text
                    : modifier.text + " → " + storedUnsigned16(parameter);
            })),
            ignoredSuffix: parsedItem.ignoredSuffix?.text || "",
            fieldTooLong: utf8ByteLength(parsedItem.raw) >= 64,
            utf8ByteLength: utf8ByteLength(parsedItem.raw),
            picks,
            probability: probMatch ? (ctx.value || "").trim() : rowGet(ctx.row, "Prob" + idx).trim(),
            perRollChance,
            atLeastOnceChance,
        },
        legacyContent: parts.join("\n"),
    };
}

// ─── gotoDefinition ───────────────────────────────────────────────────────────
function gotoDefinition(ctx: GotoDefinitionContext) : GotoDefinitionTarget | null {
    if (ctx.file !== "treasureclassex") return null;
    if (!/^item\d+$/i.test(ctx.col)) return null;

    const base = parseItemWithSpans(ctx.value || "").base.text;
    if (!base) return null;

    const packedTarget = findPackedItemTarget(base);
    if (packedTarget) {
        return { targetFile: packedTarget[0], targetCol: "code", targetValue: packedTarget[1] };
    }

    const generatedCode = generatedTcCode(base);
    if (generatedCode) {
        return { targetFile: "itemtypes", targetCol: "Code", targetValue: generatedCode };
    }

    if (lookupKey("uniqueitems", "index", base))
        return { targetFile: "uniqueitems", targetCol: "index", targetValue: base };
    if (lookupKey("setitems", "index", base))
        return { targetFile: "setitems", targetCol: "index", targetValue: base };
    if (resolvesSequentialTc(base, ctx.rowLine))
        return { targetFile: "treasureclassex", targetCol: "treasure class", targetValue: base };

    return null;
}
