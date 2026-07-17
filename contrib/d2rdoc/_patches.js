// Schema patches applied after all schema files load.
// Fixes incorrect cross-reference targets and local schema omissions without
// modifying upstream schema files.

(function () {
    function hasField(file, name) {
        return file && file.fields && file.fields.some(function (f) {
            return f.name && f.name.toLowerCase() === name.toLowerCase();
        });
    }

    function findField(stem, name) {
        var file = files[stem];
        if (!file || !file.fields) return null;
        var lower = name.toLowerCase();
        return file.fields.find(function (field) {
            if (field.name && field.name.toLowerCase() === lower) return true;
            return (field.altNames || []).some(function (alt) {
                return alt.toLowerCase() === lower;
            });
        }) || null;
    }

    function setDescription(stem, name, description) {
        var field = findField(stem, name);
        if (field) field.description = description;
    }

    function setTypeDescription(stem, name, description) {
        var field = findField(stem, name);
        if (field && field.type) field.type.description = description;
    }

    function setTableDescription(stem, name, code, description) {
        var field = findField(stem, name);
        if (!field || !field.table) return;
        var row = field.table.find(function (candidate) {
            return candidate && candidate.length > 0 && String(candidate[0]).toLowerCase() === code.toLowerCase();
        });
        if (row) row[1] = description;
    }

    function addField(stem, field) {
        var file = files[stem];
        if (!file || hasField(file, field.name)) return;
        file.fields.push(field);
    }

    function ignoreField(stem, name) {
        var file = files[stem];
        if (!file) return;
        file.ignoreFields = file.ignoreFields || [];
        if (!file.ignoreFields.some(function (f) { return f.toLowerCase() === name.toLowerCase(); })) {
            file.ignoreFields.push(name);
        }
    }

    function setFixed4ItemTypeReference(stem, name) {
        var field = findField(stem, name);
        if (!field) return;
        field.type = field.type || {};
        field.type.type = "reference";
        field.type.dataLength = 4;
        field.type.file = "itemtypes";
        field.type.field = "Code";
        field.type.resolver = "fixed4";
        field.type.unknownPolicy = "warning";
        field.description = (field.description || "Item type reference")
            + " The game uses only the first four bytes; shorter codes are filled with spaces. Letter case matters.";
    }

    // Example:
    // skills.passiveitype: schema incorrectly points to itemtypes.itemtype;
    // the correct lookup column is itemtypes.code.

    /*var skills = files["skills"];
    if (!skills) return;
    var field = skills.fields.find(function (f) { return f.name === "passiveitype"; });
    if (!field || !field.type) return;
    field.type.field = "code";*/

    // Local D2R 3.2 corpus/schema gaps found by vector-lsp phase 3:
    // - monsounds.txt uses a non-star EOL terminator column.
    // - sounds.txt carries a numeric trailing padding/header column.
    // - shrines.txt includes a numeric rarity field.
    // - soundenviron.txt includes a numeric Index field after Handle.
    ignoreField("monsounds", "EOL");
    ignoreField("sounds", "4841");
    addField("shrines", {
        "name": "rarity",
        "description": "Relative selection weight for this shrine entry.",
        "type": { "type": "int", "dataLength": 0, "memSize": 0 }
    });
    addField("soundenviron", {
        "name": "Index",
        "description": "Numeric sound environment index used by the data file.",
        "type": { "type": "int", "dataLength": 0, "memSize": 0 }
    });

    // Magic affix itype#/etype# fields use the binary fixed-width code
    // resolver. 1.13/2.4 declare the fields directly; 3.1/3.2 inherit them
    // from SharedItemMods, so patch every possible owner without guessing the
    // selected schema variant.
    ["magicprefix", "magicsuffix", "SharedItemMods"].forEach(function (stem) {
        setFixed4ItemTypeReference(stem, "itype#");
        setFixed4ItemTypeReference(stem, "etype#");
    });

    // Revalidated descriptions used by the real header-hover path.
    setDescription(
        "monstats",
        "NextInClass",
        "Points to another monster Id as the next node in the resolved class chain. The link is resolved by Id; physical row adjacency and contiguous row order are not required."
    );
    ["Explosion", "NoMultiShot"].forEach(function (name) {
        setDescription(
            "missiles",
            name,
            "Numeric 0 means false. Any numeric nonzero value means true, including negative values."
        );
    });
    setDescription(
        "treasureclassex",
        "Picks",
        "Controls how many Treasure Class selections are made.\n\n"
            + "- Positive Picks: makes that many weighted selections.\n"
            + "- Negative Picks: checks at least one numbered position, using positive Prob# values in order.\n"
            + "- Blank: uses 1.\n"
            + "- The absolute Picks value does not have to equal the Prob# total. A smaller total can produce no item; a larger total can leave entries unreachable."
    );
    setDescription(
        "treasureclassex",
        "Prob#",
        "Sets the weight for the matching Item# entry.\n\n"
            + "- The item must be present and Prob# must be positive.\n"
            + "- Blank, zero, or negative values skip that entry.\n"
            + "- Entries after the first blank Item# are ignored.\n"
            + "- With negative Picks, positive Prob# values select numbered positions; they are not guaranteed quantities."
    );
    setDescription(
        "cubemain",
        "output",
        "Defines one cube output. output, output b, and output c are handled separately.\n\n"
            + "- Item and item-type codes must match exactly; unique and set names ignore letter case.\n"
            + "- Portal names require the full spaced phrase.\n"
            + "- useitem and usetype use input 1, input 2, or input 3 for output, output b, or output c.\n"
            + "- Modifiers must be lower-case. Numeric parameters accept key=value or key,value.\n"
            + "- If the base is valid, an unknown modifier and everything after it are ignored; earlier modifiers still work."
    );
    setTypeDescription(
        "cubemain",
        "output",
        "Use an exact item or item-type code, a unique or set name whose letter case may differ, useitem/usetype, or a full spaced portal name, followed by lower-case output modifiers."
    );
    setDescription(
        "cubemain",
        "mod #",
        "Controls an output property. Letter case does not matter when matching property and property-group codes."
    );
    setDescription(
        "cubemain",
        "lvl",
        "Sets the item level for the matching output (lvl, b lvl, or c lvl) and overrides its plvl/ilvl calculation. Use a whole number from 0 through 255."
    );
    setDescription(
        "cubemain",
        "plvl",
        "Adds a player-level ratio to the matching output (plvl, b plvl, or c plvl). Use a whole number from 0 through 255."
    );
    setDescription(
        "cubemain",
        "ilvl",
        "Adds an input-item-level ratio: ilvl uses input 1, b ilvl uses input 2, and c ilvl uses input 3. Use a whole number from 0 through 255."
    );

    setTableDescription("cubemain", "output", "useitem", "Use the item from input 1 for output, input 2 for output b, or input 3 for output c.");
    setTableDescription("cubemain", "output", "usetype", "Use the item type from input 1 for output, input 2 for output b, or input 3 for output c.");
    setTableDescription("cubemain", "output", "qty=#", "Output quantity. qty=# and qty,# are equivalent. Use a whole number from 0 through 255.");
    setTableDescription("cubemain", "output", "sock", "Socket count. sock=# and sock,# are equivalent. Use a whole number from 0 through 255.");
    setTableDescription("cubemain", "output", "lvl=#", "Inline level parameter. lvl=# and lvl,# are equivalent. Use a whole number from 0 through 255. Its final gameplay effect is not fully established.");
    setTableDescription("cubemain", "output", "pre=#", "Prefix row ID. pre=# and pre,# are equivalent parameter forms.");
    setTableDescription("cubemain", "output", "suf=#", "Suffix row ID. suf=# and suf,# are equivalent parameter forms.");
})();
