/// <reference path="../../vector-lsp-plugin.d.ts" />

const pluginMetadata: PluginMetadata = {
    validateFiles: [
        "misc", "armor", "weapons", "shareditems", "setitems", "uniqueitems",
        "treasureclassex", "missiles", "monpet", "skills", "skilldesc",
    ],
};

// Validates BBE calc formula syntax for every column listed in the BBE field
// map below.  Scope identifiers (Skill / Missile / Monster) are read live from
// the workspace's skillcalc / misscalc / moncalc files so they adapt to each
// workspace without rebuilding the plugin.
//
// Grammar accepted (recursive descent):
//
//   formula   = expr EOF
//   expr      = ternary
//   ternary   = compare ('?' expr ':' expr)?
//   compare   = add (('<'|'<='|'>'|'>='|'=='|'!=') add)*
//   add       = mul (('+' | '-') mul)*
//   mul       = power (('*' | '/' | '%') power)*
//   power     = unary ('^' unary)?
//   unary     = '-' unary | primary
//   primary   = NUMBER | '(' expr ')' | IDENT '(' func_args ')' | IDENT
//
//   func_args (quoted-string style — skill / miss / stat / sklvl / sksrc / cond):
//     QUOTED ('.' (IDENT | NUM))* (',' expr)?
//
//   func_args (expression style — min / max / rand / unknown):
//     expr (',' expr)*
//
// Bare identifiers outside function call argument lists are validated against
// the scope's calc identifier set (loaded from skillcalc / misscalc / moncalc).
// Inside function argument lists (funcDepth > 0) identifiers are NOT validated
// to avoid false positives from cond() parameters like 'hell' or 'player'.
//
// Inside quoted-style function calls the following are validated:
//   skill / sksrc / sklvl  → quoted name vs skills.txt#skill
//                             dot-ident(s) vs skillcalc identifiers
//   miss                   → quoted name vs missiles.txt#Missile
//                             dot-ident vs misscalc identifiers
//   stat                   → quoted name vs itemstatcost.txt#Stat
//                             dot-ident must be one of: accr, base, mod
//   sklvl (two dot-idents) → first vs current scope ids, second vs skillcalc
//   cond                   → quoted name vs known condition list

// ─── BBE field map ────────────────────────────────────────────────────────────

const BBE_FIELDS: Record<string, Record<string, string[]>> = {
    misc: {
        "Item scope BBE":           ["len", "calc#", "spelldesccalc", "UsageConditionCalc"],
        "Treasure Class scope BBE": ["DropConditionCalc"],
    },
    armor:           { "Treasure Class scope BBE": ["DropConditionCalc"] },
    weapons:         { "Treasure Class scope BBE": ["DropConditionCalc"] },
    shareditems:     { "Treasure Class scope BBE": ["DropConditionCalc"] },
    setitems:        { "Treasure Class scope BBE": ["DropConditionCalc"] },
    uniqueitems:     { "Treasure Class scope BBE": ["DropConditionCalc"] },
    treasureclassex: { "Treasure Class scope BBE": ["ConditionCalc"] },
    missiles: {
        "Missile scope BBE": [
            "SrvCalc1", "CltCalc1", "SHitCalc1", "CHitCalc1", "DmgCalc1",
            "Range", "Radius", "DmgSymPerCalc", "EDmgSymPerCalc",
        ],
    },
    monpet: {
        "Monster scope BBE": [
            "calc#", "consumecalc#", "numunderlingcalc", "bindchancecalc", "BoundCalc#",
        ],
    },
    skills: {
        "Skill scope BBE": [
            "prgcalc#", "auralencalc", "aurarangecalc", "aurastatcalc#", "passivecalc#",
            "petmax", "sumsk#calc", "sumumod", "cltcalc#",
            "skpoints", "localdelay", "globaldelay", "perdelay",
            "calc#", "ToHitCalc", "DmgSymPerCalc", "EDmgSymPerCalc", "ELenSymPerCalc",
        ],
    },
    skilldesc: {
        "Skill scope BBE": [
            "ddam calc#", "p#dmmin", "p#dmmax",
            "desccalca#", "desccalcb#",
            "dsc2calca#", "dsc2calcb#",
            "dsc3calca#", "dsc3calcb#",
        ],
    },
};

