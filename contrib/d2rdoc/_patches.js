// Schema patches applied after all schema files load.
// Fixes incorrect cross-reference targets and local schema omissions without
// modifying upstream schema files.

(function () {
    function hasField(file, name) {
        return file && file.fields && file.fields.some(function (f) {
            return f.name && f.name.toLowerCase() === name.toLowerCase();
        });
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
})();
