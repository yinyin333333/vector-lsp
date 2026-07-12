/// <reference path="../../vector-lsp-plugin.d.ts" />

const pluginMetadata: PluginMetadata = {
    hoverFiles: ["weapons", "armor", "misc"],
};

// Shows the item code when hovering over the "name" column in weapons, armor,
// or misc, where the code is the lookup key used in cross-file references.

const ITEM_NAME_FILES = ["weapons", "armor", "misc"];

function hover(ctx: HoverContext): HoverResult | null {
    if (ctx.col !== "name") return null;
    if (ITEM_NAME_FILES.indexOf(ctx.file) === -1) return null;

    const code = ctx.row["code"];
    if (!code) return null;

    return { content: ctx.value + "\n\n**Code**: " + code };
}