// xcalc file providing scope-level bare identifiers; null = no identifiers in scope.
const SCOPE_CALC_FILE: Record<string, string | null> = {
    "Skill scope BBE":          "skillcalc",
    "Missile scope BBE":        "misscalc",
    "Monster scope BBE":        "moncalc",
    "Item scope BBE":           null,
    "Treasure Class scope BBE": null,
};

// Known condition names for cond().
const VALID_COND_NAMES: Record<string, boolean> = {
    IsType: true, IsClass: true, Desecrated: true, Difficulty: true,
    MonsterTestElite: true, ItemIsType: true, ItemIsModType: true, MonsterHasMod: true,
    IsDesecratedZonesEnabled: true,
};

// Valid parameters for stat().
const STAT_PARAMS: Record<string, boolean> = { accr: true, base: true, mod: true };

// Functions whose first argument is a quoted string + optional dot-identifiers.
const QUOTED_ARG_FUNCS: Record<string, boolean> = {
    skill: true, miss: true, stat: true, sklvl: true, sksrc: true, cond: true,
};

// ─── Tokenizer ────────────────────────────────────────────────────────────────

interface Token {
    type: string;
    value: string;
    pos: number;
}

interface CalcParseError {
    code: string;
    kind: string;
    message: string;
    pos: number;
    length?: number;
    expected?: string;
    actual?: string;
    parserPosition?: number;
    insertionPoint?: number;
    insertText?: string;
    tokenStart?: number;
    tokenEnd?: number;
    hint?: string;
}

interface NormalizedText {
    text: string;
    start: number;
    end: number;
}

interface CalcErrorMeta {
    expected?: string;
    actual?: string;
    insertionPoint?: number;
    insertText?: string;
    tokenStart?: number;
    tokenEnd?: number;
    hint?: string;
}

const TOKEN_LABELS: Record<string, string> = {
    LPAREN: "(",
    RPAREN: ")",
    LBRACK: "[",
    RBRACK: "]",
    COLON: ":",
    COMMA: ",",
    DOT: ".",
    QUOTED: "quoted string",
    IDENT: "identifier",
    NUM: "number",
    EOF: "end of formula",
};

const TOKEN_INSERT_TEXT: Record<string, string> = {
    RPAREN: ")",
    RBRACK: "]",
    COLON: ":",
    COMMA: ",",
};

function calcError(
    code: string,
    message: string,
    pos: number,
    length?: number,
    kind?: string,
    meta?: CalcErrorMeta,
): CalcParseError {
    const err: CalcParseError = {
        code,
        kind: kind || "invalid-argument",
        message,
        pos,
        length,
        parserPosition: pos,
    };
    if (meta) {
        if (meta.expected !== undefined) err.expected = meta.expected;
        if (meta.actual !== undefined) err.actual = meta.actual;
        if (meta.insertionPoint !== undefined) err.insertionPoint = meta.insertionPoint;
        if (meta.insertText !== undefined) err.insertText = meta.insertText;
        if (meta.tokenStart !== undefined) err.tokenStart = meta.tokenStart;
        if (meta.tokenEnd !== undefined) err.tokenEnd = meta.tokenEnd;
        if (meta.hint !== undefined) err.hint = meta.hint;
    }
    return err;
}

function tokenLabel(type: string): string {
    return TOKEN_LABELS[type] || type;
}

function tokenActual(tok: Token): string {
    return tok.type === "EOF" ? "EOF" : (tok.value || tok.type);
}

function tokenCodePart(type: string): string {
    return type.toLowerCase().replace(/_/g, "-");
}

