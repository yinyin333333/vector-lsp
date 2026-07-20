/// <reference path="../../vector-lsp-plugin.d.ts" />

// Shows the enum table entry when hovering over a cell in a column that has
// a table defined in the schema (e.g. "op" in cubemain.txt, "srvstfunc" in
// skills.txt, "pSpell" in misc.txt).
//
// Table convention: column 0 is always the code that matches the cell value.
// Known header names drive layout; unknown layouts fall back to a generic view.

function hover(ctx: HoverContext): HoverResult | null {
    if (!ctx.value) return null;

    const table = getEnumTable(ctx.file, ctx.col);
    if (!table) return null;

    const { headers, rows } = table;
    if (rows.length === 0) return null;

    // Find the row whose code (column 0) matches the hovered cell value.
    const matched = rows.find((r) => r[0] === ctx.value);
    if (!matched) return null;

    // Identify well-known column indices by header name.
    let nameIdx = -1;
    let paramsIdx = -1;
    let descIdx = -1;

    for (let i = 1; i < headers.length; i++) {
        const h = headers[i].toLowerCase();
        if (h === "name") nameIdx = i;
        else if (h === "parameters" || h === "params") paramsIdx = i;
        else if (h === "description" || h === "desc") descIdx = i;
    }

    const name = nameIdx >= 0 ? (matched[nameIdx] || "") : "";
    const parameters = paramsIdx >= 0 && matched[paramsIdx]
        ? matched[paramsIdx]
            .split(/[\r\n]+/)
            .map((s: string) => s.trim())
            .filter((s: string) => s.length > 0)
            .join(", ")
        : "";
    const description = descIdx >= 0 ? (matched[descIdx] || "") : "";
    const extraFields: Record<string, string> = {};
    const legacyParts: string[] = [ctx.value];
    if (name) legacyParts.push(name);
    legacyParts.push("");
    if (parameters) legacyParts.push("**Parameters:** " + parameters);
    if (description) legacyParts.push(description);
    if (nameIdx < 0 && paramsIdx < 0 && descIdx < 0) {
        for (let i = 1; i < headers.length; i++) {
            if (matched[i]) {
                extraFields[headers[i]] = matched[i];
                legacyParts.push("**" + headers[i] + ":** " + matched[i]);
            }
        }
    }
    while (legacyParts.length > 0 && legacyParts[legacyParts.length - 1] === "") legacyParts.pop();
    return {
        contentKey: "plugin.enum.hover",
        contentArgs: {
            value: ctx.value,
            name,
            parameters,
            description,
            extraFields: JSON.stringify(extraFields),
        },
        legacyContent: legacyParts.join("\n"),
    };
}
