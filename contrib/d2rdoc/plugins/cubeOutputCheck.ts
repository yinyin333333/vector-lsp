/// <reference path="../../vector-lsp-plugin.d.ts" />

const pluginMetadata: PluginMetadata = {
    validateFiles: ["cubemain"],
    hoverFiles: ["cubemain"],
};

// Validates the three CubeMain output slots.  The D2R output callback parses
// output/output b/output c independently and preserves a successfully parsed
// base plus all modifiers before the first unrecognised suffix.

const OUTPUT_COLUMNS: string[] = ["output", "output b", "output c"];
const OUTPUT_BYTE_COLUMNS: Record<string, string> = {
    lvl: "output",
    plvl: "output",
    ilvl: "output",
    "b lvl": "output b",
    "b plvl": "output b",
    "b ilvl": "output b",
    "c lvl": "output c",
    "c plvl": "output c",
    "c ilvl": "output c",
};

const SIMPLE_OUTPUT_MODS: Record<string, string> = {
    low: "Low quality",
    nor: "Normal quality",
    hiq: "Superior quality",
    mag: "Magic quality",
    set: "Set quality",
    rar: "Rare quality",
    uni: "Unique quality",
    crf: "Crafted quality",
    tmp: "Tempered quality",
    eth: "Ethereal",
    mod: "Copy input modifiers",
    uns: "Empty sockets (destroy contents)",
    rem: "Remove socket contents",
    reg: "Regenerate unique",
    exc: "Exceptional tier",
    eli: "Elite tier",
    rep: "Repair",
    rch: "Recharge",
};

const PARAM_OUTPUT_MODS: Record<string, string> = {
    qty: "Quantity",
    pre: "Prefix ID",
    suf: "Suffix ID",
    sock: "Socket count",
    lvl: "Inline level",
};

const U8_OUTPUT_MODS: Record<string, true> = {
    qty: true,
    sock: true,
    lvl: true,
};

const PORTAL_OUTPUTS: Record<string, string> = {
    "cow portal": "Cow Portal",
    "red portal": "Red Portal",
    "pandemonium portal": "Pandemonium Portal",
    "pandemonium finale portal": "Pandemonium Finale Portal",
};

interface OutputSpan {
    text: string;
    start: number;
    end: number;
}

interface AppliedOutputModifier {
    key: string;
    value: string | null;
    span: OutputSpan;
    noncanonical: boolean;
    outsideU8: boolean;
}

interface IgnoredOutputSuffix {
    span: OutputSpan;
    text: string;
    reason: string;
}

interface ParsedOutput {
    normalized: OutputSpan;
    base: OutputSpan;
    modifiers: AppliedOutputModifier[];
    ignored: IgnoredOutputSuffix | null;
}

type OutputBaseKind = "useitem" | "usetype" | "portal" | "item" | "itemtype" | "unique" | "set";

interface OutputLookupSets {
    itemCodes: Set<string>;
    itemTypes: Set<string>;
    uniqueNames: Set<string>;
    setNames: Set<string>;
    propertyCodes: Set<string>;
}

function asciiLower(value: string): string {
    return value.replace(/[A-Z]/g, (ch: string) => String.fromCharCode(ch.charCodeAt(0) + 32));
}