function tokenError(code: string, message: string, tok: Token, kind?: string, meta?: CalcErrorMeta): CalcParseError {
    const actual = tokenActual(tok);
    if (tok.type === "EOF") {
        return calcError(
            code,
            message,
            tok.pos,
            0,
            kind || "unexpected-eof",
            {
                actual,
                insertionPoint: tok.pos,
                hint: "Complete the expression before the end of the formula.",
                ...(meta || {}),
            },
        );
    }
    const length = tok.value.length || 1;
    return calcError(
        code,
        message,
        tok.pos,
        length,
        kind || "unexpected-token",
        {
            actual,
            tokenStart: tok.pos,
            tokenEnd: tok.pos + length,
            ...(meta || {}),
        },
    );
}

function expectedTokenError(type: string, tok: Token): CalcParseError {
    const expected = tokenLabel(type);
    const actual = tokenActual(tok);
    const insertText = TOKEN_INSERT_TEXT[type] || expected;
    const code = `calc.expected-${tokenCodePart(type)}${tok.type === "EOF" ? ".eof" : ""}`;
    if (tok.type === "EOF") {
        return calcError(
            code,
            `Missing '${expected}' before end of formula`,
            tok.pos,
            0,
            "missing-token",
            {
                expected,
                actual,
                insertionPoint: tok.pos,
                insertText,
                hint: `Insert '${expected}' at the end of this expression.`,
            },
        );
    }
    return calcError(
        code,
        `Missing '${expected}' before '${actual}'`,
        tok.pos,
        0,
        "missing-token",
        {
            expected,
            actual,
            insertionPoint: tok.pos,
            insertText,
            hint: `Insert '${expected}' before '${actual}'.`,
        },
    );
}

function isWhitespace(ch: string): boolean {
    return /\s/.test(ch);
}

function trimSpan(raw: string, start: number, end: number): NormalizedText {
    while (start < end && isWhitespace(raw[start])) start++;
    while (end > start && isWhitespace(raw[end - 1])) end--;
    return { text: raw.slice(start, end), start, end };
}

function normalizeFormula(raw: string): NormalizedText {
    let span = trimSpan(raw, 0, raw.length);
    if (span.text.length >= 2 && span.text[0] === '"' && span.text[span.text.length - 1] === '"') {
        span = trimSpan(raw, span.start + 1, span.end - 1);
    }
    return span;
}

function mapParseError(err: CalcParseError, normalized: NormalizedText): CalcParseError {
    const maxPos = normalized.text.length;
    const parserPos = Math.max(0, Math.min(err.pos, maxPos));
    const pos = normalized.start + parserPos;
    let length = err.length ?? 1;

    const available = Math.max(0, normalized.end - pos);
    if (available > 0) {
        length = length > 0 ? Math.max(1, Math.min(length, available)) : 0;
    } else {
        length = 0;
    }

    const mapped: CalcParseError = {
        ...err,
        pos,
        length,
        parserPosition: parserPos,
    };

    if (err.tokenStart !== undefined) {
        const tokenStart = Math.max(0, Math.min(err.tokenStart, maxPos));
        mapped.tokenStart = normalized.start + tokenStart;
    } else if (length > 0) {
        mapped.tokenStart = pos;
    }
    if (err.tokenEnd !== undefined) {
        const tokenEnd = Math.max(0, Math.min(err.tokenEnd, maxPos));
        mapped.tokenEnd = normalized.start + tokenEnd;
    } else if (length > 0) {
        mapped.tokenEnd = pos + length;
    }
    if (err.insertionPoint !== undefined) {
        const insertionPoint = Math.max(0, Math.min(err.insertionPoint, maxPos));
        mapped.insertionPoint = normalized.start + insertionPoint;
    } else if (length === 0 || err.kind === "missing-token" || err.kind === "unexpected-eof") {
        mapped.insertionPoint = pos;
    }

    return mapped;
}

