/// <reference path="../../vector-lsp-plugin.d.ts" />

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
//   compare   = add (('<'|'<='|'>'|'>='|'=='|'!=') add)?
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

function tokenize(src: string): Token[] | string {
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
            if (end === -1) return `Unterminated string literal at position ${i}`;
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

        return `Unexpected character '${ch}' at position ${i}`;
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

function eat(st: ParseState, type: string): string | null {
    if (!check(st, type)) {
        const t = peek(st);
        return `Expected '${type}' but got '${t.value || t.type}' at position ${t.pos}`;
    }
    advance(st);
    return null;
}

// ─── Grammar ──────────────────────────────────────────────────────────────────

function parseExpr(st: ParseState): string | null {
    return parseTernary(st);
}

function parseTernary(st: ParseState): string | null {
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

function parseCompare(st: ParseState): string | null {
    let err = parseAdd(st);
    if (err) return err;
    const cmpOps = ["LT", "LE", "GT", "GE", "EQ", "NEQ"];
    if (cmpOps.indexOf(peek(st).type) !== -1) {
        advance(st);
        return parseAdd(st);
    }
    return null;
}

function parseAdd(st: ParseState): string | null {
    let err = parseMul(st);
    if (err) return err;
    while (check(st, "PLUS") || check(st, "MINUS")) {
        advance(st);
        err = parseMul(st);
        if (err) return err;
    }
    return null;
}

function parseMul(st: ParseState): string | null {
    let err = parsePower(st);
    if (err) return err;
    while (check(st, "STAR") || check(st, "SLASH") || check(st, "PCT")) {
        advance(st);
        err = parsePower(st);
        if (err) return err;
    }
    return null;
}

function parsePower(st: ParseState): string | null {
    let err = parseUnary(st);
    if (err) return err;
    if (check(st, "CARET")) {
        advance(st);
        return parseUnary(st);
    }
    return null;
}

function parseUnary(st: ParseState): string | null {
    if (check(st, "MINUS")) { advance(st); return parseUnary(st); }
    return parsePrimary(st);
}

function parsePrimary(st: ParseState): string | null {
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
            return `Unknown identifier '${tok.value}' for this BBE scope`;
        }
        return null;
    }

    if (tok.type === "EOF") return `Unexpected end of formula`;
    return `Unexpected token '${tok.value}' at position ${tok.pos}`;
}

function parseFuncArgs(st: ParseState, funcName: string): string | null {
    if (check(st, "RPAREN")) return null; // zero-arg (defensive)

    if (QUOTED_ARG_FUNCS[funcName] || check(st, "QUOTED")) {
        // Quoted-string style: QUOTED ('.' (IDENT | NUM))* (',' expr)?
        if (!check(st, "QUOTED")) {
            const t = peek(st);
            return `Expected quoted string as first argument of '${funcName}()' at position ${t.pos}`;
        }
        const quotedTok = advance(st);
        const quotedVal = quotedTok.value.slice(1, -1); // strip surrounding ' '

        const nameErr = validateQuotedName(funcName, quotedVal);
        if (nameErr) return nameErr;

        // Collect dot-separated identifiers / numbers
        const dotIdents: string[] = [];
        while (check(st, "DOT")) {
            advance(st); // consume '.'
            const t = peek(st);
            if (t.type !== "IDENT" && t.type !== "NUM") {
                return `Expected identifier after '.' in '${funcName}()' at position ${t.pos}`;
            }
            dotIdents.push(advance(st).value);
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

function validateQuotedName(funcName: string, name: string): string | null {
    if (funcName === "skill" || funcName === "sksrc" || funcName === "sklvl") {
        if (hasFile("skills") && !lookupKey("skills", "skill", name)) {
            return `Unknown skill '${name}'`;
        }
    } else if (funcName === "miss") {
        if (hasFile("missiles") && !lookupKey("missiles", "Missile", name)) {
            return `Unknown missile '${name}'`;
        }
    } else if (funcName === "stat") {
        if (hasFile("itemstatcost") && !lookupKey("itemstatcost", "Stat", name)) {
            return `Unknown stat '${name}'`;
        }
    } else if (funcName === "cond") {
        if (!VALID_COND_NAMES[name]) {
            return `Unknown condition '${name}'`;
        }
    }
    return null;
}

function validateDotIdents(funcName: string, idents: string[], st: ParseState): string | null {
    if (funcName === "skill" || funcName === "sksrc") {
        if (idents.length >= 1 && st.skillIds.size > 0 && !st.skillIds.has(idents[0])) {
            return `Unknown skill identifier '${idents[0]}'`;
        }
    } else if (funcName === "miss") {
        if (idents.length >= 1 && st.missIds.size > 0 && !st.missIds.has(idents[0])) {
            return `Unknown missile identifier '${idents[0]}'`;
        }
    } else if (funcName === "stat") {
        if (idents.length >= 1 && !STAT_PARAMS[idents[0]]) {
            return `Invalid stat parameter '${idents[0]}' (expected accr, base, or mod)`;
        }
    } else if (funcName === "sklvl") {
        // First dot-ident: current-scope identifier (level source)
        if (idents.length >= 1 && st.scopeIds !== null && st.scopeIds.size > 0
                && !st.scopeIds.has(idents[0])) {
            return `Unknown scope identifier '${idents[0]}' as level in sklvl()`;
        }
        // Second dot-ident: skill-scope identifier
        if (idents.length >= 2 && st.skillIds.size > 0 && !st.skillIds.has(idents[1])) {
            return `Unknown skill identifier '${idents[1]}' in sklvl()`;
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
): string | null {
    // Strip outer double-quotes that some editors wrap around cell formulas.
    let src = raw.trim();
    if (src.length >= 2 && src[0] === '"' && src[src.length - 1] === '"') {
        src = src.slice(1, -1).trim();
    }
    if (!src) return null;

    const result = tokenize(src);
    if (typeof result === "string") return result;

    const st: ParseState = {
        tokens: result, pos: 0,
        scopeIds, skillIds, missIds,
        funcDepth: 0,
    };
    const err = parseExpr(st);
    if (err) return err;
    if (!check(st, "EOF")) {
        const t = peek(st);
        return `Unexpected token '${t.value}' at position ${t.pos}`;
    }
    return null;
}

// ─── validate ─────────────────────────────────────────────────────────────────

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
                    diags.push({
                        line:     row.__line,
                        col:      c,
                        endCol:   c + val.length,
                        severity: "error",
                        message:  `calcCheck: Invalid calc formula: ${err}`,
                    });
                }
            }
        }
    }

    return diags;
}