function utf8Bytes(value: string): number[] {
    const bytes: number[] = [];
    for (let index = 0; index < value.length; index++) {
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
    return bytes;
}

function fixed4ByteKey(value: string): string {
    const bytes = utf8Bytes(value);
    while (bytes.length < 4) bytes.push(0x20);
    return bytes.slice(0, 4).map((byte: number) => byte.toString(16).padStart(2, "0")).join("");
}

function owns(map: Record<string, unknown>, key: string): boolean {
    return Object.prototype.hasOwnProperty.call(map, key);
}

function normalizeOutput(raw: string): OutputSpan {
    if (raw.length >= 2 && raw[0] === "\"" && raw[raw.length - 1] === "\"") {
        return { text: raw.slice(1, raw.length - 1), start: 1, end: raw.length - 1 };
    }
    return { text: raw, start: 0, end: raw.length };
}

function splitOutput(raw: string, normalized: OutputSpan): OutputSpan[] {
    const parts: OutputSpan[] = [];
    let start = normalized.start;
    for (let i = normalized.start; i < normalized.end; i++) {
        if (raw[i] === ",") {
            parts.push({ text: raw.slice(start, i), start, end: i });
            start = i + 1;
        }
    }
    parts.push({ text: raw.slice(start, normalized.end), start, end: normalized.end });
    return parts;
}

function visibleOutputSpan(span: OutputSpan, raw: string): OutputSpan {
    if (span.end > span.start) return span;
    if (span.start > 0) {
        return { text: raw.slice(span.start - 1, span.start), start: span.start - 1, end: span.start };
    }
    return { text: raw.slice(0, 1), start: 0, end: Math.min(1, raw.length) };
}

function isIntegerParameter(value: string): boolean {
    return /^-?\d+$/.test(value);
}

function isOutsideU8(value) {
    const number = Number(value);
    return !Number.isFinite(number) || number < 0 || number > 255;
}

function ignoredSuffix(
    raw: string,
    normalized: OutputSpan,
    span: OutputSpan,
    reason: string,
) {
    return {
        span: visibleOutputSpan(span, raw),
        text: raw.slice(span.start, normalized.end),
        reason,
    };
}

function parseOutput(raw: string): ParsedOutput {
    const normalized = normalizeOutput(raw);
    const parts = splitOutput(raw, normalized);
    const base = parts[0] || { text: "", start: normalized.start, end: normalized.start };
    const modifiers: AppliedOutputModifier[] = [];
    let ignored: IgnoredOutputSuffix | null = null;

    for (let i = 1; i < parts.length; i++) {
        const part = parts[i];
        if (!part.text) {
            ignored = ignoredSuffix(raw, normalized, part, "Empty output modifier");
            break;
        }

        const equals = part.text.indexOf("=");
        const key = equals === -1 ? part.text : part.text.slice(0, equals);
        if (owns(SIMPLE_OUTPUT_MODS, key) && equals === -1) {
            modifiers.push({ key, value: null, span: part, noncanonical: false, outsideU8: false });
            continue;
        }

        if (owns(PARAM_OUTPUT_MODS, key)) {
            let value = "";
            let span = part;
            if (equals !== -1) {
                value = part.text.slice(equals + 1);
            } else {
                const valuePart = parts[i + 1];
                if (!valuePart || !valuePart.text) {
                    ignored = ignoredSuffix(
                        raw,
                        normalized,
                        part,
                        `Output modifier '${key}' requires an integer parameter`,
                    );
                    break;
                }
                value = valuePart.text;
                span = { text: raw.slice(part.start, valuePart.end), start: part.start, end: valuePart.end };
                i++;
            }

            if (!value) {
                ignored = ignoredSuffix(
                    raw,
                    normalized,
                    part,
                    `Output modifier '${key}' requires an integer parameter`,
                );
                break;
            }

            const canonical = isIntegerParameter(value);
            modifiers.push({
                key,
                value,
                span,
                noncanonical: !canonical,
                outsideU8: canonical && owns(U8_OUTPUT_MODS, key) && isOutsideU8(value),
            });
            continue;
        }

        const reason = key !== asciiLower(key)
            ? `Output modifiers are lower-case exact; '${key}' is not applied`
            : `Unknown output modifier '${key}'`;
        ignored = ignoredSuffix(raw, normalized, part, reason);
        break;
    }

    return { normalized, base, modifiers, ignored };
}

function outputOrdinal(column: string): number {
    if (column === "output b") return 2;
    if (column === "output c") return 3;
    return 1;
}

function outputTargetsAvailable(): boolean {
    return hasLookupTarget("weapons", "code")
        && hasLookupTarget("armor", "code")
        && hasLookupTarget("misc", "code")
        && hasLookupTarget("itemtypes", "Code")
        && hasLookupTarget("uniqueitems", "index")
        && hasLookupTarget("setitems", "index");
}

function outputLookupSets(): OutputLookupSets {
    const propertyFiles = propertyGroupsEnabled()
        ? getColumnValues("properties", "code").concat(getColumnValues("propertygroups", "code"))
        : getColumnValues("properties", "code");
    return {
        itemCodes: new Set(
            getColumnValues("weapons", "code")
                .concat(getColumnValues("armor", "code"))
                .concat(getColumnValues("misc", "code"))
                .map(fixed4ByteKey),
        ),
        itemTypes: new Set(getColumnValues("itemtypes", "Code").map(fixed4ByteKey)),
        uniqueNames: new Set(getColumnValues("uniqueitems", "index").map(asciiLower)),
        setNames: new Set(getColumnValues("setitems", "index").map(asciiLower)),
        propertyCodes: new Set(propertyFiles.map(asciiLower)),
    };
}

function resolveOutputBase(base: string, sets: OutputLookupSets): OutputBaseKind | null {
    if (base === "useitem") return "useitem";
    if (base === "usetype") return "usetype";
    if (owns(PORTAL_OUTPUTS, asciiLower(base))) return "portal";

    // The raw-code branch only runs for bases that fit four UTF-8 bytes.  Its
    // identity is then the exact first four bytes with space padding.
    if (utf8Bytes(base).length <= 4) {
        const packed = fixed4ByteKey(base);
        if (sets.itemCodes.has(packed)) return "item";
        if (sets.itemTypes.has(packed)) return "itemtype";
    }

    // Named unique/set lookups are ASCII case-insensitive and preserve all
    // whitespace bytes.
    const folded = asciiLower(base);
    if (sets.uniqueNames.has(folded)) return "unique";
    if (sets.setNames.has(folded)) return "set";
    return null;
}

function resolvedOutputSourceStem(base: string, kind: OutputBaseKind | null): string | null {
    if (kind === "item" && utf8Bytes(base).length <= 4) {
        if (lookupKeyFixed4("weapons", "code", base)) return "weapons";
        if (lookupKeyFixed4("armor", "code", base)) return "armor";
        if (lookupKeyFixed4("misc", "code", base)) return "misc";
    }
    if (kind === "itemtype" && lookupKeyFixed4("itemtypes", "Code", base)) return "itemtypes";
    if (kind === "unique" && lookupKey("uniqueitems", "index", base)) return "uniqueitems";
    if (kind === "set" && lookupKey("setitems", "index", base)) return "setitems";
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

function activeCubeRow(row: WorkspaceRow): boolean {
    const enabled = row["enabled"];
    if (enabled === undefined) return true;
    const value = String(enabled);
    return value !== "" && value !== "0";
}

function cubeOutputDiagnostic(
    row: WorkspaceRow,
    column: string,
    span: OutputSpan,
    severity: "error" | "warning",
    message: string,
    code: string,
    kind: string,
): PluginDiagnostic {
    const start = (row.__colstarts[column] || 0) + span.start;
    return {
        line: row.__line,
        col: start,
        endCol: (row.__colstarts[column] || 0) + Math.max(span.end, span.start + 1),
        severity,
        message,
        code,
        data: { rule: "cubeOutputCheck", kind },
    };
}

function validateOutputCell(
    row: WorkspaceRow,
    column: string,
    raw: string,
    sets: OutputLookupSets,
    canProveBaseInvalid: boolean,
    diags: PluginDiagnostic[],
) {
    const parsed = parseOutput(raw);
    const wholeCell = { text: raw, start: 0, end: raw.length };
    const description = String(row["description"] ?? "");
    if (!parsed.base.text) {
        diags.push(cubeOutputDiagnostic(
            row,
            column,
            wholeCell,
            "warning",
            `cubemain.txt, line ${row.__line + 1}: empty base for ${column}`
                + ` in recipe '${description}'`,
            "cube-output.invalid-base",
            "invalid-base",
        ));
        return;
    }
    const kind = resolveOutputBase(parsed.base.text, sets);

    if (kind === null) {
        if (canProveBaseInvalid) {
            diags.push(cubeOutputDiagnostic(
                row,
                column,
                wholeCell,
                "warning",
                `cubemain.txt, line ${row.__line + 1}: could not find '${parsed.base.text}'`
                    + ` for ${column} in recipe '${description}'`,
                "cube-output.invalid-base",
                "invalid-base",
            ));
        }
        // Without a resolved base there is no proven compiled output prefix
        // to preserve.  Do not add storage/suffix diagnostics that would
        // imply otherwise for this cell.
        return;
    }

    if ((kind === "useitem" || kind === "usetype")) {
        const ordinal = outputOrdinal(column);
        const inputColumn = "input " + ordinal;
        const input = row[inputColumn];
        if (input === undefined || !String(input)) {
            diags.push(cubeOutputDiagnostic(
                row,
                column,
                wholeCell,
                "warning",
                `cubemain.txt, line ${row.__line + 1}: '${parsed.base.text}' for '${column}'`
                    + ` has no matching '${inputColumn}' in recipe '${description}'`,
                "cube-output.missing-ordinal-input",
                "missing-ordinal-input",
            ));
        }
    }

    for (const modifier of parsed.modifiers) {
        if ((modifier.noncanonical || modifier.outsideU8) && owns(U8_OUTPUT_MODS, modifier.key)) {
            diags.push(cubeOutputDiagnostic(
                row,
                column,
                wholeCell,
                "warning",
                `cubemain.txt, line ${row.__line + 1}: '${modifier.span.text}' for '${column}'`
                    + ` is outside 0..255, so the game truncates it. Enter a value from 0 through 255`
                    + ` in recipe '${description}'`,
                "cube-output.u8-range",
                "storage-range",
            ));
        }
    }

    if (parsed.ignored) {
        const stoppedAt = parsed.ignored.text.split(",")[0] || "(empty modifier)";
        diags.push(cubeOutputDiagnostic(
            row,
            column,
            wholeCell,
            "warning",
            `cubemain.txt, line ${row.__line + 1}: The game stops at '${stoppedAt}'`
                + ` for '${column}' in recipe '${description}'. The base and modifiers before it still work;`
                + ` '${stoppedAt}' and everything after it are ignored.`,
            "cube-output.ignored-suffix",
            "ignored-suffix",
        ));
    }
}

function validateSeparateOutputBytes(
    row: WorkspaceRow,
    sets: OutputLookupSets,
    diags: PluginDiagnostic[],
) {
    for (const column of Object.keys(OUTPUT_BYTE_COLUMNS)) {
        if (!getColumn("cubemain", column)) continue;
        const outputColumn = OUTPUT_BYTE_COLUMNS[column];
        const outputValue = row[outputColumn];
        if (outputValue === undefined || !String(outputValue)) continue;
        const parsedOutput = parseOutput(String(outputValue));
        // A failed base never reaches the stored output record. If the lookup
        // is incomplete, a null result is unresolved rather than proven bad;
        // suppress both inline and separate-byte claims until it resolves.
        if (resolveOutputBase(parsedOutput.base.text, sets) === null) continue;
        const raw = row[column];
        const value = raw === undefined ? "" : String(raw);
        if (!value) continue;

        const unsignedByte = /^[0-9]+$/.test(value) && Number(value) <= 255;
        if (unsignedByte) continue;

        diags.push(cubeOutputDiagnostic(
            row,
            column,
            { text: value, start: 0, end: value.length },
            "warning",
            `cubemain.txt, line ${row.__line + 1}: '${column}' value '${value}'`
                + ` for '${outputColumn}' is outside 0..255, so the game truncates it.`
                + ` Enter a value from 0 through 255 in recipe '${String(row["description"] ?? "")}'`,
            "cube-output.u8-range",
            "storage-range",
        ));
    }
}

function outputPropertyTargetsAvailable(): boolean {
    return hasLookupTarget("properties", "code")
        && (!propertyGroupsEnabled() || hasLookupTarget("propertygroups", "code"));
}

function propertyGroupsEnabled(): boolean {
    const source = getWorkspaceSource("properties") ?? getWorkspaceSource("propertygroups");
    const version = source?.version;
    return version !== "1.13" && version !== "1.13c" && version !== "2.4";
}

function validateSecondaryOutputProperties(
    row: WorkspaceRow,
    sets: OutputLookupSets,
    diags: PluginDiagnostic[],
) {
    if (!outputPropertyTargetsAvailable()) return;

    // propCodeCheck owns mod 1..5.  Cover the two additional output records
    // here without duplicating diagnostics for the primary output record.
    for (const prefix of ["b mod ", "c mod "]) {
        for (let i = 1; i <= 5; i++) {
            const column = prefix + i;
            if (!getColumn("cubemain", column)) continue;
            const raw = row[column];
            const value = raw === undefined ? "" : String(raw);
            if (!value) continue;
            if (sets.propertyCodes.has(asciiLower(value))) continue;

            const span = { text: value, start: 0, end: value.length };
            diags.push(cubeOutputDiagnostic(
                row,
                column,
                span,
                "warning",
                `cubemain.txt, line ${row.__line + 1}: invalid property '${value}'`
                    + ` for '${column}' in recipe '${String(row["description"] ?? "")}'`,
                "cube-output.invalid-property",
                "invalid-property",
            ));
        }
    }
}

function validate(ctx: PluginContext): PluginDiagnostic[] {
    if (ctx.file !== "cubemain") return [];

    const diags: PluginDiagnostic[] = [];
    const canProveBaseInvalid = outputTargetsAvailable();
    const sets = outputLookupSets();

    for (const row of ctx.rows) {
        if (!activeCubeRow(row)) continue;
        for (const column of OUTPUT_COLUMNS) {
            if (!getColumn("cubemain", column)) continue;
            const value = row[column];
            const raw = value === undefined ? "" : String(value);
            if (!raw) continue;
            validateOutputCell(row, column, raw, sets, canProveBaseInvalid, diags);
        }
        validateSeparateOutputBytes(row, sets, diags);
        validateSecondaryOutputProperties(row, sets, diags);
    }

    return diags;
}

function outputBaseHover(
    base: string,
    kind: OutputBaseKind | null,
    column: string,
    canProveBaseInvalid: boolean,
): string {
    if (kind === "portal") return `**Base:** ${PORTAL_OUTPUTS[asciiLower(base)]} (special portal output)`;
    if (kind === "useitem" || kind === "usetype") {
        const inputColumn = "input " + outputOrdinal(column);
        const action = kind === "useitem" ? "item" : "item type";
        return `**Base:** \`${base}\` uses the ${action} from **${inputColumn}**`;
    }
    if (kind === "unique") return `**Base:** ${base} (Unique Item name; letter case does not matter)`;
    if (kind === "set") return `**Base:** ${base} (Set Item name; letter case does not matter)`;
    if (kind === "itemtype") return `**Base:** \`${base}\` (four-character item-type code; letter case matters)`;
    if (kind === "item") return `**Base:** \`${base}\` (four-character item code; letter case matters)`;
    if (canProveBaseInvalid) {
        return `**Base:** \`${base}\` — Unknown output value. Check the item, item-type, unique, or set name.`;
    }
    return `**Base:** \`${base}\` — This value cannot be checked because the required reference TXT is unavailable.`;
}

function hover(ctx: HoverContext): HoverResult | null {
    if (ctx.file !== "cubemain" || OUTPUT_COLUMNS.indexOf(ctx.col) === -1 || !ctx.value) return null;

    const parsed = parseOutput(ctx.value);
    if (!parsed.base.text) {
        return { content: "**Base:** empty (invalid cube output base)" };
    }
    const sets = outputLookupSets();
    const canProveBaseInvalid = outputTargetsAvailable();
    const kind = resolveOutputBase(parsed.base.text, sets);
    const parts: string[] = [
        outputBaseHover(parsed.base.text, kind, ctx.col, canProveBaseInvalid),
    ];
    const resolvedStem = resolvedOutputSourceStem(parsed.base.text, kind);
    if (resolvedStem) {
        const source = sourceDescription(resolvedStem);
        if (source) parts.push("", "Source: " + source);
    }

    if (kind === null && canProveBaseInvalid) {
        if (parsed.ignored) {
            const text = parsed.ignored.text || "(empty modifier)";
            parts.push(
                "",
                `**Ignored text:** \`${text}\` — the base is invalid, so the game does not create this output.`,
            );
        }
        return { content: parts.join("\n") };
    }

    if (kind === "useitem" || kind === "usetype") {
        const inputColumn = "input " + outputOrdinal(ctx.col);
        const inputValue = ctx.row[inputColumn] || "";
        parts.push(inputValue
            ? `**Uses ${inputColumn}:** \`${inputValue}\``
            : `**Uses ${inputColumn}:** blank`);
    }

    if (parsed.modifiers.length > 0) {
        const heading = kind === null ? "**Modifiers the game will use if the base is valid:**" : "**Applied modifiers:**";
        parts.push("", heading);
        for (const modifier of parsed.modifiers) {
            const label = modifier.value === null
                ? SIMPLE_OUTPUT_MODS[modifier.key]
                : `${PARAM_OUTPUT_MODS[modifier.key]}: ${modifier.value}`;
            let storage = "";
            if (modifier.noncanonical) {
                storage = kind === null
                    ? " (not a normal integer; if the base is valid, the game may read a different value)"
                    : " (not a normal integer; the game may read a different value)";
            } else if (modifier.outsideU8) {
                storage = kind === null
                    ? " (if the base is valid, this is outside 0..255)"
                    : " (outside 0..255; use a value in that range)";
            }
            parts.push(`- ${label}${storage}`);
        }
    }

    parts.push("", "Modifiers are lower-case exact. `key=value` and `key,value` parameter forms are equivalent.");
    if (parsed.ignored) {
        const text = parsed.ignored.text.split(",")[0] || "(empty modifier)";
        if (kind !== null) {
            parts.push(
                "",
                `**Ignored text begins at:** \`${text}\`. The base and modifiers before it still work; \`${text}\` and everything after it are ignored.`,
            );
        } else if (canProveBaseInvalid) {
            parts.push(
                "",
                `**Ignored text:** \`${text}\` — the base is invalid, so the game does not create this output.`,
            );
        } else {
            parts.push(
                "",
                `**If the base is valid, the game stops at:** \`${text}\`. The base and modifiers before it still work; \`${text}\` and everything after it are ignored.`,
            );
        }
    }

    return { content: parts.join("\n") };
}