function tokenize(src: string): Token[] | CalcParseError {
    const tokens: Token[] = [];
    let i = 0;
    while (i < src.length) {
        const ch = src[i];

        if (ch === " " || ch === "\t" || ch === "\r" || ch === "\n") { i++; continue; }

        // Number (integer or decimal)
        const numM = src.slice(i).match(/^[0-9]+(\.[0-9]*)?/);
        if (numM) {
            tokens.push({ type: "NUM", value: numM[0], pos: i });
            i += numM[0].length;
            continue;
        }

        // Single-quoted string (skill / missile / stat / condition names)
        if (ch === "'") {
            const end = src.indexOf("'", i + 1);
            if (end === -1) {
                return calcError(
                    "calc.unterminated-string",
                    "Unterminated string literal",
                    i,
                    1,
                    "unterminated-string",
                    {
                        actual: "'",
                        tokenStart: i,
                        tokenEnd: i + 1,
                        hint: "Close the string with a single quote.",
                    },
                );
            }
            tokens.push({ type: "QUOTED", value: src.slice(i, end + 1), pos: i });
            i = end + 1;
            continue;
        }

        // Identifier
        const identM = src.slice(i).match(/^[a-zA-Z_][a-zA-Z0-9_]*/);
        if (identM) {
            tokens.push({ type: "IDENT", value: identM[0], pos: i });
            i += identM[0].length;
            continue;
        }

        // Two-character operators (must precede single-char checks)
        const two = src.slice(i, i + 2);
        if (two === "<=") { tokens.push({ type: "LE",  value: two, pos: i }); i += 2; continue; }
        if (two === ">=") { tokens.push({ type: "GE",  value: two, pos: i }); i += 2; continue; }
        if (two === "==") { tokens.push({ type: "EQ",  value: two, pos: i }); i += 2; continue; }
        if (two === "!=") { tokens.push({ type: "NEQ", value: two, pos: i }); i += 2; continue; }

        // Single-character operators
        const singles: Record<string, string> = {
            "+": "PLUS",  "-": "MINUS",  "*": "STAR",     "/": "SLASH",
            "%": "PCT",   "^": "CARET",  "(": "LPAREN",   ")": "RPAREN",
            "<": "LT",    ">": "GT",     "?": "QUESTION", ":": "COLON",
            ",": "COMMA", ".": "DOT",
        };
        if (singles[ch]) {
            tokens.push({ type: singles[ch], value: ch, pos: i });
            i++;
            continue;
        }

        return calcError(
            "calc.unexpected-character",
            `Unexpected character '${ch}'`,
            i,
            1,
            "unexpected-character",
            {
                actual: ch,
                tokenStart: i,
                tokenEnd: i + 1,
            },
        );
    }
    tokens.push({ type: "EOF", value: "", pos: i });
    return tokens;
}

// ─── Parser state ─────────────────────────────────────────────────────────────

interface ParseState {
    tokens: Token[];
    pos: number;
    // null  = xcalc absent → skip bare-identifier validation (avoid false positives).
    // empty Set = Item/TC scope → any bare identifier is an error.
    scopeIds: Set<string> | null;
    // Always-available identifier sets for use inside skill() / miss() calls.
    skillIds: Set<string>;
    missIds: Set<string>;
    // >0 means we are inside function argument list(s); skip bare-ident scope check.
    funcDepth: number;
}

function peek(st: ParseState): Token { return st.tokens[st.pos]; }
function advance(st: ParseState): Token { return st.tokens[st.pos++]; }
function check(st: ParseState, type: string): boolean { return peek(st).type === type; }

function eat(st: ParseState, type: string): CalcParseError | null {
    if (!check(st, type)) {
        return expectedTokenError(type, peek(st));
    }
    advance(st);
    return null;
}

// ─── Grammar ──────────────────────────────────────────────────────────────────

function parseExpr(st: ParseState): CalcParseError | null {
    return parseTernary(st);
}

function parseTernary(st: ParseState): CalcParseError | null {
    let err = parseCompare(st);
    if (err) return err;
    if (check(st, "QUESTION")) {
        advance(st);
        err = parseExpr(st);
        if (err) return err;
        err = eat(st, "COLON");
        if (err) return err;
        return parseExpr(st);
    }
    return null;
}

function parseCompare(st: ParseState): CalcParseError | null {
    let err = parseAdd(st);
    if (err) return err;
    const cmpOps = ["LT", "LE", "GT", "GE", "EQ", "NEQ"];
    while (cmpOps.indexOf(peek(st).type) !== -1) {
        advance(st);
        err = parseAdd(st);
        if (err) return err;
    }
    return null;
}

function parseAdd(st: ParseState): CalcParseError | null {
    let err = parseMul(st);
    if (err) return err;
    while (check(st, "PLUS") || check(st, "MINUS")) {
        advance(st);
        err = parseMul(st);
        if (err) return err;
    }
    return null;
}

function parseMul(st: ParseState): CalcParseError | null {
    let err = parsePower(st);
    if (err) return err;
    while (check(st, "STAR") || check(st, "SLASH") || check(st, "PCT")) {
        advance(st);
        err = parsePower(st);
        if (err) return err;
    }
    return null;
}

function parsePower(st: ParseState): CalcParseError | null {
    let err = parseUnary(st);
    if (err) return err;
    if (check(st, "CARET")) {
        advance(st);
        return parseUnary(st);
    }
    return null;
}

function parseUnary(st: ParseState): CalcParseError | null {
    if (check(st, "MINUS")) { advance(st); return parseUnary(st); }
    return parsePrimary(st);
}

function parsePrimary(st: ParseState): CalcParseError | null {
    const tok = peek(st);

    if (tok.type === "NUM") { advance(st); return null; }

    if (tok.type === "LPAREN") {
        advance(st);
        const err = parseExpr(st);
        if (err) return err;
        return eat(st, "RPAREN");
    }

    if (tok.type === "IDENT") {
        advance(st);
        if (check(st, "LPAREN")) {
            advance(st); // consume '('
            st.funcDepth++;
            const err = parseFuncArgs(st, tok.value);
            st.funcDepth--;
            if (err) return err;
            return eat(st, "RPAREN");
        }
        // Bare identifier: validate against scope when at the top-level expression.
        if (st.funcDepth === 0 && st.scopeIds !== null && !st.scopeIds.has(tok.value)) {
            return calcError(
                "unknownIdentifier",
                `Unknown identifier '${tok.value}' for this BBE scope`,
                tok.pos,
                tok.value.length,
            );
        }
        return null;
    }

    if (tok.type === "EOF") {
        return tokenError(
            "calc.unexpected-eof",
            "Unexpected end of formula",
            tok,
            "unexpected-eof",
            { expected: "expression" },
        );
    }
    return tokenError("calc.unexpected-token", `Unexpected token '${tok.value}'`, tok);
}

function parseFuncArgs(st: ParseState, funcName: string): CalcParseError | null {
    if (check(st, "RPAREN")) return null; // zero-arg (defensive)

    if (QUOTED_ARG_FUNCS[funcName] || check(st, "QUOTED")) {
        // Quoted-string style: QUOTED ('.' (IDENT | NUM))* (',' expr)?
        if (!check(st, "QUOTED")) {
            const t = peek(st);
            return tokenError(
                "calc.expected-quoted-argument",
                `Expected quoted string as first argument of '${funcName}()'`,
                t,
                "invalid-argument",
                {
                    expected: "quoted string",
                    hint: `Wrap the first argument of '${funcName}()' in single quotes.`,
                },
            );
        }
        const quotedTok = advance(st);
        const quotedVal = quotedTok.value.slice(1, -1); // strip surrounding ' '

        const nameErr = validateQuotedName(funcName, quotedVal, quotedTok);
        if (nameErr) return nameErr;

        // Collect dot-separated identifiers / numbers
        const dotIdents: Token[] = [];
        while (check(st, "DOT")) {
            advance(st); // consume '.'
            const t = peek(st);
            if (t.type !== "IDENT" && t.type !== "NUM") {
                return tokenError(
                    "calc.expected-dot-identifier",
                    `Expected identifier after '.' in '${funcName}()'`,
                    t,
                    "invalid-parser-token",
                    {
                        expected: "identifier",
                        hint: `Add an identifier after '.' in '${funcName}()'.`,
                    },
                );
            }
            dotIdents.push(advance(st));
        }

        const dotErr = validateDotIdents(funcName, dotIdents, st);
        if (dotErr) return dotErr;

        // cond() accepts an optional second argument after a comma
        if (check(st, "COMMA")) {
            advance(st);
            const err = parseExpr(st);
            if (err) return err;
        }

        return null;
    }

    // Expression-list style: min / max / rand / unknown functions
    let err = parseExpr(st);
    if (err) return err;
    while (check(st, "COMMA")) {
        advance(st);
        err = parseExpr(st);
        if (err) return err;
    }
    return null;
}

// ─── Function argument validation ─────────────────────────────────────────────

function validateQuotedName(funcName: string, name: string, tok: Token): CalcParseError | null {
    if (funcName === "skill" || funcName === "sksrc" || funcName === "sklvl") {
        if (hasFile("skills") && !lookupKey("skills", "skill", name)) {
            return calcError("unknownSkill", `Unknown skill '${name}'`, tok.pos, tok.value.length);
        }
    } else if (funcName === "miss") {
        if (hasFile("missiles") && !lookupKey("missiles", "Missile", name)) {
            return calcError("unknownMissile", `Unknown missile '${name}'`, tok.pos, tok.value.length);
        }
    } else if (funcName === "stat") {
        if (hasFile("itemstatcost") && !lookupKey("itemstatcost", "Stat", name)) {
            return calcError("unknownStat", `Unknown stat '${name}'`, tok.pos, tok.value.length);
        }
    } else if (funcName === "cond") {
        if (!VALID_COND_NAMES[name]) {
            return calcError("unknownCondition", `Unknown condition '${name}'`, tok.pos, tok.value.length);
        }
    }
    return null;
}

function validateDotIdents(funcName: string, idents: Token[], st: ParseState): CalcParseError | null {
    if (funcName === "skill" || funcName === "sksrc") {
        if (idents.length >= 1 && st.skillIds.size > 0 && !st.skillIds.has(idents[0].value)) {
            return calcError(
                "unknownSkillIdentifier",
                `Unknown skill identifier '${idents[0].value}'`,
                idents[0].pos,
                idents[0].value.length,
            );
        }
    } else if (funcName === "miss") {
        if (idents.length >= 1 && st.missIds.size > 0 && !st.missIds.has(idents[0].value)) {
            return calcError(
                "unknownMissileIdentifier",
                `Unknown missile identifier '${idents[0].value}'`,
                idents[0].pos,
                idents[0].value.length,
            );
        }
    } else if (funcName === "stat") {
        if (idents.length >= 1 && !STAT_PARAMS[idents[0].value]) {
            return calcError(
                "invalidStatParameter",
                `Invalid stat parameter '${idents[0].value}' (expected accr, base, or mod)`,
                idents[0].pos,
                idents[0].value.length,
            );
        }
    } else if (funcName === "sklvl") {
        // First dot-ident: current-scope identifier (level source)
        if (idents.length >= 1 && st.scopeIds !== null && st.scopeIds.size > 0
                && !st.scopeIds.has(idents[0].value)) {
            return calcError(
                "unknownScopeIdentifier",
                `Unknown scope identifier '${idents[0].value}' as level in sklvl()`,
                idents[0].pos,
                idents[0].value.length,
            );
        }
        // Second dot-ident: skill-scope identifier
        if (idents.length >= 2 && st.skillIds.size > 0 && !st.skillIds.has(idents[1].value)) {
            return calcError(
                "unknownSkillIdentifier",
                `Unknown skill identifier '${idents[1].value}' in sklvl()`,
                idents[1].pos,
                idents[1].value.length,
            );
        }
    }
    return null;
}

// ─── Identifier helpers ───────────────────────────────────────────────────────

function loadCalcIds(stem: string): Set<string> {
    return new Set(getColumnValues(stem, "code").filter((v: string) => v.trim()));
}

// ─── Formula entry point ──────────────────────────────────────────────────────

function parseBBE(
    raw: string,
    scopeIds: Set<string> | null,
    skillIds: Set<string>,
    missIds: Set<string>,
): CalcParseError | null {
    // Strip outer double-quotes that some editors wrap around cell formulas.
    const normalized = normalizeFormula(raw);
    const src = normalized.text;
    if (!src) return null;

    const result = tokenize(src);
    if (!Array.isArray(result)) return mapParseError(result, normalized);

    const st: ParseState = {
        tokens: result, pos: 0,
        scopeIds, skillIds, missIds,
        funcDepth: 0,
    };
    const err = parseExpr(st);
    if (err) return mapParseError(err, normalized);
    if (!check(st, "EOF")) {
        const t = peek(st);
        return mapParseError(
            tokenError("calc.unexpected-token", `Unexpected token '${t.value}'`, t),
            normalized,
        );
    }
    return null;
}

// ─── validate ─────────────────────────────────────────────────────────────────

function addDiagnosticData(
    data: Record<string, string | number>,
    key: string,
    value: string | number | undefined,
) {
    if (value !== undefined) data[key] = value;
}

function calcDiagnosticData(err: CalcParseError): Record<string, string | number> {
    const data: Record<string, string | number> = {
        rule: "calcCheck",
        kind: err.kind,
    };
    addDiagnosticData(data, "expected", err.expected);
    addDiagnosticData(data, "actual", err.actual);
    addDiagnosticData(data, "parserPosition", err.parserPosition);
    addDiagnosticData(data, "insertionPoint", err.insertionPoint);
    addDiagnosticData(data, "insertText", err.insertText);
    addDiagnosticData(data, "tokenStart", err.tokenStart);
    addDiagnosticData(data, "tokenEnd", err.tokenEnd);
    addDiagnosticData(data, "hint", err.hint);
    return data;
}

function validate(ctx: PluginContext): PluginDiagnostic[] {
    const scopeMap = BBE_FIELDS[ctx.file];
    if (!scopeMap) return [];

    // Load xcalc identifier sets once per file validation.
    const skillIds = loadCalcIds("skillcalc");
    const missIds  = loadCalcIds("misscalc");
    const monIds   = loadCalcIds("moncalc");

    const diags: PluginDiagnostic[] = [];

    for (const scope of Object.keys(scopeMap)) {
        const patterns = scopeMap[scope];
        const calcFile: string | null | undefined = SCOPE_CALC_FILE[scope];

        let scopeIds: Set<string> | null;
        if (typeof calcFile === "string") {
            // Use the loaded ids. If the file is absent (empty set), skip
            // identifier validation rather than flooding with false positives.
            let ids: Set<string>;
            if (calcFile === "skillcalc")      ids = skillIds;
            else if (calcFile === "misscalc")  ids = missIds;
            else if (calcFile === "moncalc")   ids = monIds;
            else                               ids = new Set();
            scopeIds = ids.size > 0 ? ids : null;
        } else {
            // null = Item / TC scope: no bare identifiers allowed.
            // Use an empty Set so any bare identifier is flagged as an error.
            scopeIds = new Set();
        }

        // Expand '#' patterns into concrete column names.
        const cols: string[] = [];
        for (const pat of patterns) {
            if (pat.indexOf("#") !== -1) {
                for (let n = 1; ; n++) {
                    const col = pat.replace("#", String(n));
                    if (!getColumn(ctx.file, col)) break;
                    cols.push(col);
                }
            } else {
                cols.push(pat);
            }
        }

        for (const row of ctx.rows) {
            for (const col of cols) {
                const val = row[col] as string | undefined;
                if (!val || !val.trim()) continue;

                const err = parseBBE(val, scopeIds, skillIds, missIds);
                if (err) {
                    const c = row.__colstarts[col] ?? 0;
                    const length = err.length ?? 1;
                    diags.push({
                        line:     row.__line,
                        col:      c + err.pos,
                        endCol:   c + err.pos + length,
                        severity: "error",
                        message:  `calcCheck: Invalid calc formula: ${err.message}`,
                        code:     err.code,
                        data:     calcDiagnosticData(err),
                    });
                }
            }
        }
    }

    return diags;
}
