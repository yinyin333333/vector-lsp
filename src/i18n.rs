//! Session-safe product-message localization.
//!
//! The LSP protocol deliberately keeps a rendered `message` for clients that
//! do not understand our metadata, but the canonical representation is the
//! stable `messageKey` plus named `messageArgs` stored in `Diagnostic.data`.
//! This module is the only place where those keys are rendered.

use serde_json::{Map, Value, json};
use tower_lsp::lsp_types::Diagnostic;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum Locale {
    #[default]
    EnUs,
    ZhTw,
    DeDe,
    EsEs,
    FrFr,
    ItIt,
    KoKr,
    PlPl,
    EsMx,
    JaJp,
    PtBr,
    RuRu,
    ZhCn,
}

impl Locale {
    #[cfg(test)]
    pub const ALL: [Self; 13] = [
        Self::EnUs,
        Self::ZhTw,
        Self::DeDe,
        Self::EsEs,
        Self::FrFr,
        Self::ItIt,
        Self::KoKr,
        Self::PlPl,
        Self::EsMx,
        Self::JaJp,
        Self::PtBr,
        Self::RuRu,
        Self::ZhCn,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EnUs => "enUS",
            Self::ZhTw => "zhTW",
            Self::DeDe => "deDE",
            Self::EsEs => "esES",
            Self::FrFr => "frFR",
            Self::ItIt => "itIT",
            Self::KoKr => "koKR",
            Self::PlPl => "plPL",
            Self::EsMx => "esMX",
            Self::JaJp => "jaJP",
            Self::PtBr => "ptBR",
            Self::RuRu => "ruRU",
            Self::ZhCn => "zhCN",
        }
    }

    /// Accept only the published locale identifiers (with a forgiving case,
    /// dash, and underscore normalization for editor integrations).
    pub fn normalize(value: Option<&str>) -> Self {
        let normalized = value
            .unwrap_or_default()
            .trim()
            .replace(['-', '_'], "")
            .to_ascii_lowercase();
        match normalized.as_str() {
            "enus" => Self::EnUs,
            "zhtw" => Self::ZhTw,
            "dede" => Self::DeDe,
            "eses" => Self::EsEs,
            "frfr" => Self::FrFr,
            "itit" => Self::ItIt,
            "kokr" => Self::KoKr,
            "plpl" => Self::PlPl,
            "esmx" => Self::EsMx,
            "jajp" => Self::JaJp,
            "ptbr" => Self::PtBr,
            "ruru" => Self::RuRu,
            "zhcn" => Self::ZhCn,
            _ => Self::EnUs,
        }
    }
}

/// Public, product-authored core keys. Bundled plugin keys are listed
/// separately below; third-party plugin content remains compatibility data
/// owned by its plugin author.
#[cfg(test)]
pub const CATALOG_KEYS: &[&str] = &[
    "diag.duplicate_unique",
    "diag.reference.unresolved",
    "diag.reference.range",
    "diag.reference.monpet_consumestat",
    "diag.reference.properties_stat",
    "diag.reference.properties_stat_noeffect",
    "diag.fixed4_unknown",
    "diag.integer.backtick",
    "diag.integer.invalid",
    "diag.float.invalid",
    "diag.boolean.invalid",
    "diag.boolean.type29_invalid",
    "diag.hit.out_of_range",
    "diag.hit.noncanonical",
    "diag.hit.nu_literal",
    "diag.hit.non_numeric_outside",
    "diag.hit.non_numeric",
    "json.syntax_invalid",
    "json.id_out_of_range",
    "json.missing_id",
    "json.duplicate_id_same",
    "json.duplicate_id_cross",
    "json.duplicate_key_same",
    "json.duplicate_key_cross",
    "json.missing_fields",
    "json.unused_key",
    "hover.unknown_monpet_stat",
    "hover.unknown_property_stat",
    "hover.unknown_property_stat_noeffect",
    "hover.range_valid",
    "hover.reference_resolved",
    "hover.boolean_on",
    "hover.boolean_off",
    "hover.boolean_off_recommendation",
    "hover.game_version",
    "hover.game_version_unselected",
    "hover.hit_summon",
    "hover.hit_current_fallback",
    "hover.hit_current",
    "hover.header",
    "log.json_stopped",
    "log.json_warning",
    "log.json_path_uri",
    "log.json_parse_failed",
    "log.ignored_change",
    "log.schema_loaded",
    "log.schema_selection_failed",
    "log.schema_load_failed",
    "log.workspace_scan_failed",
    "log.watch_registration_prepare_failed",
    "log.file_watching_unavailable",
    "log.json_watch_retire_failed",
    "log.workspace_scan_root_missing",
    "log.workspace_scan_root_not_file",
    "log.scan_path_uri_conversion_failed",
    "log.scan_reference_path_uri_conversion_failed",
    "log.scan_reference_root_failed",
    "log.scan_reference_root_not_file",
    "log.scan_task_failed",
    "log.workspace_scan_skipped",
    "log.workspace_indexed",
    "log.scan_background_parse_failed",
    "log.schema_task_stopped",
    "log.reference_tables_loaded",
    "log.reference_fallback_no_version",
    "log.reference_fallback_disabled",
    "log.reference_data_load_failed",
    "log.did_close_restore_failed",
    "log.effective_config",
    "cli.single_shot_workspace_required",
    "cli.plugin_runtime_startup_failed",
    "cli.schema_task_panicked",
    "cli.plugins_loaded",
    "cli.reference_fallback_disabled",
    "cli.reference_tables_loaded",
    "cli.reference_fallback_disabled_for_run",
    "cli.workspace_read_failed",
    "cli.path_skipped",
    "cli.check_summary",
    "cli.tcp_plugin_runtime_startup_failed",
];

/// Every key emitted by the bundled d2rdoc plugins.  Keep this list in lock
/// step with their TypeScript source: the test below scans the source for
/// explicit keys and rejects any unregistered key. Dynamic `plugin.` + code
/// emitters are represented by their finite, documented code sets here.
pub const BUNDLED_PLUGIN_KEYS: &[&str] = &[
    "plugin.calc.skill-param-alias",
    "plugin.calc.unknown-missile-value",
    "plugin.calc.unterminated-string",
    "plugin.calc.unexpected-character",
    "plugin.calc.unexpected-eof",
    "plugin.calc.unexpected-token",
    "plugin.calc.wrong-arity",
    "plugin.calc.expected-quoted-argument",
    "plugin.calc.expected-dot-identifier",
    "plugin.calc.expected-rparen",
    "plugin.calc.expected-rparen.eof",
    "plugin.calc.expected-rbrack",
    "plugin.calc.expected-rbrack.eof",
    "plugin.calc.expected-colon",
    "plugin.calc.expected-colon.eof",
    "plugin.calc.expected-comma",
    "plugin.calc.expected-comma.eof",
    "plugin.unknownSkill",
    "plugin.unknownMissile",
    "plugin.unknownStat",
    "plugin.unknownCondition",
    "plugin.unknownIdentifier",
    "plugin.unknownSkillIdentifier",
    "plugin.unknownMissileIdentifier",
    "plugin.unknownScopeIdentifier",
    "plugin.calc.skilldesc-decimal-prefix",
    "plugin.calc.decimal-policy",
    "plugin.calc.prefix-stop",
    "plugin.cube-input.no-inputs",
    "plugin.cube-input.invalid-numinputs",
    "plugin.cube-input.numinputs-mismatch",
    "plugin.cube-input.empty-base",
    "plugin.cube-input.invalid-base",
    "plugin.cube-input.ignored-suffix",
    "plugin.cube-input.u8-range",
    "plugin.cube-input.hover",
    "plugin.cube-output.invalid-base",
    "plugin.cube-output.missing-ordinal-input",
    "plugin.cube-output.u8-range",
    "plugin.cube-output.ignored-suffix",
    "plugin.cube-output.invalid-property",
    "plugin.cube-output.empty-base-hover",
    "plugin.cube-output.invalid-hover",
    "plugin.cube-output.hover",
    "plugin.enum.hover",
    "plugin.item-code.unresolved",
    "plugin.item-code.unresolved-packed-policy",
    "plugin.item-code.unresolved-policy",
    "plugin.item-code.hover",
    "plugin.item-name.hover",
    "plugin.property.unknown-marker",
    "plugin.property.unknown-code",
    "plugin.property.unknown-hover",
    "plugin.property.hover",
    "plugin.tc-item.after-first-gap",
    "plugin.tc-prob.after-first-gap",
    "plugin.tc-prob.orphaned",
    "plugin.tc-item.forward-reference",
    "plugin.tc-item.unresolved-base",
    "plugin.tc-prob.blank-omission",
    "plugin.tc-prob.noncanonical",
    "plugin.tc-prob.nonpositive-omission",
    "plugin.tc-item.modifier-range",
    "plugin.tc-item.ignored-suffix",
    "plugin.tc-item.field-width",
    "plugin.treasure-class.hover-after-gap",
    "plugin.treasure-class.hover",
];

pub fn is_bundled_plugin_key(key: &str) -> bool {
    BUNDLED_PLUGIN_KEYS.contains(&key)
}

pub fn args(pairs: impl IntoIterator<Item = (&'static str, Value)>) -> Map<String, Value> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

pub fn localize(locale: Locale, key: &str, args: &Map<String, Value>) -> String {
    let mut values = values_arg(args);
    if key.starts_with("plugin.calc.") {
        values.insert("values".to_string(), Value::String(String::new()));
    }
    let mut text = interpolate(&catalog_template(locale, key), &values);
    if key == "diag.reference.unresolved"
        && let Some(note) = reference_whitespace_note(locale, args)
    {
        text.push(' ');
        text.push_str(&note);
    }
    if key == "diag.fixed4_unknown"
        && let Some(legend) = fixed4_marker_legend(locale, args)
    {
        text.push(' ');
        text.push_str(legend);
    }
    if let Some(detail) = plugin_semantic_detail(locale, key, args) {
        text.push_str("\n\n");
        text.push_str(&detail);
    }
    compact_plugin_hover_spacing(key, text)
}

fn compact_plugin_hover_spacing(key: &str, text: String) -> String {
    if !key.starts_with("plugin.") || !key.contains(".hover") {
        return text;
    }
    text.lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn value_usize(args: &Map<String, Value>, key: &str) -> usize {
    args.get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or_default()
}

fn value_bool(args: &Map<String, Value>, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Whitespace is semantic diagnostic data, not an English suffix supplied by
/// the validator.  Build the locale sentence here from counts and the trimmed
/// value so every client keeps the same correction advice.
fn reference_whitespace_note(locale: Locale, args: &Map<String, Value>) -> Option<String> {
    let leading = value_usize(args, "leadingWhitespace");
    let trailing = value_usize(args, "trailingWhitespace");
    let trimmed = args
        .get("trimmedValue")
        .and_then(Value::as_str)
        .unwrap_or("");
    if trimmed.is_empty() || (leading == 0 && trailing == 0) {
        return None;
    }
    Some(match locale {
        Locale::EnUs => format!(
            "This value has {leading} leading and {trailing} trailing whitespace character(s). Remove them to match '{trimmed}'."
        ),
        Locale::KoKr => format!(
            "입력값 앞에 공백이 {leading}개, 뒤에 공백이 {trailing}개 있습니다. '{trimmed}'와 일치시키려면 제거하세요."
        ),
        Locale::ZhCn => format!(
            "该值包含 {leading} 个前导空白和 {trailing} 个尾随空白。请删除它们以匹配“{trimmed}”。"
        ),
        Locale::ZhTw => format!(
            "此值包含 {leading} 個前置空白和 {trailing} 個尾端空白。請刪除它們以符合「{trimmed}」。"
        ),
        Locale::DeDe => format!(
            "Dieser Wert enthält {leading} führende und {trailing} nachgestellte Leerzeichen. Entfernen Sie sie, damit er mit '{trimmed}' übereinstimmt."
        ),
        Locale::EsEs => format!(
            "Este valor contiene {leading} espacios iniciales y {trailing} finales. Elimínelos para que coincida con '{trimmed}'."
        ),
        Locale::FrFr => format!(
            "Cette valeur contient {leading} espaces au début et {trailing} à la fin. Supprimez-les pour correspondre à '{trimmed}'."
        ),
        Locale::ItIt => format!(
            "Questo valore contiene {leading} spazi iniziali e {trailing} finali. Rimuovili per farlo corrispondere a '{trimmed}'."
        ),
        Locale::PlPl => format!(
            "Ta wartość zawiera {leading} początkowych i {trailing} końcowych białych znaków. Usuń je, aby dopasować '{trimmed}'."
        ),
        Locale::EsMx => format!(
            "Este valor contiene {leading} espacios al inicio y {trailing} al final. Quítalos para que coincida con '{trimmed}'."
        ),
        Locale::JaJp => format!(
            "この値には先頭に {leading} 個、末尾に {trailing} 個の空白があります。'{trimmed}' に合わせるには削除してください。"
        ),
        Locale::PtBr => format!(
            "Este valor contém {leading} espaços no início e {trailing} no fim. Remova-os para corresponder a '{trimmed}'."
        ),
        Locale::RuRu => format!(
            "Это значение содержит {leading} начальных и {trailing} конечных пробельных символов. Удалите их, чтобы получить '{trimmed}'."
        ),
    })
}

fn fixed4_marker_legend(locale: Locale, args: &Map<String, Value>) -> Option<&'static str> {
    let space = value_bool(args, "hasSpaceMarker");
    let tab = value_bool(args, "hasTabMarker");
    if !space && !tab {
        return None;
    }
    Some(match (locale, space, tab) {
        (Locale::EnUs, true, true) => "␠ = space; ⇥ = tab.",
        (Locale::EnUs, true, false) => "␠ = space.",
        (Locale::EnUs, false, true) => "⇥ = tab.",
        (Locale::KoKr, true, true) => "␠는 공백이고 ⇥는 탭입니다.",
        (Locale::KoKr, true, false) => "␠는 공백입니다.",
        (Locale::KoKr, false, true) => "⇥는 탭입니다.",
        (Locale::ZhCn, true, true) => "␠ 表示空格，⇥ 表示制表符。",
        (Locale::ZhCn, true, false) => "␠ 表示空格。",
        (Locale::ZhCn, false, true) => "⇥ 表示制表符。",
        (Locale::ZhTw, true, true) => "␠ 代表空白，⇥ 代表定位字元。",
        (Locale::ZhTw, true, false) => "␠ 代表空白。",
        (Locale::ZhTw, false, true) => "⇥ 代表定位字元。",
        (Locale::DeDe, true, true) => "␠ steht für Leerzeichen; ⇥ steht für Tabulator.",
        (Locale::DeDe, true, false) => "␠ steht für Leerzeichen.",
        (Locale::DeDe, false, true) => "⇥ steht für Tabulator.",
        (Locale::EsEs | Locale::EsMx, true, true) => {
            "␠ representa un espacio; ⇥ representa un tabulador."
        }
        (Locale::EsEs | Locale::EsMx, true, false) => "␠ representa un espacio.",
        (Locale::EsEs | Locale::EsMx, false, true) => "⇥ representa un tabulador.",
        (Locale::FrFr, true, true) => "␠ représente une espace ; ⇥ représente une tabulation.",
        (Locale::FrFr, true, false) => "␠ représente une espace.",
        (Locale::FrFr, false, true) => "⇥ représente une tabulation.",
        (Locale::ItIt, true, true) => "␠ indica uno spazio; ⇥ indica una tabulazione.",
        (Locale::ItIt, true, false) => "␠ indica uno spazio.",
        (Locale::ItIt, false, true) => "⇥ indica una tabulazione.",
        (Locale::PlPl, true, true) => "␠ oznacza spację; ⇥ oznacza tabulator.",
        (Locale::PlPl, true, false) => "␠ oznacza spację.",
        (Locale::PlPl, false, true) => "⇥ oznacza tabulator.",
        (Locale::JaJp, true, true) => "␠ は空白、⇥ はタブを表します。",
        (Locale::JaJp, true, false) => "␠ は空白を表します。",
        (Locale::JaJp, false, true) => "⇥ はタブを表します。",
        (Locale::PtBr, true, true) => "␠ representa um espaço; ⇥ representa uma tabulação.",
        (Locale::PtBr, true, false) => "␠ representa um espaço.",
        (Locale::PtBr, false, true) => "⇥ representa uma tabulação.",
        (Locale::RuRu, true, true) => "␠ обозначает пробел; ⇥ обозначает табуляцию.",
        (Locale::RuRu, true, false) => "␠ обозначает пробел.",
        (Locale::RuRu, false, true) => "⇥ обозначает табуляцию.",
        _ => unreachable!("all boolean combinations are handled"),
    })
}

fn argument_text(args: &Map<String, Value>, key: &str) -> String {
    args.get(key).map(display_value).unwrap_or_default()
}

/// The original plugin prose carries runtime facts that cannot be inferred
/// from a whole-cell value alone.  Keep those facts as named arguments and
/// render an explicit locale sentence for every non-English client.
fn plugin_semantic_detail(locale: Locale, key: &str, args: &Map<String, Value>) -> Option<String> {
    let item_column = argument_text(args, "itemColumn");
    let modifier = argument_text(args, "modifier");
    let stored = argument_text(args, "stored");
    let stopped_at = argument_text(args, "stoppedAt");
    let utf8_length = argument_text(args, "utf8ByteLength");
    match key {
        "plugin.tc-prob.noncanonical" => Some(match locale {
            Locale::EnUs => {
                format!("The value is not a whole number, so `{item_column}` may be skipped.")
            }
            Locale::KoKr => {
                format!("정수가 아닌 값 때문에 `{item_column}` 항목이 생략될 수 있습니다.")
            }
            Locale::ZhCn => format!("该值不是整数，`{item_column}` 项可能会被跳过。"),
            Locale::ZhTw => format!("此值不是整數，`{item_column}` 項目可能會被略過。"),
            Locale::DeDe => format!(
                "Der Wert ist keine ganze Zahl; der Eintrag `{item_column}` kann übersprungen werden."
            ),
            Locale::EsEs => format!(
                "El valor no es un número entero; la entrada `{item_column}` podría omitirse."
            ),
            Locale::FrFr => format!(
                "La valeur n’est pas un entier ; l’entrée `{item_column}` peut être ignorée."
            ),
            Locale::ItIt => format!(
                "Il valore non è un intero; la voce `{item_column}` potrebbe essere ignorata."
            ),
            Locale::PlPl => format!(
                "Wartość nie jest liczbą całkowitą; wpis `{item_column}` może zostać pominięty."
            ),
            Locale::EsMx => {
                format!("El valor no es un entero; la entrada `{item_column}` podría omitirse.")
            }
            Locale::JaJp => {
                format!("値が整数ではないため、`{item_column}` の項目が省略される場合があります。")
            }
            Locale::PtBr => {
                format!("O valor não é um inteiro; a entrada `{item_column}` pode ser omitida.")
            }
            Locale::RuRu => format!(
                "Значение не является целым числом, поэтому запись `{item_column}` может быть пропущена."
            ),
        }),
        "plugin.tc-item.modifier-range" => Some(match locale {
            Locale::EnUs => format!(
                "Modifier `{modifier}` in `{item_column}` is stored as {stored}. Replace it with the intended number."
            ),
            Locale::KoKr => format!(
                "게임에서 저장되는 `{item_column}` 수정자: `{modifier}` → {stored}. 의도한 숫자로 바꾸세요."
            ),
            Locale::ZhCn => format!(
                "`{item_column}` 中的修饰符 `{modifier}` 会被游戏转换为 {stored}。请改为预期的数字。"
            ),
            Locale::ZhTw => format!(
                "`{item_column}` 中的修飾符 `{modifier}` 會被遊戲轉換為 {stored}。請改為預期的數字。"
            ),
            Locale::DeDe => format!(
                "Der Modifikator `{modifier}` in `{item_column}` wird vom Spiel als {stored} gespeichert. Ersetzen Sie ihn durch die gewünschte Zahl."
            ),
            Locale::EsEs => format!(
                "El modificador `{modifier}` de `{item_column}` se guarda en el juego como {stored}. Sustitúyalo por el número deseado."
            ),
            Locale::FrFr => format!(
                "Le modificateur `{modifier}` de `{item_column}` est enregistré comme {stored} par le jeu. Remplacez-le par le nombre voulu."
            ),
            Locale::ItIt => format!(
                "Il modificatore `{modifier}` in `{item_column}` viene memorizzato dal gioco come {stored}. Sostituiscilo con il numero desiderato."
            ),
            Locale::PlPl => format!(
                "Modyfikator `{modifier}` w `{item_column}` jest zapisywany przez grę jako {stored}. Zastąp go zamierzoną liczbą."
            ),
            Locale::EsMx => format!(
                "El modificador `{modifier}` de `{item_column}` se guarda en el juego como {stored}. Cámbialo por el número que quieres usar."
            ),
            Locale::JaJp => format!(
                "`{item_column}` の修飾子 `{modifier}` はゲーム内で {stored} として保存されます。意図した数値に置き換えてください。"
            ),
            Locale::PtBr => format!(
                "O modificador `{modifier}` em `{item_column}` é armazenado pelo jogo como {stored}. Substitua-o pelo número desejado."
            ),
            Locale::RuRu => format!(
                "Модификатор `{modifier}` в `{item_column}` сохраняется игрой как {stored}. Замените его нужным числом."
            ),
        }),
        "plugin.tc-item.ignored-suffix" => Some(match locale {
            Locale::EnUs => format!(
                "The game stops at `{stopped_at}` in `{item_column}`. The base and earlier modifiers still work; that text and everything after it are ignored."
            ),
            Locale::KoKr => format!(
                "게임은 `{item_column}`의 `{stopped_at}`에서 읽기를 멈춥니다. 그 앞의 Base와 수정자는 적용되지만, 해당 텍스트와 그 뒤의 내용은 무시됩니다."
            ),
            Locale::ZhCn => format!(
                "游戏会在 `{item_column}` 的 `{stopped_at}` 处停止读取。此前的 Base 和修饰符仍生效；该文本及其后的内容会被忽略。"
            ),
            Locale::ZhTw => format!(
                "遊戲會在 `{item_column}` 的 `{stopped_at}` 處停止讀取。此前的 Base 和修飾符仍生效；該文字及其後的內容會被忽略。"
            ),
            Locale::DeDe => format!(
                "Das Spiel stoppt in `{item_column}` bei `{stopped_at}`. Base und frühere Modifikatoren wirken weiter; dieser Text und alles danach werden ignoriert."
            ),
            Locale::EsEs => format!(
                "El juego deja de leer `{item_column}` en `{stopped_at}`. La Base y los modificadores anteriores siguen funcionando; ese texto y lo posterior se ignoran."
            ),
            Locale::FrFr => format!(
                "Le jeu cesse de lire `{item_column}` à `{stopped_at}`. La Base et les modificateurs précédents restent actifs ; ce texte et la suite sont ignorés."
            ),
            Locale::ItIt => format!(
                "Il gioco smette di leggere `{item_column}` in corrispondenza di `{stopped_at}`. La Base e i modificatori precedenti restano attivi; quel testo e il resto vengono ignorati."
            ),
            Locale::PlPl => format!(
                "Gra przestaje odczytywać `{item_column}` przy `{stopped_at}`. Base i wcześniejsze modyfikatory nadal działają; ten tekst i dalsza część są ignorowane."
            ),
            Locale::EsMx => format!(
                "El juego deja de leer `{item_column}` en `{stopped_at}`. La Base y los modificadores previos siguen funcionando; ese texto y lo que sigue se ignoran."
            ),
            Locale::JaJp => format!(
                "ゲームは `{item_column}` の `{stopped_at}` で読み取りを停止します。Base と前の修飾子は有効ですが、その文字列以降は無視されます。"
            ),
            Locale::PtBr => format!(
                "O jogo para de ler `{item_column}` em `{stopped_at}`. A Base e os modificadores anteriores continuam funcionando; esse texto e o restante são ignorados."
            ),
            Locale::RuRu => format!(
                "Игра прекращает чтение `{item_column}` на `{stopped_at}`. Base и предыдущие модификаторы продолжают работать; этот текст и всё после него игнорируются."
            ),
        }),
        "plugin.tc-item.field-width" => Some(match locale {
            Locale::EnUs => {
                format!("`{item_column}` uses {utf8_length} UTF-8 bytes. Keep it below 64 bytes.")
            }
            Locale::KoKr => format!(
                "`{item_column}`의 UTF-8 길이는 {utf8_length}바이트입니다. 64바이트 미만으로 줄이세요."
            ),
            Locale::ZhCn => format!(
                "`{item_column}` 使用了 {utf8_length} 个 UTF-8 字节。请保持在 64 字节以下。"
            ),
            Locale::ZhTw => format!(
                "`{item_column}` 使用了 {utf8_length} 個 UTF-8 位元組。請保持在 64 位元組以下。"
            ),
            Locale::DeDe => format!(
                "`{item_column}` verwendet {utf8_length} UTF-8-Bytes. Halten Sie den Wert unter 64 Bytes."
            ),
            Locale::EsEs => format!(
                "`{item_column}` usa {utf8_length} bytes UTF-8. Mantenga el valor por debajo de 64 bytes."
            ),
            Locale::FrFr => format!(
                "`{item_column}` utilise {utf8_length} octets UTF-8. Gardez la valeur sous 64 octets."
            ),
            Locale::ItIt => format!(
                "`{item_column}` usa {utf8_length} byte UTF-8. Mantieni il valore sotto 64 byte."
            ),
            Locale::PlPl => format!(
                "`{item_column}` używa {utf8_length} bajtów UTF-8. Zachowaj wartość poniżej 64 bajtów."
            ),
            Locale::EsMx => format!(
                "`{item_column}` usa {utf8_length} bytes UTF-8. Mantén el valor por debajo de 64 bytes."
            ),
            Locale::JaJp => format!(
                "`{item_column}` は UTF-8 で {utf8_length} バイトです。64 バイト未満にしてください。"
            ),
            Locale::PtBr => format!(
                "`{item_column}` usa {utf8_length} bytes UTF-8. Mantenha o valor abaixo de 64 bytes."
            ),
            Locale::RuRu => format!(
                "`{item_column}` занимает {utf8_length} байт UTF-8. Сократите значение до менее чем 64 байт."
            ),
        }),
        "plugin.treasure-class.hover" => {
            let modifiers = argument_text(args, "modifierStorage");
            let ignored = argument_text(args, "ignoredSuffix");
            let picks = argument_text(args, "picks");
            let per_roll = argument_text(args, "perRollChance");
            let at_least_once = argument_text(args, "atLeastOnceChance");
            Some(match locale {
                Locale::EnUs => format!(
                    "Runtime details: per-roll chance {per_roll}; at least once in {picks} picks {at_least_once}; modifier storage {modifiers}; ignored suffix `{ignored}`."
                ),
                Locale::KoKr => format!(
                    "**게임 처리 정보**\n회당 확률: {per_roll}\n{picks}회 추첨 중 한 번 이상: {at_least_once}\n수정자 저장값: {modifiers}\n무시되는 접미사: `{ignored}`"
                ),
                Locale::ZhCn => format!(
                    "**游戏处理信息**\n单次概率：{per_roll}\n{picks} 次抽取中至少一次：{at_least_once}\n修饰符存储值：{modifiers}\n忽略的后缀：`{ignored}`"
                ),
                Locale::ZhTw => format!(
                    "**遊戲處理資訊**\n單次機率：{per_roll}\n{picks} 次抽取中至少一次：{at_least_once}\n修飾符儲存值：{modifiers}\n忽略的字尾：`{ignored}`"
                ),
                Locale::DeDe => format!(
                    "**Laufzeitdetails**\nChance pro Ziehung: {per_roll}\nMindestens einmal bei {picks} Ziehungen: {at_least_once}\nGespeicherte Modifikatoren: {modifiers}\nIgnorierter Suffix: `{ignored}`"
                ),
                Locale::EsEs => format!(
                    "**Detalles de ejecución**\nProbabilidad por tirada: {per_roll}\nAl menos una vez en {picks} tiradas: {at_least_once}\nValores almacenados de modificadores: {modifiers}\nSufijo ignorado: `{ignored}`"
                ),
                Locale::FrFr => format!(
                    "**Détails d’exécution**\nChance par tirage : {per_roll}\nAu moins une fois en {picks} tirages : {at_least_once}\nValeurs stockées des modificateurs : {modifiers}\nSuffixe ignoré : `{ignored}`"
                ),
                Locale::ItIt => format!(
                    "**Dettagli di esecuzione**\nProbabilità per estrazione: {per_roll}\nAlmeno una volta in {picks} estrazioni: {at_least_once}\nValori memorizzati dei modificatori: {modifiers}\nSuffisso ignorato: `{ignored}`"
                ),
                Locale::PlPl => format!(
                    "**Szczegóły działania gry**\nSzansa na losowanie: {per_roll}\nCo najmniej raz w {picks} losowaniach: {at_least_once}\nZapisane wartości modyfikatorów: {modifiers}\nIgnorowany sufiks: `{ignored}`"
                ),
                Locale::EsMx => format!(
                    "**Detalles de ejecución**\nProbabilidad por tirada: {per_roll}\nAl menos una vez en {picks} tiradas: {at_least_once}\nValores guardados de modificadores: {modifiers}\nSufijo ignorado: `{ignored}`"
                ),
                Locale::JaJp => format!(
                    "**ゲーム処理の詳細**\n1 回ごとの確率: {per_roll}\n{picks} 回の抽選で少なくとも 1 回: {at_least_once}\n修飾子の保存値: {modifiers}\n無視される接尾辞: `{ignored}`"
                ),
                Locale::PtBr => format!(
                    "**Detalhes de execução**\nChance por sorteio: {per_roll}\nPelo menos uma vez em {picks} sorteios: {at_least_once}\nValores armazenados dos modificadores: {modifiers}\nSufixo ignorado: `{ignored}`"
                ),
                Locale::RuRu => format!(
                    "**Сведения об обработке игрой**\nШанс за бросок: {per_roll}\nХотя бы раз за {picks} бросков: {at_least_once}\nСохранённые значения модификаторов: {modifiers}\nИгнорируемый суффикс: `{ignored}`"
                ),
            })
        }
        _ => None,
    }
}

fn catalog_template(locale: Locale, key: &str) -> String {
    if is_bundled_plugin_key(key) {
        return bundled_plugin_template(locale, key);
    }
    if let Some(template) = operational_log_template(locale, key) {
        return template;
    }
    if let Some(template) = boolean_template(locale, key) {
        return template.to_string();
    }
    match locale {
        Locale::EnUs => english(key).to_string(),
        Locale::KoKr => korean(key).to_string(),
        Locale::ZhCn => chinese_simplified(key).to_string(),
        Locale::ZhTw => chinese_traditional(key).to_string(),
        Locale::DeDe => german(key).to_string(),
        Locale::EsEs => spanish(key).to_string(),
        Locale::FrFr => french(key).to_string(),
        Locale::ItIt => italian(key).to_string(),
        Locale::PlPl => polish(key).to_string(),
        Locale::EsMx => mexican_spanish(key).to_string(),
        Locale::JaJp => japanese(key).to_string(),
        Locale::PtBr => brazilian_portuguese(key).to_string(),
        Locale::RuRu => russian(key).to_string(),
    }
}

fn boolean_template(locale: Locale, key: &str) -> Option<&'static str> {
    Some(match (locale, key) {
        (Locale::EnUs, "diag.boolean.type29_invalid") => {
            "'{value}' is not a number format accepted in this field. Enter 0 to turn it off or 1 to turn it on."
        }
        (Locale::EnUs, "hover.boolean_on") => "The current value is treated as on by the game.",
        (Locale::EnUs, "hover.boolean_off") => "The current value is treated as off by the game.",
        (Locale::EnUs, "hover.boolean_off_recommendation") => {
            "The current value is treated as off by the game. Enter 1 to turn it on."
        }

        (Locale::KoKr, "diag.boolean.type29_invalid") => {
            "'{value}'는 이 칸에서 사용할 수 있는 숫자 형식이 아닙니다. 끄려면 0, 켜려면 1을 입력하세요."
        }
        (Locale::KoKr, "hover.boolean_on") => "현재 값은 게임에서 켜짐으로 처리됩니다.",
        (Locale::KoKr, "hover.boolean_off") => "현재 값은 게임에서 꺼짐으로 처리됩니다.",
        (Locale::KoKr, "hover.boolean_off_recommendation") => {
            "현재 값은 게임에서 꺼짐으로 처리됩니다. 켜려면 1을 입력하세요."
        }

        (Locale::ZhCn, "diag.boolean.type29_invalid") => {
            "“{value}”不是此字段可接受的数字格式。要关闭请输入 0，要开启请输入 1。"
        }
        (Locale::ZhCn, "hover.boolean_on") => "当前值在游戏中会被视为开启。",
        (Locale::ZhCn, "hover.boolean_off") => "当前值在游戏中会被视为关闭。",
        (Locale::ZhCn, "hover.boolean_off_recommendation") => {
            "当前值在游戏中会被视为关闭。要开启请输入 1。"
        }

        (Locale::ZhTw, "diag.boolean.type29_invalid") => {
            "「{value}」不是此欄位可接受的數字格式。若要關閉請輸入 0，若要開啟請輸入 1。"
        }
        (Locale::ZhTw, "hover.boolean_on") => "目前值在遊戲中會被視為開啟。",
        (Locale::ZhTw, "hover.boolean_off") => "目前值在遊戲中會被視為關閉。",
        (Locale::ZhTw, "hover.boolean_off_recommendation") => {
            "目前值在遊戲中會被視為關閉。若要開啟請輸入 1。"
        }

        (Locale::DeDe, "diag.boolean.type29_invalid") => {
            "„{value}“ ist kein Zahlenformat, das in diesem Feld akzeptiert wird. Geben Sie 0 zum Ausschalten oder 1 zum Einschalten ein."
        }
        (Locale::DeDe, "hover.boolean_on") => {
            "Der aktuelle Wert wird vom Spiel als eingeschaltet behandelt."
        }
        (Locale::DeDe, "hover.boolean_off") => {
            "Der aktuelle Wert wird vom Spiel als ausgeschaltet behandelt."
        }
        (Locale::DeDe, "hover.boolean_off_recommendation") => {
            "Der aktuelle Wert wird vom Spiel als ausgeschaltet behandelt. Geben Sie zum Einschalten 1 ein."
        }

        (Locale::EsEs, "diag.boolean.type29_invalid") => {
            "'{value}' no tiene un formato numérico aceptado en este campo. Introduce 0 para desactivarlo o 1 para activarlo."
        }
        (Locale::EsEs, "hover.boolean_on") => "El juego trata el valor actual como activado.",
        (Locale::EsEs, "hover.boolean_off") => "El juego trata el valor actual como desactivado.",
        (Locale::EsEs, "hover.boolean_off_recommendation") => {
            "El juego trata el valor actual como desactivado. Introduce 1 para activarlo."
        }

        (Locale::FrFr, "diag.boolean.type29_invalid") => {
            "« {value} » n’est pas un format numérique accepté dans ce champ. Saisissez 0 pour désactiver ou 1 pour activer."
        }
        (Locale::FrFr, "hover.boolean_on") => "Le jeu traite la valeur actuelle comme activée.",
        (Locale::FrFr, "hover.boolean_off") => "Le jeu traite la valeur actuelle comme désactivée.",
        (Locale::FrFr, "hover.boolean_off_recommendation") => {
            "Le jeu traite la valeur actuelle comme désactivée. Saisissez 1 pour l’activer."
        }

        (Locale::ItIt, "diag.boolean.type29_invalid") => {
            "'{value}' non è un formato numerico accettato in questo campo. Inserisci 0 per disattivare o 1 per attivarlo."
        }
        (Locale::ItIt, "hover.boolean_on") => "Il gioco considera attivato il valore corrente.",
        (Locale::ItIt, "hover.boolean_off") => "Il gioco considera disattivato il valore corrente.",
        (Locale::ItIt, "hover.boolean_off_recommendation") => {
            "Il gioco considera disattivato il valore corrente. Inserisci 1 per attivarlo."
        }

        (Locale::PlPl, "diag.boolean.type29_invalid") => {
            "„{value}” nie ma formatu liczbowego akceptowanego w tym polu. Wpisz 0, aby wyłączyć, lub 1, aby włączyć."
        }
        (Locale::PlPl, "hover.boolean_on") => "Gra traktuje bieżącą wartość jako włączoną.",
        (Locale::PlPl, "hover.boolean_off") => "Gra traktuje bieżącą wartość jako wyłączoną.",
        (Locale::PlPl, "hover.boolean_off_recommendation") => {
            "Gra traktuje bieżącą wartość jako wyłączoną. Wpisz 1, aby włączyć."
        }

        (Locale::EsMx, "diag.boolean.type29_invalid") => {
            "'{value}' no tiene un formato numérico aceptado en este campo. Ingresa 0 para desactivarlo o 1 para activarlo."
        }
        (Locale::EsMx, "hover.boolean_on") => "El juego trata el valor actual como activado.",
        (Locale::EsMx, "hover.boolean_off") => "El juego trata el valor actual como desactivado.",
        (Locale::EsMx, "hover.boolean_off_recommendation") => {
            "El juego trata el valor actual como desactivado. Ingresa 1 para activarlo."
        }

        (Locale::JaJp, "diag.boolean.type29_invalid") => {
            "「{value}」は、このフィールドで使用できる数値形式ではありません。オフにするには 0、オンにするには 1 を入力してください。"
        }
        (Locale::JaJp, "hover.boolean_on") => "現在の値はゲームでオンとして扱われます。",
        (Locale::JaJp, "hover.boolean_off") => "現在の値はゲームでオフとして扱われます。",
        (Locale::JaJp, "hover.boolean_off_recommendation") => {
            "現在の値はゲームでオフとして扱われます。オンにするには 1 を入力してください。"
        }

        (Locale::PtBr, "diag.boolean.type29_invalid") => {
            "'{value}' não está em um formato numérico aceito neste campo. Digite 0 para desativar ou 1 para ativar."
        }
        (Locale::PtBr, "hover.boolean_on") => "O jogo trata o valor atual como ativado.",
        (Locale::PtBr, "hover.boolean_off") => "O jogo trata o valor atual como desativado.",
        (Locale::PtBr, "hover.boolean_off_recommendation") => {
            "O jogo trata o valor atual como desativado. Digite 1 para ativar."
        }

        (Locale::RuRu, "diag.boolean.type29_invalid") => {
            "«{value}» имеет формат числа, который не принимается в этом поле. Введите 0, чтобы выключить, или 1, чтобы включить."
        }
        (Locale::RuRu, "hover.boolean_on") => "Игра обрабатывает текущее значение как включённое.",
        (Locale::RuRu, "hover.boolean_off") => {
            "Игра обрабатывает текущее значение как выключенное."
        }
        (Locale::RuRu, "hover.boolean_off_recommendation") => {
            "Игра обрабатывает текущее значение как выключенное. Введите 1, чтобы включить."
        }
        _ => return None,
    })
}

/// Operational logs are keyed just like diagnostics, but share a compact
/// locale-specific sentence frame. Their named values are data (paths,
/// counts, URIs, and underlying I/O errors), never product-authored English.
fn operational_log_template(locale: Locale, key: &str) -> Option<String> {
    let _placeholders: &[&str] = match key {
        "log.watch_registration_prepare_failed" => &["error"],
        "log.file_watching_unavailable" => &["baseUri", "error"],
        "log.json_watch_retire_failed" => &["error"],
        "log.workspace_scan_root_missing" | "log.reference_fallback_no_version" => &[],
        "log.workspace_scan_root_not_file" | "log.scan_reference_root_not_file" => &["rootUri"],
        "log.workspace_scan_failed"
        | "log.scan_reference_root_failed"
        | "log.scan_task_failed"
        | "log.scan_background_parse_failed"
        | "log.schema_task_stopped"
        | "log.reference_fallback_disabled"
        | "log.reference_data_load_failed" => &["error"],
        "log.scan_path_uri_conversion_failed" | "log.scan_reference_path_uri_conversion_failed" => {
            &[]
        }
        "log.workspace_scan_skipped" => &["path", "reason"],
        "log.workspace_indexed" => &["count", "skippedCount"],
        "log.reference_tables_loaded" => &["count", "version", "digest"],
        "log.did_close_restore_failed" => &["uri", "error"],
        "log.effective_config" => &["summary"],
        "cli.single_shot_workspace_required" | "cli.reference_fallback_disabled_for_run" => &[],
        "cli.plugin_runtime_startup_failed"
        | "cli.schema_task_panicked"
        | "cli.reference_fallback_disabled"
        | "cli.tcp_plugin_runtime_startup_failed" => &["error"],
        "cli.plugins_loaded" => &["count"],
        "cli.reference_tables_loaded" => &["count", "gameVersion", "checksum"],
        "cli.workspace_read_failed" | "cli.path_skipped" => &["path", "error"],
        "cli.check_summary" => &[
            "errors",
            "warnings",
            "infos",
            "hints",
            "fileCount",
            "parsedFileCount",
        ],
        _ => return None,
    };
    if locale == Locale::EnUs {
        return Some(
            match key {
                "log.watch_registration_prepare_failed" => {
                    "Could not prepare watched-files registration: {error}"
                }
                "log.file_watching_unavailable" => {
                    "File watching is unavailable for '{baseUri}': {error}"
                }
                "log.json_watch_retire_failed" => {
                    "Could not retire stale JSON file watchers; the next JSON scope sync will retry: {error}"
                }
                "log.workspace_scan_root_missing" => {
                    "Workspace scan failed: initialize did not provide a root URI"
                }
                "log.workspace_scan_root_not_file" => {
                    "Workspace scan failed: root URI is not a file path: {rootUri}"
                }
                "log.workspace_scan_failed" => "Workspace scan failed: {error}",
                "log.scan_path_uri_conversion_failed" => "Could not convert scan path to a URI",
                "log.scan_reference_path_uri_conversion_failed" => {
                    "Could not convert reference scan path to a URI"
                }
                "log.scan_reference_root_failed" => "Reference scan failed: {error}",
                "log.scan_reference_root_not_file" => {
                    "Reference root URI is not a file path: {rootUri}"
                }
                "log.scan_task_failed" => "Workspace scan task failed: {error}",
                "log.workspace_scan_skipped" => "Workspace scan skipped '{path}': {reason}",
                "log.workspace_indexed" => {
                    "Indexed {count} workspace files; skipped {skippedCount} path(s)."
                }
                "log.scan_background_parse_failed" => "Background scan parse failed: {error}",
                "log.schema_task_stopped" => {
                    "Could not load the schema because its background task stopped: {error}"
                }
                "log.reference_tables_loaded" => {
                    "Loaded {count} hidden reference tables for game version {version} ({digest})."
                }
                "log.reference_fallback_no_version" => {
                    "Bundled reference fallback disabled: no explicit or inferable game version."
                }
                "log.reference_fallback_disabled" => {
                    "Bundled reference fallback disabled for this session: {error}"
                }
                "log.reference_data_load_failed" => {
                    "Bundled reference data could not be loaded: {error}"
                }
                "log.did_close_restore_failed" => "didClose disk restore failed for {uri}: {error}",
                "log.effective_config" => "Effective vector-lsp config: {summary}",
                "cli.single_shot_workspace_required" => {
                    "error: single_shot mode requires `workspace_path` in config"
                }
                "cli.plugin_runtime_startup_failed" => "error: {error}",
                "cli.schema_task_panicked" => "error: schema task panicked: {error}",
                "cli.plugins_loaded" => "Loaded {count} plugin file(s).",
                "cli.reference_fallback_disabled" => {
                    "warning: bundled reference fallback disabled for this run: {error}"
                }
                "cli.reference_tables_loaded" => {
                    "Loaded {count} hidden reference tables for game version {gameVersion} ({checksum})."
                }
                "cli.reference_fallback_disabled_for_run" => {
                    "Bundled reference fallback is disabled for this run."
                }
                "cli.workspace_read_failed" => {
                    "error: cannot read workspace directory '{path}': {error}"
                }
                "cli.path_skipped" => "warning: skipping '{path}': {error}",
                "cli.check_summary" => {
                    "{errors} error(s), {warnings} warning(s), {infos} info, {hints} hint diagnostic(s) across {fileCount} file(s); {parsedFileCount} parsed file(s)."
                }
                "cli.tcp_plugin_runtime_startup_failed" => {
                    "vector-lsp: TCP plugin runtime startup failed: {error}"
                }
                _ => unreachable!("registered operational key must have an English baseline"),
            }
            .to_string(),
        );
    }
    if let Some(template) = crate::i18n_operations_cjk::template(locale, key) {
        return Some(template.to_string());
    }
    if let Some(template) = crate::i18n_operations_europe_a::template(locale, key) {
        return Some(template.to_string());
    }
    // Mexican Spanish intentionally uses the full esES operational catalog;
    // it is a natural Spanish rendering, not an English compatibility fallback.
    if locale == Locale::EsMx
        && let Some(template) = crate::i18n_operations_europe_a::template(Locale::EsEs, key)
    {
        return Some(template.to_string());
    }
    if let Some(template) = crate::i18n_operations_pl_ru::template(locale, key) {
        return Some(template.to_string());
    }
    if let Some(template) = crate::i18n_operations_ptbr::template(locale, key) {
        return Some(template.to_string());
    }
    unreachable!("every registered operational key requires a detailed locale template")
}

/// Bundled plugin rules retain their code and all named arguments as data. The
/// rendered wording is deliberately generated here, never from plugin English
/// compatibility text. This prevents a non-English client from falling back
/// to `legacyMessage` or `legacyContent`.
fn bundled_plugin_template(locale: Locale, key: &str) -> String {
    if locale == Locale::PlPl {
        return plugin_detail_pl(key)
            .expect("every bundled plugin key has a Polish translation")
            .to_string();
    }
    if locale == Locale::ItIt {
        return plugin_detail_it(key)
            .expect("every bundled plugin key has an Italian translation")
            .to_string();
    }
    if locale == Locale::FrFr {
        return plugin_detail_fr(key)
            .expect("every bundled plugin key has a French translation")
            .to_string();
    }
    if locale == Locale::DeDe {
        return plugin_detail_de(key)
            .expect("every bundled plugin key has a German translation")
            .to_string();
    }
    if locale == Locale::EsEs {
        return plugin_detail_es(key)
            .expect("every bundled plugin key has a Spanish translation")
            .to_string();
    }
    if locale == Locale::EsMx {
        return plugin_detail_es_mx(key)
            .expect("every bundled plugin key has a Mexican Spanish translation")
            .to_string();
    }
    if locale == Locale::KoKr {
        return plugin_detail_ko(key)
            .expect("every bundled plugin key has a Korean translation")
            .to_string();
    }
    if locale == Locale::ZhCn {
        return plugin_detail_zh_cn(key)
            .expect("every bundled plugin key has a Simplified Chinese translation")
            .to_string();
    }
    if locale == Locale::ZhTw {
        return plugin_detail_zh_tw(key)
            .expect("every bundled plugin key has a Traditional Chinese translation")
            .to_string();
    }
    if locale == Locale::JaJp {
        return plugin_detail_ja(key)
            .expect("every bundled plugin key has a Japanese translation")
            .to_string();
    }
    if locale == Locale::PtBr {
        return plugin_detail_pt_br(key)
            .expect("every bundled plugin key has a Brazilian Portuguese translation")
            .to_string();
    }
    if locale == Locale::RuRu {
        return plugin_detail_ru(key)
            .expect("every bundled plugin key has a Russian translation")
            .to_string();
    }
    let category = if key.contains("calc") || key.starts_with("plugin.unknown") {
        localized_plugin_label(locale, "calculation formula")
    } else if key.contains("cube-input") {
        localized_plugin_label(locale, "cube input")
    } else if key.contains("cube-output") {
        localized_plugin_label(locale, "cube output")
    } else if key.contains("treasure-class") || key.contains("tc-") {
        localized_plugin_label(locale, "treasure class")
    } else if key.contains("property") {
        localized_plugin_label(locale, "property code")
    } else if key.contains("item") {
        localized_plugin_label(locale, "item reference")
    } else {
        localized_plugin_label(locale, "enumeration")
    };
    let action = if key.contains("hover") {
        localized_plugin_label(locale, "details")
    } else if key.contains("unresolved") || key.contains("invalid") || key.contains("unknown") {
        localized_plugin_label(locale, "invalid reference")
    } else if key.contains("ignored") || key.contains("omission") || key.contains("gap") {
        localized_plugin_label(locale, "runtime behavior")
    } else {
        localized_plugin_label(locale, "validation result")
    };
    let placeholders = plugin_detail_ko(key)
        .map(placeholder_names)
        .unwrap_or_default()
        .into_iter()
        .map(|name| format!("{{{name}}}"))
        .collect::<Vec<_>>()
        .join(" · ");
    format!("{category} — {action} ({key}): {placeholders}")
}

/// Italian wording for every diagnostic and hover emitted by the bundled
/// d2rdoc plugins. User supplied values remain named placeholders rather
/// than being translated or modified.
/// Polish wording for every diagnostic and hover emitted by the bundled d2rdoc plugins.
fn plugin_detail_pl(key: &str) -> Option<&'static str> {
    Some(match key {
        "plugin.calc.skill-param-alias" => {
            "Użyto aliasu parametru obliczenia `{alias}`; kanoniczny identyfikator to `{identifier}`. {values}"
        }
        "plugin.calc.unknown-missile-value" => {
            "Nieznana wartość pocisku `{identifier}`. Gra traktuje ją jako 0, więc ta część obliczenia nie działa."
        }
        "plugin.calc.unterminated-string" => {
            "Łańcuch w wyrażeniu obliczenia nie został zakończony. {values}"
        }
        "plugin.calc.unexpected-character" => {
            "Wyrażenie obliczenia zawiera niedozwolony znak lub symbol. Wskazana pozycja zawiera `{actual}`."
        }
        "plugin.calc.unexpected-eof" => {
            "Wyrażenie obliczenia kończy się przed ukończeniem. {values}"
        }
        "plugin.calc.unexpected-token" => {
            "Wyrażenie obliczenia zawiera nieoczekiwany token `{actual}`."
        }
        "plugin.calc.wrong-arity" => {
            "Funkcja obliczenia otrzymuje niewłaściwą liczbę argumentów. {values}"
        }
        "plugin.calc.expected-quoted-argument" => {
            "Pierwszy argument tej funkcji obliczenia musi być nazwą w cudzysłowie. {values}"
        }
        "plugin.calc.expected-dot-identifier" => {
            "Po `.` wymagany jest identyfikator obliczenia. {values}"
        }
        "plugin.calc.expected-rparen" => {
            "Wyrażenie obliczenia oczekuje `{expected}`, a nie `{actual}`."
        }
        "plugin.calc.expected-rparen.eof" => {
            "Wyrażenie obliczenia oczekuje `{expected}` przed końcem. Gra może użyć tylko poprzedniego poprawnego prefiksu."
        }
        "plugin.calc.expected-rbrack" => {
            "Wyrażenie obliczenia oczekuje `{expected}`, a nie `{actual}`."
        }
        "plugin.calc.expected-rbrack.eof" => {
            "Wyrażenie obliczenia oczekuje `{expected}` przed końcem."
        }
        "plugin.calc.expected-colon" => {
            "Warunkowe wyrażenie obliczenia oczekuje `{expected}`, a nie `{actual}`."
        }
        "plugin.calc.expected-colon.eof" => {
            "Warunkowe wyrażenie obliczenia oczekuje `{expected}` przed końcem."
        }
        "plugin.calc.expected-comma" => {
            "Argumenty funkcji oczekują `{expected}`, a nie `{actual}`."
        }
        "plugin.calc.expected-comma.eof" => "Koniec listy argumentów oczekuje `{expected}`.",
        "plugin.unknownSkill" => {
            "Nieznana nazwa umiejętności `{identifier}`. Zastąp ją dokładną nazwą z skills.txt."
        }
        "plugin.unknownMissile" => {
            "Nieznana nazwa pocisku `{identifier}`. Zastąp ją dokładną nazwą z missiles.txt."
        }
        "plugin.unknownStat" => {
            "Nieznana nazwa Stat `{identifier}`. Zastąp ją dokładną nazwą Stat z itemstatcost.txt."
        }
        "plugin.unknownCondition" => {
            "Nieznana nazwa warunku `{identifier}` w obliczeniu. Zastąp ją obsługiwanym warunkiem."
        }
        "plugin.unknownIdentifier" => {
            "Identyfikator `{identifier}` jest nieznany w bieżącym zakresie obliczenia."
        }
        "plugin.unknownSkillIdentifier" => {
            "Identyfikator obliczenia umiejętności `{identifier}` jest nieznany."
        }
        "plugin.unknownMissileIdentifier" => {
            "Identyfikator obliczenia pocisku `{identifier}` jest nieznany."
        }
        "plugin.unknownScopeIdentifier" => {
            "Identyfikator `{identifier}` jest nieznany w bieżącym zakresie obliczenia."
        }
        "plugin.calc.skilldesc-decimal-prefix" => {
            "W wyrażeniu SkillDesc `{actual}` gra używa tylko prefiksu całkowitego `{consumedPrefix}` i ignoruje `{ignoredSuffix}`."
        }
        "plugin.calc.decimal-policy" => {
            "To pole obliczenia wymaga postaci całkowitej. Gra może inaczej oceniać wyrażenia dziesiętne. {values}"
        }
        "plugin.calc.prefix-stop" => {
            "Gra odczytuje tylko rozpoznawalny prefiks tego wyrażenia obliczenia i ignoruje resztę. {values}"
        }
        "plugin.cube-input.no-inputs" => {
            "Receptura `{recipe}` w cubemain.txt, wiersz {line}, nie ma danych wejściowych."
        }
        "plugin.cube-input.invalid-numinputs" => {
            "Receptura `{recipe}` w cubemain.txt, wiersz {line}, ma nieprawidłową wartość numinputs `{value}`."
        }
        "plugin.cube-input.numinputs-mismatch" => {
            "Liczba numinputs receptury `{recipe}` w cubemain.txt, wiersz {line}, nie zgadza się: oczekiwano {expected}, znaleziono {actual}."
        }
        "plugin.cube-input.empty-base" => {
            "Podstawa wejścia w `{column}` receptury `{recipe}` w cubemain.txt, wiersz {line}, jest pusta."
        }
        "plugin.cube-input.invalid-base" => {
            "Nie znaleziono podstawy `{base}` w `{column}` receptury `{recipe}` w cubemain.txt, wiersz {line}."
        }
        "plugin.cube-input.ignored-suffix" => {
            "Receptura `{recipe}` w cubemain.txt, wiersz {line}, przestaje analizować `{column}` przy `{stoppedAt}`. Późniejszy tekst jest ignorowany."
        }
        "plugin.cube-input.u8-range" => {
            "Ilość `{quantity}` w `{column}` receptury `{recipe}` w cubemain.txt, wiersz {line}, jest poza zakresem 0..255. Gra zapisuje {storedQuantity} i używa {effectiveQuantity}."
        }
        "plugin.cube-input.hover" => {
            "**Wejście kostki**\nPodstawa: `{base}`\nModyfikatory: {modifiers}"
        }
        "plugin.cube-output.invalid-base" => {
            "Podstawa wyjścia `{value}` w `{column}` receptury `{recipe}` w cubemain.txt, wiersz {line}, jest nieprawidłowa."
        }
        "plugin.cube-output.missing-ordinal-input" => {
            "Wyjście w `{column}` receptury `{recipe}` w cubemain.txt, wiersz {line}, odwołuje się do brakującego porządkowego slotu wejściowego."
        }
        "plugin.cube-output.u8-range" => {
            "Wartość `{value}` w `{column}` receptury `{recipe}` w cubemain.txt, wiersz {line}, jest poza zakresem 0..255 i zostanie obcięta przez grę."
        }
        "plugin.cube-output.ignored-suffix" => {
            "Receptura `{recipe}` w cubemain.txt, wiersz {line}, odczytuje tylko `{value}` w `{column}` i ignoruje resztę."
        }
        "plugin.cube-output.invalid-property" => {
            "Właściwość `{value}` w `{column}` receptury `{recipe}` w cubemain.txt, wiersz {line}, jest nieprawidłowa."
        }
        "plugin.cube-output.empty-base-hover" => {
            "**Podstawa:** pusta (brakuje podstawy wyjścia kostki)"
        }
        "plugin.cube-output.invalid-hover" => {
            "**Nieprawidłowa podstawa wyjścia kostki**\n\nNie znaleziono `{base}`. Ignorowany sufiks: `{ignoredSuffix}`."
        }
        "plugin.cube-output.hover" => {
            "**Wyjście kostki**\nPodstawa: `{base}` ({kind})\nWejście: `{input}`\nModyfikatory: {modifiers}\nIgnorowany sufiks: `{ignoredSuffix}`"
        }
        "plugin.enum.hover" => {
            "**Wartość wyliczenia**\n\nWartość: `{value}`\nNazwa: {name}\nParametry: {parameters}\n\n{description}\n\nDodatkowe pola: {extraFields}"
        }
        "plugin.item-code.unresolved" => {
            "Nieznany kod przedmiotu `{value}`. Sprawdź kod i wielkość liter."
        }
        "plugin.item-code.unresolved-packed-policy" => {
            "Nie znaleziono pasującego przedmiotu. To pole nie jest interpretowane jako spakowany kod przedmiotu, a tekst zostanie obcięty; sprawdź, czy `{value}` jest zamierzoną wartością."
        }
        "plugin.item-code.unresolved-policy" => {
            "Kod przedmiotu `{value}` nie istnieje w weapons, armor ani misc. Sprawdź, czy jest to zamierzony kod."
        }
        "plugin.item-code.hover" => "**Kod przedmiotu**\n`{code}`\n{name}",
        "plugin.item-name.hover" => "{name}\n\n**Kod:** {code}",
        "plugin.property.unknown-marker" => {
            "Wartość `{value}` zaczyna się od `*`, ale nie jest znanym kodem właściwości. Używaj tego znacznika tylko dla obsługiwanych kodów właściwości."
        }
        "plugin.property.unknown-code" => {
            "Nieznany kod właściwości `{value}`. Użyj kodu z properties.txt lub propertygroups.txt."
        }
        "plugin.property.unknown-hover" => {
            "**Nieznany kod właściwości**\n\n`{value}` nie występuje w properties.txt ani propertygroups.txt."
        }
        "plugin.property.hover" => "**Właściwość**\n`{value}` (kod w {sourceFile}.txt)",
        "plugin.tc-item.after-first-gap" => {
            "Wartość `{value}` w `{column}` klasy skarbu `{treasureClass}` znajduje się po pierwszym pustym slocie przedmiotu i jest ignorowana przez grę."
        }
        "plugin.tc-prob.after-first-gap" => {
            "Prawdopodobieństwo w `{column}` klasy skarbu `{treasureClass}` znajduje się po pierwszym pustym slocie przedmiotu i jest ignorowane przez grę."
        }
        "plugin.tc-prob.orphaned" => {
            "Prawdopodobieństwo `{value}` w `{column}` klasy skarbu `{treasureClass}` nie ma odpowiadającego wpisu przedmiotu i jest ignorowane."
        }
        "plugin.tc-item.forward-reference" => {
            "Wartość `{value}` w `{column}` klasy skarbu `{treasureClass}` odwołuje się do TC zdefiniowanej później."
        }
        "plugin.tc-item.unresolved-base" => {
            "Nie znaleziono podstawy przedmiotu `{value}` w `{column}` klasy skarbu `{treasureClass}`."
        }
        "plugin.tc-prob.blank-omission" => {
            "Puste pole `{column}` klasy skarbu `{treasureClass}` pomija odpowiadający wpis."
        }
        "plugin.tc-prob.noncanonical" => {
            "Wartość `{value}` w `{column}` klasy skarbu `{treasureClass}` nie jest kanonicznym prawdopodobieństwem wpisu."
        }
        "plugin.tc-prob.nonpositive-omission" => {
            "Niedodatnia wartość `{value}` w `{column}` klasy skarbu `{treasureClass}` pomija wpis."
        }
        "plugin.tc-item.modifier-range" => {
            "Modyfikator `{value}` w `{column}` klasy skarbu `{treasureClass}` jest poza zakresem 0..65535 i zostanie obcięty."
        }
        "plugin.tc-item.ignored-suffix" => {
            "Klasa skarbu `{treasureClass}` odczytuje tylko `{value}` w `{column}` i ignoruje resztę."
        }
        "plugin.tc-item.field-width" => {
            "Wartość `{value}` w `{column}` klasy skarbu `{treasureClass}` przekracza 64 bajty UTF-8."
        }
        "plugin.treasure-class.hover-after-gap" => {
            "**Wpis klasy skarbu po luce**\n\nOd pustego slotu przedmiotu {firstEmptySlot} gra ignoruje podstawę przedmiotu `{base}`."
        }
        "plugin.treasure-class.hover" => {
            "**Klasa skarbu**\n`{base}`\n{name}\nSlot: {slot}\nLosowania: {picks}\nPrawdopodobieństwo: {probability}\nModyfikatory: {modifiers}\nIgnorowany sufiks: `{ignoredSuffix}`"
        }
        _ => return None,
    })
}

fn plugin_detail_it(key: &str) -> Option<&'static str> {
    Some(match key {
        "plugin.calc.skill-param-alias" => {
            "E stato usato l'alias del parametro di calcolo `{alias}`; l'identificatore canonico e `{identifier}`. {values}"
        }
        "plugin.calc.unknown-missile-value" => {
            "Valore missile sconosciuto `{identifier}`. Il gioco lo tratta come 0, quindi questa parte del calcolo non ha effetto."
        }
        "plugin.calc.unterminated-string" => {
            "Una stringa nell'espressione di calcolo non termina. {values}"
        }
        "plugin.calc.unexpected-character" => {
            "L'espressione di calcolo contiene un carattere o simbolo non consentito. La posizione indicata contiene `{actual}`."
        }
        "plugin.calc.unexpected-eof" => {
            "L'espressione di calcolo termina prima di essere completa. {values}"
        }
        "plugin.calc.unexpected-token" => {
            "L'espressione di calcolo contiene il token inatteso `{actual}`."
        }
        "plugin.calc.wrong-arity" => {
            "Una funzione di calcolo riceve un numero errato di argomenti. {values}"
        }
        "plugin.calc.expected-quoted-argument" => {
            "Il primo argomento di questa funzione di calcolo deve essere un nome tra virgolette. {values}"
        }
        "plugin.calc.expected-dot-identifier" => {
            "Dopo `.` e richiesto un identificatore di calcolo. {values}"
        }
        "plugin.calc.expected-rparen" => {
            "L'espressione di calcolo attende `{expected}` invece di `{actual}`."
        }
        "plugin.calc.expected-rparen.eof" => {
            "L'espressione di calcolo attende `{expected}` prima della fine. Il gioco puo usare solo il prefisso valido precedente."
        }
        "plugin.calc.expected-rbrack" => {
            "L'espressione di calcolo attende `{expected}` invece di `{actual}`."
        }
        "plugin.calc.expected-rbrack.eof" => {
            "L'espressione di calcolo attende `{expected}` prima della fine."
        }
        "plugin.calc.expected-colon" => {
            "L'espressione di calcolo condizionale attende `{expected}` invece di `{actual}`."
        }
        "plugin.calc.expected-colon.eof" => {
            "L'espressione di calcolo condizionale attende `{expected}` prima della fine."
        }
        "plugin.calc.expected-comma" => {
            "Gli argomenti della funzione attendono `{expected}` invece di `{actual}`."
        }
        "plugin.calc.expected-comma.eof" => {
            "La fine degli argomenti della funzione attende `{expected}`."
        }
        "plugin.unknownSkill" => {
            "Nome abilita sconosciuto `{identifier}`. Sostituirlo con il nome esatto in skills.txt."
        }
        "plugin.unknownMissile" => {
            "Nome missile sconosciuto `{identifier}`. Sostituirlo con il nome esatto in missiles.txt."
        }
        "plugin.unknownStat" => {
            "Nome Stat sconosciuto `{identifier}`. Sostituirlo con il nome Stat esatto in itemstatcost.txt."
        }
        "plugin.unknownCondition" => {
            "Nome di condizione sconosciuto `{identifier}` nel calcolo. Sostituirlo con una condizione supportata."
        }
        "plugin.unknownIdentifier" => {
            "L'identificatore `{identifier}` e sconosciuto nell'ambito di calcolo corrente."
        }
        "plugin.unknownSkillIdentifier" => {
            "L'identificatore di calcolo dell'abilita `{identifier}` e sconosciuto."
        }
        "plugin.unknownMissileIdentifier" => {
            "L'identificatore di calcolo del missile `{identifier}` e sconosciuto."
        }
        "plugin.unknownScopeIdentifier" => {
            "L'identificatore `{identifier}` e sconosciuto nell'ambito di calcolo corrente."
        }
        "plugin.calc.skilldesc-decimal-prefix" => {
            "Nell'espressione SkillDesc `{actual}`, il gioco usa solo il prefisso intero `{consumedPrefix}` e ignora `{ignoredSuffix}`."
        }
        "plugin.calc.decimal-policy" => {
            "Questo campo di calcolo richiede una forma intera. Il gioco puo valutare diversamente le espressioni decimali. {values}"
        }
        "plugin.calc.prefix-stop" => {
            "Il gioco legge soltanto il prefisso riconoscibile di questa espressione di calcolo e ignora il resto. {values}"
        }
        "plugin.cube-input.no-inputs" => {
            "La ricetta `{recipe}` in cubemain.txt, riga {line}, non ha input."
        }
        "plugin.cube-input.invalid-numinputs" => {
            "La ricetta `{recipe}` in cubemain.txt, riga {line}, ha il valore numinputs non valido `{value}`."
        }
        "plugin.cube-input.numinputs-mismatch" => {
            "Il numinputs della ricetta `{recipe}` in cubemain.txt, riga {line}, non corrisponde: previsti {expected}, presenti {actual}."
        }
        "plugin.cube-input.empty-base" => {
            "La base di input in `{column}` della ricetta `{recipe}` in cubemain.txt, riga {line}, è vuota."
        }
        "plugin.cube-input.invalid-base" => {
            "La base `{base}` in `{column}` della ricetta `{recipe}` in cubemain.txt, riga {line}, non è stata trovata."
        }
        "plugin.cube-input.ignored-suffix" => {
            "La ricetta `{recipe}` in cubemain.txt, riga {line}, interrompe l'analisi di `{column}` in corrispondenza di `{stoppedAt}`. Il testo successivo viene ignorato."
        }
        "plugin.cube-input.u8-range" => {
            "La quantità `{quantity}` in `{column}` della ricetta `{recipe}` in cubemain.txt, riga {line}, è fuori da 0..255. Il gioco memorizza {storedQuantity} e usa {effectiveQuantity}."
        }
        "plugin.cube-input.hover" => {
            "**Input del cubo**\nBase: `{base}`\nModificatori: {modifiers}"
        }
        "plugin.cube-output.invalid-base" => {
            "La base di output `{value}` in `{column}` della ricetta `{recipe}` in cubemain.txt, riga {line}, non è valida."
        }
        "plugin.cube-output.missing-ordinal-input" => {
            "L'output in `{column}` della ricetta `{recipe}` in cubemain.txt, riga {line}, fa riferimento a uno slot di input ordinale mancante."
        }
        "plugin.cube-output.u8-range" => {
            "Il valore `{value}` in `{column}` della ricetta `{recipe}` in cubemain.txt, riga {line}, è fuori da 0..255 e verrà troncato dal gioco."
        }
        "plugin.cube-output.ignored-suffix" => {
            "La ricetta `{recipe}` in cubemain.txt, riga {line}, legge solo `{value}` in `{column}` e ignora il resto."
        }
        "plugin.cube-output.invalid-property" => {
            "La proprietà `{value}` in `{column}` della ricetta `{recipe}` in cubemain.txt, riga {line}, non è valida."
        }
        "plugin.cube-output.empty-base-hover" => {
            "**Base:** vuota (manca la base di output del cubo)"
        }
        "plugin.cube-output.invalid-hover" => {
            "**Base di output del cubo non valida**\n\n`{base}` non è stata trovata. Suffisso ignorato: `{ignoredSuffix}`."
        }
        "plugin.cube-output.hover" => {
            "**Output del cubo**\nBase: `{base}` ({kind})\nInput: `{input}`\nModificatori: {modifiers}\nSuffisso ignorato: `{ignoredSuffix}`"
        }
        "plugin.enum.hover" => {
            "**Valore di enumerazione**\n\nValore: `{value}`\nNome: {name}\nParametri: {parameters}\n\n{description}\n\nCampi aggiuntivi: {extraFields}"
        }
        "plugin.item-code.unresolved" => {
            "Codice oggetto sconosciuto `{value}`. Controllare il codice e le maiuscole/minuscole."
        }
        "plugin.item-code.unresolved-packed-policy" => {
            "Non è stato trovato alcun oggetto corrispondente. Questo campo non viene interpretato come codice oggetto compresso e il testo verrà troncato; verificare che `{value}` sia il valore desiderato."
        }
        "plugin.item-code.unresolved-policy" => {
            "Il codice oggetto `{value}` non esiste in weapons, armor o misc. Verificare che sia il codice desiderato."
        }
        "plugin.item-code.hover" => "**Codice oggetto**\n`{code}`\n{name}",
        "plugin.item-name.hover" => "{name}\n\n**Codice:** {code}",
        "plugin.property.unknown-marker" => {
            "Il valore `{value}` inizia con `*`, ma non è un codice proprietà noto. Usare questo marcatore solo per codici proprietà supportati."
        }
        "plugin.property.unknown-code" => {
            "Codice proprietà sconosciuto `{value}`. Usare un codice da properties.txt o propertygroups.txt."
        }
        "plugin.property.unknown-hover" => {
            "**Codice proprietà sconosciuto**\n\n`{value}` non è presente in properties.txt o propertygroups.txt."
        }
        "plugin.property.hover" => "**Proprieta**\n`{value}` (codice in {sourceFile}.txt)",
        "plugin.tc-item.after-first-gap" => {
            "Il valore `{value}` in `{column}` della classe tesoro `{treasureClass}` si trova dopo il primo slot oggetto vuoto ed è ignorato dal gioco."
        }
        "plugin.tc-prob.after-first-gap" => {
            "La probabilità in `{column}` della classe tesoro `{treasureClass}` si trova dopo il primo slot oggetto vuoto ed è ignorata dal gioco."
        }
        "plugin.tc-prob.orphaned" => {
            "La probabilità `{value}` in `{column}` della classe tesoro `{treasureClass}` non ha una voce oggetto corrispondente ed è ignorata."
        }
        "plugin.tc-item.forward-reference" => {
            "Il valore `{value}` in `{column}` della classe tesoro `{treasureClass}` fa riferimento a una TC definita più avanti."
        }
        "plugin.tc-item.unresolved-base" => {
            "La base oggetto `{value}` in `{column}` della classe tesoro `{treasureClass}` non è stata trovata."
        }
        "plugin.tc-prob.blank-omission" => {
            "Il campo vuoto `{column}` della classe tesoro `{treasureClass}` omette la voce corrispondente."
        }
        "plugin.tc-prob.noncanonical" => {
            "Il valore `{value}` in `{column}` della classe tesoro `{treasureClass}` non è una probabilità di voce canonica."
        }
        "plugin.tc-prob.nonpositive-omission" => {
            "Il valore non positivo `{value}` in `{column}` della classe tesoro `{treasureClass}` omette la voce."
        }
        "plugin.tc-item.modifier-range" => {
            "Il modificatore `{value}` in `{column}` della classe tesoro `{treasureClass}` è fuori da 0..65535 e verrà troncato."
        }
        "plugin.tc-item.ignored-suffix" => {
            "La classe tesoro `{treasureClass}` legge solo `{value}` in `{column}` e ignora il resto."
        }
        "plugin.tc-item.field-width" => {
            "Il valore `{value}` in `{column}` della classe tesoro `{treasureClass}` supera 64 byte UTF-8."
        }
        "plugin.treasure-class.hover-after-gap" => {
            "**Voce della classe tesoro dopo un vuoto**\n\nDallo slot oggetto vuoto {firstEmptySlot}, il gioco ignora la base oggetto `{base}`."
        }
        "plugin.treasure-class.hover" => {
            "**Classe tesoro**\n`{base}`\n{name}\nSlot: {slot}\nScelte: {picks}\nProbabilità: {probability}\nModificatori: {modifiers}\nSuffisso ignorato: `{ignoredSuffix}`"
        }
        _ => return None,
    })
}

/// French wording for every diagnostic and hover emitted by the bundled
/// d2rdoc plugins. Each message preserves the structured arguments supplied
/// by the plugin so that user-provided identifiers remain exact.
fn plugin_detail_fr(key: &str) -> Option<&'static str> {
    Some(match key {
        "plugin.calc.skill-param-alias" => {
            "L'alias de paramètre de calcul `{alias}` a été utilisé ; l'identifiant canonique est `{identifier}`. {values}"
        }
        "plugin.calc.unknown-missile-value" => {
            "Valeur de missile inconnue `{identifier}`. Le jeu traite cette valeur comme 0 ; cette partie du calcul n'a donc aucun effet."
        }
        "plugin.calc.unterminated-string" => {
            "Une chaîne dans l'expression de calcul n'est pas terminée. {values}"
        }
        "plugin.calc.unexpected-character" => {
            "L'expression de calcul contient un caractère ou symbole non autorisé. La position signalée contient `{actual}`."
        }
        "plugin.calc.unexpected-eof" => {
            "L'expression de calcul se termine avant d'être complète. {values}"
        }
        "plugin.calc.unexpected-token" => {
            "L'expression de calcul contient le jeton inattendu `{actual}`."
        }
        "plugin.calc.wrong-arity" => {
            "Une fonction de calcul reçoit un nombre incorrect d'arguments. {values}"
        }
        "plugin.calc.expected-quoted-argument" => {
            "Le premier argument de cette fonction de calcul doit être un nom entre guillemets. {values}"
        }
        "plugin.calc.expected-dot-identifier" => {
            "Un identifiant de calcul est attendu après `.`. {values}"
        }
        "plugin.calc.expected-rparen" => {
            "L'expression de calcul attend `{expected}` au lieu de `{actual}`."
        }
        "plugin.calc.expected-rparen.eof" => {
            "L'expression de calcul attend `{expected}` avant sa fin. Le jeu ne peut utiliser que le préfixe valide qui le précède."
        }
        "plugin.calc.expected-rbrack" => {
            "L'expression de calcul attend `{expected}` au lieu de `{actual}`."
        }
        "plugin.calc.expected-rbrack.eof" => {
            "L'expression de calcul attend `{expected}` avant sa fin."
        }
        "plugin.calc.expected-colon" => {
            "L'expression de calcul conditionnelle attend `{expected}` au lieu de `{actual}`."
        }
        "plugin.calc.expected-colon.eof" => {
            "L'expression de calcul conditionnelle attend `{expected}` avant sa fin."
        }
        "plugin.calc.expected-comma" => {
            "Les arguments de fonction attendent `{expected}` au lieu de `{actual}`."
        }
        "plugin.calc.expected-comma.eof" => "La fin des arguments de fonction attend `{expected}`.",
        "plugin.unknownSkill" => {
            "Nom de compétence inconnu `{identifier}`. Remplacez-le par le nom exact de skills.txt."
        }
        "plugin.unknownMissile" => {
            "Nom de missile inconnu `{identifier}`. Remplacez-le par le nom exact de missiles.txt."
        }
        "plugin.unknownStat" => {
            "Nom de statistique inconnu `{identifier}`. Remplacez-le par le nom exact de itemstatcost.txt."
        }
        "plugin.unknownCondition" => {
            "Nom de condition inconnu `{identifier}` dans le calcul. Remplacez-le par une condition prise en charge."
        }
        "plugin.unknownIdentifier" => {
            "L'identifiant `{identifier}` est inconnu dans la portée de calcul actuelle."
        }
        "plugin.unknownSkillIdentifier" => {
            "L'identifiant de calcul de compétence `{identifier}` est inconnu."
        }
        "plugin.unknownMissileIdentifier" => {
            "L'identifiant de calcul de missile `{identifier}` est inconnu."
        }
        "plugin.unknownScopeIdentifier" => {
            "L'identifiant `{identifier}` est inconnu dans la portée de calcul actuelle."
        }
        "plugin.calc.skilldesc-decimal-prefix" => {
            "Dans l'expression SkillDesc `{actual}`, le jeu utilise uniquement le préfixe entier `{consumedPrefix}` et ignore `{ignoredSuffix}`."
        }
        "plugin.calc.decimal-policy" => {
            "Ce champ de calcul exige une écriture entière. Le jeu peut évaluer différemment les expressions de calcul décimales. {values}"
        }
        "plugin.calc.prefix-stop" => {
            "Le jeu ne lit que le préfixe reconnaissable de cette expression de calcul et ignore le reste. {values}"
        }
        "plugin.cube-input.no-inputs" => {
            "La recette `{recipe}` de cubemain.txt, ligne {line}, ne possède aucune entrée."
        }
        "plugin.cube-input.invalid-numinputs" => {
            "La recette `{recipe}` de cubemain.txt, ligne {line}, possède une valeur numinputs non valide `{value}`."
        }
        "plugin.cube-input.numinputs-mismatch" => {
            "La valeur numinputs de la recette `{recipe}` de cubemain.txt, ligne {line}, ne correspond pas : {expected} attendus, {actual} présents."
        }
        "plugin.cube-input.empty-base" => {
            "La base d'entrée dans `{column}` de la recette `{recipe}` de cubemain.txt, ligne {line}, est vide."
        }
        "plugin.cube-input.invalid-base" => {
            "La base `{base}` dans `{column}` de la recette `{recipe}` de cubemain.txt, ligne {line}, est introuvable."
        }
        "plugin.cube-input.ignored-suffix" => {
            "La recette `{recipe}` de cubemain.txt, ligne {line}, arrête l'analyse de `{column}` à `{stoppedAt}`. Le texte qui suit est ignoré."
        }
        "plugin.cube-input.u8-range" => {
            "La quantité `{quantity}` dans `{column}` de la recette `{recipe}` de cubemain.txt, ligne {line}, est hors de 0..255. Le jeu enregistre {storedQuantity} et utilise {effectiveQuantity}."
        }
        "plugin.cube-input.hover" => {
            "**Entrée du cube**\nBase : `{base}`\nModificateurs : {modifiers}"
        }
        "plugin.cube-output.invalid-base" => {
            "La base de sortie `{value}` dans `{column}` de la recette `{recipe}` de cubemain.txt, ligne {line}, n'est pas valide."
        }
        "plugin.cube-output.missing-ordinal-input" => {
            "La sortie dans `{column}` de la recette `{recipe}` de cubemain.txt, ligne {line}, fait référence à un emplacement d'entrée ordinal absent."
        }
        "plugin.cube-output.u8-range" => {
            "La valeur `{value}` dans `{column}` de la recette `{recipe}` de cubemain.txt, ligne {line}, est hors de 0..255 et sera tronquée par le jeu."
        }
        "plugin.cube-output.ignored-suffix" => {
            "La recette `{recipe}` de cubemain.txt, ligne {line}, ne lit que `{value}` dans `{column}` et ignore le reste."
        }
        "plugin.cube-output.invalid-property" => {
            "La propriété `{value}` dans `{column}` de la recette `{recipe}` de cubemain.txt, ligne {line}, n'est pas valide."
        }
        "plugin.cube-output.empty-base-hover" => "**Base :** vide (base de sortie du cube absente)",
        "plugin.cube-output.invalid-hover" => {
            "**Base de sortie du cube non valide**\n\n`{base}` est introuvable. Suffixe ignoré : `{ignoredSuffix}`."
        }
        "plugin.cube-output.hover" => {
            "**Sortie du cube**\nBase : `{base}` ({kind})\nEntrée : `{input}`\nModificateurs : {modifiers}\nSuffixe ignoré : `{ignoredSuffix}`"
        }
        "plugin.enum.hover" => {
            "**Valeur d'énumération**\n\nValeur : `{value}`\nNom : {name}\nParamètres : {parameters}\n\n{description}\n\nChamps supplémentaires : {extraFields}"
        }
        "plugin.item-code.unresolved" => {
            "Code d'objet inconnu `{value}`. Vérifiez le code et la casse."
        }
        "plugin.item-code.unresolved-packed-policy" => {
            "Aucun objet correspondant n'a été trouvé. Ce champ n'est pas interprété comme un code d'objet compacté et le texte sera tronqué ; vérifiez que `{value}` est bien la valeur voulue."
        }
        "plugin.item-code.unresolved-policy" => {
            "Le code d'objet `{value}` n'existe pas dans weapons, armor ou misc. Vérifiez qu'il s'agit bien du code voulu."
        }
        "plugin.item-code.hover" => "**Code d'objet**\n`{code}`\n{name}",
        "plugin.item-name.hover" => "{name}\n\n**Code** : {code}",
        "plugin.property.unknown-marker" => {
            "La valeur `{value}` commence par `*`, mais ce n'est pas un code de propriété connu. Utilisez ce marqueur uniquement pour les codes de propriété pris en charge."
        }
        "plugin.property.unknown-code" => {
            "Code de propriété inconnu `{value}`. Utilisez un code de properties.txt ou propertygroups.txt."
        }
        "plugin.property.unknown-hover" => {
            "**Code de propriété inconnu**\n\n`{value}` est introuvable dans properties.txt ou propertygroups.txt."
        }
        "plugin.property.hover" => "**Propriété**\n`{value}` (code de {sourceFile}.txt)",
        "plugin.tc-item.after-first-gap" => {
            "La valeur `{value}` dans `{column}` de la classe de trésor `{treasureClass}` se trouve après le premier emplacement d'objet vide et est ignorée par le jeu."
        }
        "plugin.tc-prob.after-first-gap" => {
            "La probabilité dans `{column}` de la classe de trésor `{treasureClass}` se trouve après le premier emplacement d'objet vide et est ignorée par le jeu."
        }
        "plugin.tc-prob.orphaned" => {
            "La probabilité `{value}` dans `{column}` de la classe de trésor `{treasureClass}` n'a pas d'entrée d'objet correspondante et est ignorée."
        }
        "plugin.tc-item.forward-reference" => {
            "La valeur `{value}` dans `{column}` de la classe de trésor `{treasureClass}` fait référence à une TC définie plus loin."
        }
        "plugin.tc-item.unresolved-base" => {
            "La base d'objet `{value}` dans `{column}` de la classe de trésor `{treasureClass}` est introuvable."
        }
        "plugin.tc-prob.blank-omission" => {
            "Le champ vide `{column}` de la classe de trésor `{treasureClass}` omet l'entrée correspondante."
        }
        "plugin.tc-prob.noncanonical" => {
            "La valeur `{value}` dans `{column}` de la classe de trésor `{treasureClass}` n'est pas une probabilité d'entrée canonique."
        }
        "plugin.tc-prob.nonpositive-omission" => {
            "La valeur non positive `{value}` dans `{column}` de la classe de trésor `{treasureClass}` omet l'entrée."
        }
        "plugin.tc-item.modifier-range" => {
            "Le modificateur `{value}` dans `{column}` de la classe de trésor `{treasureClass}` est hors de 0..65535 et sera tronqué."
        }
        "plugin.tc-item.ignored-suffix" => {
            "La classe de trésor `{treasureClass}` ne lit que `{value}` dans `{column}` et ignore le reste."
        }
        "plugin.tc-item.field-width" => {
            "La valeur `{value}` dans `{column}` de la classe de trésor `{treasureClass}` dépasse 64 octets UTF-8."
        }
        "plugin.treasure-class.hover-after-gap" => {
            "**Entrée de classe de trésor après un vide**\n\nÀ partir de l'emplacement d'objet vide {firstEmptySlot}, le jeu ignore la base d'objet `{base}`."
        }
        "plugin.treasure-class.hover" => {
            "**Classe de trésor**\n`{base}`\n{name}\nEmplacement : {slot}\nChoix : {picks}\nProbabilité : {probability}\nModificateurs : {modifiers}\nSuffixe ignoré : `{ignoredSuffix}`"
        }
        _ => return None,
    })
}

/// Spanish (Spain) wording for every diagnostic and hover emitted by the
/// bundled d2rdoc plugins.  This remains explicit so rule-specific game
/// behaviour is not replaced by a category fallback.
fn plugin_detail_es(key: &str) -> Option<&'static str> {
    Some(match key {
        "plugin.calc.skill-param-alias" => {
            "Se utilizó el alias de parámetro de cálculo `{alias}`; el identificador canónico es `{identifier}`. {values}"
        }
        "plugin.calc.unknown-missile-value" => {
            "Valor de misil desconocido `{identifier}`. El juego trata este valor como 0, por lo que esta parte del cálculo no tiene efecto."
        }
        "plugin.calc.unterminated-string" => {
            "Una cadena de la expresión de cálculo no está terminada. {values}"
        }
        "plugin.calc.unexpected-character" => {
            "La expresión de cálculo contiene un carácter o símbolo no permitido. La posición indicada contiene `{actual}`."
        }
        "plugin.calc.unexpected-eof" => {
            "La expresión de cálculo termina antes de estar completa. {values}"
        }
        "plugin.calc.unexpected-token" => {
            "La expresión de cálculo contiene el token inesperado `{actual}`."
        }
        "plugin.calc.wrong-arity" => {
            "Una función de cálculo recibe un número incorrecto de argumentos. {values}"
        }
        "plugin.calc.expected-quoted-argument" => {
            "El primer argumento de esta función de cálculo debe ser un nombre entre comillas. {values}"
        }
        "plugin.calc.expected-dot-identifier" => {
            "Se esperaba un identificador de cálculo después de `.`. {values}"
        }
        "plugin.calc.expected-rparen" => {
            "La expresión de cálculo esperaba `{expected}` en lugar de `{actual}`."
        }
        "plugin.calc.expected-rparen.eof" => {
            "La expresión de cálculo esperaba `{expected}` antes de terminar. El juego solo puede usar el prefijo válido anterior."
        }
        "plugin.calc.expected-rbrack" => {
            "La expresión de cálculo esperaba `{expected}` en lugar de `{actual}`."
        }
        "plugin.calc.expected-rbrack.eof" => {
            "La expresión de cálculo esperaba `{expected}` antes de terminar."
        }
        "plugin.calc.expected-colon" => {
            "La expresión de cálculo condicional esperaba `{expected}` en lugar de `{actual}`."
        }
        "plugin.calc.expected-colon.eof" => {
            "La expresión de cálculo condicional esperaba `{expected}` antes de terminar."
        }
        "plugin.calc.expected-comma" => {
            "Los argumentos de función esperaban `{expected}` en lugar de `{actual}`."
        }
        "plugin.calc.expected-comma.eof" => {
            "El final de los argumentos de función esperaba `{expected}`."
        }
        "plugin.unknownSkill" => {
            "Nombre de habilidad desconocido `{identifier}`. Sustitúyalo por el nombre exacto de skills.txt."
        }
        "plugin.unknownMissile" => {
            "Nombre de misil desconocido `{identifier}`. Sustitúyalo por el nombre exacto de missiles.txt."
        }
        "plugin.unknownStat" => {
            "Nombre de estadística desconocido `{identifier}`. Sustitúyalo por el nombre Stat exacto de itemstatcost.txt."
        }
        "plugin.unknownCondition" => {
            "Nombre de condición desconocido `{identifier}` en el cálculo. Sustitúyalo por una condición admitida."
        }
        "plugin.unknownIdentifier" => {
            "El identificador `{identifier}` es desconocido en el ámbito de cálculo actual."
        }
        "plugin.unknownSkillIdentifier" => {
            "El identificador de cálculo de habilidad `{identifier}` es desconocido."
        }
        "plugin.unknownMissileIdentifier" => {
            "El identificador de cálculo de misil `{identifier}` es desconocido."
        }
        "plugin.unknownScopeIdentifier" => {
            "El identificador `{identifier}` es desconocido en el ámbito de cálculo actual."
        }
        "plugin.calc.skilldesc-decimal-prefix" => {
            "En la expresión SkillDesc `{actual}`, el juego solo utiliza el prefijo entero `{consumedPrefix}` e ignora `{ignoredSuffix}`."
        }
        "plugin.calc.decimal-policy" => {
            "Este campo de cálculo requiere una expresión entera. El juego puede evaluar de otra manera las expresiones decimales. {values}"
        }
        "plugin.calc.prefix-stop" => {
            "El juego lee solo el prefijo reconocible de esta expresión de cálculo e ignora el resto. {values}"
        }
        "plugin.cube-input.no-inputs" => {
            "La receta `{recipe}` de cubemain.txt, línea {line}, no tiene entradas."
        }
        "plugin.cube-input.invalid-numinputs" => {
            "La receta `{recipe}` de cubemain.txt, línea {line}, tiene un valor numinputs no válido `{value}`."
        }
        "plugin.cube-input.numinputs-mismatch" => {
            "El valor numinputs de la receta `{recipe}` de cubemain.txt, línea {line}, no coincide: se esperaban {expected}, pero hay {actual}."
        }
        "plugin.cube-input.empty-base" => {
            "La base de entrada de `{column}` en la receta `{recipe}` de cubemain.txt, línea {line}, está vacía."
        }
        "plugin.cube-input.invalid-base" => {
            "No se encuentra la base `{base}` de `{column}` en la receta `{recipe}` de cubemain.txt, línea {line}."
        }
        "plugin.cube-input.ignored-suffix" => {
            "La receta `{recipe}` de cubemain.txt, línea {line}, deja de analizar `{column}` en `{stoppedAt}`. Se ignora el texto posterior."
        }
        "plugin.cube-input.u8-range" => {
            "La cantidad `{quantity}` de `{column}` en la receta `{recipe}` de cubemain.txt, línea {line}, está fuera de 0..255. El juego guarda {storedQuantity} y usa {effectiveQuantity}."
        }
        "plugin.cube-input.hover" => {
            "**Entrada del cubo**\nBase: `{base}`\nModificadores: {modifiers}"
        }
        "plugin.cube-output.invalid-base" => {
            "La base de salida `{value}` de `{column}` en la receta `{recipe}` de cubemain.txt, línea {line}, no es válida."
        }
        "plugin.cube-output.missing-ordinal-input" => {
            "La salida de `{column}` en la receta `{recipe}` de cubemain.txt, línea {line}, hace referencia a una posición ordinal de entrada inexistente."
        }
        "plugin.cube-output.u8-range" => {
            "El valor `{value}` de `{column}` en la receta `{recipe}` de cubemain.txt, línea {line}, está fuera de 0..255 y el juego lo truncará."
        }
        "plugin.cube-output.ignored-suffix" => {
            "La receta `{recipe}` de cubemain.txt, línea {line}, solo lee `{value}` de `{column}` e ignora el resto."
        }
        "plugin.cube-output.invalid-property" => {
            "La propiedad `{value}` de `{column}` en la receta `{recipe}` de cubemain.txt, línea {line}, no es válida."
        }
        "plugin.cube-output.empty-base-hover" => {
            "**Base:** vacía (falta la base de salida del cubo)"
        }
        "plugin.cube-output.invalid-hover" => {
            "**Base de salida de cubo no válida**\n\nNo se encuentra `{base}`. Sufijo ignorado: `{ignoredSuffix}`."
        }
        "plugin.cube-output.hover" => {
            "**Salida del cubo**\nBase: `{base}` ({kind})\nEntrada: `{input}`\nModificadores: {modifiers}\nSufijo ignorado: `{ignoredSuffix}`"
        }
        "plugin.enum.hover" => {
            "**Valor de enumeración**\n\nValor: `{value}`\nNombre: {name}\nParámetros: {parameters}\n\n{description}\n\nCampos adicionales: {extraFields}"
        }
        "plugin.item-code.unresolved" => {
            "Código de objeto desconocido `{value}`. Compruebe el código y las mayúsculas/minúsculas."
        }
        "plugin.item-code.unresolved-packed-policy" => {
            "No se encontró ningún objeto coincidente. Este campo no se interpreta como código de objeto empaquetado y el texto se truncará; compruebe que `{value}` sea el valor deseado."
        }
        "plugin.item-code.unresolved-policy" => {
            "El código de objeto `{value}` no existe en weapons, armor ni misc. Compruebe que sea el código deseado."
        }
        "plugin.item-code.hover" => "**Código de objeto**\n`{code}`\n{name}",
        "plugin.item-name.hover" => "{name}\n\n**Código:** {code}",
        "plugin.property.unknown-marker" => {
            "El valor `{value}` empieza por `*`, pero no es un código de propiedad conocido. Use este marcador solo con códigos de propiedad admitidos."
        }
        "plugin.property.unknown-code" => {
            "Código de propiedad desconocido `{value}`. Use un código de properties.txt o propertygroups.txt."
        }
        "plugin.property.unknown-hover" => {
            "**Código de propiedad desconocido**\n\nNo se encuentra `{value}` en properties.txt ni propertygroups.txt."
        }
        "plugin.property.hover" => "**Propiedad**\n`{value}` (código de {sourceFile}.txt)",
        "plugin.tc-item.after-first-gap" => {
            "El valor `{value}` de `{column}` en la clase de tesoro `{treasureClass}` aparece después de la primera posición de objeto vacía y el juego lo ignora."
        }
        "plugin.tc-prob.after-first-gap" => {
            "La probabilidad de `{column}` en la clase de tesoro `{treasureClass}` aparece después de la primera posición de objeto vacía y el juego la ignora."
        }
        "plugin.tc-prob.orphaned" => {
            "La probabilidad `{value}` de `{column}` en la clase de tesoro `{treasureClass}` no tiene una entrada de objeto correspondiente y se ignora."
        }
        "plugin.tc-item.forward-reference" => {
            "El valor `{value}` de `{column}` en la clase de tesoro `{treasureClass}` hace referencia a una TC definida más adelante."
        }
        "plugin.tc-item.unresolved-base" => {
            "No se encuentra la base de objeto `{value}` de `{column}` en la clase de tesoro `{treasureClass}`."
        }
        "plugin.tc-prob.blank-omission" => {
            "El campo vacío `{column}` de la clase de tesoro `{treasureClass}` omite la entrada correspondiente."
        }
        "plugin.tc-prob.noncanonical" => {
            "El valor `{value}` de `{column}` en la clase de tesoro `{treasureClass}` no es una probabilidad de entrada canónica."
        }
        "plugin.tc-prob.nonpositive-omission" => {
            "El valor no positivo `{value}` de `{column}` en la clase de tesoro `{treasureClass}` omite la entrada."
        }
        "plugin.tc-item.modifier-range" => {
            "El modificador `{value}` de `{column}` en la clase de tesoro `{treasureClass}` está fuera de 0..65535 y se truncará."
        }
        "plugin.tc-item.ignored-suffix" => {
            "La clase de tesoro `{treasureClass}` solo lee `{value}` de `{column}` e ignora el resto."
        }
        "plugin.tc-item.field-width" => {
            "El valor `{value}` de `{column}` en la clase de tesoro `{treasureClass}` supera los 64 bytes UTF-8."
        }
        "plugin.treasure-class.hover-after-gap" => {
            "**Entrada de clase de tesoro tras un hueco**\n\nDesde la posición de objeto vacía {firstEmptySlot}, el juego ignora la base de objeto `{base}`."
        }
        "plugin.treasure-class.hover" => {
            "**Clase de tesoro**\n`{base}`\n{name}\nPosición: {slot}\nSelecciones: {picks}\nProbabilidad: {probability}\nModificadores: {modifiers}\nSufijo ignorado: `{ignoredSuffix}`"
        }
        _ => return None,
    })
}

/// Brazilian Portuguese wording for every diagnostic and hover emitted by the
/// bundled d2rdoc plugins. Each entry preserves the exact named arguments
/// emitted by the plugin so rendering is language-only and never changes game
/// data.
fn plugin_detail_pt_br(key: &str) -> Option<&'static str> {
    Some(match key {
        "plugin.calc.skill-param-alias" => {
            "Foi usado o alias de parametro de calculo `{alias}`; o identificador canonico e `{identifier}`. {values}"
        }
        "plugin.calc.unknown-missile-value" => {
            "Valor de missil desconhecido `{identifier}`. O jogo trata esse valor como 0, portanto esta parte do calculo nao tem efeito."
        }
        "plugin.calc.unterminated-string" => {
            "Uma cadeia na expressao de calculo nao foi terminada. {values}"
        }
        "plugin.calc.unexpected-character" => {
            "A expressao de calculo contem um caractere ou simbolo invalido. A posicao indicada contem `{actual}`."
        }
        "plugin.calc.unexpected-eof" => {
            "A expressao de calculo termina antes de estar completa. {values}"
        }
        "plugin.calc.unexpected-token" => {
            "A expressao de calculo contem o token inesperado `{actual}`."
        }
        "plugin.calc.wrong-arity" => {
            "Uma funcao de calculo recebeu uma quantidade incorreta de argumentos. {values}"
        }
        "plugin.calc.expected-quoted-argument" => {
            "O primeiro argumento desta funcao de calculo deve ser um nome entre aspas. {values}"
        }
        "plugin.calc.expected-dot-identifier" => {
            "Era esperado um identificador de calculo apos `.`. {values}"
        }
        "plugin.calc.expected-rparen" => {
            "A expressao de calculo esperava `{expected}`, nao `{actual}`."
        }
        "plugin.calc.expected-rparen.eof" => {
            "A expressao de calculo esperava `{expected}` antes do fim. O jogo pode usar somente o prefixo valido anterior."
        }
        "plugin.calc.expected-rbrack" => {
            "A expressao de calculo esperava `{expected}`, nao `{actual}`."
        }
        "plugin.calc.expected-rbrack.eof" => {
            "A expressao de calculo esperava `{expected}` antes do fim."
        }
        "plugin.calc.expected-colon" => {
            "A expressao condicional de calculo esperava `{expected}`, nao `{actual}`."
        }
        "plugin.calc.expected-colon.eof" => {
            "A expressao condicional de calculo esperava `{expected}` antes do fim."
        }
        "plugin.calc.expected-comma" => {
            "Os argumentos da funcao esperavam `{expected}`, nao `{actual}`."
        }
        "plugin.calc.expected-comma.eof" => "O fim dos argumentos da funcao esperava `{expected}`.",
        "plugin.unknownSkill" => {
            "Nome de habilidade desconhecido `{identifier}`. Substitua-o pelo nome exato de skills.txt."
        }
        "plugin.unknownMissile" => {
            "Nome de missil desconhecido `{identifier}`. Substitua-o pelo nome exato de missiles.txt."
        }
        "plugin.unknownStat" => {
            "Nome de Stat desconhecido `{identifier}`. Substitua-o pelo nome exato de Stat em itemstatcost.txt."
        }
        "plugin.unknownCondition" => {
            "Nome de condicao desconhecido `{identifier}` no calculo. Substitua-o por uma condicao aceita."
        }
        "plugin.unknownIdentifier" => {
            "O identificador `{identifier}` e desconhecido no escopo atual do calculo."
        }
        "plugin.unknownSkillIdentifier" => {
            "O identificador de calculo de habilidade `{identifier}` e desconhecido."
        }
        "plugin.unknownMissileIdentifier" => {
            "O identificador de calculo de missil `{identifier}` e desconhecido."
        }
        "plugin.unknownScopeIdentifier" => {
            "O identificador `{identifier}` e desconhecido no escopo atual do calculo."
        }
        "plugin.calc.skilldesc-decimal-prefix" => {
            "Na expressao SkillDesc `{actual}`, o jogo usa apenas o prefixo inteiro `{consumedPrefix}` e ignora `{ignoredSuffix}`."
        }
        "plugin.calc.decimal-policy" => {
            "Este campo de calculo exige uma expressao inteira. O jogo pode avaliar expressoes decimais de modo diferente. {values}"
        }
        "plugin.calc.prefix-stop" => {
            "O jogo le somente o prefixo reconhecivel desta expressao de calculo e ignora o restante. {values}"
        }
        "plugin.cube-input.no-inputs" => {
            "A receita `{recipe}` em cubemain.txt, linha {line}, não possui entradas."
        }
        "plugin.cube-input.invalid-numinputs" => {
            "A receita `{recipe}` em cubemain.txt, linha {line}, tem o valor numinputs inválido `{value}`."
        }
        "plugin.cube-input.numinputs-mismatch" => {
            "O numinputs da receita `{recipe}` em cubemain.txt, linha {line}, não confere: eram esperados {expected}, mas foram encontrados {actual}."
        }
        "plugin.cube-input.empty-base" => {
            "A base de entrada em `{column}` da receita `{recipe}` em cubemain.txt, linha {line}, está vazia."
        }
        "plugin.cube-input.invalid-base" => {
            "A base `{base}` em `{column}` da receita `{recipe}` em cubemain.txt, linha {line}, não foi encontrada."
        }
        "plugin.cube-input.ignored-suffix" => {
            "A receita `{recipe}` em cubemain.txt, linha {line}, para de analisar `{column}` em `{stoppedAt}`. O texto posterior é ignorado."
        }
        "plugin.cube-input.u8-range" => {
            "A quantidade `{quantity}` em `{column}` da receita `{recipe}` em cubemain.txt, linha {line}, está fora de 0..255. O jogo armazena {storedQuantity} e usa {effectiveQuantity}."
        }
        "plugin.cube-input.hover" => {
            "**Entrada do cubo**\nBase: `{base}`\nModificadores: {modifiers}"
        }
        "plugin.cube-output.invalid-base" => {
            "A base de saída `{value}` em `{column}` da receita `{recipe}` em cubemain.txt, linha {line}, não é válida."
        }
        "plugin.cube-output.missing-ordinal-input" => {
            "A saida em `{column}` da receita `{recipe}` em cubemain.txt, linha {line}, faz referencia a uma posicao ordinal de entrada inexistente."
        }
        "plugin.cube-output.u8-range" => {
            "O valor `{value}` em `{column}` da receita `{recipe}` em cubemain.txt, linha {line}, está fora de 0..255 e será truncado pelo jogo."
        }
        "plugin.cube-output.ignored-suffix" => {
            "A receita `{recipe}` em cubemain.txt, linha {line}, le somente `{value}` em `{column}` e ignora o restante."
        }
        "plugin.cube-output.invalid-property" => {
            "A propriedade `{value}` em `{column}` da receita `{recipe}` em cubemain.txt, linha {line}, não é válida."
        }
        "plugin.cube-output.empty-base-hover" => "**Base:** vazia (falta a base de saída do cubo)",
        "plugin.cube-output.invalid-hover" => {
            "**Base de saída do cubo inválida**\n\n`{base}` não foi encontrada. Sufixo ignorado: `{ignoredSuffix}`."
        }
        "plugin.cube-output.hover" => {
            "**Saída do cubo**\nBase: `{base}` ({kind})\nEntrada: `{input}`\nModificadores: {modifiers}\nSufixo ignorado: `{ignoredSuffix}`"
        }
        "plugin.enum.hover" => {
            "**Valor da enumeracao**\n\nValor: `{value}`\nNome: {name}\nParametros: {parameters}\n\n{description}\n\nCampos adicionais: {extraFields}"
        }
        "plugin.item-code.unresolved" => {
            "Código de item desconhecido `{value}`. Verifique o código e as letras maiúsculas/minúsculas."
        }
        "plugin.item-code.unresolved-packed-policy" => {
            "Nenhum item correspondente foi encontrado. Este campo não é interpretado como código de item compactado e o texto será truncado; verifique se `{value}` é o valor desejado."
        }
        "plugin.item-code.unresolved-policy" => {
            "O código de item `{value}` não existe em weapons, armor ou misc. Verifique se este é o código desejado."
        }
        "plugin.item-code.hover" => "**Código do item**\n`{code}`\n{name}",
        "plugin.item-name.hover" => "{name}\n\n**Codigo:** {code}",
        "plugin.property.unknown-marker" => {
            "O valor `{value}` comeca com `*`, mas nao e um codigo de propriedade conhecido. Use este marcador somente com codigos de propriedade aceitos."
        }
        "plugin.property.unknown-code" => {
            "Codigo de propriedade desconhecido `{value}`. Use um codigo de properties.txt ou propertygroups.txt."
        }
        "plugin.property.unknown-hover" => {
            "**Codigo de propriedade desconhecido**\n\n`{value}` nao foi encontrado em properties.txt nem em propertygroups.txt."
        }
        "plugin.property.hover" => "**Propriedade**\n`{value}` (codigo de {sourceFile}.txt)",
        "plugin.tc-item.after-first-gap" => {
            "O valor `{value}` em `{column}` na classe de tesouro `{treasureClass}` aparece depois do primeiro espaço de item vazio e o jogo o ignora."
        }
        "plugin.tc-prob.after-first-gap" => {
            "A probabilidade em `{column}` na classe de tesouro `{treasureClass}` aparece depois do primeiro espaço de item vazio e o jogo a ignora."
        }
        "plugin.tc-prob.orphaned" => {
            "A probabilidade `{value}` em `{column}` na classe de tesouro `{treasureClass}` nao possui uma entrada de item correspondente e e ignorada."
        }
        "plugin.tc-item.forward-reference" => {
            "O valor `{value}` em `{column}` na classe de tesouro `{treasureClass}` faz referencia a uma TC definida mais adiante."
        }
        "plugin.tc-item.unresolved-base" => {
            "A base de item `{value}` em `{column}` na classe de tesouro `{treasureClass}` nao foi encontrada."
        }
        "plugin.tc-prob.blank-omission" => {
            "O campo vazio `{column}` na classe de tesouro `{treasureClass}` omite a entrada correspondente."
        }
        "plugin.tc-prob.noncanonical" => {
            "O valor `{value}` em `{column}` na classe de tesouro `{treasureClass}` nao e uma probabilidade de entrada canonica."
        }
        "plugin.tc-prob.nonpositive-omission" => {
            "O valor nao positivo `{value}` em `{column}` na classe de tesouro `{treasureClass}` omite a entrada."
        }
        "plugin.tc-item.modifier-range" => {
            "O modificador `{value}` em `{column}` na classe de tesouro `{treasureClass}` está fora de 0..65535 e será truncado."
        }
        "plugin.tc-item.ignored-suffix" => {
            "A classe de tesouro `{treasureClass}` le somente `{value}` em `{column}` e ignora o restante."
        }
        "plugin.tc-item.field-width" => {
            "O valor `{value}` em `{column}` na classe de tesouro `{treasureClass}` excede 64 bytes UTF-8."
        }
        "plugin.treasure-class.hover-after-gap" => {
            "**Entrada de classe de tesouro após uma lacuna**\n\nA partir do espaço de item vazio {firstEmptySlot}, o jogo ignora a base de item `{base}`."
        }
        "plugin.treasure-class.hover" => {
            "**Classe de tesouro**\n`{base}`\n{name}\nPosição: {slot}\nSeleções: {picks}\nProbabilidade: {probability}\nModificadores: {modifiers}\nSufixo ignorado: `{ignoredSuffix}`"
        }
        _ => return None,
    })
}

/// Mexican Spanish wording for every diagnostic and hover emitted by the
/// bundled d2rdoc plugins. This is deliberately separate from the Spanish
/// (Spain) catalog: it uses the terminology and direct, friendly tone used by
/// the Mexican editor UI while retaining each rule's game-facing detail.
fn plugin_detail_es_mx(key: &str) -> Option<&'static str> {
    Some(match key {
        "plugin.calc.skill-param-alias" => {
            "Se uso el alias de parametro `{alias}`; el identificador canonico es `{identifier}`. {values}"
        }
        "plugin.calc.unknown-missile-value" => {
            "Valor de misil desconocido `{identifier}`. El juego lo toma como 0, asi que esta parte del calculo no tiene efecto."
        }
        "plugin.calc.unterminated-string" => {
            "Una cadena de la expresion de calculo no esta cerrada. {values}"
        }
        "plugin.calc.unexpected-character" => {
            "La expresion de calculo tiene un caracter o simbolo no permitido. En la posicion indicada aparece `{actual}`."
        }
        "plugin.calc.unexpected-eof" => {
            "La expresion de calculo termina antes de completarse. {values}"
        }
        "plugin.calc.unexpected-token" => {
            "La expresion de calculo contiene el token inesperado `{actual}`."
        }
        "plugin.calc.wrong-arity" => {
            "La funcion de calculo recibe una cantidad incorrecta de argumentos. {values}"
        }
        "plugin.calc.expected-quoted-argument" => {
            "El primer argumento de esta funcion de calculo debe ser un nombre entre comillas. {values}"
        }
        "plugin.calc.expected-dot-identifier" => {
            "Se esperaba un identificador de calculo despues de `.`. {values}"
        }
        "plugin.calc.expected-rparen" => {
            "La expresion de calculo esperaba `{expected}` en vez de `{actual}`."
        }
        "plugin.calc.expected-rparen.eof" => {
            "La expresion de calculo esperaba `{expected}` antes de terminar. El juego solo puede usar el prefijo valido anterior."
        }
        "plugin.calc.expected-rbrack" => {
            "La expresion de calculo esperaba `{expected}` en vez de `{actual}`."
        }
        "plugin.calc.expected-rbrack.eof" => {
            "La expresion de calculo esperaba `{expected}` antes de terminar."
        }
        "plugin.calc.expected-colon" => {
            "La expresion condicional de calculo esperaba `{expected}` en vez de `{actual}`."
        }
        "plugin.calc.expected-colon.eof" => {
            "La expresion condicional de calculo esperaba `{expected}` antes de terminar."
        }
        "plugin.calc.expected-comma" => {
            "Los argumentos de la funcion esperaban `{expected}` en vez de `{actual}`."
        }
        "plugin.calc.expected-comma.eof" => {
            "El final de los argumentos de la funcion esperaba `{expected}`."
        }
        "plugin.unknownSkill" => {
            "Nombre de habilidad desconocido `{identifier}`. Cambialo por el nombre exacto de skills.txt."
        }
        "plugin.unknownMissile" => {
            "Nombre de misil desconocido `{identifier}`. Cambialo por el nombre exacto de missiles.txt."
        }
        "plugin.unknownStat" => {
            "Nombre de estadistica desconocido `{identifier}`. Usa el nombre Stat exacto de itemstatcost.txt."
        }
        "plugin.unknownCondition" => {
            "Nombre de condicion desconocido `{identifier}` en el calculo. Cambialo por una condicion admitida."
        }
        "plugin.unknownIdentifier" => {
            "El identificador `{identifier}` no existe en el ambito actual del calculo."
        }
        "plugin.unknownSkillIdentifier" => {
            "El identificador de calculo de habilidad `{identifier}` es desconocido."
        }
        "plugin.unknownMissileIdentifier" => {
            "El identificador de calculo de misil `{identifier}` es desconocido."
        }
        "plugin.unknownScopeIdentifier" => {
            "El identificador `{identifier}` no existe en el ambito actual del calculo."
        }
        "plugin.calc.skilldesc-decimal-prefix" => {
            "En la expresion SkillDesc `{actual}`, el juego solo usa el prefijo entero `{consumedPrefix}` e ignora `{ignoredSuffix}`."
        }
        "plugin.calc.decimal-policy" => {
            "Este campo de calculo requiere una expresion entera. El juego puede evaluar distinto las expresiones decimales. {values}"
        }
        "plugin.calc.prefix-stop" => {
            "El juego lee solo el prefijo reconocible de esta expresion de calculo e ignora lo demas. {values}"
        }
        "plugin.cube-input.no-inputs" => {
            "La receta `{recipe}` de cubemain.txt, línea {line}, no tiene entradas."
        }
        "plugin.cube-input.invalid-numinputs" => {
            "La receta `{recipe}` de cubemain.txt, línea {line}, tiene un valor numinputs no válido `{value}`."
        }
        "plugin.cube-input.numinputs-mismatch" => {
            "El numinputs de la receta `{recipe}` de cubemain.txt, línea {line}, no coincide: se esperaban {expected}, pero hay {actual}."
        }
        "plugin.cube-input.empty-base" => {
            "La base de entrada de `{column}` en la receta `{recipe}` de cubemain.txt, línea {line}, está vacía."
        }
        "plugin.cube-input.invalid-base" => {
            "No se encontró la base `{base}` de `{column}` en la receta `{recipe}` de cubemain.txt, línea {line}."
        }
        "plugin.cube-input.ignored-suffix" => {
            "La receta `{recipe}` de cubemain.txt, línea {line}, deja de analizar `{column}` en `{stoppedAt}`. El texto que sigue se ignora."
        }
        "plugin.cube-input.u8-range" => {
            "La cantidad `{quantity}` de `{column}` en la receta `{recipe}` de cubemain.txt, línea {line}, está fuera de 0..255. El juego guarda {storedQuantity} y usa {effectiveQuantity}."
        }
        "plugin.cube-input.hover" => {
            "**Entrada del cubo**\nBase: `{base}`\nModificadores: {modifiers}"
        }
        "plugin.cube-output.invalid-base" => {
            "La base de salida `{value}` de `{column}` en la receta `{recipe}` de cubemain.txt, línea {line}, no es válida."
        }
        "plugin.cube-output.missing-ordinal-input" => {
            "La salida de `{column}` en la receta `{recipe}` de cubemain.txt, línea {line}, se refiere a una posición ordinal de entrada que no existe."
        }
        "plugin.cube-output.u8-range" => {
            "El valor `{value}` de `{column}` en la receta `{recipe}` de cubemain.txt, línea {line}, está fuera de 0..255 y el juego lo va a truncar."
        }
        "plugin.cube-output.ignored-suffix" => {
            "La receta `{recipe}` de cubemain.txt, línea {line}, solo lee `{value}` de `{column}` e ignora lo demás."
        }
        "plugin.cube-output.invalid-property" => {
            "La propiedad `{value}` de `{column}` en la receta `{recipe}` de cubemain.txt, línea {line}, no es válida."
        }
        "plugin.cube-output.empty-base-hover" => {
            "**Base:** vacía (falta la base de salida del cubo)"
        }
        "plugin.cube-output.invalid-hover" => {
            "**Base de salida del cubo no válida**\n\nNo se encontró `{base}`. Sufijo ignorado: `{ignoredSuffix}`."
        }
        "plugin.cube-output.hover" => {
            "**Salida del cubo**\nBase: `{base}` ({kind})\nEntrada: `{input}`\nModificadores: {modifiers}\nSufijo ignorado: `{ignoredSuffix}`"
        }
        "plugin.enum.hover" => {
            "**Valor de enumeracion**\n\nValor: `{value}`\nNombre: {name}\nParametros: {parameters}\n\n{description}\n\nCampos extra: {extraFields}"
        }
        "plugin.item-code.unresolved" => {
            "Código de objeto desconocido `{value}`. Revisa el código y las mayúsculas/minúsculas."
        }
        "plugin.item-code.unresolved-packed-policy" => {
            "No se encontró ningún objeto que coincida. Este campo no se interpreta como código de objeto empaquetado y el texto se truncará; revisa que `{value}` sea el valor que querías."
        }
        "plugin.item-code.unresolved-policy" => {
            "El código de objeto `{value}` no existe en weapons, armor ni misc. Revisa que sea el código que querías."
        }
        "plugin.item-code.hover" => "**Código de objeto**\n`{code}`\n{name}",
        "plugin.item-name.hover" => "{name}\n\n**Codigo:** {code}",
        "plugin.property.unknown-marker" => {
            "El valor `{value}` empieza con `*`, pero no es un codigo de propiedad conocido. Usa ese marcador solo con codigos de propiedad admitidos."
        }
        "plugin.property.unknown-code" => {
            "Codigo de propiedad desconocido `{value}`. Usa un codigo de properties.txt o propertygroups.txt."
        }
        "plugin.property.unknown-hover" => {
            "**Codigo de propiedad desconocido**\n\nNo se encontro `{value}` en properties.txt ni propertygroups.txt."
        }
        "plugin.property.hover" => "**Propiedad**\n`{value}` (codigo de {sourceFile}.txt)",
        "plugin.tc-item.after-first-gap" => {
            "El valor `{value}` de `{column}` en la clase de tesoro `{treasureClass}` aparece después del primer espacio vacío de objeto y el juego lo ignora."
        }
        "plugin.tc-prob.after-first-gap" => {
            "La probabilidad de `{column}` en la clase de tesoro `{treasureClass}` aparece después del primer espacio vacío de objeto y el juego la ignora."
        }
        "plugin.tc-prob.orphaned" => {
            "La probabilidad `{value}` de `{column}` en la clase de tesoro `{treasureClass}` no tiene una entrada de objeto correspondiente y se ignora."
        }
        "plugin.tc-item.forward-reference" => {
            "El valor `{value}` de `{column}` en la clase de tesoro `{treasureClass}` se refiere a una TC definida más adelante."
        }
        "plugin.tc-item.unresolved-base" => {
            "No se encontro la base de objeto `{value}` de `{column}` en la clase de tesoro `{treasureClass}`."
        }
        "plugin.tc-prob.blank-omission" => {
            "El campo vacio `{column}` de la clase de tesoro `{treasureClass}` omite la entrada correspondiente."
        }
        "plugin.tc-prob.noncanonical" => {
            "El valor `{value}` de `{column}` en la clase de tesoro `{treasureClass}` no es una probabilidad de entrada canonica."
        }
        "plugin.tc-prob.nonpositive-omission" => {
            "El valor no positivo `{value}` de `{column}` en la clase de tesoro `{treasureClass}` omite la entrada."
        }
        "plugin.tc-item.modifier-range" => {
            "El modificador `{value}` de `{column}` en la clase de tesoro `{treasureClass}` está fuera de 0..65535 y se truncará."
        }
        "plugin.tc-item.ignored-suffix" => {
            "La clase de tesoro `{treasureClass}` solo lee `{value}` de `{column}` e ignora lo demas."
        }
        "plugin.tc-item.field-width" => {
            "El valor `{value}` de `{column}` en la clase de tesoro `{treasureClass}` supera los 64 bytes UTF-8."
        }
        "plugin.treasure-class.hover-after-gap" => {
            "**Entrada de clase de tesoro después de un espacio vacío**\n\nDesde el espacio vacío de objeto {firstEmptySlot}, el juego ignora la base de objeto `{base}`."
        }
        "plugin.treasure-class.hover" => {
            "**Clase de tesoro**\n`{base}`\n{name}\nPosición: {slot}\nSelecciones: {picks}\nProbabilidad: {probability}\nModificadores: {modifiers}\nSufijo ignorado: `{ignoredSuffix}`"
        }
        _ => return None,
    })
}

/// German wording for every diagnostic and hover emitted by the bundled
/// d2rdoc plugins. Keep this exhaustive rather than relying on the generic
/// category fallback: each rule explains the actual game-facing behavior.
fn plugin_detail_de(key: &str) -> Option<&'static str> {
    Some(match key {
        "plugin.calc.skill-param-alias" => {
            "Der Berechnungsparameter-Alias `{alias}` wurde verwendet; der kanonische Bezeichner ist `{identifier}`. {values}"
        }
        "plugin.calc.unknown-missile-value" => {
            "Unbekannter missile-Wert `{identifier}`. Das Spiel behandelt diesen Wert als 0; dieser Teil der Berechnung hat daher keine Wirkung."
        }
        "plugin.calc.unterminated-string" => {
            "Eine Zeichenfolge im Berechnungsausdruck wurde nicht geschlossen. {values}"
        }
        "plugin.calc.unexpected-character" => {
            "Der Berechnungsausdruck enthält ein nicht zulässiges Zeichen oder Symbol. An der Fehlerposition steht `{actual}`."
        }
        "plugin.calc.unexpected-eof" => {
            "Der Berechnungsausdruck endet, bevor er vollständig ist. {values}"
        }
        "plugin.calc.unexpected-token" => {
            "Im Berechnungsausdruck wurde das unerwartete Token `{actual}` gefunden."
        }
        "plugin.calc.wrong-arity" => {
            "Eine Berechnungsfunktion erhält die falsche Anzahl von Argumenten. {values}"
        }
        "plugin.calc.expected-quoted-argument" => {
            "Das erste Argument dieser Berechnungsfunktion muss ein in Anführungszeichen gesetzter Name sein. {values}"
        }
        "plugin.calc.expected-dot-identifier" => {
            "Nach `.` wird ein Berechnungsbezeichner erwartet. {values}"
        }
        "plugin.calc.expected-rparen" => {
            "Im Berechnungsausdruck wird `{expected}` statt `{actual}` erwartet."
        }
        "plugin.calc.expected-rparen.eof" => {
            "Vor dem Ende des Berechnungsausdrucks wird `{expected}` erwartet. Das Spiel kann nur den davor gültigen Präfix verwenden."
        }
        "plugin.calc.expected-rbrack" => {
            "Im Berechnungsausdruck wird `{expected}` statt `{actual}` erwartet."
        }
        "plugin.calc.expected-rbrack.eof" => {
            "Vor dem Ende des Berechnungsausdrucks wird `{expected}` erwartet."
        }
        "plugin.calc.expected-colon" => {
            "Im bedingten Berechnungsausdruck wird `{expected}` statt `{actual}` erwartet."
        }
        "plugin.calc.expected-colon.eof" => {
            "Vor dem Ende des bedingten Berechnungsausdrucks wird `{expected}` erwartet."
        }
        "plugin.calc.expected-comma" => {
            "Zwischen Funktionsargumenten wird `{expected}` statt `{actual}` erwartet."
        }
        "plugin.calc.expected-comma.eof" => {
            "Am Ende der Funktionsargumente wird `{expected}` erwartet."
        }
        "plugin.unknownSkill" => {
            "Unbekannter Skillname `{identifier}`. Ersetzen Sie ihn durch den exakten Namen aus skills.txt."
        }
        "plugin.unknownMissile" => {
            "Unbekannter missile-Name `{identifier}`. Ersetzen Sie ihn durch den exakten Namen aus missiles.txt."
        }
        "plugin.unknownStat" => {
            "Unbekannter stat-Name `{identifier}`. Ersetzen Sie ihn durch den exakten Namen aus itemstatcost.txt."
        }
        "plugin.unknownCondition" => {
            "Unbekannter condition-Name `{identifier}` in der Berechnung. Ersetzen Sie ihn durch eine unterstützte condition."
        }
        "plugin.unknownIdentifier" => {
            "Der Bezeichner `{identifier}` ist im aktuellen Berechnungsbereich nicht bekannt."
        }
        "plugin.unknownSkillIdentifier" => {
            "Der Skill-Berechnungsbezeichner `{identifier}` ist nicht bekannt."
        }
        "plugin.unknownMissileIdentifier" => {
            "Der missile-Berechnungsbezeichner `{identifier}` ist nicht bekannt."
        }
        "plugin.unknownScopeIdentifier" => {
            "Der Bezeichner `{identifier}` ist im aktuellen Berechnungsbereich nicht bekannt."
        }
        "plugin.calc.skilldesc-decimal-prefix" => {
            "Im SkillDesc-Ausdruck `{actual}` verwendet das Spiel nur das ganzzahlige Präfix `{consumedPrefix}` und ignoriert `{ignoredSuffix}`."
        }
        "plugin.calc.decimal-policy" => {
            "Dieses Berechnungsfeld verlangt eine ganzzahlige Schreibweise. Dezimale Berechnungsausdrücke können vom Spiel anders ausgewertet werden. {values}"
        }
        "plugin.calc.prefix-stop" => {
            "Das Spiel liest nur den erkennbaren Präfix dieses Berechnungsausdrucks und ignoriert den Rest. {values}"
        }
        "plugin.cube-input.no-inputs" => {
            "Das Rezept `{recipe}` in cubemain.txt, Zeile {line}, hat keine Eingaben."
        }
        "plugin.cube-input.invalid-numinputs" => {
            "Das Rezept `{recipe}` in cubemain.txt, Zeile {line}, hat einen ungültigen numinputs-Wert `{value}`."
        }
        "plugin.cube-input.numinputs-mismatch" => {
            "Der numinputs-Wert des Rezepts `{recipe}` in cubemain.txt, Zeile {line}, stimmt nicht überein: erwartet wird {expected}, vorhanden ist {actual}."
        }
        "plugin.cube-input.empty-base" => {
            "Die Eingabe-Base in `{column}` des Rezepts `{recipe}` in cubemain.txt, Zeile {line}, ist leer."
        }
        "plugin.cube-input.invalid-base" => {
            "Die Base `{base}` in `{column}` des Rezepts `{recipe}` in cubemain.txt, Zeile {line}, wurde nicht gefunden."
        }
        "plugin.cube-input.ignored-suffix" => {
            "Das Rezept `{recipe}` in cubemain.txt, Zeile {line}, beendet das Parsen von `{column}` bei `{stoppedAt}`. Der nachfolgende Text wird ignoriert."
        }
        "plugin.cube-input.u8-range" => {
            "Die Menge `{quantity}` in `{column}` des Rezepts `{recipe}` in cubemain.txt, Zeile {line}, liegt außerhalb von 0..255. Das Spiel speichert {storedQuantity} und verwendet {effectiveQuantity}."
        }
        "plugin.cube-input.hover" => {
            "**Würfel-Eingabe**\nBase: `{base}`\nModifikatoren: {modifiers}"
        }
        "plugin.cube-output.invalid-base" => {
            "Die Ausgabe-Base `{value}` in `{column}` des Rezepts `{recipe}` in cubemain.txt, Zeile {line}, ist ungültig."
        }
        "plugin.cube-output.missing-ordinal-input" => {
            "Die Ausgabe in `{column}` des Rezepts `{recipe}` in cubemain.txt, Zeile {line}, verweist auf einen fehlenden ordinalen Eingabe-Slot."
        }
        "plugin.cube-output.u8-range" => {
            "Der Wert `{value}` in `{column}` des Rezepts `{recipe}` in cubemain.txt, Zeile {line}, liegt außerhalb von 0..255 und wird vom Spiel gekürzt."
        }
        "plugin.cube-output.ignored-suffix" => {
            "Das Rezept `{recipe}` in cubemain.txt, Zeile {line}, liest in `{column}` nur `{value}` und ignoriert den Rest."
        }
        "plugin.cube-output.invalid-property" => {
            "Die property `{value}` in `{column}` des Rezepts `{recipe}` in cubemain.txt, Zeile {line}, ist ungültig."
        }
        "plugin.cube-output.empty-base-hover" => "**Base:** leer (fehlende Würfel-Ausgabe-Base)",
        "plugin.cube-output.invalid-hover" => {
            "**Ungültige Würfel-Ausgabe-Base**\n\n`{base}` kann nicht aufgelöst werden. Ignorierter Suffix: `{ignoredSuffix}`."
        }
        "plugin.cube-output.hover" => {
            "**Würfel-Ausgabe**\nBase: `{base}` ({kind})\nEingabe: `{input}`\nModifikatoren: {modifiers}\nIgnorierter Suffix: `{ignoredSuffix}`"
        }
        "plugin.enum.hover" => {
            "**Aufzählungswert**\n\nWert: `{value}`\nName: {name}\nParameter: {parameters}\n\n{description}\n\nZusätzliche Felder: {extraFields}"
        }
        "plugin.item-code.unresolved" => {
            "Unbekannter Gegenstandscode `{value}`. Prüfen Sie den Code und die Groß-/Kleinschreibung."
        }
        "plugin.item-code.unresolved-packed-policy" => {
            "Es wurde kein passender Gegenstand gefunden. Dieses Feld wird nicht als gepackter Gegenstandscode interpretiert und der Text wird gekürzt; prüfen Sie, ob `{value}` der beabsichtigte Wert ist."
        }
        "plugin.item-code.unresolved-policy" => {
            "Der Gegenstandscode `{value}` ist nicht in weapons, armor oder misc vorhanden. Prüfen Sie, ob dies der beabsichtigte Code ist."
        }
        "plugin.item-code.hover" => "**Gegenstandscode**\n`{code}`\n{name}",
        "plugin.item-name.hover" => "{name}\n\n**Code**: {code}",
        "plugin.property.unknown-marker" => {
            "Der Wert `{value}` beginnt mit `*`, ist aber kein bekannter property-Code. Verwenden Sie den Marker nur für unterstützte property-Codes."
        }
        "plugin.property.unknown-code" => {
            "Unbekannter property-Code `{value}`. Verwenden Sie einen Code aus properties.txt oder propertygroups.txt."
        }
        "plugin.property.unknown-hover" => {
            "**Unbekannter property-Code**\n\n`{value}` wurde weder in properties.txt noch in propertygroups.txt gefunden."
        }
        "plugin.property.hover" => "**Property**\n`{value}` ({sourceFile}.txt-Code)",
        "plugin.tc-item.after-first-gap" => {
            "Der Wert `{value}` in `{column}` der Treasure Class `{treasureClass}` liegt nach dem ersten leeren Item-Slot und wird vom Spiel ignoriert."
        }
        "plugin.tc-prob.after-first-gap" => {
            "Die Wahrscheinlichkeit in `{column}` der Treasure Class `{treasureClass}` liegt nach dem ersten leeren Item-Slot und wird vom Spiel ignoriert."
        }
        "plugin.tc-prob.orphaned" => {
            "Die Wahrscheinlichkeit `{value}` in `{column}` der Treasure Class `{treasureClass}` hat keinen zugehörigen Item-Eintrag und wird ignoriert."
        }
        "plugin.tc-item.forward-reference" => {
            "Der Wert `{value}` in `{column}` der Treasure Class `{treasureClass}` verweist auf eine später definierte TC."
        }
        "plugin.tc-item.unresolved-base" => {
            "Die Item-Base `{value}` in `{column}` der Treasure Class `{treasureClass}` wurde nicht gefunden."
        }
        "plugin.tc-prob.blank-omission" => {
            "Das leere Feld `{column}` der Treasure Class `{treasureClass}` lässt den zugehörigen Eintrag aus."
        }
        "plugin.tc-prob.noncanonical" => {
            "Der Wert `{value}` in `{column}` der Treasure Class `{treasureClass}` ist keine kanonische Eintragswahrscheinlichkeit."
        }
        "plugin.tc-prob.nonpositive-omission" => {
            "Der nicht positive Wert `{value}` in `{column}` der Treasure Class `{treasureClass}` lässt den Eintrag aus."
        }
        "plugin.tc-item.modifier-range" => {
            "Der Modifikator `{value}` in `{column}` der Treasure Class `{treasureClass}` liegt außerhalb von 0..65535 und wird gekürzt."
        }
        "plugin.tc-item.ignored-suffix" => {
            "Die Treasure Class `{treasureClass}` liest in `{column}` nur `{value}` und ignoriert den Rest."
        }
        "plugin.tc-item.field-width" => {
            "Der Wert `{value}` in `{column}` der Treasure Class `{treasureClass}` überschreitet 64 UTF-8-Bytes."
        }
        "plugin.treasure-class.hover-after-gap" => {
            "**Treasure-Class-Eintrag nach Lücke**\n\nAb dem leeren Item-Slot {firstEmptySlot} wird die Item-Base `{base}` vom Spiel ignoriert."
        }
        "plugin.treasure-class.hover" => {
            "**Treasure Class**\n`{base}`\n{name}\nSlot: {slot}\nPicks: {picks}\nWahrscheinlichkeit: {probability}\nModifikatoren: {modifiers}\nIgnorierter Suffix: `{ignoredSuffix}`"
        }
        _ => return None,
    })
}

/// Bundled d2rdoc plugins expose structured arguments rather than completed
/// English text. Keep every Korean rule distinct so a Korean client never
/// falls back to a category-only or legacy-English explanation.
fn plugin_detail_ko(key: &str) -> Option<&'static str> {
    Some(match key {
        "plugin.calc.skill-param-alias" => {
            "계산식 매개변수 별칭: `{alias}`. 올바른 식별자: `{identifier}`. {values}"
        }
        "plugin.calc.unknown-missile-value" => {
            "알 수 없는 Missile 값: `{identifier}`. 게임은 이 값을 0으로 처리하므로 계산식의 이 부분은 적용되지 않습니다."
        }
        "plugin.calc.unterminated-string" => "계산식의 따옴표 문자열이 닫히지 않았습니다. {values}",
        "plugin.calc.unexpected-character" => {
            "계산식에 허용되지 않는 문자 또는 접미사가 있습니다. 문제 위치의 값: `{actual}`."
        }
        "plugin.calc.unexpected-eof" => "계산식이 완성되기 전에 끝났습니다. {values}",
        "plugin.calc.unexpected-token" => {
            "계산식에서 예상하지 못한 토큰을 발견했습니다: `{actual}`."
        }
        "plugin.calc.wrong-arity" => "계산 함수의 인수 개수가 맞지 않습니다. {values}",
        "plugin.calc.expected-quoted-argument" => {
            "이 계산 함수의 첫 번째 인수는 작은따옴표로 감싼 이름이어야 합니다. {values}"
        }
        "plugin.calc.expected-dot-identifier" => "`.` 뒤에 계산 식별자가 필요합니다. {values}",
        "plugin.calc.expected-rparen" => {
            "계산식에 필요한 기호: `{expected}`. 발견한 기호: `{actual}`."
        }
        "plugin.calc.expected-rparen.eof" => {
            "계산식 끝 전에 필요한 기호: `{expected}`. 게임은 그 앞의 유효한 부분만 사용할 수 있습니다."
        }
        "plugin.calc.expected-rbrack" => {
            "계산식에 필요한 기호: `{expected}`. 발견한 기호: `{actual}`."
        }
        "plugin.calc.expected-rbrack.eof" => "계산식 끝 전에 필요한 기호: `{expected}`.",
        "plugin.calc.expected-colon" => {
            "삼항 계산식에 필요한 기호: `{expected}`. 발견한 기호: `{actual}`."
        }
        "plugin.calc.expected-colon.eof" => "삼항 계산식 끝 전에 필요한 기호: `{expected}`.",
        "plugin.calc.expected-comma" => {
            "함수 인수 사이에 필요한 기호: `{expected}`. 발견한 기호: `{actual}`."
        }
        "plugin.calc.expected-comma.eof" => "함수 인수 끝에 필요한 기호: `{expected}`.",
        "plugin.unknownSkill" => {
            "skills.txt에 `{identifier}`라는 Skill 항목이 없습니다. 정확한 Skill 이름으로 바꾸세요."
        }
        "plugin.unknownMissile" => {
            "missiles.txt에 `{identifier}`라는 Missile 항목이 없습니다. 정확한 Missile 이름으로 바꾸세요."
        }
        "plugin.unknownStat" => {
            "itemstatcost.txt에 `{identifier}`라는 Stat 항목이 없습니다. 정확한 Stat 이름으로 바꾸세요."
        }
        "plugin.unknownCondition" => {
            "계산식에서 지원하지 않는 condition 이름: `{identifier}`. 지원되는 condition으로 바꾸세요."
        }
        "plugin.unknownIdentifier" => "현재 계산 범위에서 찾을 수 없는 식별자: `{identifier}`.",
        "plugin.unknownSkillIdentifier" => "Skill 계산식에서 찾을 수 없는 식별자: `{identifier}`.",
        "plugin.unknownMissileIdentifier" => {
            "Missile 계산식에서 찾을 수 없는 식별자: `{identifier}`."
        }
        "plugin.unknownScopeIdentifier" => {
            "현재 계산 범위에서 찾을 수 없는 식별자: `{identifier}`."
        }
        "plugin.calc.skilldesc-decimal-prefix" => {
            "SkillDesc 계산식에 소수가 포함되어 있습니다 (`{actual}`). 게임은 정수 부분만 사용하고 소수 부분은 무시합니다 (사용: `{consumedPrefix}`, 무시: `{ignoredSuffix}`)."
        }
        "plugin.calc.decimal-policy" => {
            "이 계산 필드는 정수 표현을 기대합니다. 소수 계산식은 게임에서 다르게 해석될 수 있습니다. {values}"
        }
        "plugin.calc.prefix-stop" => {
            "게임은 계산식의 인식 가능한 접두부까지만 읽고 나머지를 무시합니다. {values}"
        }
        "plugin.cube-input.no-inputs" => {
            "cubemain.txt {line}행의 recipe `{recipe}`에는 입력이 없습니다."
        }
        "plugin.cube-input.invalid-numinputs" => {
            "cubemain.txt {line}행의 recipe `{recipe}`: 올바르지 않은 numinputs 값: `{value}`."
        }
        "plugin.cube-input.numinputs-mismatch" => {
            "cubemain.txt {line}행 recipe `{recipe}`의 numinputs가 일치하지 않습니다. 예상값은 {expected}, 입력값은 {actual}입니다."
        }
        "plugin.cube-input.empty-base" => {
            "cubemain.txt {line}행 recipe `{recipe}`의 `{column}` 입력 base가 비어 있습니다."
        }
        "plugin.cube-input.invalid-base" => {
            "cubemain.txt {line}행의 recipe `{recipe}`: `{column}`에서 찾을 수 없는 Base: `{base}`."
        }
        "plugin.cube-input.ignored-suffix" => {
            "cubemain.txt {line}행의 recipe `{recipe}`: `{column}` 처리 중 `{stoppedAt}` 지점에서 읽기를 멈춥니다. 그 뒤의 텍스트는 무시됩니다."
        }
        "plugin.cube-input.u8-range" => {
            "cubemain.txt {line}행의 recipe `{recipe}`: `{column}` 수량 값은 `{quantity}`입니다. 허용 범위는 0..255입니다. 게임이 읽는 값: {storedQuantity}. 사용하는 수량: {effectiveQuantity}개."
        }
        "plugin.cube-input.hover" => "**큐브 입력**\nBase: `{base}`\n수정자: {modifiers}",
        "plugin.cube-output.invalid-base" => {
            "cubemain.txt {line}행의 recipe `{recipe}`: 올바르지 않은 `{column}` 출력 Base: `{value}`."
        }
        "plugin.cube-output.missing-ordinal-input" => {
            "cubemain.txt {line}행의 recipe `{recipe}`: `{column}` 출력에 대응하는 입력 슬롯이 없습니다."
        }
        "plugin.cube-output.u8-range" => {
            "cubemain.txt {line}행의 recipe `{recipe}`: `{column}` 값이 0..255 범위를 벗어나 게임에서 잘립니다. 입력값: `{value}`."
        }
        "plugin.cube-output.ignored-suffix" => {
            "cubemain.txt {line}행의 recipe `{recipe}`: 게임은 `{column}`에서 `{value}`까지만 읽고 나머지를 무시합니다."
        }
        "plugin.cube-output.invalid-property" => {
            "cubemain.txt {line}행의 recipe `{recipe}`: 유효하지 않은 `{column}` property 값: `{value}`."
        }
        "plugin.cube-output.empty-base-hover" => {
            "**Base:** 비어 있음(유효하지 않은 큐브 출력 Base)"
        }
        "plugin.cube-output.invalid-hover" => {
            "**유효하지 않은 큐브 출력 Base**\n\n해석할 수 없는 Base: `{base}`. 무시되는 접미사: `{ignoredSuffix}`."
        }
        "plugin.cube-output.hover" => {
            "**큐브 출력**\nBase: `{base}` ({kind})\n입력: `{input}`\n수정자: {modifiers}\n무시되는 접미사: `{ignoredSuffix}`"
        }
        "plugin.enum.hover" => {
            "**열거 값**\n\n값: `{value}`\n이름: {name}\n매개변수: {parameters}\n\n{description}\n\n추가 필드: {extraFields}"
        }
        "plugin.item-code.unresolved" => {
            "알 수 없는 아이템 코드: `{value}`. weapons.txt, armor.txt 또는 misc.txt의 코드를 확인하세요."
        }
        "plugin.item-code.unresolved-packed-policy" => {
            "일치하는 아이템을 찾지 못했습니다. 이 필드는 아이템으로 해석하지 않고 텍스트를 유지할 수 있으므로 의도한 값인지 확인하세요. 값: `{value}`."
        }
        "plugin.item-code.unresolved-policy" => {
            "weapons.txt, armor.txt 또는 misc.txt에서 찾을 수 없는 아이템 코드: `{value}`. 의도한 코드인지 확인하세요."
        }
        "plugin.item-code.hover" => "**아이템 코드**\n`{code}`\n{name}",
        "plugin.item-name.hover" => "{name}\n\n**코드**: {code}",
        "plugin.property.unknown-marker" => {
            "`*`로 시작하지만 알려진 property code가 아닌 값: `{value}`. 의도적인 marker일 때만 유지하세요."
        }
        "plugin.property.unknown-code" => {
            "알 수 없는 property code: `{value}`. properties.txt 또는 propertygroups.txt의 코드를 사용하세요."
        }
        "plugin.property.unknown-hover" => {
            "**알 수 없는 property code**\n\nproperties.txt 또는 propertygroups.txt에서 찾을 수 없는 값: `{value}`."
        }
        "plugin.property.hover" => "**Property**\n`{value}` ({sourceFile}.txt code)",
        "plugin.tc-item.after-first-gap" => {
            "Treasure Class `{treasureClass}`에서 첫 번째 빈 Item 슬롯 뒤라 게임이 무시하는 `{column}` 값: `{value}`."
        }
        "plugin.tc-prob.after-first-gap" => {
            "Treasure Class `{treasureClass}`의 `{column}` 확률은 첫 번째 빈 Item 슬롯 뒤에 있으므로 게임에서 무시됩니다."
        }
        "plugin.tc-prob.orphaned" => {
            "Treasure Class `{treasureClass}`의 `{column}` 확률 `{value}`에는 대응하는 Item 항목이 없어 무시됩니다."
        }
        "plugin.tc-item.forward-reference" => {
            "Treasure Class `{treasureClass}`의 `{column}`에서 아직 정의되지 않은 TC를 참조하는 값: `{value}`."
        }
        "plugin.tc-item.unresolved-base" => {
            "Treasure Class `{treasureClass}`의 `{column}`에서 찾을 수 없는 Item Base: `{value}`."
        }
        "plugin.tc-prob.blank-omission" => {
            "Treasure Class `{treasureClass}`에서 빈 열 때문에 해당 항목이 생략됩니다: `{column}`."
        }
        "plugin.tc-prob.noncanonical" => {
            "Treasure Class `{treasureClass}`의 `{column}`에서 정수가 아닌 값 때문에 항목이 생략될 수 있습니다: `{value}`."
        }
        "plugin.tc-prob.nonpositive-omission" => {
            "Treasure Class `{treasureClass}`의 `{column}` 값이 0 이하라 항목이 생략됩니다. 입력값: `{value}`."
        }
        "plugin.tc-item.modifier-range" => {
            "Treasure Class `{treasureClass}`의 `{column}`에서 0..65535 범위를 벗어난 수정자 값: `{value}`."
        }
        "plugin.tc-item.ignored-suffix" => {
            "Treasure Class `{treasureClass}`에서 게임이 읽는 `{column}` 값: `{value}`."
        }
        "plugin.tc-item.field-width" => {
            "Treasure Class `{treasureClass}`의 `{column}`에서 UTF-8 길이 제한을 초과한 값: `{value}`."
        }
        "plugin.treasure-class.hover-after-gap" => {
            "**무시되는 Treasure Class 항목**\n\n첫 번째 빈 Item 슬롯은 Item{firstEmptySlot}입니다. `{base}` 항목은 게임에서 사용되지 않습니다."
        }
        "plugin.treasure-class.hover" => {
            "**Treasure Class 항목**\n`{base}`\n{name}\n슬롯: {slot}\n추첨 횟수: {picks}\n확률 값: {probability}\n수정자: {modifiers}\n무시되는 접미사: `{ignoredSuffix}`"
        }
        _ => return None,
    })
}

/// Japanese wording for every diagnostic and hover emitted by the bundled
/// d2rdoc plugins. Values supplied by a TXT file are kept as placeholders.
fn plugin_detail_ja(key: &str) -> Option<&'static str> {
    Some(match key {
        "plugin.calc.skill-param-alias" => {
            "計算パラメーターの別名 `{alias}` を使用しています。正規の識別子は `{identifier}` です。{values}"
        }
        "plugin.calc.unknown-missile-value" => {
            "不明な missile 値 `{identifier}` です。ゲームでは 0 として扱われるため、この計算部分は機能しません。"
        }
        "plugin.calc.unterminated-string" => "計算式内の文字列が閉じられていません。{values}",
        "plugin.calc.unexpected-character" => {
            "計算式に使用できない文字または記号があります。問題の位置の値は `{actual}` です。"
        }
        "plugin.calc.unexpected-eof" => "計算式が完結する前に終了しています。{values}",
        "plugin.calc.unexpected-token" => "計算式に予期しないトークン `{actual}` があります。",
        "plugin.calc.wrong-arity" => "計算関数に渡された引数の数が正しくありません。{values}",
        "plugin.calc.expected-quoted-argument" => {
            "この計算関数の第 1 引数は引用符で囲んだ名前である必要があります。{values}"
        }
        "plugin.calc.expected-dot-identifier" => "`.` の後には計算識別子が必要です。{values}",
        "plugin.calc.expected-rparen" => "計算式には `{actual}` ではなく `{expected}` が必要です。",
        "plugin.calc.expected-rparen.eof" => {
            "計算式の終端前に `{expected}` が必要です。ゲームでは直前の有効な接頭辞だけを使用する可能性があります。"
        }
        "plugin.calc.expected-rbrack" => "計算式には `{actual}` ではなく `{expected}` が必要です。",
        "plugin.calc.expected-rbrack.eof" => "計算式の終端前に `{expected}` が必要です。",
        "plugin.calc.expected-colon" => {
            "条件計算式には `{actual}` ではなく `{expected}` が必要です。"
        }
        "plugin.calc.expected-colon.eof" => "条件計算式の終端前に `{expected}` が必要です。",
        "plugin.calc.expected-comma" => {
            "関数引数の間には `{actual}` ではなく `{expected}` が必要です。"
        }
        "plugin.calc.expected-comma.eof" => "引数リストの終端前に `{expected}` が必要です。",
        "plugin.unknownSkill" => {
            "不明な skill 名 `{identifier}` です。skills.txt にある正確な名前に置き換えてください。"
        }
        "plugin.unknownMissile" => {
            "不明な missile 名 `{identifier}` です。missiles.txt にある正確な名前に置き換えてください。"
        }
        "plugin.unknownStat" => {
            "不明な Stat 名 `{identifier}` です。itemstatcost.txt にある正確な Stat 名に置き換えてください。"
        }
        "plugin.unknownCondition" => {
            "計算式に不明な condition 名 `{identifier}` があります。対応する条件名に置き換えてください。"
        }
        "plugin.unknownIdentifier" => "現在の計算スコープに識別子 `{identifier}` はありません。",
        "plugin.unknownSkillIdentifier" => "skill 計算の識別子 `{identifier}` は不明です。",
        "plugin.unknownMissileIdentifier" => "missile 計算の識別子 `{identifier}` は不明です。",
        "plugin.unknownScopeIdentifier" => {
            "現在の計算スコープに識別子 `{identifier}` はありません。"
        }
        "plugin.calc.skilldesc-decimal-prefix" => {
            "SkillDesc 計算式 `{actual}` では、ゲームは整数部分 `{consumedPrefix}` だけを使用し、`{ignoredSuffix}` を無視します。"
        }
        "plugin.calc.decimal-policy" => {
            "この計算フィールドには整数形式が必要です。ゲームでは小数を含む計算式を異なる方法で評価する場合があります。{values}"
        }
        "plugin.calc.prefix-stop" => {
            "ゲームはこの計算式で認識できる接頭辞だけを読み取り、残りを無視します。{values}"
        }
        "plugin.cube-input.no-inputs" => {
            "cubemain.txt の行 {line}、レシピ `{recipe}` には入力がありません。"
        }
        "plugin.cube-input.invalid-numinputs" => {
            "cubemain.txt の行 {line}、レシピ `{recipe}` の numinputs 値 `{value}` は無効です。"
        }
        "plugin.cube-input.numinputs-mismatch" => {
            "cubemain.txt の行 {line}、レシピ `{recipe}` の numinputs が一致しません。期待値は {expected}、実際の値は {actual} です。"
        }
        "plugin.cube-input.empty-base" => {
            "cubemain.txt の行 {line}、レシピ `{recipe}` の `{column}` 入力 base が空です。"
        }
        "plugin.cube-input.invalid-base" => {
            "cubemain.txt の行 {line}、レシピ `{recipe}` の `{column}` に base `{base}` が見つかりません。"
        }
        "plugin.cube-input.ignored-suffix" => {
            "cubemain.txt の行 {line}、レシピ `{recipe}` は `{column}` を `{stoppedAt}` で解析終了します。その後の文字列は無視されます。"
        }
        "plugin.cube-input.u8-range" => {
            "cubemain.txt の行 {line}、レシピ `{recipe}` の `{column}` にある数量 `{quantity}` は 0..255 の範囲外です。ゲームは {storedQuantity} を保存し、{effectiveQuantity} を使用します。"
        }
        "plugin.cube-input.hover" => "**キューブ入力**\n基本項目: `{base}`\n修飾子: {modifiers}",
        "plugin.cube-output.invalid-base" => {
            "cubemain.txt の行 {line}、レシピ `{recipe}` の `{column}` 出力の基本項目 `{value}` は無効です。"
        }
        "plugin.cube-output.missing-ordinal-input" => {
            "cubemain.txt の行 {line}、レシピ `{recipe}` の `{column}` 出力が存在しない序数入力スロットを参照しています。"
        }
        "plugin.cube-output.u8-range" => {
            "cubemain.txt の行 {line}、レシピ `{recipe}` の `{column}` にある値 `{value}` は 0..255 の範囲外で、ゲームによって切り詰められます。"
        }
        "plugin.cube-output.ignored-suffix" => {
            "cubemain.txt の行 {line}、レシピ `{recipe}` は `{column}` の `{value}` だけを読み取り、残りを無視します。"
        }
        "plugin.cube-output.invalid-property" => {
            "cubemain.txt の行 {line}、レシピ `{recipe}` の `{column}` にあるプロパティ `{value}` は無効です。"
        }
        "plugin.cube-output.empty-base-hover" => {
            "**基本項目:** 空です（キューブ出力の基本項目がありません）"
        }
        "plugin.cube-output.invalid-hover" => {
            "**無効なキューブ出力の基本項目**\n\n`{base}` は見つかりません。無視される接尾辞: `{ignoredSuffix}`。"
        }
        "plugin.cube-output.hover" => {
            "**キューブ出力**\n基本項目: `{base}` ({kind})\n入力: `{input}`\n修飾子: {modifiers}\n無視される接尾辞: `{ignoredSuffix}`"
        }
        "plugin.enum.hover" => {
            "**列挙値**\n\n値: `{value}`\n名前: {name}\nパラメーター: {parameters}\n\n{description}\n\n追加フィールド: {extraFields}"
        }
        "plugin.item-code.unresolved" => {
            "不明なアイテムコード `{value}` です。コードと大文字・小文字を確認してください。"
        }
        "plugin.item-code.unresolved-packed-policy" => {
            "一致するアイテムが見つかりません。このフィールドはパック済みアイテムコードとして解釈されず、文字列は切り詰められます。生の値 `{value}` を確認してください。"
        }
        "plugin.item-code.unresolved-policy" => {
            "アイテムコード `{value}` は weapons、armor、または misc に存在しません。コードを確認してください。"
        }
        "plugin.item-code.hover" => "**アイテムコード**\n`{code}`\n{name}",
        "plugin.item-name.hover" => "{name}\n\n**コード**: {code}",
        "plugin.property.unknown-marker" => {
            "値 `{value}` は `*` で始まっていますが、既知の property code ではありません。正しい marker とコードだけを使用してください。"
        }
        "plugin.property.unknown-code" => {
            "不明な property code `{value}` です。properties.txt または propertygroups.txt にあるコードを使用してください。"
        }
        "plugin.property.unknown-hover" => {
            "**不明な property code**\n\n`{value}` は properties.txt または propertygroups.txt に見つかりません。"
        }
        "plugin.property.hover" => "**Property**\n`{value}` ({sourceFile}.txt code)",
        "plugin.tc-item.after-first-gap" => {
            "Treasure Class `{treasureClass}` の `{column}` 値 `{value}` は最初の空のアイテムスロットより後にあるため、ゲームでは無視されます。"
        }
        "plugin.tc-prob.after-first-gap" => {
            "Treasure Class `{treasureClass}` の `{column}` の確率は最初の空のアイテムスロットより後にあるため、ゲームでは無視されます。"
        }
        "plugin.tc-prob.orphaned" => {
            "Treasure Class `{treasureClass}` の `{column}` の確率 `{value}` には対応するアイテムがなく、無視されます。"
        }
        "plugin.tc-item.forward-reference" => {
            "Treasure Class `{treasureClass}` の `{column}` 値 `{value}` は、まだ定義されていない TC を参照しています。"
        }
        "plugin.tc-item.unresolved-base" => {
            "Treasure Class `{treasureClass}` の `{column}` にアイテム基本項目 `{value}` が見つかりません。"
        }
        "plugin.tc-prob.blank-omission" => {
            "Treasure Class `{treasureClass}` の `{column}` は空のため、この項目は省略されます。"
        }
        "plugin.tc-prob.noncanonical" => {
            "Treasure Class `{treasureClass}` の `{column}` 値 `{value}` は整数ではないため、この項目は省略される場合があります。"
        }
        "plugin.tc-prob.nonpositive-omission" => {
            "Treasure Class `{treasureClass}` の `{column}` 値 `{value}` は 0 以下のため、この項目は省略されます。"
        }
        "plugin.tc-item.modifier-range" => {
            "Treasure Class `{treasureClass}` の `{column}` 修飾子 `{value}` は 0..65535 の範囲外で、ゲームによって変換されます。"
        }
        "plugin.tc-item.ignored-suffix" => {
            "Treasure Class `{treasureClass}` は `{column}` の値 `{value}` で接尾辞以降を無視します。"
        }
        "plugin.tc-item.field-width" => {
            "Treasure Class `{treasureClass}` の `{column}` 値 `{value}` は UTF-8 で 64 バイトを超えています。"
        }
        "plugin.treasure-class.hover-after-gap" => {
            "**Treasure Class の項目は無視されます**\n\n最初の空のアイテムスロットは Item{firstEmptySlot} です。そのため `{base}` は使用されません。"
        }
        "plugin.treasure-class.hover" => {
            "**Treasure Class の項目**\n`{base}`\n{name}\nスロット: {slot}\n抽選回数: {picks}\n確率: {probability}\n修飾子: {modifiers}\n無視される接尾辞: `{ignoredSuffix}`"
        }
        _ => return None,
    })
}

/// Russian wording for every diagnostic and hover emitted by the bundled
/// d2rdoc plugins. Values from data files remain untouched placeholders.
fn plugin_detail_ru(key: &str) -> Option<&'static str> {
    Some(match key {
        "plugin.calc.skill-param-alias" => {
            "Использован псевдоним параметра формулы `{alias}`; канонический идентификатор — `{identifier}`. {values}"
        }
        "plugin.calc.unknown-missile-value" => {
            "Неизвестное значение missile `{identifier}`. Игра считает его равным 0, поэтому эта часть формулы не действует."
        }
        "plugin.calc.unterminated-string" => "Строка в формуле не завершена. {values}",
        "plugin.calc.unexpected-character" => {
            "Формула содержит недопустимый символ. В указанной позиции находится `{actual}`."
        }
        "plugin.calc.unexpected-eof" => "Формула заканчивается до завершения выражения. {values}",
        "plugin.calc.unexpected-token" => "Формула содержит неожиданный токен `{actual}`.",
        "plugin.calc.wrong-arity" => "Функция формулы получила неверное число аргументов. {values}",
        "plugin.calc.expected-quoted-argument" => {
            "Первый аргумент этой функции формулы должен быть именем в кавычках. {values}"
        }
        "plugin.calc.expected-dot-identifier" => {
            "После `.` ожидается идентификатор формулы. {values}"
        }
        "plugin.calc.expected-rparen" => "В формуле ожидался `{expected}`, а получен `{actual}`.",
        "plugin.calc.expected-rparen.eof" => {
            "В формуле перед концом ожидается `{expected}`. Игра может использовать только предыдущий корректный префикс."
        }
        "plugin.calc.expected-rbrack" => "В формуле ожидался `{expected}`, а получен `{actual}`.",
        "plugin.calc.expected-rbrack.eof" => "В формуле перед концом ожидается `{expected}`.",
        "plugin.calc.expected-colon" => {
            "В условном выражении формулы ожидался `{expected}`, а получен `{actual}`."
        }
        "plugin.calc.expected-colon.eof" => {
            "В условном выражении перед концом ожидается `{expected}`."
        }
        "plugin.calc.expected-comma" => {
            "В списке аргументов ожидался `{expected}`, а получен `{actual}`."
        }
        "plugin.calc.expected-comma.eof" => "В конце списка аргументов ожидается `{expected}`.",
        "plugin.unknownSkill" => {
            "Неизвестное имя навыка `{identifier}`. Замените его точным именем из skills.txt."
        }
        "plugin.unknownMissile" => {
            "Неизвестное имя missile `{identifier}`. Замените его точным именем из missiles.txt."
        }
        "plugin.unknownStat" => {
            "Неизвестное имя Stat `{identifier}`. Замените его точным именем Stat из itemstatcost.txt."
        }
        "plugin.unknownCondition" => {
            "Неизвестное условие `{identifier}` в формуле. Замените его поддерживаемым условием."
        }
        "plugin.unknownIdentifier" => {
            "Идентификатор `{identifier}` неизвестен в текущей области видимости формулы."
        }
        "plugin.unknownSkillIdentifier" => {
            "Идентификатор формулы навыка `{identifier}` неизвестен."
        }
        "plugin.unknownMissileIdentifier" => {
            "Идентификатор формулы missile `{identifier}` неизвестен."
        }
        "plugin.unknownScopeIdentifier" => {
            "Идентификатор `{identifier}` неизвестен в текущей области видимости формулы."
        }
        "plugin.calc.skilldesc-decimal-prefix" => {
            "В выражении SkillDesc `{actual}` игра использует только целую часть `{consumedPrefix}` и игнорирует `{ignoredSuffix}`."
        }
        "plugin.calc.decimal-policy" => {
            "Это поле формулы требует целочисленного значения. Игра может иначе вычислять десятичные выражения. {values}"
        }
        "plugin.calc.prefix-stop" => {
            "Игра читает только распознаваемый префикс этой формулы и игнорирует остаток. {values}"
        }
        "plugin.cube-input.no-inputs" => {
            "У рецепта `{recipe}` в cubemain.txt, строка {line}, нет входных данных."
        }
        "plugin.cube-input.invalid-numinputs" => {
            "У рецепта `{recipe}` в cubemain.txt, строка {line}, недопустимое значение numinputs `{value}`."
        }
        "plugin.cube-input.numinputs-mismatch" => {
            "Число numinputs для рецепта `{recipe}` в cubemain.txt, строка {line}, не совпадает: ожидалось {expected}, найдено {actual}."
        }
        "plugin.cube-input.empty-base" => {
            "Базовый предмет входа в `{column}` рецепта `{recipe}` в cubemain.txt, строка {line}, пуст."
        }
        "plugin.cube-input.invalid-base" => {
            "Базовый предмет `{base}` не найден в `{column}` рецепта `{recipe}` в cubemain.txt, строка {line}."
        }
        "plugin.cube-input.ignored-suffix" => {
            "Рецепт `{recipe}` в cubemain.txt, строка {line}, перестаёт разбирать `{column}` на `{stoppedAt}`. Последующий текст игнорируется."
        }
        "plugin.cube-input.u8-range" => {
            "Количество `{quantity}` в `{column}` рецепта `{recipe}` в cubemain.txt, строка {line}, вне диапазона 0..255. Игра сохранит {storedQuantity} и использует {effectiveQuantity}."
        }
        "plugin.cube-input.hover" => "**Вход куба**\nБаза: `{base}`\nМодификаторы: {modifiers}",
        "plugin.cube-output.invalid-base" => {
            "Базовый предмет выхода `{value}` в `{column}` рецепта `{recipe}` в cubemain.txt, строка {line}, недопустим."
        }
        "plugin.cube-output.missing-ordinal-input" => {
            "Выход в `{column}` рецепта `{recipe}` в cubemain.txt, строка {line}, ссылается на отсутствующий порядковый входной слот."
        }
        "plugin.cube-output.u8-range" => {
            "Значение `{value}` в `{column}` рецепта `{recipe}` в cubemain.txt, строка {line}, вне диапазона 0..255 и будет усечено игрой."
        }
        "plugin.cube-output.ignored-suffix" => {
            "Рецепт `{recipe}` в cubemain.txt, строка {line}, читает в `{column}` только `{value}` и игнорирует остаток."
        }
        "plugin.cube-output.invalid-property" => {
            "Свойство `{value}` в `{column}` рецепта `{recipe}` в cubemain.txt, строка {line}, недопустимо."
        }
        "plugin.cube-output.empty-base-hover" => {
            "**База:** пуста (не указан базовый предмет выхода куба)"
        }
        "plugin.cube-output.invalid-hover" => {
            "**Недопустимая база выхода куба**\n\n`{base}` не найден. Игнорируемый суффикс: `{ignoredSuffix}`."
        }
        "plugin.cube-output.hover" => {
            "**Выход куба**\nБаза: `{base}` ({kind})\nВход: `{input}`\nМодификаторы: {modifiers}\nИгнорируемый суффикс: `{ignoredSuffix}`"
        }
        "plugin.enum.hover" => {
            "**Перечисление**\n\nЗначение: `{value}`\nИмя: {name}\nПараметры: {parameters}\n\n{description}\n\nДополнительные поля: {extraFields}"
        }
        "plugin.item-code.unresolved" => {
            "Неизвестный код предмета `{value}`. Укажите существующий код базового предмета."
        }
        "plugin.item-code.unresolved-packed-policy" => {
            "Упакованный код предмета `{value}` не удаётся разрешить по правилам чтения игры."
        }
        "plugin.item-code.unresolved-policy" => {
            "Код предмета `{value}` не найден в weapons, armor или misc. Укажите существующий код базового предмета."
        }
        "plugin.item-code.hover" => "**Код предмета**\n`{code}`\n{name}",
        "plugin.item-name.hover" => "{name}\n\n**Код предмета:** {code}",
        "plugin.property.unknown-marker" => {
            "Значение `{value}` начинается с `*`, но не является известным кодом свойства. Проверьте маркер и код свойства."
        }
        "plugin.property.unknown-code" => {
            "Неизвестный код свойства `{value}`. Проверьте properties.txt и propertygroups.txt."
        }
        "plugin.property.unknown-hover" => {
            "**Неизвестный код свойства**\n\n`{value}` не найден в properties.txt или propertygroups.txt."
        }
        "plugin.property.hover" => "**Свойство**\n`{value}` (код из {sourceFile}.txt)",
        "plugin.tc-item.after-first-gap" => {
            "В Treasure Class `{treasureClass}` значение `{value}` в `{column}` находится после первого пустого слота Item и игнорируется."
        }
        "plugin.tc-prob.after-first-gap" => {
            "В Treasure Class `{treasureClass}` значение в `{column}` находится после первого пустого слота Item и игнорируется."
        }
        "plugin.tc-prob.orphaned" => {
            "В Treasure Class `{treasureClass}` значение `{value}` в `{column}` не соответствует записи Item и будет проигнорировано."
        }
        "plugin.tc-item.forward-reference" => {
            "В Treasure Class `{treasureClass}` значение `{value}` в `{column}` ссылается на TC, объявленный позднее."
        }
        "plugin.tc-item.unresolved-base" => {
            "База предмета `{value}` в `{column}` Treasure Class `{treasureClass}` не найдена."
        }
        "plugin.tc-prob.blank-omission" => {
            "Пустое значение в `{column}` Treasure Class `{treasureClass}` пропускает запись."
        }
        "plugin.tc-prob.noncanonical" => {
            "Значение `{value}` в `{column}` Treasure Class `{treasureClass}` неканонично и пропускает запись."
        }
        "plugin.tc-prob.nonpositive-omission" => {
            "Значение `{value}` в `{column}` Treasure Class `{treasureClass}` не больше 0 и пропускает запись."
        }
        "plugin.tc-item.modifier-range" => {
            "Модификатор `{value}` в `{column}` Treasure Class `{treasureClass}` вне диапазона 0..65535 и будет усечён игрой."
        }
        "plugin.tc-item.ignored-suffix" => {
            "В Treasure Class `{treasureClass}` игра читает в `{column}` только `{value}` и игнорирует остаток."
        }
        "plugin.tc-item.field-width" => {
            "Значение `{value}` в `{column}` Treasure Class `{treasureClass}` длиннее 64 байт UTF-8."
        }
        "plugin.treasure-class.hover-after-gap" => {
            "**Запись Treasure Class игнорируется**\n\n`{base}` находится после первого пустого слота Item {firstEmptySlot}, поэтому игра не читает этот слот Item."
        }
        "plugin.treasure-class.hover" => {
            "**Запись Treasure Class**\n`{base}`\n{name}\nСлот: {slot}, picks: {picks}, вероятность: {probability}\nМодификаторы: {modifiers}\nИгнорируемый суффикс: `{ignoredSuffix}`"
        }
        _ => return None,
    })
}

fn plugin_detail_zh_cn(key: &str) -> Option<&'static str> {
    Some(match key {
        "plugin.calc.skill-param-alias" => {
            "计算公式使用了参数别名 `{alias}`。当前标识符：`{identifier}`。{values}"
        }
        "plugin.calc.unknown-missile-value" => {
            "未知的 missile 值 `{identifier}`。游戏会将其视为 0，这部分计算不会生效。"
        }
        "plugin.calc.unterminated-string" => "计算公式中的引号字符串未闭合。{values}",
        "plugin.calc.unexpected-character" => {
            "计算公式含有不允许的字符或后缀。问题值：`{actual}`。"
        }
        "plugin.calc.unexpected-eof" => "计算公式尚未完成便结束了。{values}",
        "plugin.calc.unexpected-token" => "计算公式中出现了意外的 token `{actual}`。",
        "plugin.calc.wrong-arity" => "计算函数的参数数量不正确。{values}",
        "plugin.calc.expected-quoted-argument" => {
            "此计算函数的第一个参数必须是用单引号括起的名称。{values}"
        }
        "plugin.calc.expected-dot-identifier" => "`.` 后需要计算标识符。{values}",
        "plugin.calc.expected-rparen" => "计算公式中 `{actual}` 前需要 `{expected}`。",
        "plugin.calc.expected-rparen.eof" => {
            "计算公式结束前需要 `{expected}`；游戏仍可能使用此前有效的部分。"
        }
        "plugin.calc.expected-rbrack" => "计算公式中 `{actual}` 前需要 `{expected}`。",
        "plugin.calc.expected-rbrack.eof" => "计算公式结束前需要 `{expected}`。",
        "plugin.calc.expected-colon" => "三元计算公式中 `{actual}` 前需要 `{expected}`。",
        "plugin.calc.expected-colon.eof" => "三元计算公式结束前需要 `{expected}`。",
        "plugin.calc.expected-comma" => "函数参数之间、`{actual}` 前需要 `{expected}`。",
        "plugin.calc.expected-comma.eof" => "函数参数结尾需要 `{expected}`。",
        "plugin.unknownSkill" => {
            "未知的 skill 名称。请将 `{identifier}` 改为 skills.txt 中的准确名称。"
        }
        "plugin.unknownMissile" => {
            "未知的 missile 名称。请将 `{identifier}` 改为 missiles.txt 中的准确名称。"
        }
        "plugin.unknownStat" => {
            "未知的 stat 名称。请将 `{identifier}` 改为 itemstatcost.txt 中的准确名称。"
        }
        "plugin.unknownCondition" => {
            "未知的计算 condition 名称。请将 `{identifier}` 改为受支持的 condition。"
        }
        "plugin.unknownIdentifier" => "当前计算范围中找不到标识符 `{identifier}`。",
        "plugin.unknownSkillIdentifier" => "找不到 skill 计算标识符 `{identifier}`。",
        "plugin.unknownMissileIdentifier" => "找不到 missile 计算标识符 `{identifier}`。",
        "plugin.unknownScopeIdentifier" => "当前计算 scope 中找不到标识符 `{identifier}`。",
        "plugin.calc.skilldesc-decimal-prefix" => {
            "在 SkillDesc 计算公式 `{actual}` 中，游戏只使用整数部分 `{consumedPrefix}`，并忽略 `{ignoredSuffix}`。"
        }
        "plugin.calc.decimal-policy" => {
            "此计算字段应使用整数写法；小数公式可能被游戏以不同方式解释。{values}"
        }
        "plugin.calc.prefix-stop" => "游戏只会读取可识别的计算前缀，并忽略其余部分。{values}",
        "plugin.cube-input.no-inputs" => "cubemain.txt 第 {line} 行的 recipe `{recipe}` 没有输入。",
        "plugin.cube-input.invalid-numinputs" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 numinputs 值 `{value}` 无效。"
        }
        "plugin.cube-input.numinputs-mismatch" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 numinputs 不匹配：应为 {expected}，实际为 {actual}。"
        }
        "plugin.cube-input.empty-base" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 `{column}` 输入 base 为空。"
        }
        "plugin.cube-input.invalid-base" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 `{column}` 找不到 base `{base}`。"
        }
        "plugin.cube-input.ignored-suffix" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 `{column}` 会在 `{stoppedAt}` 停止解析；之后的文本会被忽略。"
        }
        "plugin.cube-input.u8-range" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 `{column}` 数量 `{quantity}` 超出 0..255。游戏读取为 {storedQuantity}，并使用 {effectiveQuantity} 个。"
        }
        "plugin.cube-input.hover" => "**魔盒输入**\nBase: `{base}`\n修饰符：{modifiers}",
        "plugin.cube-output.invalid-base" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 `{column}` 输出 base `{value}` 无效。"
        }
        "plugin.cube-output.missing-ordinal-input" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 `{column}` 输出没有对应的 input slot。"
        }
        "plugin.cube-output.u8-range" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 `{column}` 值 `{value}` 超出 0..255，游戏会截断它。"
        }
        "plugin.cube-output.ignored-suffix" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 `{column}` 会忽略 `{value}` 的一部分。"
        }
        "plugin.cube-output.invalid-property" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 `{column}` property `{value}` 无效。"
        }
        "plugin.cube-output.empty-base-hover" => "**Base：**为空（无效的 cube output base）",
        "plugin.cube-output.invalid-hover" => {
            "**无效的魔盒输出 base**\n\n无法解析 `{base}`。被忽略的后缀：`{ignoredSuffix}`。"
        }
        "plugin.cube-output.hover" => {
            "**魔盒输出**\n基础项：`{base}` ({kind})\n输入：`{input}`\n修饰符：{modifiers}\n忽略的后缀：`{ignoredSuffix}`"
        }
        "plugin.enum.hover" => {
            "**枚举值**\n\n值：`{value}`\n名称：{name}\n参数：{parameters}\n\n{description}\n\n额外字段：{extraFields}"
        }
        "plugin.item-code.unresolved" => "未知的物品代码 `{value}`。请检查代码和字母大小写。",
        "plugin.item-code.unresolved-packed-policy" => {
            "未找到匹配的物品。此字段可保留未解析的文本，请确认值 `{value}` 是否符合预期。"
        }
        "plugin.item-code.unresolved-policy" => {
            "物品代码 `{value}` 不在 weapons、armor 或 misc 中。请确认该代码是否符合预期。"
        }
        "plugin.item-code.hover" => "**物品代码**\n`{code}`\n{name}",
        "plugin.item-name.hover" => "{name}\n\n**代码：**{code}",
        "plugin.property.unknown-marker" => {
            "值 `{value}` 以 `*` 开头，但不是已知的 property code。仅在它是有意的 marker 时保留。"
        }
        "plugin.property.unknown-code" => {
            "未知的 property code `{value}`。请使用 properties.txt 或 propertygroups.txt 中的代码。"
        }
        "plugin.property.unknown-hover" => {
            "**未知的 property code**\n\n在 properties.txt 或 propertygroups.txt 中找不到 `{value}`。"
        }
        "plugin.property.hover" => "**Property**\n`{value}` ({sourceFile}.txt code)",
        "plugin.tc-item.after-first-gap" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 值 `{value}` 位于第一个空物品槽位之后，游戏会忽略它。"
        }
        "plugin.tc-prob.after-first-gap" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 概率位于第一个空物品槽位之后，游戏会忽略它。"
        }
        "plugin.tc-prob.orphaned" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 概率 `{value}` 没有对应的物品，因而被忽略。"
        }
        "plugin.tc-item.forward-reference" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 值 `{value}` 引用了尚未定义的 TC。"
        }
        "plugin.tc-item.unresolved-base" => {
            "找不到 Treasure Class `{treasureClass}` 的 `{column}` 物品基础项 `{value}`。"
        }
        "plugin.tc-prob.blank-omission" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 为空，因此该项会被省略。"
        }
        "plugin.tc-prob.noncanonical" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 值 `{value}` 不是整数，该项可能被省略。"
        }
        "plugin.tc-prob.nonpositive-omission" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 值 `{value}` 小于等于 0，因此该项会被省略。"
        }
        "plugin.tc-item.modifier-range" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 修饰符 `{value}` 超出 0..65535，游戏会进行转换。"
        }
        "plugin.tc-item.ignored-suffix" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 会忽略 `{value}` 后的后缀。"
        }
        "plugin.tc-item.field-width" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 值 `{value}` 的 UTF-8 长度至少为 64 字节。"
        }
        "plugin.treasure-class.hover-after-gap" => {
            "**Treasure Class 项目被忽略**\n\n第一个空物品槽位是 Item{firstEmptySlot}，因此 `{base}` 不会被使用。"
        }
        "plugin.treasure-class.hover" => {
            "**Treasure Class 项目**\n`{base}`\n{name}\n槽位：{slot}\n抽取次数：{picks}\n概率：{probability}\n修饰符：{modifiers}\n忽略的后缀：`{ignoredSuffix}`"
        }
        _ => return None,
    })
}

// Traditional Chinese templates keep the same structured placeholder
// contract as zhCN while rendering every bundled-plugin explanation locally.
fn plugin_detail_zh_tw(key: &str) -> Option<&'static str> {
    Some(match key {
        "plugin.calc.skill-param-alias" => {
            "計算公式使用了引數別名 `{alias}`。當前識別符號：`{identifier}`。{values}"
        }
        "plugin.calc.unknown-missile-value" => {
            "未知的 missile 值 `{identifier}`。遊戲會將其視為 0，這部分計算不會生效。"
        }
        "plugin.calc.unterminated-string" => "計算公式中的引號字串未閉合。{values}",
        "plugin.calc.unexpected-character" => {
            "計算公式含有不允許的字元或字尾。問題值：`{actual}`。"
        }
        "plugin.calc.unexpected-eof" => "計算公式尚未完成便結束了。{values}",
        "plugin.calc.unexpected-token" => "計算公式中出現了意外的 token `{actual}`。",
        "plugin.calc.wrong-arity" => "計算函式的引數數量不正確。{values}",
        "plugin.calc.expected-quoted-argument" => {
            "此計算函式的第一個引數必須是用單引號括起的名稱。{values}"
        }
        "plugin.calc.expected-dot-identifier" => "`.` 後需要計算識別符號。{values}",
        "plugin.calc.expected-rparen" => "計算公式中 `{actual}` 前需要 `{expected}`。",
        "plugin.calc.expected-rparen.eof" => {
            "計算公式結束前需要 `{expected}`；遊戲仍可能使用此前有效的部分。"
        }
        "plugin.calc.expected-rbrack" => "計算公式中 `{actual}` 前需要 `{expected}`。",
        "plugin.calc.expected-rbrack.eof" => "計算公式結束前需要 `{expected}`。",
        "plugin.calc.expected-colon" => "三元計算公式中 `{actual}` 前需要 `{expected}`。",
        "plugin.calc.expected-colon.eof" => "三元計算公式結束前需要 `{expected}`。",
        "plugin.calc.expected-comma" => "函式引數之間、`{actual}` 前需要 `{expected}`。",
        "plugin.calc.expected-comma.eof" => "函式引數結尾需要 `{expected}`。",
        "plugin.unknownSkill" => {
            "未知的 skill 名稱。請將 `{identifier}` 改為 skills.txt 中的準確名稱。"
        }
        "plugin.unknownMissile" => {
            "未知的 missile 名稱。請將 `{identifier}` 改為 missiles.txt 中的準確名稱。"
        }
        "plugin.unknownStat" => {
            "未知的 stat 名稱。請將 `{identifier}` 改為 itemstatcost.txt 中的準確名稱。"
        }
        "plugin.unknownCondition" => {
            "未知的計算 condition 名稱。請將 `{identifier}` 改為受支援的 condition。"
        }
        "plugin.unknownIdentifier" => "當前計算範圍中找不到識別符號 `{identifier}`。",
        "plugin.unknownSkillIdentifier" => "找不到 skill 計算識別符號 `{identifier}`。",
        "plugin.unknownMissileIdentifier" => "找不到 missile 計算識別符號 `{identifier}`。",
        "plugin.unknownScopeIdentifier" => "當前計算 scope 中找不到識別符號 `{identifier}`。",
        "plugin.calc.skilldesc-decimal-prefix" => {
            "在 SkillDesc 計算公式 `{actual}` 中，遊戲只使用整數部分 `{consumedPrefix}`，並忽略 `{ignoredSuffix}`。"
        }
        "plugin.calc.decimal-policy" => {
            "此計算欄位應使用整數寫法；小數公式可能被遊戲以不同方式解釋。{values}"
        }
        "plugin.calc.prefix-stop" => "遊戲只會讀取可識別的計算字首，並忽略其餘部分。{values}",
        "plugin.cube-input.no-inputs" => "cubemain.txt 第 {line} 行的 recipe `{recipe}` 沒有輸入。",
        "plugin.cube-input.invalid-numinputs" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 numinputs 值 `{value}` 無效。"
        }
        "plugin.cube-input.numinputs-mismatch" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 numinputs 不匹配：應為 {expected}，實際為 {actual}。"
        }
        "plugin.cube-input.empty-base" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 `{column}` 輸入 base 為空。"
        }
        "plugin.cube-input.invalid-base" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 `{column}` 找不到 base `{base}`。"
        }
        "plugin.cube-input.ignored-suffix" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 `{column}` 會在 `{stoppedAt}` 停止解析；之後的文字會被忽略。"
        }
        "plugin.cube-input.u8-range" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 `{column}` 數量 `{quantity}` 超出 0..255。遊戲讀取為 {storedQuantity}，並使用 {effectiveQuantity} 個。"
        }
        "plugin.cube-input.hover" => "**魔盒輸入**\nBase: `{base}`\n修飾符：{modifiers}",
        "plugin.cube-output.invalid-base" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 `{column}` 輸出 base `{value}` 無效。"
        }
        "plugin.cube-output.missing-ordinal-input" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 `{column}` 輸出沒有對應的 input slot。"
        }
        "plugin.cube-output.u8-range" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 `{column}` 值 `{value}` 超出 0..255，遊戲會截斷它。"
        }
        "plugin.cube-output.ignored-suffix" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 `{column}` 會忽略 `{value}` 的一部分。"
        }
        "plugin.cube-output.invalid-property" => {
            "cubemain.txt 第 {line} 行 recipe `{recipe}` 的 `{column}` property `{value}` 無效。"
        }
        "plugin.cube-output.empty-base-hover" => "**Base：**為空（無效的 cube output base）",
        "plugin.cube-output.invalid-hover" => {
            "**無效的魔盒輸出 base**\n\n無法解析 `{base}`。被忽略的字尾：`{ignoredSuffix}`。"
        }
        "plugin.cube-output.hover" => {
            "**魔盒輸出**\n基底：`{base}` ({kind})\n輸入：`{input}`\n修飾符：{modifiers}\n忽略的字尾：`{ignoredSuffix}`"
        }
        "plugin.enum.hover" => {
            "**列舉值**\n\n值：`{value}`\n名稱：{name}\n引數：{parameters}\n\n{description}\n\n額外欄位：{extraFields}"
        }
        "plugin.item-code.unresolved" => "未知的物品程式碼 `{value}`。請檢查程式碼和字母大小寫。",
        "plugin.item-code.unresolved-packed-policy" => {
            "未找到匹配的物品。此欄位可保留未解析的文字，請確認值 `{value}` 是否符合預期。"
        }
        "plugin.item-code.unresolved-policy" => {
            "物品程式碼 `{value}` 不在 weapons、armor 或 misc 中。請確認該程式碼是否符合預期。"
        }
        "plugin.item-code.hover" => "**物品程式碼**\n`{code}`\n{name}",
        "plugin.item-name.hover" => "{name}\n\n**程式碼：**{code}",
        "plugin.property.unknown-marker" => {
            "值 `{value}` 以 `*` 開頭，但不是已知的 property code。僅在它是有意的 marker 時保留。"
        }
        "plugin.property.unknown-code" => {
            "未知的 property code `{value}`。請使用 properties.txt 或 propertygroups.txt 中的程式碼。"
        }
        "plugin.property.unknown-hover" => {
            "**未知的 property code**\n\n在 properties.txt 或 propertygroups.txt 中找不到 `{value}`。"
        }
        "plugin.property.hover" => "**Property**\n`{value}` ({sourceFile}.txt code)",
        "plugin.tc-item.after-first-gap" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 值 `{value}` 位於第一個空物品欄位之後，遊戲會忽略它。"
        }
        "plugin.tc-prob.after-first-gap" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 機率位於第一個空物品欄位之後，遊戲會忽略它。"
        }
        "plugin.tc-prob.orphaned" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 機率 `{value}` 沒有對應的物品，因而被忽略。"
        }
        "plugin.tc-item.forward-reference" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 值 `{value}` 引用了尚未定義的 TC。"
        }
        "plugin.tc-item.unresolved-base" => {
            "找不到 Treasure Class `{treasureClass}` 的 `{column}` 物品基底 `{value}`。"
        }
        "plugin.tc-prob.blank-omission" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 為空，因此該項目會被省略。"
        }
        "plugin.tc-prob.noncanonical" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 值 `{value}` 不是整數，該項目可能被省略。"
        }
        "plugin.tc-prob.nonpositive-omission" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 值 `{value}` 小於等於 0，因此該項目會被省略。"
        }
        "plugin.tc-item.modifier-range" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 修飾符 `{value}` 超出 0..65535，遊戲會進行轉換。"
        }
        "plugin.tc-item.ignored-suffix" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 會忽略 `{value}` 後的字尾。"
        }
        "plugin.tc-item.field-width" => {
            "Treasure Class `{treasureClass}` 的 `{column}` 值 `{value}` 的 UTF-8 長度至少為 64 位元組。"
        }
        "plugin.treasure-class.hover-after-gap" => {
            "**Treasure Class 項目被忽略**\n\n第一個空物品欄位是 Item{firstEmptySlot}，因此 `{base}` 不會被使用。"
        }
        "plugin.treasure-class.hover" => {
            "**Treasure Class 項目**\n`{base}`\n{name}\n欄位：{slot}\n抽取次數：{picks}\n機率：{probability}\n修飾符：{modifiers}\n忽略的字尾：`{ignoredSuffix}`"
        }
        _ => return None,
    })
}

fn localized_plugin_label(locale: Locale, english_label: &str) -> &'static str {
    match (locale, english_label) {
        (Locale::EnUs, "calculation formula") => "calculation formula",
        (Locale::EnUs, "cube input") => "cube input",
        (Locale::EnUs, "cube output") => "cube output",
        (Locale::EnUs, "treasure class") => "treasure class",
        (Locale::EnUs, "property code") => "property code",
        (Locale::EnUs, "item reference") => "item reference",
        (Locale::EnUs, "enumeration") => "enumeration",
        (Locale::EnUs, "details") => "details",
        (Locale::EnUs, "invalid reference") => "invalid reference",
        (Locale::EnUs, "runtime behavior") => "runtime behavior",
        (Locale::EnUs, _) => "validation result",
        (Locale::KoKr, "calculation formula") => "계산식",
        (Locale::KoKr, "cube input") => "큐브 입력",
        (Locale::KoKr, "cube output") => "큐브 출력",
        (Locale::KoKr, "treasure class") => "Treasure Class",
        (Locale::KoKr, "property code") => "property 코드",
        (Locale::KoKr, "item reference") => "아이템 참조",
        (Locale::KoKr, "enumeration") => "열거 값",
        (Locale::KoKr, "details") => "세부 정보",
        (Locale::KoKr, "invalid reference") => "잘못된 참조",
        (Locale::KoKr, "runtime behavior") => "게임 실행 동작",
        (Locale::KoKr, _) => "검증 결과",
        (Locale::ZhCn, "calculation formula") => "计算公式",
        (Locale::ZhCn, "cube input") => "魔盒输入",
        (Locale::ZhCn, "cube output") => "魔盒输出",
        (Locale::ZhCn, "treasure class") => "Treasure Class",
        (Locale::ZhCn, "property code") => "property 代码",
        (Locale::ZhCn, "item reference") => "物品引用",
        (Locale::ZhCn, "enumeration") => "枚举值",
        (Locale::ZhCn, "details") => "详细信息",
        (Locale::ZhCn, "invalid reference") => "无效引用",
        (Locale::ZhCn, "runtime behavior") => "游戏运行行为",
        (Locale::ZhCn, _) => "验证结果",
        (Locale::ZhTw, "calculation formula") => "計算公式",
        (Locale::ZhTw, "cube input") => "魔方輸入",
        (Locale::ZhTw, "cube output") => "魔方輸出",
        (Locale::ZhTw, "treasure class") => "Treasure Class",
        (Locale::ZhTw, "property code") => "property 代碼",
        (Locale::ZhTw, "item reference") => "物品參照",
        (Locale::ZhTw, "enumeration") => "列舉值",
        (Locale::ZhTw, "details") => "詳細資料",
        (Locale::ZhTw, "invalid reference") => "無效參照",
        (Locale::ZhTw, "runtime behavior") => "遊戲執行行為",
        (Locale::ZhTw, _) => "驗證結果",
        (Locale::DeDe, "calculation formula") => "Berechnungsformel",
        (Locale::DeDe, "cube input") => "Würfeleingabe",
        (Locale::DeDe, "cube output") => "Würfelausgabe",
        (Locale::DeDe, "treasure class") => "Treasure Class",
        (Locale::DeDe, "property code") => "Property-Code",
        (Locale::DeDe, "item reference") => "Gegenstandsreferenz",
        (Locale::DeDe, "enumeration") => "Aufzählungswert",
        (Locale::DeDe, "details") => "Details",
        (Locale::DeDe, "invalid reference") => "Ungültige Referenz",
        (Locale::DeDe, "runtime behavior") => "Laufzeitverhalten",
        (Locale::DeDe, _) => "Prüfergebnis",
        (Locale::EsEs | Locale::EsMx, "calculation formula") => "Fórmula de cálculo",
        (Locale::EsEs | Locale::EsMx, "cube input") => "Entrada de cubo",
        (Locale::EsEs | Locale::EsMx, "cube output") => "Salida de cubo",
        (Locale::EsEs | Locale::EsMx, "treasure class") => "Treasure Class",
        (Locale::EsEs | Locale::EsMx, "property code") => "Código property",
        (Locale::EsEs | Locale::EsMx, "item reference") => "Referencia de objeto",
        (Locale::EsEs | Locale::EsMx, "enumeration") => "Valor de enumeración",
        (Locale::EsEs | Locale::EsMx, "details") => "Detalles",
        (Locale::EsEs | Locale::EsMx, "invalid reference") => "Referencia no válida",
        (Locale::EsEs | Locale::EsMx, "runtime behavior") => "Comportamiento del juego",
        (Locale::EsEs | Locale::EsMx, _) => "Resultado de validación",
        (Locale::FrFr, "calculation formula") => "Formule de calcul",
        (Locale::FrFr, "cube input") => "Entrée du cube",
        (Locale::FrFr, "cube output") => "Sortie du cube",
        (Locale::FrFr, "treasure class") => "Treasure Class",
        (Locale::FrFr, "property code") => "Code property",
        (Locale::FrFr, "item reference") => "Référence d’objet",
        (Locale::FrFr, "enumeration") => "Valeur d’énumération",
        (Locale::FrFr, "details") => "Détails",
        (Locale::FrFr, "invalid reference") => "Référence non valide",
        (Locale::FrFr, "runtime behavior") => "Comportement du jeu",
        (Locale::FrFr, _) => "Résultat de validation",
        (Locale::ItIt, "calculation formula") => "Formula di calcolo",
        (Locale::ItIt, "cube input") => "Input del cubo",
        (Locale::ItIt, "cube output") => "Output del cubo",
        (Locale::ItIt, "treasure class") => "Treasure Class",
        (Locale::ItIt, "property code") => "Codice property",
        (Locale::ItIt, "item reference") => "Riferimento oggetto",
        (Locale::ItIt, "enumeration") => "Valore di enumerazione",
        (Locale::ItIt, "details") => "Dettagli",
        (Locale::ItIt, "invalid reference") => "Riferimento non valido",
        (Locale::ItIt, "runtime behavior") => "Comportamento del gioco",
        (Locale::ItIt, _) => "Risultato della convalida",
        (Locale::PlPl, "calculation formula") => "Formuła obliczeń",
        (Locale::PlPl, "cube input") => "Wejście kostki",
        (Locale::PlPl, "cube output") => "Wynik kostki",
        (Locale::PlPl, "treasure class") => "Treasure Class",
        (Locale::PlPl, "property code") => "Kod property",
        (Locale::PlPl, "item reference") => "Odwołanie do przedmiotu",
        (Locale::PlPl, "enumeration") => "Wartość wyliczenia",
        (Locale::PlPl, "details") => "Szczegóły",
        (Locale::PlPl, "invalid reference") => "Nieprawidłowe odwołanie",
        (Locale::PlPl, "runtime behavior") => "Działanie gry",
        (Locale::PlPl, _) => "Wynik sprawdzenia",
        (Locale::JaJp, "calculation formula") => "計算式",
        (Locale::JaJp, "cube input") => "キューブ入力",
        (Locale::JaJp, "cube output") => "キューブ出力",
        (Locale::JaJp, "treasure class") => "Treasure Class",
        (Locale::JaJp, "property code") => "property コード",
        (Locale::JaJp, "item reference") => "アイテム参照",
        (Locale::JaJp, "enumeration") => "列挙値",
        (Locale::JaJp, "details") => "詳細",
        (Locale::JaJp, "invalid reference") => "無効な参照",
        (Locale::JaJp, "runtime behavior") => "ゲーム実行時の動作",
        (Locale::JaJp, _) => "検証結果",
        (Locale::PtBr, "calculation formula") => "Fórmula de cálculo",
        (Locale::PtBr, "cube input") => "Entrada do cubo",
        (Locale::PtBr, "cube output") => "Saída do cubo",
        (Locale::PtBr, "treasure class") => "Treasure Class",
        (Locale::PtBr, "property code") => "Código property",
        (Locale::PtBr, "item reference") => "Referência de item",
        (Locale::PtBr, "enumeration") => "Valor de enumeração",
        (Locale::PtBr, "details") => "Detalhes",
        (Locale::PtBr, "invalid reference") => "Referência inválida",
        (Locale::PtBr, "runtime behavior") => "Comportamento do jogo",
        (Locale::PtBr, _) => "Resultado da validação",
        (Locale::RuRu, "calculation formula") => "Формула расчёта",
        (Locale::RuRu, "cube input") => "Ввод куба",
        (Locale::RuRu, "cube output") => "Вывод куба",
        (Locale::RuRu, "treasure class") => "Treasure Class",
        (Locale::RuRu, "property code") => "Код property",
        (Locale::RuRu, "item reference") => "Ссылка на предмет",
        (Locale::RuRu, "enumeration") => "Значение перечисления",
        (Locale::RuRu, "details") => "Сведения",
        (Locale::RuRu, "invalid reference") => "Недопустимая ссылка",
        (Locale::RuRu, "runtime behavior") => "Поведение игры",
        (Locale::RuRu, _) => "Результат проверки",
    }
}

pub fn localized_diagnostic(
    locale: Locale,
    key: &'static str,
    args: Map<String, Value>,
    mut diagnostic: Diagnostic,
) -> Diagnostic {
    diagnostic.message = localize(locale, key, &args);
    diagnostic.data = Some(merge_data(
        diagnostic.data.take(),
        key,
        Value::Object(args),
        is_bundled_plugin_key(key),
    ));
    diagnostic
}

pub fn localized_plugin_diagnostic(
    locale: Locale,
    key: &str,
    args: Map<String, Value>,
    fallback: Option<String>,
    mut diagnostic: Diagnostic,
) -> Diagnostic {
    // Bundled plugins use stable keys.  Custom plugins can keep an explicit
    // message for enUS compatibility; non-English sessions never expose that
    // completed English sentence as their translated product UI.
    let message = if locale == Locale::EnUs {
        fallback
            .clone()
            .unwrap_or_else(|| localize(locale, key, &args))
    } else if is_bundled_plugin_key(key) {
        localize(locale, key, &args)
    } else {
        // An unknown key belongs to third-party content. Its author owns the
        // wording, so preserve the supplied compatibility string rather than
        // pretending it was translated by vector-lsp.
        fallback.unwrap_or_else(|| localize(locale, key, &args))
    };
    let message = if locale == Locale::EnUs && key.starts_with("plugin.calc.") {
        strip_english_plugin_guidance(key, message, diagnostic.data.as_ref())
    } else {
        message
    };
    diagnostic.message = compact_plugin_hover_spacing(key, message)
        .trim()
        .to_string();
    let mut data = merge_data(
        diagnostic.data.take(),
        key,
        Value::Object(args),
        locale != Locale::EnUs && is_bundled_plugin_key(key),
    );
    if let Some((heading, guidance)) = localized_plugin_guidance(locale, key, &data)
        && let Value::Object(fields) = &mut data
    {
        fields.insert(
            "localizedGuidanceHeading".to_string(),
            Value::String(heading),
        );
        fields.insert("localizedGuidance".to_string(), Value::String(guidance));
    }
    diagnostic.data = Some(data);
    diagnostic
}

fn strip_english_plugin_guidance(key: &str, message: String, data: Option<&Value>) -> String {
    let mut result = message.trim().to_string();
    let hint = data
        .and_then(Value::as_object)
        .and_then(|fields| fields.get("hint"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(hint) = hint
        && result.ends_with(hint)
    {
        result.truncate(result.len() - hint.len());
        result = result.trim_end().to_string();
    }
    if key == "plugin.calc.expected-rparen.eof"
        && let Some(prefix) = result.strip_suffix(" Add the missing ')'.")
    {
        result = prefix.trim_end().to_string();
    }
    result
}

fn localized_plugin_guidance(locale: Locale, key: &str, data: &Value) -> Option<(String, String)> {
    let action = match key {
        "plugin.calc.skill-param-alias" => "alias",
        "plugin.calc.unterminated-string" => "close-string",
        "plugin.calc.unexpected-character" => "replace-character",
        "plugin.calc.unexpected-eof" => "complete-expression",
        "plugin.calc.unexpected-token" => "replace-token",
        "plugin.calc.wrong-arity" => "argument-count",
        "plugin.calc.expected-quoted-argument" => "quote-argument",
        "plugin.calc.expected-dot-identifier" => "dot-identifier",
        "plugin.calc.skilldesc-decimal-prefix" => "integer-expression",
        "plugin.calc.decimal-policy" => "integer-policy",
        "plugin.calc.prefix-stop" => "rewrite-expression",
        key if key.starts_with("plugin.calc.expected-") && key.ends_with(".eof") => "insert-at-end",
        key if key.starts_with("plugin.calc.expected-") => "insert-before",
        _ => return None,
    };
    let (heading, template) = localized_guidance_template(locale, action)?;
    let fields = data.as_object()?;
    Some((heading.to_string(), interpolate(template, fields)))
}

fn localized_guidance_template(
    locale: Locale,
    action: &str,
) -> Option<(&'static str, &'static str)> {
    let (heading, template) = match locale {
        Locale::EnUs => (
            "What to do",
            match action {
                "alias" => "Use `{suggestion}` to reference `{parameter}`.",
                "close-string" => "Close the string with a single quote.",
                "replace-character" => "Remove or replace `{actual}` at the marked position.",
                "complete-expression" => "Complete the expression before the end of the formula.",
                "replace-token" => "Remove or replace the token `{actual}`.",
                "argument-count" => "Use exactly {expected} arguments.",
                "quote-argument" => "Wrap the first argument in single quotes.",
                "dot-identifier" => "Add an identifier after `.`.",
                "insert-at-end" => "Insert `{insertText}` at the end of the expression.",
                "insert-before" => "Insert `{insertText}` before `{actual}`.",
                "integer-expression" => "Use an integer expression that matches your intent.",
                "integer-policy" => {
                    "Use an integer expression unless this field is known to support decimals."
                }
                "rewrite-expression" => {
                    "Rewrite the expression if the ignored part is intended to run."
                }
                _ => return None,
            },
        ),
        Locale::PlPl => (
            "Co zrobić",
            match action {
                "alias" => "Aby odwołać się do `{parameter}`, użyj `{suggestion}`.",
                "close-string" => "Zamknij ciąg pojedynczym cudzysłowem.",
                "replace-character" => "Usuń lub zastąp znak `{actual}` we wskazanym miejscu.",
                "complete-expression" => "Uzupełnij wyrażenie przed końcem formuły.",
                "replace-token" => "Usuń lub zastąp token `{actual}`.",
                "argument-count" => "Użyj dokładnie {expected} argumentów.",
                "quote-argument" => "Ujmij pierwszy argument w pojedyncze cudzysłowy.",
                "dot-identifier" => "Dodaj identyfikator po `.`.",
                "insert-at-end" => "Dodaj `{insertText}` na końcu wyrażenia.",
                "insert-before" => "Dodaj `{insertText}` przed `{actual}`.",
                "integer-expression" => "Użyj wyrażenia całkowitego zgodnego z zamiarem.",
                "integer-policy" => {
                    "Użyj wyrażenia całkowitego, chyba że to pole obsługuje wartości dziesiętne."
                }
                "rewrite-expression" => {
                    "Przepisz wyrażenie, jeśli ignorowana część ma zostać wykonana."
                }
                _ => return None,
            },
        ),
        Locale::ItIt => (
            "Come risolvere",
            match action {
                "alias" => "Usa `{suggestion}` per fare riferimento a `{parameter}`.",
                "close-string" => "Chiudi la stringa con un apice singolo.",
                "replace-character" => "Rimuovi o sostituisci `{actual}` nella posizione indicata.",
                "complete-expression" => "Completa l'espressione prima della fine della formula.",
                "replace-token" => "Rimuovi o sostituisci il token `{actual}`.",
                "argument-count" => "Usa esattamente {expected} argomenti.",
                "quote-argument" => "Racchiudi il primo argomento tra apici singoli.",
                "dot-identifier" => "Aggiungi un identificatore dopo `.`.",
                "insert-at-end" => "Aggiungi `{insertText}` alla fine dell'espressione.",
                "insert-before" => "Aggiungi `{insertText}` prima di `{actual}`.",
                "integer-expression" => {
                    "Usa un'espressione intera coerente con il risultato desiderato."
                }
                "integer-policy" => {
                    "Usa un'espressione intera, salvo che il campo supporti i decimali."
                }
                "rewrite-expression" => {
                    "Riscrivi l'espressione se anche la parte ignorata deve essere eseguita."
                }
                _ => return None,
            },
        ),
        Locale::FrFr => (
            "Que faire",
            match action {
                "alias" => "Utilisez `{suggestion}` pour référencer `{parameter}`.",
                "close-string" => "Fermez la chaîne avec une apostrophe.",
                "replace-character" => "Supprimez ou remplacez `{actual}` à l'emplacement indiqué.",
                "complete-expression" => "Complétez l'expression avant la fin de la formule.",
                "replace-token" => "Supprimez ou remplacez le jeton `{actual}`.",
                "argument-count" => "Utilisez exactement {expected} arguments.",
                "quote-argument" => "Entourez le premier argument d'apostrophes.",
                "dot-identifier" => "Ajoutez un identifiant après `.`.",
                "insert-at-end" => "Ajoutez `{insertText}` à la fin de l'expression.",
                "insert-before" => "Ajoutez `{insertText}` avant `{actual}`.",
                "integer-expression" => {
                    "Utilisez une expression entière correspondant au résultat voulu."
                }
                "integer-policy" => {
                    "Utilisez une expression entière, sauf si ce champ accepte les décimales."
                }
                "rewrite-expression" => {
                    "Réécrivez l'expression si la partie ignorée doit être exécutée."
                }
                _ => return None,
            },
        ),
        Locale::EsEs | Locale::EsMx => (
            "Qué hacer",
            match action {
                "alias" => "Usa `{suggestion}` para hacer referencia a `{parameter}`.",
                "close-string" => "Cierra la cadena con una comilla simple.",
                "replace-character" => "Elimina o sustituye `{actual}` en la posición indicada.",
                "complete-expression" => "Completa la expresión antes del final de la fórmula.",
                "replace-token" => "Elimina o sustituye el token `{actual}`.",
                "argument-count" => "Usa exactamente {expected} argumentos.",
                "quote-argument" => "Escribe el primer argumento entre comillas simples.",
                "dot-identifier" => "Añade un identificador después de `.`.",
                "insert-at-end" => "Añade `{insertText}` al final de la expresión.",
                "insert-before" => "Añade `{insertText}` antes de `{actual}`.",
                "integer-expression" => {
                    "Usa una expresión entera que coincida con el resultado deseado."
                }
                "integer-policy" => {
                    "Usa una expresión entera, salvo que este campo admita decimales."
                }
                "rewrite-expression" => {
                    "Reescribe la expresión si la parte ignorada debe ejecutarse."
                }
                _ => return None,
            },
        ),
        Locale::PtBr => (
            "O que fazer",
            match action {
                "alias" => "Use `{suggestion}` para referenciar `{parameter}`.",
                "close-string" => "Feche a cadeia com uma aspa simples.",
                "replace-character" => "Remova ou substitua `{actual}` na posição indicada.",
                "complete-expression" => "Complete a expressão antes do fim da fórmula.",
                "replace-token" => "Remova ou substitua o token `{actual}`.",
                "argument-count" => "Use exatamente {expected} argumentos.",
                "quote-argument" => "Coloque o primeiro argumento entre aspas simples.",
                "dot-identifier" => "Adicione um identificador depois de `.`.",
                "insert-at-end" => "Adicione `{insertText}` ao final da expressão.",
                "insert-before" => "Adicione `{insertText}` antes de `{actual}`.",
                "integer-expression" => {
                    "Use uma expressão inteira de acordo com o resultado desejado."
                }
                "integer-policy" => {
                    "Use uma expressão inteira, a menos que este campo aceite decimais."
                }
                "rewrite-expression" => {
                    "Reescreva a expressão se a parte ignorada precisar ser executada."
                }
                _ => return None,
            },
        ),
        Locale::DeDe => (
            "Vorgehensweise",
            match action {
                "alias" => "Verwenden Sie `{suggestion}`, um auf `{parameter}` zu verweisen.",
                "close-string" => {
                    "Schließen Sie die Zeichenfolge mit einem einfachen Anführungszeichen."
                }
                "replace-character" => {
                    "Entfernen oder ersetzen Sie `{actual}` an der markierten Stelle."
                }
                "complete-expression" => {
                    "Vervollständigen Sie den Ausdruck vor dem Ende der Formel."
                }
                "replace-token" => "Entfernen oder ersetzen Sie das Token `{actual}`.",
                "argument-count" => "Verwenden Sie genau {expected} Argumente.",
                "quote-argument" => "Setzen Sie das erste Argument in einfache Anführungszeichen.",
                "dot-identifier" => "Fügen Sie nach `.` einen Bezeichner ein.",
                "insert-at-end" => "Fügen Sie `{insertText}` am Ende des Ausdrucks ein.",
                "insert-before" => "Fügen Sie `{insertText}` vor `{actual}` ein.",
                "integer-expression" => {
                    "Verwenden Sie einen ganzzahligen Ausdruck für das gewünschte Ergebnis."
                }
                "integer-policy" => {
                    "Verwenden Sie einen ganzzahligen Ausdruck, sofern das Feld keine Dezimalwerte unterstützt."
                }
                "rewrite-expression" => {
                    "Schreiben Sie den Ausdruck neu, wenn der ignorierte Teil ausgeführt werden soll."
                }
                _ => return None,
            },
        ),
        Locale::KoKr => (
            "수정 방법",
            match action {
                "alias" => "매개변수 `{parameter}` 참조에는 `{suggestion}` 식별자를 사용하세요.",
                "close-string" => "문자열 끝에 작은따옴표를 추가하세요.",
                "replace-character" => {
                    "표시된 위치의 `{actual}` 문자를 제거하거나 올바른 문자로 바꾸세요."
                }
                "complete-expression" => "계산식 끝에 누락된 내용을 추가하여 식을 완성하세요.",
                "replace-token" => "`{actual}` 토큰을 제거하거나 올바른 토큰으로 바꾸세요.",
                "argument-count" => "인수를 정확히 {expected}개 사용하세요.",
                "quote-argument" => "첫 번째 인수를 작은따옴표로 감싸세요.",
                "dot-identifier" => "`.` 뒤에 식별자를 추가하세요.",
                "insert-at-end" => "계산식 끝에 `{insertText}` 기호를 추가하세요.",
                "insert-before" => "`{actual}` 앞에 `{insertText}` 기호를 추가하세요.",
                "integer-expression" => "의도에 맞는 정수 계산식으로 바꾸세요.",
                "integer-policy" => {
                    "이 필드가 소수를 지원한다고 확인된 경우가 아니면 정수 계산식을 사용하세요."
                }
                "rewrite-expression" => "무시된 부분도 실행해야 한다면 계산식을 다시 작성하세요.",
                _ => return None,
            },
        ),
        Locale::JaJp => (
            "対処方法",
            match action {
                "alias" => "`{parameter}` を参照するには `{suggestion}` を使用してください。",
                "close-string" => "文字列の末尾に単一引用符を追加してください。",
                "replace-character" => {
                    "指定位置の `{actual}` を削除するか、正しい文字に置き換えてください。"
                }
                "complete-expression" => {
                    "式の末尾に不足している内容を追加して、式を完成させてください。"
                }
                "replace-token" => {
                    "トークン `{actual}` を削除するか、正しいトークンに置き換えてください。"
                }
                "argument-count" => "引数を正確に {expected} 個指定してください。",
                "quote-argument" => "最初の引数を単一引用符で囲んでください。",
                "dot-identifier" => "`.` の後に識別子を追加してください。",
                "insert-at-end" => "式の末尾に `{insertText}` を追加してください。",
                "insert-before" => "`{actual}` の前に `{insertText}` を追加してください。",
                "integer-expression" => "意図した結果に合う整数式を使用してください。",
                "integer-policy" => {
                    "このフィールドが小数をサポートする場合を除き、整数式を使用してください。"
                }
                "rewrite-expression" => {
                    "無視された部分も実行する必要がある場合は、式を書き直してください。"
                }
                _ => return None,
            },
        ),
        Locale::RuRu => (
            "Что делать",
            match action {
                "alias" => "Для ссылки на `{parameter}` используйте `{suggestion}`.",
                "close-string" => "Добавьте одинарную кавычку в конец строки.",
                "replace-character" => "Удалите или замените `{actual}` в отмеченной позиции.",
                "complete-expression" => "Добавьте недостающую часть и завершите выражение.",
                "replace-token" => "Удалите или замените токен `{actual}`.",
                "argument-count" => "Используйте ровно {expected} аргументов.",
                "quote-argument" => "Заключите первый аргумент в одинарные кавычки.",
                "dot-identifier" => "Добавьте идентификатор после `.`.",
                "insert-at-end" => "Добавьте `{insertText}` в конец выражения.",
                "insert-before" => "Добавьте `{insertText}` перед `{actual}`.",
                "integer-expression" => {
                    "Используйте целочисленное выражение для нужного результата."
                }
                "integer-policy" => {
                    "Используйте целочисленное выражение, если поле не поддерживает дробные значения."
                }
                "rewrite-expression" => {
                    "Перепишите выражение, если игнорируемая часть должна выполняться."
                }
                _ => return None,
            },
        ),
        Locale::ZhCn => (
            "修正方法",
            match action {
                "alias" => "请使用 `{suggestion}` 引用 `{parameter}`。",
                "close-string" => "请在字符串末尾补上单引号。",
                "replace-character" => "请删除或替换标记位置的 `{actual}`。",
                "complete-expression" => "请补全公式末尾缺失的内容。",
                "replace-token" => "请删除或替换 token `{actual}`。",
                "argument-count" => "请准确使用 {expected} 个参数。",
                "quote-argument" => "请用单引号括起第一个参数。",
                "dot-identifier" => "请在 `.` 后添加标识符。",
                "insert-at-end" => "请在公式末尾添加 `{insertText}`。",
                "insert-before" => "请在 `{actual}` 前添加 `{insertText}`。",
                "integer-expression" => "请使用符合预期结果的整数公式。",
                "integer-policy" => "除非此字段明确支持小数，否则请使用整数公式。",
                "rewrite-expression" => "如果被忽略的部分也需要执行，请重写公式。",
                _ => return None,
            },
        ),
        Locale::ZhTw => (
            "修正方法",
            match action {
                "alias" => "請使用 `{suggestion}` 參照 `{parameter}`。",
                "close-string" => "請在字串末尾補上單引號。",
                "replace-character" => "請刪除或取代標記位置的 `{actual}`。",
                "complete-expression" => "請補全公式末尾缺少的內容。",
                "replace-token" => "請刪除或取代 token `{actual}`。",
                "argument-count" => "請準確使用 {expected} 個引數。",
                "quote-argument" => "請用單引號括住第一個引數。",
                "dot-identifier" => "請在 `.` 後加入識別符號。",
                "insert-at-end" => "請在公式末尾加入 `{insertText}`。",
                "insert-before" => "請在 `{actual}` 前加入 `{insertText}`。",
                "integer-expression" => "請使用符合預期結果的整數公式。",
                "integer-policy" => "除非此欄位明確支援小數，否則請使用整數公式。",
                "rewrite-expression" => "如果被忽略的部分也需要執行，請重寫公式。",
                _ => return None,
            },
        ),
    };
    Some((heading, template))
}

fn merge_data(
    existing: Option<Value>,
    key: &str,
    message_args: Value,
    localized_message: bool,
) -> Value {
    let mut data = match existing {
        Some(Value::Object(data)) => data,
        Some(value) => {
            let mut data = Map::new();
            data.insert("pluginData".to_string(), value);
            data
        }
        None => Map::new(),
    };
    data.insert("messageKey".to_string(), Value::String(key.to_string()));
    data.insert("messageArgs".to_string(), message_args);
    data.insert(
        "localizedMessage".to_string(),
        Value::Bool(localized_message),
    );
    Value::Object(data)
}

fn interpolate(template: &str, values: &Map<String, Value>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start + 1..].find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &rest[start + 1..start + 1 + end];
        if let Some(value) = values.get(name) {
            out.push_str(&display_value(value));
        } else {
            out.push('{');
            out.push_str(name);
            out.push('}');
        }
        rest = &rest[start + 2 + end..];
    }
    out.push_str(rest);
    out
}

fn display_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => String::new(),
        value => value.to_string(),
    }
}

// The English catalog deliberately retains the server's historical messages
// for existing clients and command-line consumers.
fn english(key: &str) -> &'static str {
    match key {
        "diag.duplicate_unique" => {
            "Duplicate value '{value}' in unique key column '{column}' (first seen at line {line}, column {firstColumn})"
        }
        "diag.reference.unresolved" => "Reference value '{value}' not found in {file}.{column}.",
        "diag.reference.range" => "Unknown range code. Use one of: none, h2h, rng, both, loc.",
        "diag.reference.monpet_consumestat" => {
            "Unknown stat name '{value}'. This Consume bonus is not applied; other Consume slots still work. Use the exact Stat name from itemstatcost.txt."
        }
        "diag.reference.properties_stat" => {
            "Unknown stat name '{value}'. Use the exact Stat name from itemstatcost.txt."
        }
        "diag.reference.properties_stat_noeffect" => {
            "Unknown stat name '{value}'. This property has no effect. Use the exact Stat name from itemstatcost.txt."
        }
        "diag.fixed4_unknown" => {
            "Unknown code '{value}'. The game reads this code as '{effective}'. Check the four-character code and letter case."
        }
        "diag.integer.backtick" => {
            "'`' is not written as a normal integer. The game converts it to 48. Replace it with the number you actually want."
        }
        "diag.integer.invalid" => {
            "'{value}' is not a standard integer for '{column}'. Use a plain whole number; the game may read a different value."
        }
        "diag.float.invalid" => "'{value}' is not a valid number for column '{column}'",
        "diag.boolean.invalid" => {
            "'{value}' is not a standard boolean value for '{column}'. Use 0 for false or 1 for true."
        }
        "diag.boolean.type29_invalid" => {
            "'{value}' is not a number for '{column}'. Use 0 for false or any nonzero integer for true."
        }
        "diag.hit.out_of_range" => {
            "'{value}' is outside the HitSummon mode range 0 through 15. The game uses 1 (NU). Enter a value from 0 through 15."
        }
        "diag.hit.noncanonical" => {
            "'{value}' does not directly name a HitSummon mode from 0 through 15. The game reads it as {effective} ({mode}). Replace it with the mode number you actually want."
        }
        "diag.hit.nu_literal" => {
            "'NU' is not a numeric mode ID here. The game replaces it with 1 (NU). Use 1 for neutral mode."
        }
        "diag.hit.non_numeric_outside" => {
            "'{value}' is not a numeric mode ID here. The game replaces it with 1 (NU). Enter a value from 0 through 15."
        }
        "diag.hit.non_numeric" => {
            "'{value}' is not a numeric mode ID here. The game reads it as {effective} ({mode}). Replace it with the mode number you actually want from 0 through 15."
        }
        "json.syntax_invalid" => "Invalid localization JSON: {error}",
        "json.id_out_of_range" => {
            "{file}.json: id {id} is outside the runtime string ID range 0..65535; the game stores this namespace as uint16"
        }
        "json.missing_id" => "{file}.json: missing id on entry {entry}",
        "json.duplicate_id_same" => {
            "{file}.json: duplicate id {id} found on entries {first} and {entry}; entry {entry} is ignored, so neither its ID nor Key is registered"
        }
        "json.duplicate_id_cross" => {
            "{file}.json: duplicate id {id} found in {otherFile}.json; entry {entry} is ignored, so neither its ID nor Key is registered"
        }
        "json.duplicate_key_same" => {
            "{file}.json: duplicate Key '{keyValue}' found on entries {first} and {entry}; entry {entry} is ignored, so neither its ID nor Key is registered"
        }
        "json.duplicate_key_cross" => {
            "{file}.json: duplicate Key '{keyValue}' found in {otherFile}.json; entry {entry} is ignored, so neither its ID nor Key is registered"
        }
        "json.missing_fields" => {
            "{file}.json: entry {entry} ({keyValue}) is missing fields: {fields}"
        }
        "json.unused_key" => {
            "{file}.json: Key '{keyValue}' (id: {id}) is not referenced as an entry in any .txt or layout .json file"
        }
        "hover.unknown_monpet_stat" => {
            "**Unknown stat name**\n\n`{value}` is not a known stat. This Consume bonus is not applied; other Consume slots still work. Use the exact Stat name from `itemstatcost.txt`."
        }
        "hover.unknown_property_stat" => {
            "**Unknown stat name**\n\n`{value}` is not a known stat. Use the exact Stat name from `itemstatcost.txt`."
        }
        "hover.unknown_property_stat_noeffect" => {
            "**Unknown stat name**\n\n`{value}` is not a known stat. This property has no effect. Use the exact Stat name from `itemstatcost.txt`."
        }
        "hover.range_valid" => {
            "**Range code**\n\n`{value}` is valid. The game uses range code `{stored}`."
        }
        "hover.reference_resolved" => {
            "**Reference**\n\n`{value}` → `{stored}` in `{file}.{column}`"
        }
        "hover.boolean_value" => {
            "**Boolean value**\n\n`{value}` → **{result}** (0 means false; any nonzero number means true)\n\n{version}"
        }
        "hover.game_version" => "Game version: {version}",
        "hover.game_version_unselected" => "Game version: not selected",
        "hover.hit_summon" => {
            "**HitSummon monster mode**\n\nThe second server parameter uses a monster mode number from 0 through 15.\n\n0=DT, 1=NU, 2=WL, 3=GH, 4=A1, 5=A2, 6=BL, 7=SC, 8=S1, 9=S2, 10=S3, 11=S4, 12=DD, 13=KB, 14=xx, 15=RN.\n\nValues outside 0 through 15 use 1=NU.\n\n{current}"
        }
        "hover.hit_current_fallback" => "Current value: `{value}` -> 1 (NU)",
        "hover.hit_current" => "Current value: `{value}` -> {effective} ({mode})",
        "hover.header" => "**{column}**\n\n{description}",
        "log.json_stopped" => "Localization JSON diagnostics stopped: {error}",
        "log.json_warning" => "{warning}",
        "log.json_path_uri" => "Could not convert localization JSON path to a URI: {path}",
        "log.json_parse_failed" => "Could not parse localization JSON {path}: {error}",
        "log.ignored_change" => "Ignored didChange for unopened document {uri}",
        "log.schema_loaded" => "Schema loaded successfully.",
        "log.schema_selection_failed" => "Could not select the schema: {error}",
        "log.schema_load_failed" => "Could not load the schema: {error}",
        "log.workspace_scan_failed" => "Workspace scan failed: {error}",
        _ => "Plugin diagnostic: {values}",
    }
}

// These two most-used CJK catalogs are written explicitly.  The arguments are
// data, identifiers, paths, and game terms and therefore intentionally stay
// verbatim after interpolation.
fn korean(key: &str) -> &'static str {
    match key {
        "diag.duplicate_unique" => {
            "고유 키 열 '{column}'에서 중복된 값: '{value}'(처음 발견한 위치: {line}행 {firstColumn}열)."
        }
        "diag.reference.unresolved" => "{file}.{column}에서 찾을 수 없는 참조 값: '{value}'.",
        "diag.reference.range" => {
            "알 수 없는 range 코드입니다. none, h2h, rng, both, loc 중 하나를 사용하세요."
        }
        "diag.reference.monpet_consumestat" => {
            "itemstatcost.txt에 '{value}'라는 Stat 항목이 없습니다. 이 Consume 보너스는 적용되지 않지만 다른 Consume 슬롯은 계속 동작합니다. 정확히 일치하는 Stat 이름을 사용하세요."
        }
        "diag.reference.properties_stat" => {
            "itemstatcost.txt에 '{value}'라는 Stat 항목이 없습니다. 정확히 일치하는 Stat 이름을 사용하세요."
        }
        "diag.reference.properties_stat_noeffect" => {
            "itemstatcost.txt에 '{value}'라는 Stat 항목이 없습니다. 이 property에는 효과가 적용되지 않습니다. 정확히 일치하는 Stat 이름을 사용하세요."
        }
        "diag.fixed4_unknown" => {
            "입력한 코드: '{value}'. 게임은 이를 '{effective}'로 읽습니다. 네 글자 코드와 대소문자를 확인하세요."
        }
        "diag.integer.backtick" => {
            "문자 '`'는 일반 정수 표기가 아닙니다. 게임에서는 이를 48로 변환합니다. 원하는 숫자로 바꾸세요."
        }
        "diag.integer.invalid" => {
            "'{column}' 열의 입력값: '{value}'. 이 열에는 일반 정수만 사용하세요. 게임에서 다른 값으로 해석될 수 있습니다."
        }
        "diag.float.invalid" => "'{column}' 열의 입력값: '{value}'. 올바른 숫자가 아닙니다.",
        "diag.boolean.invalid" => {
            "'{column}' 열의 입력값: '{value}'. 표준 불리언 값은 false의 경우 0, true의 경우 1입니다."
        }
        "diag.boolean.type29_invalid" => {
            "'{column}' 열의 입력값: '{value}'. false에는 0, true에는 0이 아닌 정수를 사용하세요."
        }
        "diag.hit.out_of_range" => {
            "입력한 HitSummon 모드 값: '{value}'. 허용 범위는 0~15이며, 게임은 범위 밖의 값을 1(NU)로 처리합니다."
        }
        "diag.hit.noncanonical" => {
            "입력한 HitSummon 모드 값: '{value}'. 게임에서 읽는 값은 {effective}({mode})입니다. 원하는 모드 번호로 바꾸세요."
        }
        "diag.hit.nu_literal" => {
            "여기서 'NU'는 숫자 모드 ID가 아닙니다. 게임은 이를 1(NU)로 바꿉니다. 중립 모드에는 1을 사용하세요."
        }
        "diag.hit.non_numeric_outside" => {
            "입력한 값: '{value}'. 이 위치에는 숫자 모드 ID가 필요합니다. 게임은 이를 1(NU)로 처리합니다. 0~15 값을 입력하세요."
        }
        "diag.hit.non_numeric" => {
            "입력한 값: '{value}'. 이 위치에는 숫자 모드 ID가 필요합니다. 게임에서 읽는 값은 {effective}({mode})입니다. 원하는 0~15 모드 번호로 바꾸세요."
        }
        "json.syntax_invalid" => "로컬라이제이션 JSON이 올바르지 않습니다: {error}",
        "json.id_out_of_range" => {
            "{file}.json의 id 값: {id}. 런타임 문자열 ID 범위 0..65535 밖이며, 게임은 이 네임스페이스를 uint16으로 저장합니다."
        }
        "json.missing_id" => "{file}.json: {entry}번 항목에 id가 없습니다.",
        "json.duplicate_id_same" => {
            "{file}.json의 중복 id: {id}. {first}번 항목과 {entry}번 항목이 같은 id를 사용합니다. {entry}번 항목은 무시되므로 ID와 Key가 등록되지 않습니다."
        }
        "json.duplicate_id_cross" => {
            "{file}.json의 id 값이 {otherFile}.json과 충돌합니다. 충돌한 id: {id}. {entry}번 항목은 무시되므로 ID와 Key가 등록되지 않습니다."
        }
        "json.duplicate_key_same" => {
            "{file}.json의 중복 Key: '{keyValue}'. {first}번 항목과 {entry}번 항목이 같은 Key를 사용합니다. {entry}번 항목은 무시되므로 ID와 Key가 등록되지 않습니다."
        }
        "json.duplicate_key_cross" => {
            "{file}.json과 {otherFile}.json에서 충돌하는 Key: '{keyValue}'. {entry}번 항목은 무시되므로 ID와 Key가 등록되지 않습니다."
        }
        "json.missing_fields" => {
            "{file}.json: {entry}번 항목({keyValue})에 필드가 없습니다: {fields}"
        }
        "json.unused_key" => {
            "{file}.json: 참조되지 않는 Key '{keyValue}'(id: {id}). 어떤 .txt 또는 layout .json 파일의 항목에서도 사용하지 않습니다."
        }
        "hover.unknown_monpet_stat" => {
            "**알 수 없는 Stat 이름**\n\n`itemstatcost.txt`에 `{value}`라는 Stat 항목이 없습니다. 이 Consume 보너스는 적용되지 않지만 다른 Consume 슬롯은 계속 동작합니다. 정확히 일치하는 Stat 이름을 사용하세요."
        }
        "hover.unknown_property_stat" => {
            "**알 수 없는 Stat 이름**\n\n`itemstatcost.txt`에 `{value}`라는 Stat 항목이 없습니다. 정확히 일치하는 Stat 이름을 사용하세요."
        }
        "hover.unknown_property_stat_noeffect" => {
            "**알 수 없는 Stat 이름**\n\n`itemstatcost.txt`에 `{value}`라는 Stat 항목이 없습니다. 이 property에는 효과가 적용되지 않습니다. 정확히 일치하는 Stat 이름을 사용하세요."
        }
        "hover.range_valid" => {
            "**Range 코드**\n\n입력한 값: `{value}`. 게임에서 사용하는 range 코드: `{stored}`."
        }
        "hover.reference_resolved" => "**참조 정보**\n\n`{value}` → `{stored}` (`{file}.{column}`)",
        "hover.boolean_value" => {
            "**불리언 값**\n\n`{value}` → **{result}** (0은 false, 0이 아닌 수는 true)\n\n{version}"
        }
        "hover.game_version" => "게임 버전: {version}",
        "hover.game_version_unselected" => "게임 버전: 선택되지 않음",
        "hover.hit_summon" => {
            "**HitSummon 몬스터 모드**\n\n두 번째 서버 매개변수는 0~15 몬스터 모드 번호를 사용합니다.\n\n0=DT, 1=NU, 2=WL, 3=GH, 4=A1, 5=A2, 6=BL, 7=SC, 8=S1, 9=S2, 10=S3, 11=S4, 12=DD, 13=KB, 14=xx, 15=RN.\n\n0~15 밖의 값은 1=NU를 사용합니다.\n\n{current}"
        }
        "hover.hit_current_fallback" => "현재 값: `{value}` → 1 (NU)",
        "hover.hit_current" => "현재 값: `{value}` → {effective} ({mode})",
        "hover.header" => "**{column}**\n\n{description}",
        "log.json_stopped" => "로컬라이제이션 JSON 진단이 중지되었습니다: {error}",
        "log.json_warning" => "{warning}",
        "log.json_path_uri" => "현지화 JSON 경로를 URI로 변환할 수 없습니다: {path}",
        "log.json_parse_failed" => "현지화 JSON 해석에 실패했습니다. 경로: {path}. 오류: {error}",
        "log.ignored_change" => "열리지 않은 문서의 didChange를 무시했습니다: {uri}",
        "log.schema_loaded" => "스키마를 성공적으로 불러왔습니다.",
        "log.schema_selection_failed" => "스키마를 선택할 수 없습니다: {error}",
        "log.schema_load_failed" => "스키마를 불러올 수 없습니다: {error}",
        "log.workspace_scan_failed" => "작업 영역 검사에 실패했습니다: {error}",
        _ => "플러그인 진단: {values}",
    }
}

fn chinese_simplified(key: &str) -> &'static str {
    match key {
        "diag.duplicate_unique" => {
            "唯一键列“{column}”中有重复值“{value}”（首次出现于第 {line} 行、第 {firstColumn} 列）。"
        }
        "diag.reference.unresolved" => "在 {file}.{column} 中找不到引用值“{value}”。",
        "diag.reference.range" => "未知的 range 代码。请使用 none、h2h、rng、both 或 loc 之一。",
        "diag.reference.monpet_consumestat" => {
            "未知的 Stat 名称“{value}”。此 Consume 奖励不会生效，但其他 Consume 槽位仍会生效。请使用 itemstatcost.txt 中准确的 Stat 名称。"
        }
        "diag.reference.properties_stat" => {
            "未知的 Stat 名称“{value}”。请使用 itemstatcost.txt 中准确的 Stat 名称。"
        }
        "diag.reference.properties_stat_noeffect" => {
            "未知的 Stat 名称“{value}”。此 property 没有效果。请使用 itemstatcost.txt 中准确的 Stat 名称。"
        }
        "diag.fixed4_unknown" => {
            "未知代码“{value}”。游戏会将此代码读取为“{effective}”。请检查四字符代码及字母大小写。"
        }
        "diag.integer.backtick" => {
            "“`”不是普通整数写法。游戏会将其转换为 48。请替换为实际需要的数字。"
        }
        "diag.integer.invalid" => {
            "“{value}”不是“{column}”的标准整数。请使用普通整数；游戏可能读取为不同的值。"
        }
        "diag.float.invalid" => "“{value}”不是列“{column}”的有效数字。",
        "diag.boolean.invalid" => {
            "“{value}”不是“{column}”的标准布尔值。false 使用 0，true 使用 1。"
        }
        "diag.boolean.type29_invalid" => {
            "“{value}”不是“{column}”的数字。false 使用 0，true 使用任意非零整数。"
        }
        "diag.hit.out_of_range" => {
            "“{value}”超出 HitSummon 模式范围 0 到 15。游戏会使用 1 (NU)。请输入 0 到 15 的值。"
        }
        "diag.hit.noncanonical" => {
            "“{value}”并未直接表示 0 到 15 的 HitSummon 模式。游戏会读取为 {effective} ({mode})。请替换为所需的模式编号。"
        }
        "diag.hit.nu_literal" => {
            "此处“NU”不是数字模式 ID。游戏会将其替换为 1 (NU)。中立模式请使用 1。"
        }
        "diag.hit.non_numeric_outside" => {
            "“{value}”此处不是数字模式 ID。游戏会将其替换为 1 (NU)。请输入 0 到 15 的值。"
        }
        "diag.hit.non_numeric" => {
            "“{value}”此处不是数字模式 ID。游戏会读取为 {effective} ({mode})。请替换为所需的 0 到 15 模式编号。"
        }
        "json.syntax_invalid" => "本地化 JSON 无效：{error}",
        "json.id_out_of_range" => {
            "{file}.json：id {id} 超出运行时字符串 ID 范围 0..65535；游戏将此命名空间存为 uint16。"
        }
        "json.missing_id" => "{file}.json：第 {entry} 项缺少 id。",
        "json.duplicate_id_same" => {
            "{file}.json：id {id} 在第 {first} 项和第 {entry} 项重复。第 {entry} 项会被忽略，其 ID 和 Key 都不会注册。"
        }
        "json.duplicate_id_cross" => {
            "{file}.json：id {id} 在 {otherFile}.json 中重复。第 {entry} 项会被忽略，其 ID 和 Key 都不会注册。"
        }
        "json.duplicate_key_same" => {
            "{file}.json：Key“{keyValue}”在第 {first} 项和第 {entry} 项重复。第 {entry} 项会被忽略，其 ID 和 Key 都不会注册。"
        }
        "json.duplicate_key_cross" => {
            "{file}.json：Key“{keyValue}”在 {otherFile}.json 中重复。第 {entry} 项会被忽略，其 ID 和 Key 都不会注册。"
        }
        "json.missing_fields" => "{file}.json：第 {entry} 项（{keyValue}）缺少字段：{fields}",
        "json.unused_key" => {
            "{file}.json：Key“{keyValue}”(id: {id}) 未作为条目被任何 .txt 或 layout .json 文件引用。"
        }
        "hover.unknown_monpet_stat" => {
            "**未知的 Stat 名称**\n\n`{value}` 不是已知的 Stat。此 Consume 奖励不会生效，但其他 Consume 槽位仍会生效。请使用 `itemstatcost.txt` 中准确的 Stat 名称。"
        }
        "hover.unknown_property_stat" => {
            "**未知的 Stat 名称**\n\n`{value}` 不是已知的 Stat。请使用 `itemstatcost.txt` 中准确的 Stat 名称。"
        }
        "hover.unknown_property_stat_noeffect" => {
            "**未知的 Stat 名称**\n\n`{value}` 不是已知的 Stat。此 property 没有效果。请使用 `itemstatcost.txt` 中准确的 Stat 名称。"
        }
        "hover.range_valid" => "**Range 代码**\n\n`{value}` 有效。游戏使用 range 代码 `{stored}`。",
        "hover.reference_resolved" => {
            "**引用信息**\n\n`{value}` → `{stored}`，位于 `{file}.{column}`"
        }
        "hover.boolean_value" => {
            "**布尔值**\n\n`{value}` → **{result}**（0 表示 false，任意非零数字表示 true）\n\n{version}"
        }
        "hover.game_version" => "游戏版本：{version}",
        "hover.game_version_unselected" => "游戏版本：未选择",
        "hover.hit_summon" => {
            "**HitSummon 怪物模式**\n\n第二个服务器参数使用 0 到 15 的怪物模式编号。\n\n0=DT, 1=NU, 2=WL, 3=GH, 4=A1, 5=A2, 6=BL, 7=SC, 8=S1, 9=S2, 10=S3, 11=S4, 12=DD, 13=KB, 14=xx, 15=RN。\n\n超出 0 到 15 的值会使用 1=NU。\n\n{current}"
        }
        "hover.hit_current_fallback" => "当前值：`{value}` → 1 (NU)",
        "hover.hit_current" => "当前值：`{value}` → {effective} ({mode})",
        "hover.header" => "**{column}**\n\n{description}",
        "log.json_stopped" => "本地化 JSON 诊断已停止：{error}",
        "log.json_warning" => "{warning}",
        "log.json_path_uri" => "无法将本地化 JSON 路径转换为 URI：{path}",
        "log.json_parse_failed" => "无法解析本地化 JSON {path}：{error}",
        "log.ignored_change" => "已忽略未打开文档的 didChange：{uri}",
        "log.schema_loaded" => "架构加载成功。",
        "log.schema_selection_failed" => "无法选择架构：{error}",
        "log.schema_load_failed" => "无法加载架构：{error}",
        "log.workspace_scan_failed" => "工作区扫描失败：{error}",
        _ => "插件诊断：{values}",
    }
}

fn german(key: &str) -> &'static str {
    match key {
        "diag.duplicate_unique" => {
            "Doppelter Wert '{value}' in eindeutiger Schlüsselspalte '{column}' (zuerst in Zeile {line}, Spalte {firstColumn})"
        }
        "diag.reference.unresolved" => {
            "Referenzwert '{value}' wurde in {file}.{column} nicht gefunden."
        }
        "diag.reference.range" => {
            "Unbekannter Bereichscode. Verwenden Sie einen von: none, h2h, rng, both, loc."
        }
        "diag.reference.monpet_consumestat" => {
            "Unbekannter Stat-Name '{value}'. Dieser Consume-Bonus wird nicht angewendet; andere Consume-Slots funktionieren weiter. Verwenden Sie den exakten Stat-Namen aus itemstatcost.txt."
        }
        "diag.reference.properties_stat" => {
            "Unbekannter Stat-Name '{value}'. Verwenden Sie den exakten Stat-Namen aus itemstatcost.txt."
        }
        "diag.reference.properties_stat_noeffect" => {
            "Unbekannter Stat-Name '{value}'. Diese Eigenschaft hat keine Wirkung. Verwenden Sie den exakten Stat-Namen aus itemstatcost.txt."
        }
        "diag.fixed4_unknown" => {
            "Unbekannter Code '{value}'. Das Spiel liest diesen Code als '{effective}'. Prüfen Sie den vierstelligen Code und die Groß-/Kleinschreibung."
        }
        "diag.integer.backtick" => {
            "'`' wird nicht als normale Ganzzahl geschrieben. Das Spiel wandelt ihn in 48 um. Ersetzen Sie ihn durch die gewünschte Zahl."
        }
        "diag.integer.invalid" => {
            "'{value}' ist keine Standard-Ganzzahl für '{column}'. Verwenden Sie eine einfache ganze Zahl; das Spiel könnte einen anderen Wert lesen."
        }
        "diag.float.invalid" => "'{value}' ist keine gültige Zahl für Spalte '{column}'",
        "diag.boolean.invalid" => {
            "'{value}' ist kein Standard-Boolean-Wert für '{column}'. Verwenden Sie 0 für false oder 1 für true."
        }
        "diag.boolean.type29_invalid" => {
            "'{value}' ist keine Zahl für '{column}'. Verwenden Sie 0 für false oder eine von null verschiedene Ganzzahl für true."
        }
        "diag.hit.out_of_range" => {
            "'{value}' liegt außerhalb des HitSummon-Modusbereichs 0 bis 15. Das Spiel verwendet 1 (NU). Geben Sie einen Wert von 0 bis 15 ein."
        }
        "diag.hit.noncanonical" => {
            "'{value}' bezeichnet keinen HitSummon-Modus von 0 bis 15 direkt. Das Spiel liest ihn als {effective} ({mode}). Ersetzen Sie ihn durch die gewünschte Modusnummer."
        }
        "diag.hit.nu_literal" => {
            "'NU' ist hier keine numerische Modus-ID. Das Spiel ersetzt ihn durch 1 (NU). Verwenden Sie 1 für den neutralen Modus."
        }
        "diag.hit.non_numeric_outside" => {
            "'{value}' ist hier keine numerische Modus-ID. Das Spiel ersetzt ihn durch 1 (NU). Geben Sie einen Wert von 0 bis 15 ein."
        }
        "diag.hit.non_numeric" => {
            "'{value}' ist hier keine numerische Modus-ID. Das Spiel liest ihn als {effective} ({mode}). Ersetzen Sie ihn durch die gewünschte Modusnummer von 0 bis 15."
        }
        "json.syntax_invalid" => "Ungültiges Lokalisierungs-JSON: {error}",
        "json.id_out_of_range" => {
            "{file}.json: ID {id} liegt außerhalb des Laufzeitbereichs für String-IDs 0..65535; das Spiel speichert diesen Namensraum als uint16"
        }
        "json.missing_id" => "{file}.json: ID fehlt bei Eintrag {entry}",
        "json.duplicate_id_same" => {
            "{file}.json: doppelte ID {id} bei Einträgen {first} und {entry}; Eintrag {entry} wird ignoriert, daher werden weder seine ID noch sein Key registriert"
        }
        "json.duplicate_id_cross" => {
            "{file}.json: doppelte ID {id} in {otherFile}.json; Eintrag {entry} wird ignoriert, daher werden weder seine ID noch sein Key registriert"
        }
        "json.duplicate_key_same" => {
            "{file}.json: doppelter Key '{keyValue}' bei Einträgen {first} und {entry}; Eintrag {entry} wird ignoriert, daher werden weder seine ID noch sein Key registriert"
        }
        "json.duplicate_key_cross" => {
            "{file}.json: doppelter Key '{keyValue}' in {otherFile}.json; Eintrag {entry} wird ignoriert, daher werden weder seine ID noch sein Key registriert"
        }
        "json.missing_fields" => {
            "{file}.json: Eintrag {entry} ({keyValue}) fehlen Felder: {fields}"
        }
        "json.unused_key" => {
            "{file}.json: Key '{keyValue}' (ID: {id}) wird in keiner .txt- oder Layout-.json-Datei als Eintrag referenziert"
        }
        "hover.unknown_monpet_stat" => {
            "**Unbekannter Stat-Name**\n\n`{value}` ist kein bekannter Stat. Dieser Consume-Bonus wird nicht angewendet; andere Consume-Slots funktionieren weiter. Verwenden Sie den exakten Stat-Namen aus `itemstatcost.txt`."
        }
        "hover.unknown_property_stat" => {
            "**Unbekannter Stat-Name**\n\n`{value}` ist kein bekannter Stat. Verwenden Sie den exakten Stat-Namen aus `itemstatcost.txt`."
        }
        "hover.unknown_property_stat_noeffect" => {
            "**Unbekannter Stat-Name**\n\n`{value}` ist kein bekannter Stat. Diese Eigenschaft hat keine Wirkung. Verwenden Sie den exakten Stat-Namen aus `itemstatcost.txt`."
        }
        "hover.range_valid" => {
            "**Bereichscode**\n\n`{value}` ist gültig. Das Spiel verwendet Bereichscode `{stored}`."
        }
        "hover.reference_resolved" => "**Referenz**\n\n`{value}` → `{stored}` in `{file}.{column}`",
        "hover.boolean_value" => {
            "**Boolean-Wert**\n\n`{value}` → **{result}** (0 bedeutet false; jede von null verschiedene Zahl bedeutet true)\n\n{version}"
        }
        "hover.game_version" => "Spielversion: {version}",
        "hover.game_version_unselected" => "Spielversion: nicht ausgewählt",
        "hover.hit_summon" => {
            "**HitSummon-Monstermodus**\n\nDer zweite Serverparameter verwendet eine Monstermodusnummer von 0 bis 15.\n\n0=DT, 1=NU, 2=WL, 3=GH, 4=A1, 5=A2, 6=BL, 7=SC, 8=S1, 9=S2, 10=S3, 11=S4, 12=DD, 13=KB, 14=xx, 15=RN.\n\nWerte außerhalb von 0 bis 15 verwenden 1=NU.\n\n{current}"
        }
        "hover.hit_current_fallback" => "Aktueller Wert: `{value}` → 1 (NU)",
        "hover.hit_current" => "Aktueller Wert: `{value}` → {effective} ({mode})",
        "hover.header" => "**{column}**\n\n{description}",
        "log.json_stopped" => "Lokalisierungs-JSON-Diagnosen beendet: {error}",
        "log.json_warning" => "{warning}",
        "log.json_path_uri" => {
            "Lokalisierungs-JSON-Pfad konnte nicht in eine URI umgewandelt werden: {path}"
        }
        "log.json_parse_failed" => {
            "Lokalisierungs-JSON {path} konnte nicht analysiert werden: {error}"
        }
        "log.ignored_change" => "didChange für nicht geöffnetes Dokument {uri} ignoriert",
        "log.schema_loaded" => "Schema erfolgreich geladen.",
        "log.schema_selection_failed" => "Schema konnte nicht ausgewählt werden: {error}",
        "log.schema_load_failed" => "Schema konnte nicht geladen werden: {error}",
        "log.workspace_scan_failed" => "Scan des Arbeitsbereichs fehlgeschlagen: {error}",
        _ => "Plugin-Diagnose: {values}",
    }
}

fn french(key: &str) -> &'static str {
    match key {
        "diag.duplicate_unique" => {
            "Valeur dupliquée '{value}' dans la colonne de clé unique '{column}' (première occurrence ligne {line}, colonne {firstColumn})"
        }
        "diag.reference.unresolved" => {
            "Valeur de référence '{value}' introuvable dans {file}.{column}."
        }
        "diag.reference.range" => "Code de plage inconnu. Utilisez : none, h2h, rng, both ou loc.",
        "diag.reference.monpet_consumestat" => {
            "Nom de Stat inconnu '{value}'. Ce bonus Consume n’est pas appliqué ; les autres emplacements Consume fonctionnent. Utilisez le nom Stat exact de itemstatcost.txt."
        }
        "diag.reference.properties_stat" => {
            "Nom de Stat inconnu '{value}'. Utilisez le nom Stat exact de itemstatcost.txt."
        }
        "diag.reference.properties_stat_noeffect" => {
            "Nom de Stat inconnu '{value}'. Cette propriété n’a aucun effet. Utilisez le nom Stat exact de itemstatcost.txt."
        }
        "diag.fixed4_unknown" => {
            "Code inconnu '{value}'. Le jeu lit ce code comme '{effective}'. Vérifiez le code à quatre caractères et la casse."
        }
        "diag.integer.backtick" => {
            "'`' ne s’écrit pas comme un entier normal. Le jeu le convertit en 48. Remplacez-le par le nombre voulu."
        }
        "diag.integer.invalid" => {
            "'{value}' n’est pas un entier standard pour '{column}'. Utilisez un entier simple ; le jeu peut lire une autre valeur."
        }
        "diag.float.invalid" => "'{value}' n’est pas un nombre valide pour la colonne '{column}'",
        "diag.boolean.invalid" => {
            "'{value}' n’est pas une valeur booléenne standard pour '{column}'. Utilisez 0 pour false ou 1 pour true."
        }
        "diag.boolean.type29_invalid" => {
            "'{value}' n’est pas un nombre pour '{column}'. Utilisez 0 pour false ou tout entier non nul pour true."
        }
        "diag.hit.out_of_range" => {
            "'{value}' est hors de la plage HitSummon 0 à 15. Le jeu utilise 1 (NU). Saisissez une valeur de 0 à 15."
        }
        "diag.hit.noncanonical" => {
            "'{value}' ne désigne pas directement un mode HitSummon de 0 à 15. Le jeu le lit comme {effective} ({mode}). Remplacez-le par le numéro voulu."
        }
        "diag.hit.nu_literal" => {
            "'NU' n’est pas ici un ID de mode numérique. Le jeu le remplace par 1 (NU). Utilisez 1 pour le mode neutre."
        }
        "diag.hit.non_numeric_outside" => {
            "'{value}' n’est pas ici un ID de mode numérique. Le jeu le remplace par 1 (NU). Saisissez une valeur de 0 à 15."
        }
        "diag.hit.non_numeric" => {
            "'{value}' n’est pas ici un ID de mode numérique. Le jeu le lit comme {effective} ({mode}). Remplacez-le par un numéro de 0 à 15."
        }
        "json.syntax_invalid" => "JSON de localisation invalide : {error}",
        "json.id_out_of_range" => {
            "{file}.json : l’id {id} est hors de la plage 0..65535 ; le jeu stocke cet espace de noms en uint16"
        }
        "json.missing_id" => "{file}.json : id manquant dans l’entrée {entry}",
        "json.duplicate_id_same" => {
            "{file}.json : id {id} dupliqué dans {first} et {entry} ; l’entrée {entry} est ignorée, donc ni son ID ni sa Key ne sont enregistrés"
        }
        "json.duplicate_id_cross" => {
            "{file}.json : id {id} dupliqué dans {otherFile}.json ; l’entrée {entry} est ignorée, donc ni son ID ni sa Key ne sont enregistrés"
        }
        "json.duplicate_key_same" => {
            "{file}.json : Key '{keyValue}' dupliquée dans {first} et {entry} ; l’entrée {entry} est ignorée, donc ni son ID ni sa Key ne sont enregistrés"
        }
        "json.duplicate_key_cross" => {
            "{file}.json : Key '{keyValue}' dupliquée dans {otherFile}.json ; l’entrée {entry} est ignorée, donc ni son ID ni sa Key ne sont enregistrés"
        }
        "json.missing_fields" => {
            "{file}.json : champs manquants dans l’entrée {entry} ({keyValue}) : {fields}"
        }
        "json.unused_key" => {
            "{file}.json : la Key '{keyValue}' (id : {id}) n’est référencée dans aucun fichier .txt ou layout .json"
        }
        "hover.unknown_monpet_stat" => {
            "**Nom de Stat inconnu**\n\n`{value}` n’est pas un Stat connu. Ce bonus Consume n’est pas appliqué ; les autres emplacements Consume fonctionnent. Utilisez le nom Stat exact de `itemstatcost.txt`."
        }
        "hover.unknown_property_stat" => {
            "**Nom de Stat inconnu**\n\n`{value}` n’est pas un Stat connu. Utilisez le nom Stat exact de `itemstatcost.txt`."
        }
        "hover.unknown_property_stat_noeffect" => {
            "**Nom de Stat inconnu**\n\n`{value}` n’est pas un Stat connu. Cette propriété n’a aucun effet. Utilisez le nom Stat exact de `itemstatcost.txt`."
        }
        "hover.range_valid" => {
            "**Code de plage**\n\n`{value}` est valide. Le jeu utilise le code `{stored}`."
        }
        "hover.reference_resolved" => {
            "**Référence**\n\n`{value}` → `{stored}` dans `{file}.{column}`"
        }
        "hover.boolean_value" => {
            "**Valeur booléenne**\n\n`{value}` → **{result}** (0 signifie false ; tout nombre non nul signifie true)\n\n{version}"
        }
        "hover.game_version" => "Version du jeu : {version}",
        "hover.game_version_unselected" => "Version du jeu : non sélectionnée",
        "hover.hit_summon" => {
            "**Mode de monstre HitSummon**\n\nLe second paramètre serveur utilise un numéro de mode de monstre de 0 à 15.\n\n0=DT, 1=NU, 2=WL, 3=GH, 4=A1, 5=A2, 6=BL, 7=SC, 8=S1, 9=S2, 10=S3, 11=S4, 12=DD, 13=KB, 14=xx, 15=RN.\n\nLes valeurs hors de 0 à 15 utilisent 1=NU.\n\n{current}"
        }
        "hover.hit_current_fallback" => "Valeur actuelle : `{value}` → 1 (NU)",
        "hover.hit_current" => "Valeur actuelle : `{value}` → {effective} ({mode})",
        "hover.header" => "**{column}**\n\n{description}",
        "log.json_stopped" => "Diagnostics JSON de localisation arrêtés : {error}",
        "log.json_warning" => "{warning}",
        "log.json_path_uri" => {
            "Impossible de convertir le chemin JSON de localisation en URI : {path}"
        }
        "log.json_parse_failed" => "Impossible d’analyser le JSON de localisation {path} : {error}",
        "log.ignored_change" => "didChange ignoré pour le document non ouvert {uri}",
        "log.schema_loaded" => "Schéma chargé avec succès.",
        "log.schema_selection_failed" => "Impossible de sélectionner le schéma : {error}",
        "log.schema_load_failed" => "Impossible de charger le schéma : {error}",
        "log.workspace_scan_failed" => "Échec de l’analyse de l’espace de travail : {error}",
        _ => "Diagnostic du plugin : {values}",
    }
}

fn italian(key: &str) -> &'static str {
    match key {
        "diag.duplicate_unique" => {
            "Valore duplicato '{value}' nella colonna chiave univoca '{column}' (prima occorrenza alla riga {line}, colonna {firstColumn})"
        }
        "diag.reference.unresolved" => {
            "Valore di riferimento '{value}' non trovato in {file}.{column}."
        }
        "diag.reference.range" => {
            "Codice intervallo sconosciuto. Usare uno tra: none, h2h, rng, both, loc."
        }
        "diag.reference.monpet_consumestat" => {
            "Nome Stat sconosciuto '{value}'. Questo bonus Consume non viene applicato; gli altri slot Consume continuano a funzionare. Usare il nome Stat esatto da itemstatcost.txt."
        }
        "diag.reference.properties_stat" => {
            "Nome Stat sconosciuto '{value}'. Usare il nome Stat esatto da itemstatcost.txt."
        }
        "diag.reference.properties_stat_noeffect" => {
            "Nome Stat sconosciuto '{value}'. Questa proprietà non ha effetto. Usare il nome Stat esatto da itemstatcost.txt."
        }
        "diag.fixed4_unknown" => {
            "Codice sconosciuto '{value}'. Il gioco legge questo codice come '{effective}'. Controllare il codice di quattro caratteri e le maiuscole/minuscole."
        }
        "diag.integer.backtick" => {
            "'`' non viene scritto come un normale intero. Il gioco lo converte in 48. Sostituirlo con il numero desiderato."
        }
        "diag.integer.invalid" => {
            "'{value}' non è un intero standard per '{column}'. Usare un numero intero semplice; il gioco potrebbe leggere un valore diverso."
        }
        "diag.float.invalid" => "'{value}' non è un numero valido per la colonna '{column}'",
        "diag.boolean.invalid" => {
            "'{value}' non è un valore booleano standard per '{column}'. Usare 0 per false o 1 per true."
        }
        "diag.boolean.type29_invalid" => {
            "'{value}' non è un numero per '{column}'. Usare 0 per false o un intero diverso da zero per true."
        }
        "diag.hit.out_of_range" => {
            "'{value}' è fuori dall’intervallo HitSummon da 0 a 15. Il gioco usa 1 (NU). Inserire un valore da 0 a 15."
        }
        "diag.hit.noncanonical" => {
            "'{value}' non identifica direttamente una modalità HitSummon da 0 a 15. Il gioco lo legge come {effective} ({mode}). Sostituirlo con il numero desiderato."
        }
        "diag.hit.nu_literal" => {
            "'NU' non è qui un ID numerico della modalità. Il gioco lo sostituisce con 1 (NU). Usare 1 per la modalità neutra."
        }
        "diag.hit.non_numeric_outside" => {
            "'{value}' non è qui un ID numerico della modalità. Il gioco lo sostituisce con 1 (NU). Inserire un valore da 0 a 15."
        }
        "diag.hit.non_numeric" => {
            "'{value}' non è qui un ID numerico della modalità. Il gioco lo legge come {effective} ({mode}). Sostituirlo con un numero da 0 a 15."
        }
        "json.syntax_invalid" => "JSON di localizzazione non valido: {error}",
        "json.id_out_of_range" => {
            "{file}.json: id {id} fuori dall’intervallo 0..65535; il gioco memorizza questo namespace come uint16"
        }
        "json.missing_id" => "{file}.json: id mancante nella voce {entry}",
        "json.duplicate_id_same" => {
            "{file}.json: id duplicato {id} nelle voci {first} e {entry}; la voce {entry} viene ignorata, quindi non vengono registrati né ID né Key"
        }
        "json.duplicate_id_cross" => {
            "{file}.json: id duplicato {id} in {otherFile}.json; la voce {entry} viene ignorata, quindi non vengono registrati né ID né Key"
        }
        "json.duplicate_key_same" => {
            "{file}.json: Key duplicata '{keyValue}' nelle voci {first} e {entry}; la voce {entry} viene ignorata, quindi non vengono registrati né ID né Key"
        }
        "json.duplicate_key_cross" => {
            "{file}.json: Key duplicata '{keyValue}' in {otherFile}.json; la voce {entry} viene ignorata, quindi non vengono registrati né ID né Key"
        }
        "json.missing_fields" => {
            "{file}.json: nella voce {entry} ({keyValue}) mancano i campi: {fields}"
        }
        "json.unused_key" => {
            "{file}.json: la Key '{keyValue}' (id: {id}) non è usata in alcun file .txt o layout .json"
        }
        "hover.unknown_monpet_stat" => {
            "**Nome Stat sconosciuto**\n\n`{value}` non è uno Stat noto. Questo bonus Consume non viene applicato; gli altri slot Consume funzionano. Usare il nome Stat esatto da `itemstatcost.txt`."
        }
        "hover.unknown_property_stat" => {
            "**Nome Stat sconosciuto**\n\n`{value}` non è uno Stat noto. Usare il nome Stat esatto da `itemstatcost.txt`."
        }
        "hover.unknown_property_stat_noeffect" => {
            "**Nome Stat sconosciuto**\n\n`{value}` non è uno Stat noto. Questa proprietà non ha effetto. Usare il nome Stat esatto da `itemstatcost.txt`."
        }
        "hover.range_valid" => {
            "**Codice intervallo**\n\n`{value}` è valido. Il gioco usa il codice `{stored}`."
        }
        "hover.reference_resolved" => {
            "**Riferimento**\n\n`{value}` → `{stored}` in `{file}.{column}`"
        }
        "hover.boolean_value" => {
            "**Valore booleano**\n\n`{value}` → **{result}** (0 significa false; ogni numero diverso da zero significa true)\n\n{version}"
        }
        "hover.game_version" => "Versione del gioco: {version}",
        "hover.game_version_unselected" => "Versione del gioco: non selezionata",
        "hover.hit_summon" => {
            "**Modalità mostro HitSummon**\n\nIl secondo parametro del server usa una modalità mostro da 0 a 15.\n\n0=DT, 1=NU, 2=WL, 3=GH, 4=A1, 5=A2, 6=BL, 7=SC, 8=S1, 9=S2, 10=S3, 11=S4, 12=DD, 13=KB, 14=xx, 15=RN.\n\nI valori fuori da 0 a 15 usano 1=NU.\n\n{current}"
        }
        "hover.hit_current_fallback" => "Valore corrente: `{value}` → 1 (NU)",
        "hover.hit_current" => "Valore corrente: `{value}` → {effective} ({mode})",
        "hover.header" => "**{column}**\n\n{description}",
        "log.json_stopped" => "Diagnostica JSON di localizzazione arrestata: {error}",
        "log.json_warning" => "{warning}",
        "log.json_path_uri" => {
            "Impossibile convertire il percorso JSON di localizzazione in un URI: {path}"
        }
        "log.json_parse_failed" => {
            "Impossibile analizzare il JSON di localizzazione {path}: {error}"
        }
        "log.ignored_change" => "didChange ignorato per il documento non aperto {uri}",
        "log.schema_loaded" => "Schema caricato correttamente.",
        "log.schema_selection_failed" => "Impossibile selezionare lo schema: {error}",
        "log.schema_load_failed" => "Impossibile caricare lo schema: {error}",
        "log.workspace_scan_failed" => "Scansione dell’area di lavoro non riuscita: {error}",
        _ => "Diagnostica del plugin: {values}",
    }
}

fn chinese_traditional(key: &str) -> &'static str {
    match key {
        "diag.duplicate_unique" => {
            "唯一鍵欄位 '{column}' 中的值 '{value}' 重複（首次出現在第 {line} 行、第 {firstColumn} 欄）"
        }
        "diag.reference.unresolved" => "在 {file}.{column} 中找不到參照值 '{value}'。",
        "diag.reference.range" => {
            "未知的 range 代碼。請使用下列其中一項：none、h2h、rng、both、loc。"
        }
        "diag.reference.monpet_consumestat" => {
            "未知的 Stat 名稱 '{value}'。此 Consume 加成不會生效；其他 Consume 欄位仍可運作。請使用 itemstatcost.txt 中完全一致的 Stat 名稱。"
        }
        "diag.reference.properties_stat" => {
            "未知的 Stat 名稱 '{value}'。請使用 itemstatcost.txt 中完全一致的 Stat 名稱。"
        }
        "diag.reference.properties_stat_noeffect" => {
            "未知的 Stat 名稱 '{value}'。此 property 不會生效。請使用 itemstatcost.txt 中完全一致的 Stat 名稱。"
        }
        "diag.fixed4_unknown" => {
            "未知代碼 '{value}'。遊戲會將此代碼讀為 '{effective}'。請檢查四字元代碼及字母大小寫。"
        }
        "diag.integer.backtick" => {
            "'`' 不是一般整數的寫法。遊戲會將它轉為 48。請改為實際需要的數字。"
        }
        "diag.integer.invalid" => {
            "'{value}' 不是欄位 '{column}' 的標準整數。請使用純整數；遊戲可能會讀取不同的值。"
        }
        "diag.float.invalid" => "'{value}' 不是欄位 '{column}' 的有效數字。",
        "diag.boolean.invalid" => {
            "'{value}' 不是欄位 '{column}' 的標準布林值。false 請使用 0，true 請使用 1。"
        }
        "diag.boolean.type29_invalid" => {
            "'{value}' 不是欄位 '{column}' 的數字。false 請使用 0，true 請使用任何非零整數。"
        }
        "diag.hit.out_of_range" => {
            "'{value}' 超出 HitSummon 模式 0 到 15 的範圍。遊戲會使用 1 (NU)。請輸入 0 到 15 的值。"
        }
        "diag.hit.noncanonical" => {
            "'{value}' 並非直接指定 0 到 15 的 HitSummon 模式。遊戲會將它讀為 {effective} ({mode})。請改為實際需要的模式編號。"
        }
        "diag.hit.nu_literal" => {
            "此處的 'NU' 不是數字模式 ID。遊戲會將它替換成 1 (NU)。中立模式請使用 1。"
        }
        "diag.hit.non_numeric_outside" => {
            "'{value}' 此處不是數字模式 ID。遊戲會將它替換成 1 (NU)。請輸入 0 到 15 的值。"
        }
        "diag.hit.non_numeric" => {
            "'{value}' 此處不是數字模式 ID。遊戲會將它讀為 {effective} ({mode})。請改為實際需要的 0 到 15 模式編號。"
        }
        "json.syntax_invalid" => "本地化 JSON 無效：{error}",
        "json.id_out_of_range" => {
            "{file}.json：id {id} 超出執行期字串 ID 範圍 0..65535；遊戲會將此命名空間儲存為 uint16。"
        }
        "json.missing_id" => "{file}.json：項目 {entry} 缺少 id。",
        "json.duplicate_id_same" => {
            "{file}.json：在項目 {first} 與 {entry} 發現重複的 id {id}；項目 {entry} 會被忽略，因此其 ID 與 Key 都不會登錄。"
        }
        "json.duplicate_id_cross" => {
            "{file}.json：在 {otherFile}.json 發現重複的 id {id}；項目 {entry} 會被忽略，因此其 ID 與 Key 都不會登錄。"
        }
        "json.duplicate_key_same" => {
            "{file}.json：在項目 {first} 與 {entry} 發現重複的 Key '{keyValue}'；項目 {entry} 會被忽略，因此其 ID 與 Key 都不會登錄。"
        }
        "json.duplicate_key_cross" => {
            "{file}.json：在 {otherFile}.json 發現重複的 Key '{keyValue}'；項目 {entry} 會被忽略，因此其 ID 與 Key 都不會登錄。"
        }
        "json.missing_fields" => "{file}.json：項目 {entry}（{keyValue}）缺少欄位：{fields}",
        "json.unused_key" => {
            "{file}.json：Key '{keyValue}'（id：{id}）未在任何 .txt 或 layout .json 檔案中作為項目被參照。"
        }
        "hover.unknown_monpet_stat" => {
            "**未知的 Stat 名稱**\n\n`{value}` 不是已知的 Stat。此 Consume 加成不會生效；其他 Consume 欄位仍可運作。請使用 `itemstatcost.txt` 中完全一致的 Stat 名稱。"
        }
        "hover.unknown_property_stat" => {
            "**未知的 Stat 名稱**\n\n`{value}` 不是已知的 Stat。請使用 `itemstatcost.txt` 中完全一致的 Stat 名稱。"
        }
        "hover.unknown_property_stat_noeffect" => {
            "**未知的 Stat 名稱**\n\n`{value}` 不是已知的 Stat。此 property 不會生效。請使用 `itemstatcost.txt` 中完全一致的 Stat 名稱。"
        }
        "hover.range_valid" => "**Range 代碼**\n\n`{value}` 有效。遊戲使用 range 代碼 `{stored}`。",
        "hover.reference_resolved" => {
            "**參照資訊**\n\n`{value}` → `{stored}` 位於 `{file}.{column}`"
        }
        "hover.boolean_value" => {
            "**布林值**\n\n`{value}` → **{result}**（0 為 false；任何非零數字皆為 true）\n\n{version}"
        }
        "hover.game_version" => "遊戲版本：{version}",
        "hover.game_version_unselected" => "遊戲版本：未選取",
        "hover.hit_summon" => {
            "**HitSummon 怪物模式**\n\n第二個伺服器參數使用 0 到 15 的怪物模式編號。\n\n0=DT，1=NU，2=WL，3=GH，4=A1，5=A2，6=BL，7=SC，8=S1，9=S2，10=S3，11=S4，12=DD，13=KB，14=xx，15=RN。\n\n超出 0 到 15 的值會使用 1=NU。\n\n{current}"
        }
        "hover.hit_current_fallback" => "目前值：`{value}` → 1 (NU)",
        "hover.hit_current" => "目前值：`{value}` → {effective} ({mode})",
        "hover.header" => "**{column}**\n\n{description}",
        "log.json_stopped" => "本地化 JSON 診斷已停止：{error}",
        "log.json_warning" => "{warning}",
        "log.json_path_uri" => "無法將本地化 JSON 路徑轉換為 URI：{path}",
        "log.json_parse_failed" => "無法解析本地化 JSON {path}：{error}",
        "log.ignored_change" => "已忽略未開啟文件的 didChange：{uri}",
        "log.schema_loaded" => "架構載入成功。",
        "log.schema_selection_failed" => "無法選擇架構：{error}",
        "log.schema_load_failed" => "無法載入架構：{error}",
        "log.workspace_scan_failed" => "工作區掃描失敗：{error}",
        _ => "外掛程式診斷：{values}",
    }
}

fn japanese(key: &str) -> &'static str {
    match key {
        "diag.duplicate_unique" => {
            "一意キー列 '{column}' に値 '{value}' が重複しています（最初の出現: 行 {line}、列 {firstColumn}）。"
        }
        "diag.reference.unresolved" => "参照値 '{value}' が {file}.{column} に見つかりません。",
        "diag.reference.range" => {
            "不明な range コードです。none、h2h、rng、both、loc のいずれかを使用してください。"
        }
        "diag.reference.monpet_consumestat" => {
            "不明な Stat 名 '{value}' です。この Consume ボーナスは適用されませんが、ほかの Consume スロットは機能します。itemstatcost.txt にある完全一致の Stat 名を使用してください。"
        }
        "diag.reference.properties_stat" => {
            "不明な Stat 名 '{value}' です。itemstatcost.txt にある完全一致の Stat 名を使用してください。"
        }
        "diag.reference.properties_stat_noeffect" => {
            "不明な Stat 名 '{value}' です。この property は効果を持ちません。itemstatcost.txt にある完全一致の Stat 名を使用してください。"
        }
        "diag.fixed4_unknown" => {
            "不明なコード '{value}' です。ゲームはこのコードを '{effective}' として読みます。4 文字コードと大文字・小文字を確認してください。"
        }
        "diag.integer.backtick" => {
            "'`' は通常の整数表記ではありません。ゲームはこれを 48 に変換します。必要な数値に置き換えてください。"
        }
        "diag.integer.invalid" => {
            "'{value}' は列 '{column}' の標準的な整数ではありません。単純な整数を使用してください。ゲームは別の値として読む可能性があります。"
        }
        "diag.float.invalid" => "'{value}' は列 '{column}' の有効な数値ではありません。",
        "diag.boolean.invalid" => {
            "'{value}' は列 '{column}' の標準的な真偽値ではありません。false には 0、true には 1 を使用してください。"
        }
        "diag.boolean.type29_invalid" => {
            "'{value}' は列 '{column}' の数値ではありません。false には 0、true にはゼロ以外の整数を使用してください。"
        }
        "diag.hit.out_of_range" => {
            "'{value}' は HitSummon モードの範囲 0 から 15 の外です。ゲームは 1 (NU) を使用します。0 から 15 の値を入力してください。"
        }
        "diag.hit.noncanonical" => {
            "'{value}' は 0 から 15 の HitSummon モードを直接指定していません。ゲームは {effective} ({mode}) として読みます。必要なモード番号に置き換えてください。"
        }
        "diag.hit.nu_literal" => {
            "ここでの 'NU' は数値モード ID ではありません。ゲームはこれを 1 (NU) に置き換えます。ニュートラルモードには 1 を使用してください。"
        }
        "diag.hit.non_numeric_outside" => {
            "'{value}' はここでは数値モード ID ではありません。ゲームはこれを 1 (NU) に置き換えます。0 から 15 の値を入力してください。"
        }
        "diag.hit.non_numeric" => {
            "'{value}' はここでは数値モード ID ではありません。ゲームは {effective} ({mode}) として読みます。必要な 0 から 15 のモード番号に置き換えてください。"
        }
        "json.syntax_invalid" => "ローカライズ JSON が無効です: {error}",
        "json.id_out_of_range" => {
            "{file}.json: id {id} は実行時文字列 ID の範囲 0..65535 の外です。ゲームはこの名前空間を uint16 として保存します。"
        }
        "json.missing_id" => "{file}.json: エントリ {entry} に id がありません。",
        "json.duplicate_id_same" => {
            "{file}.json: エントリ {first} と {entry} に重複した id {id} があります。エントリ {entry} は無視されるため、その ID と Key は登録されません。"
        }
        "json.duplicate_id_cross" => {
            "{file}.json: {otherFile}.json に重複した id {id} があります。エントリ {entry} は無視されるため、その ID と Key は登録されません。"
        }
        "json.duplicate_key_same" => {
            "{file}.json: エントリ {first} と {entry} に重複した Key '{keyValue}' があります。エントリ {entry} は無視されるため、その ID と Key は登録されません。"
        }
        "json.duplicate_key_cross" => {
            "{file}.json: {otherFile}.json に重複した Key '{keyValue}' があります。エントリ {entry} は無視されるため、その ID と Key は登録されません。"
        }
        "json.missing_fields" => {
            "{file}.json: エントリ {entry}（{keyValue}）にフィールドがありません: {fields}"
        }
        "json.unused_key" => {
            "{file}.json: Key '{keyValue}'（id: {id}）は、どの .txt または layout .json ファイルでもエントリとして参照されていません。"
        }
        "hover.unknown_monpet_stat" => {
            "**不明な Stat 名**\n\n`{value}` は既知の Stat ではありません。この Consume ボーナスは適用されませんが、ほかの Consume スロットは機能します。`itemstatcost.txt` にある完全一致の Stat 名を使用してください。"
        }
        "hover.unknown_property_stat" => {
            "**不明な Stat 名**\n\n`{value}` は既知の Stat ではありません。`itemstatcost.txt` にある完全一致の Stat 名を使用してください。"
        }
        "hover.unknown_property_stat_noeffect" => {
            "**不明な Stat 名**\n\n`{value}` は既知の Stat ではありません。この property は効果を持ちません。`itemstatcost.txt` にある完全一致の Stat 名を使用してください。"
        }
        "hover.range_valid" => {
            "**Range コード**\n\n`{value}` は有効です。ゲームは range コード `{stored}` を使用します。"
        }
        "hover.reference_resolved" => "**参照情報**\n\n`{value}` → `{stored}`（`{file}.{column}`）",
        "hover.boolean_value" => {
            "**真偽値**\n\n`{value}` → **{result}**（0 は false、ゼロ以外の数値は true）\n\n{version}"
        }
        "hover.game_version" => "ゲームバージョン: {version}",
        "hover.game_version_unselected" => "ゲームバージョン: 未選択",
        "hover.hit_summon" => {
            "**HitSummon モンスターモード**\n\n2 番目のサーバーパラメーターには、0 から 15 のモンスターモード番号を使用します。\n\n0=DT、1=NU、2=WL、3=GH、4=A1、5=A2、6=BL、7=SC、8=S1、9=S2、10=S3、11=S4、12=DD、13=KB、14=xx、15=RN。\n\n0 から 15 の外の値では 1=NU が使用されます。\n\n{current}"
        }
        "hover.hit_current_fallback" => "現在の値: `{value}` → 1 (NU)",
        "hover.hit_current" => "現在の値: `{value}` → {effective} ({mode})",
        "hover.header" => "**{column}**\n\n{description}",
        "log.json_stopped" => "ローカライズ JSON 診断を停止しました: {error}",
        "log.json_warning" => "{warning}",
        "log.json_path_uri" => "ローカライズ JSON のパスを URI に変換できませんでした: {path}",
        "log.json_parse_failed" => "ローカライズ JSON {path} を解析できませんでした: {error}",
        "log.ignored_change" => "開いていないドキュメントの didChange を無視しました: {uri}",
        "log.schema_loaded" => "スキーマを読み込みました。",
        "log.schema_selection_failed" => "スキーマを選択できませんでした: {error}",
        "log.schema_load_failed" => "スキーマを読み込めませんでした: {error}",
        "log.workspace_scan_failed" => "ワークスペースのスキャンに失敗しました: {error}",
        _ => "プラグイン診断: {values}",
    }
}

fn brazilian_portuguese(key: &str) -> &'static str {
    match key {
        "diag.duplicate_unique" => {
            "Valor duplicado '{value}' na coluna de chave única '{column}' (primeira ocorrência na linha {line}, coluna {firstColumn})."
        }
        "diag.reference.unresolved" => {
            "O valor de referência '{value}' não foi encontrado em {file}.{column}."
        }
        "diag.reference.range" => {
            "Código range desconhecido. Use um destes: none, h2h, rng, both, loc."
        }
        "diag.reference.monpet_consumestat" => {
            "Nome de Stat desconhecido '{value}'. Este bônus Consume não é aplicado; os outros slots Consume continuam funcionando. Use o nome de Stat exatamente como aparece em itemstatcost.txt."
        }
        "diag.reference.properties_stat" => {
            "Nome de Stat desconhecido '{value}'. Use o nome de Stat exatamente como aparece em itemstatcost.txt."
        }
        "diag.reference.properties_stat_noeffect" => {
            "Nome de Stat desconhecido '{value}'. Esta property não tem efeito. Use o nome de Stat exatamente como aparece em itemstatcost.txt."
        }
        "diag.fixed4_unknown" => {
            "Código desconhecido '{value}'. O jogo lê este código como '{effective}'. Verifique o código de quatro caracteres e as letras maiúsculas/minúsculas."
        }
        "diag.integer.backtick" => {
            "'`' não é escrito como um inteiro normal. O jogo o converte em 48. Substitua-o pelo número que você realmente deseja."
        }
        "diag.integer.invalid" => {
            "'{value}' não é um inteiro padrão para '{column}'. Use um número inteiro simples; o jogo pode ler um valor diferente."
        }
        "diag.float.invalid" => "'{value}' não é um número válido para a coluna '{column}'.",
        "diag.boolean.invalid" => {
            "'{value}' não é um valor booleano padrão para '{column}'. Use 0 para false ou 1 para true."
        }
        "diag.boolean.type29_invalid" => {
            "'{value}' não é um número para '{column}'. Use 0 para false ou qualquer inteiro diferente de zero para true."
        }
        "diag.hit.out_of_range" => {
            "'{value}' está fora do intervalo de modo HitSummon de 0 a 15. O jogo usa 1 (NU). Informe um valor de 0 a 15."
        }
        "diag.hit.noncanonical" => {
            "'{value}' não nomeia diretamente um modo HitSummon de 0 a 15. O jogo o lê como {effective} ({mode}). Substitua-o pelo número de modo desejado."
        }
        "diag.hit.nu_literal" => {
            "'NU' não é um ID de modo numérico aqui. O jogo o substitui por 1 (NU). Use 1 para o modo neutro."
        }
        "diag.hit.non_numeric_outside" => {
            "'{value}' não é um ID de modo numérico aqui. O jogo o substitui por 1 (NU). Informe um valor de 0 a 15."
        }
        "diag.hit.non_numeric" => {
            "'{value}' não é um ID de modo numérico aqui. O jogo o lê como {effective} ({mode}). Substitua-o pelo número de modo desejado de 0 a 15."
        }
        "json.syntax_invalid" => "JSON de localização inválido: {error}",
        "json.id_out_of_range" => {
            "{file}.json: o id {id} está fora do intervalo de ID de string em tempo de execução 0..65535; o jogo armazena este namespace como uint16."
        }
        "json.missing_id" => "{file}.json: falta id na entrada {entry}.",
        "json.duplicate_id_same" => {
            "{file}.json: id duplicado {id} encontrado nas entradas {first} e {entry}; a entrada {entry} é ignorada, portanto nem seu ID nem sua Key são registrados."
        }
        "json.duplicate_id_cross" => {
            "{file}.json: id duplicado {id} encontrado em {otherFile}.json; a entrada {entry} é ignorada, portanto nem seu ID nem sua Key são registrados."
        }
        "json.duplicate_key_same" => {
            "{file}.json: Key duplicada '{keyValue}' encontrada nas entradas {first} e {entry}; a entrada {entry} é ignorada, portanto nem seu ID nem sua Key são registrados."
        }
        "json.duplicate_key_cross" => {
            "{file}.json: Key duplicada '{keyValue}' encontrada em {otherFile}.json; a entrada {entry} é ignorada, portanto nem seu ID nem sua Key são registrados."
        }
        "json.missing_fields" => {
            "{file}.json: faltam campos na entrada {entry} ({keyValue}): {fields}"
        }
        "json.unused_key" => {
            "{file}.json: a Key '{keyValue}' (id: {id}) não é referenciada como uma entrada em nenhum arquivo .txt ou layout .json."
        }
        "hover.unknown_monpet_stat" => {
            "**Nome de Stat desconhecido**\n\n`{value}` não é um Stat conhecido. Este bônus Consume não é aplicado; os outros slots Consume continuam funcionando. Use o nome de Stat exatamente como aparece em `itemstatcost.txt`."
        }
        "hover.unknown_property_stat" => {
            "**Nome de Stat desconhecido**\n\n`{value}` não é um Stat conhecido. Use o nome de Stat exatamente como aparece em `itemstatcost.txt`."
        }
        "hover.unknown_property_stat_noeffect" => {
            "**Nome de Stat desconhecido**\n\n`{value}` não é um Stat conhecido. Esta property não tem efeito. Use o nome de Stat exatamente como aparece em `itemstatcost.txt`."
        }
        "hover.range_valid" => {
            "**Código range**\n\n`{value}` é válido. O jogo usa o código range `{stored}`."
        }
        "hover.reference_resolved" => {
            "**Referência**\n\n`{value}` → `{stored}` em `{file}.{column}`"
        }
        "hover.boolean_value" => {
            "**Valor booleano**\n\n`{value}` → **{result}** (0 significa false; qualquer número diferente de zero significa true)\n\n{version}"
        }
        "hover.game_version" => "Versão do jogo: {version}",
        "hover.game_version_unselected" => "Versão do jogo: não selecionada",
        "hover.hit_summon" => {
            "**Modo de monstro HitSummon**\n\nO segundo parâmetro do servidor usa um número de modo de monstro de 0 a 15.\n\n0=DT, 1=NU, 2=WL, 3=GH, 4=A1, 5=A2, 6=BL, 7=SC, 8=S1, 9=S2, 10=S3, 11=S4, 12=DD, 13=KB, 14=xx, 15=RN.\n\nValores fora de 0 a 15 usam 1=NU.\n\n{current}"
        }
        "hover.hit_current_fallback" => "Valor atual: `{value}` → 1 (NU)",
        "hover.hit_current" => "Valor atual: `{value}` → {effective} ({mode})",
        "hover.header" => "**{column}**\n\n{description}",
        "log.json_stopped" => "Diagnósticos de JSON de localização interrompidos: {error}",
        "log.json_warning" => "{warning}",
        "log.json_path_uri" => {
            "Não foi possível converter o caminho JSON de localização em URI: {path}"
        }
        "log.json_parse_failed" => {
            "Não foi possível analisar o JSON de localização {path}: {error}"
        }
        "log.ignored_change" => "didChange ignorado para documento não aberto {uri}",
        "log.schema_loaded" => "Esquema carregado com sucesso.",
        "log.schema_selection_failed" => "Não foi possível selecionar o esquema: {error}",
        "log.schema_load_failed" => "Não foi possível carregar o esquema: {error}",
        "log.workspace_scan_failed" => "Falha ao examinar o espaço de trabalho: {error}",
        _ => "Diagnóstico do plugin: {values}",
    }
}

fn russian(key: &str) -> &'static str {
    match key {
        "diag.duplicate_unique" => {
            "Повторяющееся значение '{value}' в столбце уникального ключа '{column}' (впервые найдено в строке {line}, столбце {firstColumn})"
        }
        "diag.reference.unresolved" => {
            "Справочное значение '{value}' не найдено в {file}.{column}."
        }
        "diag.reference.range" => {
            "Неизвестный код range. Используйте один из следующих: none, h2h, rng, both, loc."
        }
        "diag.reference.monpet_consumestat" => {
            "Неизвестное имя Stat '{value}'. Этот бонус Consume не применяется, но остальные слоты Consume продолжают работать. Используйте точное имя Stat из itemstatcost.txt."
        }
        "diag.reference.properties_stat" => {
            "Неизвестное имя Stat '{value}'. Используйте точное имя Stat из itemstatcost.txt."
        }
        "diag.reference.properties_stat_noeffect" => {
            "Неизвестное имя Stat '{value}'. Это свойство не действует. Используйте точное имя Stat из itemstatcost.txt."
        }
        "diag.fixed4_unknown" => {
            "Неизвестный код '{value}'. Игра читает этот код как '{effective}'. Проверьте четырёхсимвольный код и регистр букв."
        }
        "diag.integer.backtick" => {
            "'`' записан не как обычное целое число. Игра преобразует его в 48. Замените его нужным числом."
        }
        "diag.integer.invalid" => {
            "'{value}' не является стандартным целым числом для '{column}'. Используйте простое целое число; игра может прочитать другое значение."
        }
        "diag.float.invalid" => "'{value}' не является допустимым числом для столбца '{column}'.",
        "diag.boolean.invalid" => {
            "'{value}' не является стандартным логическим значением для '{column}'. Используйте 0 для false или 1 для true."
        }
        "diag.boolean.type29_invalid" => {
            "'{value}' не является числом для '{column}'. Используйте 0 для false или любое ненулевое целое число для true."
        }
        "diag.hit.out_of_range" => {
            "'{value}' выходит за диапазон режима HitSummon от 0 до 15. Игра использует 1 (NU). Введите значение от 0 до 15."
        }
        "diag.hit.noncanonical" => {
            "'{value}' не задаёт напрямую режим HitSummon от 0 до 15. Игра читает его как {effective} ({mode}). Замените его нужным номером режима."
        }
        "diag.hit.nu_literal" => {
            "'NU' здесь не является числовым ID режима. Игра заменяет его на 1 (NU). Для нейтрального режима используйте 1."
        }
        "diag.hit.non_numeric_outside" => {
            "'{value}' здесь не является числовым ID режима. Игра заменяет его на 1 (NU). Введите значение от 0 до 15."
        }
        "diag.hit.non_numeric" => {
            "'{value}' здесь не является числовым ID режима. Игра читает его как {effective} ({mode}). Замените его нужным номером режима от 0 до 15."
        }
        "json.syntax_invalid" => "Недопустимый JSON локализации: {error}",
        "json.id_out_of_range" => {
            "{file}.json: id {id} выходит за диапазон ID строк времени выполнения 0..65535; игра хранит это пространство имён как uint16."
        }
        "json.missing_id" => "{file}.json: у записи {entry} отсутствует id.",
        "json.duplicate_id_same" => {
            "{file}.json: дублирующийся id {id} найден в записях {first} и {entry}; запись {entry} игнорируется, поэтому ни её ID, ни Key не регистрируются."
        }
        "json.duplicate_id_cross" => {
            "{file}.json: дублирующийся id {id} найден в {otherFile}.json; запись {entry} игнорируется, поэтому ни её ID, ни Key не регистрируются."
        }
        "json.duplicate_key_same" => {
            "{file}.json: дублирующийся Key '{keyValue}' найден в записях {first} и {entry}; запись {entry} игнорируется, поэтому ни её ID, ни Key не регистрируются."
        }
        "json.duplicate_key_cross" => {
            "{file}.json: дублирующийся Key '{keyValue}' найден в {otherFile}.json; запись {entry} игнорируется, поэтому ни её ID, ни Key не регистрируются."
        }
        "json.missing_fields" => {
            "{file}.json: в записи {entry} ({keyValue}) отсутствуют поля: {fields}"
        }
        "json.unused_key" => {
            "{file}.json: Key '{keyValue}' (id: {id}) не используется как запись ни в одном файле .txt или layout .json."
        }
        "hover.unknown_monpet_stat" => {
            "**Неизвестное имя Stat**\n\n`{value}` не является известным Stat. Этот бонус Consume не применяется, но остальные слоты Consume продолжают работать. Используйте точное имя Stat из `itemstatcost.txt`."
        }
        "hover.unknown_property_stat" => {
            "**Неизвестное имя Stat**\n\n`{value}` не является известным Stat. Используйте точное имя Stat из `itemstatcost.txt`."
        }
        "hover.unknown_property_stat_noeffect" => {
            "**Неизвестное имя Stat**\n\n`{value}` не является известным Stat. Это свойство не действует. Используйте точное имя Stat из `itemstatcost.txt`."
        }
        "hover.range_valid" => {
            "**Код range**\n\n`{value}` допустим. Игра использует код range `{stored}`."
        }
        "hover.reference_resolved" => "**Ссылка**\n\n`{value}` → `{stored}` в `{file}.{column}`",
        "hover.boolean_value" => {
            "**Логическое значение**\n\n`{value}` → **{result}** (0 означает false; любое ненулевое число означает true)\n\n{version}"
        }
        "hover.game_version" => "Версия игры: {version}",
        "hover.game_version_unselected" => "Версия игры: не выбрана",
        "hover.hit_summon" => {
            "**Режим монстра HitSummon**\n\nВторой параметр сервера использует номер режима монстра от 0 до 15.\n\n0=DT, 1=NU, 2=WL, 3=GH, 4=A1, 5=A2, 6=BL, 7=SC, 8=S1, 9=S2, 10=S3, 11=S4, 12=DD, 13=KB, 14=xx, 15=RN.\n\nДля значений вне диапазона от 0 до 15 используется 1=NU.\n\n{current}"
        }
        "hover.hit_current_fallback" => "Текущее значение: `{value}` → 1 (NU)",
        "hover.hit_current" => "Текущее значение: `{value}` → {effective} ({mode})",
        "hover.header" => "**{column}**\n\n{description}",
        "log.json_stopped" => "Диагностика JSON локализации остановлена: {error}",
        "log.json_warning" => "{warning}",
        "log.json_path_uri" => "Не удалось преобразовать путь JSON локализации в URI: {path}",
        "log.json_parse_failed" => "Не удалось разобрать JSON локализации {path}: {error}",
        "log.ignored_change" => "didChange для неоткрытого документа {uri} проигнорирован",
        "log.schema_loaded" => "Схема успешно загружена.",
        "log.schema_selection_failed" => "Не удалось выбрать схему: {error}",
        "log.schema_load_failed" => "Не удалось загрузить схему: {error}",
        "log.workspace_scan_failed" => "Не удалось просканировать рабочую область: {error}",
        _ => "Диагностика плагина: {values}",
    }
}

fn spanish(key: &str) -> &'static str {
    match key {
        "diag.duplicate_unique" => {
            "Valor duplicado '{value}' en la columna de clave única '{column}' (visto por primera vez en la línea {line}, columna {firstColumn})"
        }
        "diag.reference.unresolved" => {
            "No se encontró el valor de referencia '{value}' en {file}.{column}."
        }
        "diag.reference.range" => {
            "Código de rango desconocido. Use uno de: none, h2h, rng, both, loc."
        }
        "diag.reference.monpet_consumestat" => {
            "Nombre de Stat desconocido '{value}'. Esta bonificación Consume no se aplica; los demás espacios Consume siguen funcionando. Use el nombre Stat exacto de itemstatcost.txt."
        }
        "diag.reference.properties_stat" => {
            "Nombre de Stat desconocido '{value}'. Use el nombre Stat exacto de itemstatcost.txt."
        }
        "diag.reference.properties_stat_noeffect" => {
            "Nombre de Stat desconocido '{value}'. Esta propiedad no tiene efecto. Use el nombre Stat exacto de itemstatcost.txt."
        }
        "diag.fixed4_unknown" => {
            "Código desconocido '{value}'. El juego lee este código como '{effective}'. Compruebe el código de cuatro caracteres y las mayúsculas."
        }
        "diag.integer.backtick" => {
            "'`' no se escribe como un entero normal. El juego lo convierte en 48. Sustitúyalo por el número que realmente desea."
        }
        "diag.integer.invalid" => {
            "'{value}' no es un entero estándar para '{column}'. Use un número entero simple; el juego podría leer otro valor."
        }
        "diag.float.invalid" => "'{value}' no es un número válido para la columna '{column}'",
        "diag.boolean.invalid" => {
            "'{value}' no es un valor booleano estándar para '{column}'. Use 0 para false o 1 para true."
        }
        "diag.boolean.type29_invalid" => {
            "'{value}' no es un número para '{column}'. Use 0 para false o cualquier entero distinto de cero para true."
        }
        "diag.hit.out_of_range" => {
            "'{value}' está fuera del rango de modo HitSummon de 0 a 15. El juego usa 1 (NU). Introduzca un valor de 0 a 15."
        }
        "diag.hit.noncanonical" => {
            "'{value}' no nombra directamente un modo HitSummon de 0 a 15. El juego lo lee como {effective} ({mode}). Sustitúyalo por el número de modo que desea."
        }
        "diag.hit.nu_literal" => {
            "'NU' no es aquí un ID de modo numérico. El juego lo sustituye por 1 (NU). Use 1 para el modo neutral."
        }
        "diag.hit.non_numeric_outside" => {
            "'{value}' no es aquí un ID de modo numérico. El juego lo sustituye por 1 (NU). Introduzca un valor de 0 a 15."
        }
        "diag.hit.non_numeric" => {
            "'{value}' no es aquí un ID de modo numérico. El juego lo lee como {effective} ({mode}). Sustitúyalo por el número de modo deseado de 0 a 15."
        }
        "json.syntax_invalid" => "JSON de localización no válido: {error}",
        "json.id_out_of_range" => {
            "{file}.json: el id {id} está fuera del rango de ID de cadenas en tiempo de ejecución 0..65535; el juego almacena este espacio de nombres como uint16"
        }
        "json.missing_id" => "{file}.json: falta el id en la entrada {entry}",
        "json.duplicate_id_same" => {
            "{file}.json: id duplicado {id} en las entradas {first} y {entry}; se ignora la entrada {entry}, por lo que no se registra ni su ID ni su Key"
        }
        "json.duplicate_id_cross" => {
            "{file}.json: id duplicado {id} en {otherFile}.json; se ignora la entrada {entry}, por lo que no se registra ni su ID ni su Key"
        }
        "json.duplicate_key_same" => {
            "{file}.json: Key duplicada '{keyValue}' en las entradas {first} y {entry}; se ignora la entrada {entry}, por lo que no se registra ni su ID ni su Key"
        }
        "json.duplicate_key_cross" => {
            "{file}.json: Key duplicada '{keyValue}' en {otherFile}.json; se ignora la entrada {entry}, por lo que no se registra ni su ID ni su Key"
        }
        "json.missing_fields" => {
            "{file}.json: faltan campos en la entrada {entry} ({keyValue}): {fields}"
        }
        "json.unused_key" => {
            "{file}.json: la Key '{keyValue}' (id: {id}) no se referencia como entrada en ningún archivo .txt o layout .json"
        }
        "hover.unknown_monpet_stat" => {
            "**Nombre de Stat desconocido**\n\n`{value}` no es un Stat conocido. Esta bonificación Consume no se aplica; los demás espacios Consume siguen funcionando. Use el nombre Stat exacto de `itemstatcost.txt`."
        }
        "hover.unknown_property_stat" => {
            "**Nombre de Stat desconocido**\n\n`{value}` no es un Stat conocido. Use el nombre Stat exacto de `itemstatcost.txt`."
        }
        "hover.unknown_property_stat_noeffect" => {
            "**Nombre de Stat desconocido**\n\n`{value}` no es un Stat conocido. Esta propiedad no tiene efecto. Use el nombre Stat exacto de `itemstatcost.txt`."
        }
        "hover.range_valid" => {
            "**Código de rango**\n\n`{value}` es válido. El juego usa el código de rango `{stored}`."
        }
        "hover.reference_resolved" => {
            "**Referencia**\n\n`{value}` → `{stored}` en `{file}.{column}`"
        }
        "hover.boolean_value" => {
            "**Valor booleano**\n\n`{value}` → **{result}** (0 significa false; cualquier número distinto de cero significa true)\n\n{version}"
        }
        "hover.game_version" => "Versión del juego: {version}",
        "hover.game_version_unselected" => "Versión del juego: no seleccionada",
        "hover.hit_summon" => {
            "**Modo de monstruo HitSummon**\n\nEl segundo parámetro del servidor usa un número de modo de monstruo de 0 a 15.\n\n0=DT, 1=NU, 2=WL, 3=GH, 4=A1, 5=A2, 6=BL, 7=SC, 8=S1, 9=S2, 10=S3, 11=S4, 12=DD, 13=KB, 14=xx, 15=RN.\n\nLos valores fuera de 0 a 15 usan 1=NU.\n\n{current}"
        }
        "hover.hit_current_fallback" => "Valor actual: `{value}` → 1 (NU)",
        "hover.hit_current" => "Valor actual: `{value}` → {effective} ({mode})",
        "hover.header" => "**{column}**\n\n{description}",
        "log.json_stopped" => "Se detuvieron los diagnósticos de JSON de localización: {error}",
        "log.json_warning" => "{warning}",
        "log.json_path_uri" => {
            "No se pudo convertir la ruta JSON de localización en un URI: {path}"
        }
        "log.json_parse_failed" => "No se pudo analizar el JSON de localización {path}: {error}",
        "log.ignored_change" => "Se ignoró didChange para el documento no abierto {uri}",
        "log.schema_loaded" => "Esquema cargado correctamente.",
        "log.schema_selection_failed" => "No se pudo seleccionar el esquema: {error}",
        "log.schema_load_failed" => "No se pudo cargar el esquema: {error}",
        "log.workspace_scan_failed" => "Falló el análisis del espacio de trabajo: {error}",
        _ => "Diagnóstico del plugin: {values}",
    }
}

fn polish(key: &str) -> &'static str {
    match key {
        "diag.duplicate_unique" => {
            "Zduplikowana wartość '{value}' w kolumnie unikalnego klucza '{column}' (pierwsze wystąpienie: wiersz {line}, kolumna {firstColumn})"
        }
        "diag.reference.unresolved" => {
            "Nie znaleziono wartości referencyjnej '{value}' w {file}.{column}."
        }
        "diag.reference.range" => {
            "Nieznany kod zakresu. Użyj jednego z: none, h2h, rng, both, loc."
        }
        "diag.reference.monpet_consumestat" => {
            "Nieznana nazwa Stat '{value}'. Ten bonus Consume nie zostanie zastosowany; pozostałe sloty Consume nadal działają. Użyj dokładnej nazwy Stat z itemstatcost.txt."
        }
        "diag.reference.properties_stat" => {
            "Nieznana nazwa Stat '{value}'. Użyj dokładnej nazwy Stat z itemstatcost.txt."
        }
        "diag.reference.properties_stat_noeffect" => {
            "Nieznana nazwa Stat '{value}'. Ta właściwość nie ma efektu. Użyj dokładnej nazwy Stat z itemstatcost.txt."
        }
        "diag.fixed4_unknown" => {
            "Nieznany kod '{value}'. Gra odczytuje ten kod jako '{effective}'. Sprawdź czteroznakowy kod i wielkość liter."
        }
        "diag.integer.backtick" => {
            "'`' nie jest zapisywany jako zwykła liczba całkowita. Gra konwertuje go na 48. Zastąp go liczbą, której faktycznie chcesz użyć."
        }
        "diag.integer.invalid" => {
            "'{value}' nie jest standardową liczbą całkowitą dla '{column}'. Użyj zwykłej liczby całkowitej; gra może odczytać inną wartość."
        }
        "diag.float.invalid" => "'{value}' nie jest prawidłową liczbą dla kolumny '{column}'",
        "diag.boolean.invalid" => {
            "'{value}' nie jest standardową wartością logiczną dla '{column}'. Użyj 0 dla false albo 1 dla true."
        }
        "diag.boolean.type29_invalid" => {
            "'{value}' nie jest liczbą dla '{column}'. Użyj 0 dla false albo dowolnej niezerowej liczby całkowitej dla true."
        }
        "diag.hit.out_of_range" => {
            "'{value}' jest poza zakresem trybu HitSummon od 0 do 15. Gra używa 1 (NU). Wprowadź wartość od 0 do 15."
        }
        "diag.hit.noncanonical" => {
            "'{value}' nie wskazuje bezpośrednio trybu HitSummon od 0 do 15. Gra odczytuje go jako {effective} ({mode}). Zastąp go numerem trybu, którego chcesz użyć."
        }
        "diag.hit.nu_literal" => {
            "'NU' nie jest tutaj numerycznym ID trybu. Gra zastępuje je przez 1 (NU). Użyj 1 dla trybu neutralnego."
        }
        "diag.hit.non_numeric_outside" => {
            "'{value}' nie jest tutaj numerycznym ID trybu. Gra zastępuje go przez 1 (NU). Wprowadź wartość od 0 do 15."
        }
        "diag.hit.non_numeric" => {
            "'{value}' nie jest tutaj numerycznym ID trybu. Gra odczytuje go jako {effective} ({mode}). Zastąp go numerem wybranego trybu od 0 do 15."
        }
        "json.syntax_invalid" => "Nieprawidłowy lokalizacyjny JSON: {error}",
        "json.id_out_of_range" => {
            "{file}.json: id {id} jest poza zakresem identyfikatorów tekstów środowiska uruchomieniowego 0..65535; gra zapisuje tę przestrzeń nazw jako uint16"
        }
        "json.missing_id" => "{file}.json: brak id we wpisie {entry}",
        "json.duplicate_id_same" => {
            "{file}.json: zduplikowane id {id} we wpisach {first} i {entry}; wpis {entry} jest ignorowany, więc ani jego ID, ani Key nie zostaną zarejestrowane"
        }
        "json.duplicate_id_cross" => {
            "{file}.json: zduplikowane id {id} w {otherFile}.json; wpis {entry} jest ignorowany, więc ani jego ID, ani Key nie zostaną zarejestrowane"
        }
        "json.duplicate_key_same" => {
            "{file}.json: zduplikowany Key '{keyValue}' we wpisach {first} i {entry}; wpis {entry} jest ignorowany, więc ani jego ID, ani Key nie zostaną zarejestrowane"
        }
        "json.duplicate_key_cross" => {
            "{file}.json: zduplikowany Key '{keyValue}' w {otherFile}.json; wpis {entry} jest ignorowany, więc ani jego ID, ani Key nie zostaną zarejestrowane"
        }
        "json.missing_fields" => {
            "{file}.json: we wpisie {entry} ({keyValue}) brakuje pól: {fields}"
        }
        "json.unused_key" => {
            "{file}.json: Key '{keyValue}' (id: {id}) nie jest używany jako wpis w żadnym pliku .txt ani layout .json"
        }
        "hover.unknown_monpet_stat" => {
            "**Nieznana nazwa Stat**\n\n`{value}` nie jest znanym Stat. Ten bonus Consume nie zostanie zastosowany; pozostałe sloty Consume nadal działają. Użyj dokładnej nazwy Stat z `itemstatcost.txt`."
        }
        "hover.unknown_property_stat" => {
            "**Nieznana nazwa Stat**\n\n`{value}` nie jest znanym Stat. Użyj dokładnej nazwy Stat z `itemstatcost.txt`."
        }
        "hover.unknown_property_stat_noeffect" => {
            "**Nieznana nazwa Stat**\n\n`{value}` nie jest znanym Stat. Ta właściwość nie ma efektu. Użyj dokładnej nazwy Stat z `itemstatcost.txt`."
        }
        "hover.range_valid" => {
            "**Kod zakresu**\n\n`{value}` jest prawidłowy. Gra używa kodu zakresu `{stored}`."
        }
        "hover.reference_resolved" => "**Odwołanie**\n\n`{value}` → `{stored}` w `{file}.{column}`",
        "hover.boolean_value" => {
            "**Wartość logiczna**\n\n`{value}` → **{result}** (0 oznacza false; każda niezerowa liczba oznacza true)\n\n{version}"
        }
        "hover.game_version" => "Wersja gry: {version}",
        "hover.game_version_unselected" => "Wersja gry: nie wybrano",
        "hover.hit_summon" => {
            "**Tryb potwora HitSummon**\n\nDrugi parametr serwera używa numeru trybu potwora od 0 do 15.\n\n0=DT, 1=NU, 2=WL, 3=GH, 4=A1, 5=A2, 6=BL, 7=SC, 8=S1, 9=S2, 10=S3, 11=S4, 12=DD, 13=KB, 14=xx, 15=RN.\n\nWartości poza zakresem od 0 do 15 używają 1=NU.\n\n{current}"
        }
        "hover.hit_current_fallback" => "Bieżąca wartość: `{value}` → 1 (NU)",
        "hover.hit_current" => "Bieżąca wartość: `{value}` → {effective} ({mode})",
        "hover.header" => "**{column}**\n\n{description}",
        "log.json_stopped" => "Zatrzymano diagnostykę lokalizacyjnego JSON: {error}",
        "log.json_warning" => "{warning}",
        "log.json_path_uri" => "Nie można przekształcić ścieżki lokalizacyjnego JSON w URI: {path}",
        "log.json_parse_failed" => "Nie można przeanalizować lokalizacyjnego JSON {path}: {error}",
        "log.ignored_change" => "Zignorowano didChange dla nieotwartego dokumentu {uri}",
        "log.schema_loaded" => "Schemat został pomyślnie załadowany.",
        "log.schema_selection_failed" => "Nie można wybrać schematu: {error}",
        "log.schema_load_failed" => "Nie można załadować schematu: {error}",
        "log.workspace_scan_failed" => "Skanowanie obszaru roboczego nie powiodło się: {error}",
        _ => "Diagnostyka wtyczki: {values}",
    }
}

fn mexican_spanish(key: &str) -> &'static str {
    spanish(key)
}

/// An enumerated catalog is exposed for parity tests and build-time auditing.
#[cfg(test)]
pub fn catalog(locale: Locale) -> Vec<(&'static str, String)> {
    CATALOG_KEYS
        .iter()
        .chain(BUNDLED_PLUGIN_KEYS.iter())
        .map(|key| (*key, catalog_template(locale, key)))
        .collect()
}

pub fn placeholder_names(template: &str) -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::new();
    let mut remaining = template;
    while let Some(start) = remaining.find('{') {
        let after = &remaining[start + 1..];
        let Some(end) = after.find('}') else { break };
        let name = &after[..end];
        if !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            names.insert(name.to_string());
        }
        remaining = &after[end + 1..];
    }
    names
}

#[cfg(test)]
pub fn catalog_parity_errors() -> Vec<String> {
    let en = catalog(Locale::EnUs);
    let en_keys = en
        .iter()
        .map(|(key, _)| *key)
        .collect::<std::collections::BTreeSet<_>>();
    let en_placeholders = en
        .into_iter()
        .map(|(key, template)| (key, placeholder_names(&template)))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut errors = Vec::new();
    for locale in Locale::ALL {
        let translated = catalog(locale);
        let keys = translated
            .iter()
            .map(|(key, _)| *key)
            .collect::<std::collections::BTreeSet<_>>();
        if keys != en_keys {
            errors.push(format!("{} catalog key mismatch", locale.as_str()));
        }
        for (key, template) in translated {
            let actual = placeholder_names(&template);
            let expected = en_placeholders.get(key).expect("catalog key exists");
            if actual != *expected {
                errors.push(format!(
                    "{} placeholder mismatch for {key}",
                    locale.as_str()
                ));
            }
        }
    }
    errors
}

pub fn values_arg(args: &Map<String, Value>) -> Map<String, Value> {
    let mut copy = args.clone();
    let compact = args
        .values()
        .map(display_value)
        .collect::<Vec<_>>()
        .join(" · ");
    copy.insert("values".to_string(), json!(compact));
    copy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_accepts_the_published_locales_and_defaults_invalid_to_english() {
        for locale in Locale::ALL {
            assert_eq!(Locale::normalize(Some(locale.as_str())), locale);
        }
        assert_eq!(Locale::normalize(Some("ko-KR")), Locale::KoKr);
        assert_eq!(Locale::normalize(Some("invalid")), Locale::EnUs);
        assert_eq!(Locale::normalize(None), Locale::EnUs);
    }

    #[test]
    fn every_catalog_has_the_same_keys_and_placeholders() {
        assert!(
            catalog_parity_errors().is_empty(),
            "{:?}",
            catalog_parity_errors()
        );
        for locale in Locale::ALL {
            for (key, template) in catalog(locale) {
                assert!(
                    !template.contains("Plugin diagnostic"),
                    "{key} used the English compatibility fallback for {locale:?}"
                );
            }
        }
    }

    #[test]
    fn english_calc_message_and_guidance_are_rendered_as_separate_sections() {
        let diagnostic = localized_plugin_diagnostic(
            Locale::EnUs,
            "plugin.calc.skilldesc-decimal-prefix",
            args([
                ("actual", json!("-6.25")),
                ("consumedPrefix", json!("-6")),
                ("ignoredSuffix", json!(".25")),
            ]),
            Some(
                "Decimal values are not supported here. The game reads '-6.25' as '-6' and ignores '.25'. Use an integer expression that matches your intent."
                    .to_string(),
            ),
            Diagnostic {
                data: Some(json!({
                    "kind": "decimal-policy",
                    "hint": "Use an integer expression that matches your intent."
                })),
                ..Diagnostic::default()
            },
        );
        assert_eq!(
            diagnostic.message,
            "Decimal values are not supported here. The game reads '-6.25' as '-6' and ignores '.25'."
        );
        assert_eq!(
            diagnostic.data.as_ref().unwrap()["localizedGuidanceHeading"],
            "What to do"
        );
        assert_eq!(
            diagnostic.data.as_ref().unwrap()["localizedGuidance"],
            "Use an integer expression that matches your intent."
        );
    }

    #[test]
    fn bundled_plugin_catalog_has_no_non_english_compatibility_fallbacks() {
        for key in BUNDLED_PLUGIN_KEYS {
            let english = catalog_template(Locale::EnUs, key);
            for locale in Locale::ALL
                .into_iter()
                .filter(|locale| *locale != Locale::EnUs)
            {
                let translated = catalog_template(locale, key);
                assert_ne!(
                    translated, english,
                    "{key} must be translated for {locale:?}"
                );
                assert!(
                    !translated.contains("Plugin diagnostic"),
                    "{key} must not use the old generic plugin fallback"
                );
            }
        }
    }

    #[test]
    fn every_bundled_plugin_key_has_a_detailed_template_in_every_non_english_locale() {
        for key in BUNDLED_PLUGIN_KEYS {
            assert!(
                plugin_detail_pl(key).is_some(),
                "missing Polish template: {key}"
            );
            assert!(
                plugin_detail_it(key).is_some(),
                "missing Italian template: {key}"
            );
            assert!(
                plugin_detail_fr(key).is_some(),
                "missing French template: {key}"
            );
            assert!(
                plugin_detail_de(key).is_some(),
                "missing German template: {key}"
            );
            assert!(
                plugin_detail_es(key).is_some(),
                "missing Spanish template: {key}"
            );
            assert!(
                plugin_detail_es_mx(key).is_some(),
                "missing Mexican Spanish template: {key}"
            );
            assert!(
                plugin_detail_ko(key).is_some(),
                "missing Korean template: {key}"
            );
            assert!(
                plugin_detail_zh_cn(key).is_some(),
                "missing Simplified Chinese template: {key}"
            );
            assert!(
                plugin_detail_zh_tw(key).is_some(),
                "missing Traditional Chinese template: {key}"
            );
            assert!(
                plugin_detail_ja(key).is_some(),
                "missing Japanese template: {key}"
            );
            assert!(
                plugin_detail_pt_br(key).is_some(),
                "missing Brazilian Portuguese template: {key}"
            );
            assert!(
                plugin_detail_ru(key).is_some(),
                "missing Russian template: {key}"
            );
        }
    }

    #[test]
    fn explicit_plugin_keys_in_bundled_sources_are_registered() {
        const SOURCES: &[&str] = &[
            include_str!("../contrib/d2rdoc/plugins/calcCheck.ts"),
            include_str!("../contrib/d2rdoc/plugins/cubeInputCheck.ts"),
            include_str!("../contrib/d2rdoc/plugins/cubeOutputCheck.ts"),
            include_str!("../contrib/d2rdoc/plugins/enumHover.ts"),
            include_str!("../contrib/d2rdoc/plugins/itemCodeCheck.ts"),
            include_str!("../contrib/d2rdoc/plugins/itemNameHover.ts"),
            include_str!("../contrib/d2rdoc/plugins/propCodeCheck.ts"),
            include_str!("../contrib/d2rdoc/plugins/tcItemCheck.ts"),
        ];
        for source in SOURCES {
            for candidate in source
                .split('"')
                .filter(|value| value.starts_with("plugin.") && value.len() > "plugin.".len())
            {
                assert!(
                    is_bundled_plugin_key(candidate),
                    "bundled plugin key {candidate} is missing from BUNDLED_PLUGIN_KEYS"
                );
            }
        }
        assert_eq!(BUNDLED_PLUGIN_KEYS.len(), 67);
    }

    #[test]
    fn product_log_and_cli_keys_referenced_by_rust_sources_are_registered() {
        const SOURCES: &[&str] = &[include_str!("backend.rs"), include_str!("main.rs")];
        for source in SOURCES {
            for candidate in source
                .split('"')
                .filter(|value| value.starts_with("log.") || value.starts_with("cli."))
            {
                assert!(
                    CATALOG_KEYS.contains(&candidate),
                    "product message key {candidate} is missing from CATALOG_KEYS"
                );
            }
        }
    }

    #[test]
    fn every_operational_message_has_a_detailed_non_english_sentence() {
        for key in CATALOG_KEYS
            .iter()
            .copied()
            .filter(|key| key.starts_with("log.") || key.starts_with("cli."))
        {
            let Some(english) = operational_log_template(Locale::EnUs, key) else {
                continue;
            };
            for locale in Locale::ALL
                .into_iter()
                .filter(|locale| *locale != Locale::EnUs)
            {
                let translated = catalog_template(locale, key);
                assert_ne!(
                    translated, english,
                    "{key} must be translated for {locale:?}"
                );
                assert!(
                    !translated.starts_with("Operation status")
                        && !translated.starts_with("Command-line status"),
                    "{key} must not use a generic operational frame for {locale:?}"
                );
            }
        }
    }

    #[test]
    fn interpolation_preserves_the_original_parameter_value() {
        let values = args([("value", json!("Save Bits")), ("column", json!("itemtype"))]);
        assert!(localize(Locale::KoKr, "diag.float.invalid", &values).contains("Save Bits"));
        assert!(localize(Locale::ZhCn, "diag.float.invalid", &values).contains("itemtype"));
    }

    #[test]
    fn boolean_messages_are_localized_friendly_and_hide_implementation_details() {
        let forbidden = [
            "type-29",
            "raw byte",
            "raw-byte",
            "low byte",
            "least-significant byte",
            "u32",
            "bitfield",
            "serialization",
            "storage layout",
            "signed decimal integer",
        ];
        let english = localize(
            Locale::EnUs,
            "diag.boolean.type29_invalid",
            &args([("value", json!("true")), ("column", json!("enabled"))]),
        );
        for locale in Locale::ALL {
            let diagnostic = localize(
                locale,
                "diag.boolean.type29_invalid",
                &args([("value", json!("true")), ("column", json!("enabled"))]),
            );
            let on = localize(locale, "hover.boolean_on", &args([]));
            let off = localize(locale, "hover.boolean_off", &args([]));
            let recommendation = localize(locale, "hover.boolean_off_recommendation", &args([]));
            let number_format = match locale {
                Locale::EnUs => "number format",
                Locale::KoKr => "숫자 형식",
                Locale::ZhCn => "数字格式",
                Locale::ZhTw => "數字格式",
                Locale::DeDe => "Zahlenformat",
                Locale::EsEs | Locale::EsMx | Locale::PtBr => "formato numérico",
                Locale::FrFr => "format numérique",
                Locale::ItIt => "formato numerico",
                Locale::PlPl => "formatu liczbowego",
                Locale::JaJp => "数値形式",
                Locale::RuRu => "формат числа",
            };
            assert!(diagnostic.contains("true"), "{locale:?}: {diagnostic}");
            assert!(
                diagnostic.contains(number_format),
                "{locale:?}: {diagnostic}"
            );
            assert!(diagnostic.contains('0') && diagnostic.contains('1'));
            assert_ne!(on, off, "{locale:?}");
            assert!(recommendation.contains('1'), "{locale:?}: {recommendation}");
            let visible = format!("{diagnostic}\n{on}\n{off}\n{recommendation}").to_lowercase();
            for term in forbidden {
                assert!(!visible.contains(term), "{locale:?}:{term}: {visible}");
            }
            if locale != Locale::EnUs {
                assert_ne!(diagnostic, english, "{locale:?}");
                assert_ne!(
                    on, "The current value is treated as on by the game.",
                    "{locale:?}"
                );
            }
        }
    }

    #[test]
    fn localized_diagnostics_keep_the_key_and_named_arguments_in_lsp_data() {
        let diagnostic = localized_diagnostic(
            Locale::KoKr,
            "diag.float.invalid",
            args([("value", json!("Save Bits")), ("column", json!("itemtype"))]),
            Diagnostic::default(),
        );
        assert!(diagnostic.message.contains("Save Bits"));
        assert_eq!(
            diagnostic.data.as_ref().unwrap()["messageKey"],
            "diag.float.invalid"
        );
        assert_eq!(
            diagnostic.data.as_ref().unwrap()["messageArgs"]["column"],
            "itemtype"
        );
    }

    #[test]
    fn skilldesc_decimal_policy_localizes_values_and_correction_without_internal_metadata() {
        let message_args = args([
            ("code", json!("calc.skilldesc-decimal-prefix")),
            ("identifier", json!("")),
            ("expected", json!("")),
            ("actual", json!("-6.25")),
            ("insertText", json!("")),
            (
                "hint",
                json!("Use an integer expression that matches your intent."),
            ),
            ("consumedPrefix", json!("-6")),
            ("ignoredSuffix", json!(".25")),
            ("alias", json!("")),
            ("policyWarning", json!(true)),
        ]);
        for locale in Locale::ALL
            .into_iter()
            .filter(|locale| *locale != Locale::EnUs)
        {
            let diagnostic = localized_plugin_diagnostic(
                locale,
                "plugin.calc.skilldesc-decimal-prefix",
                message_args.clone(),
                None,
                Diagnostic::default(),
            );
            assert!(diagnostic.message.contains("-6.25"), "{locale:?}");
            assert!(diagnostic.message.contains("-6"), "{locale:?}");
            assert!(diagnostic.message.contains(".25"), "{locale:?}");
            assert!(!diagnostic.message.contains("calc.skilldesc"), "{locale:?}");
            assert!(!diagnostic.message.contains("Use an integer"), "{locale:?}");
            assert!(!diagnostic.message.contains("true"), "{locale:?}");
            assert_eq!(diagnostic.data.as_ref().unwrap()["localizedMessage"], true);
            assert!(
                diagnostic.data.as_ref().unwrap()["localizedGuidanceHeading"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty()),
                "{locale:?}"
            );
            assert!(
                diagnostic.data.as_ref().unwrap()["localizedGuidance"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty()),
                "{locale:?}"
            );
        }
    }

    #[test]
    fn every_structured_calc_guidance_is_localized_without_raw_hint_leakage() {
        let keys = [
            "plugin.calc.skill-param-alias",
            "plugin.calc.unterminated-string",
            "plugin.calc.unexpected-character",
            "plugin.calc.unexpected-eof",
            "plugin.calc.unexpected-token",
            "plugin.calc.wrong-arity",
            "plugin.calc.expected-quoted-argument",
            "plugin.calc.expected-dot-identifier",
            "plugin.calc.expected-rparen",
            "plugin.calc.expected-rparen.eof",
            "plugin.calc.expected-rbrack",
            "plugin.calc.expected-rbrack.eof",
            "plugin.calc.expected-colon",
            "plugin.calc.expected-colon.eof",
            "plugin.calc.expected-comma",
            "plugin.calc.expected-comma.eof",
            "plugin.calc.skilldesc-decimal-prefix",
            "plugin.calc.decimal-policy",
            "plugin.calc.prefix-stop",
        ];
        let message_args = args([
            ("code", json!("INTERNAL_CALC_CODE")),
            ("identifier", json!("par12")),
            ("expected", json!("2")),
            ("actual", json!("bad-token")),
            ("insertText", json!(")")),
            ("hint", json!("RAW_ENGLISH_GUIDANCE_SENTINEL")),
            ("consumedPrefix", json!("-6")),
            ("ignoredSuffix", json!(".25")),
            ("alias", json!("par12")),
            ("policyWarning", json!(true)),
        ]);
        let structured_data = json!({
            "kind": "invalid-argument",
            "expected": "2",
            "actual": "bad-token",
            "insertText": ")",
            "hint": "RAW_ENGLISH_GUIDANCE_SENTINEL",
            "suggestion": "pa12",
            "parameter": "Param12"
        });
        for locale in Locale::ALL {
            for key in keys {
                let diagnostic = localized_plugin_diagnostic(
                    locale,
                    key,
                    message_args.clone(),
                    None,
                    Diagnostic {
                        data: Some(structured_data.clone()),
                        ..Diagnostic::default()
                    },
                );
                let data = diagnostic.data.as_ref().unwrap();
                let heading = data["localizedGuidanceHeading"].as_str().unwrap();
                let guidance = data["localizedGuidance"].as_str().unwrap();
                assert!(!heading.is_empty(), "{locale:?} {key}");
                assert!(!guidance.is_empty(), "{locale:?} {key}");
                assert!(!guidance.contains('{'), "{locale:?} {key}: {guidance}");
                assert!(!guidance.contains('}'), "{locale:?} {key}: {guidance}");
                assert!(
                    !diagnostic.message.contains("RAW_ENGLISH_GUIDANCE_SENTINEL"),
                    "{locale:?} {key}: {}",
                    diagnostic.message
                );
                assert!(
                    !diagnostic.message.contains("INTERNAL_CALC_CODE"),
                    "{locale:?} {key}: {}",
                    diagnostic.message
                );
            }
        }
    }

    #[test]
    fn korean_templates_do_not_use_ambiguous_placeholder_particles() {
        let forbidden = ["(은/는)", "(이/가)", "(을/를)", "(으)로"];
        let placeholder_particles = [
            "}은", "}는", "}이", "}가", "}을", "}를", "}`은", "}`는", "}`이", "}`가", "}`을",
            "}`를", "}'은", "}'는", "}'이", "}'가", "}'을", "}'를", "}\"은", "}\"는", "}\"이",
            "}\"가", "}\"을", "}\"를",
        ];
        for key in CATALOG_KEYS {
            let template = korean(key);
            for particle in forbidden {
                assert!(
                    !template.contains(particle),
                    "Korean core template {key} contains ambiguous particle {particle}"
                );
            }
            for particle in placeholder_particles {
                assert!(
                    !template.contains(particle),
                    "Korean core template {key} attaches a particle to placeholder: {particle}"
                );
            }
        }
        for key in BUNDLED_PLUGIN_KEYS {
            let template = plugin_detail_ko(key).expect("Korean plugin template exists");
            for particle in forbidden {
                assert!(
                    !template.contains(particle),
                    "Korean plugin template {key} contains ambiguous particle {particle}"
                );
            }
            for particle in placeholder_particles {
                assert!(
                    !template.contains(particle),
                    "Korean plugin template {key} attaches a particle to placeholder: {particle}"
                );
            }
        }
    }

    #[test]
    fn structured_reference_hints_are_localized() {
        let reference_args = args([
            ("value", json!(" Diablo ")),
            ("file", json!("levels")),
            ("column", json!("Name")),
            ("trimmedValue", json!("Diablo")),
            ("leadingWhitespace", json!(1)),
            ("trailingWhitespace", json!(2)),
        ]);
        for locale in Locale::ALL {
            let text = localize(locale, "diag.reference.unresolved", &reference_args);
            assert!(text.contains("Diablo"), "{locale:?} lost the trimmed value");
            assert!(
                !text.contains("this value has"),
                "{locale:?} received the old validator-authored English note"
            );
        }

        let fixed4_args = args([
            ("value", json!("abc␠")),
            ("effective", json!("abc␠")),
            ("hasSpaceMarker", json!(true)),
            ("hasTabMarker", json!(false)),
        ]);
        let korean = localize(Locale::KoKr, "diag.fixed4_unknown", &fixed4_args);
        assert!(korean.contains("공백"));
        assert!(!korean.contains("space"));
    }

    #[test]
    fn tc_semantic_sentinels_survive_in_every_non_english_locale() {
        let tc_args = args([
            ("treasureClass", json!("Act 1 (H)")),
            ("column", json!("Item1")),
            ("itemColumn", json!("Item1")),
            ("value", json!("ma=70000")),
            ("modifier", json!("ma=70000")),
            ("stored", json!(4464)),
            ("stoppedAt", json!("bad=1")),
            ("utf8ByteLength", json!(64)),
        ]);
        for locale in Locale::ALL
            .into_iter()
            .filter(|locale| *locale != Locale::EnUs)
        {
            let modifier = localize(locale, "plugin.tc-item.modifier-range", &tc_args);
            assert!(modifier.contains("ma=70000"), "{locale:?} lost modifier");
            assert!(modifier.contains("4464"), "{locale:?} lost stored value");
            assert!(modifier.contains("Item1"), "{locale:?} lost item column");

            let suffix = localize(locale, "plugin.tc-item.ignored-suffix", &tc_args);
            assert!(suffix.contains("bad=1"), "{locale:?} lost stopping suffix");
            assert!(suffix.contains("Item1"), "{locale:?} lost stopping column");

            let width = localize(locale, "plugin.tc-item.field-width", &tc_args);
            assert!(width.contains("64"), "{locale:?} lost byte length");
        }
    }

    #[test]
    fn range_and_reference_hovers_do_not_include_source_provenance() {
        for locale in Locale::ALL {
            let range = catalog_template(locale, "hover.range_valid");
            assert!(range.contains("{value}") && range.contains("{stored}"));
            assert!(!range.contains("{source}"));

            let reference = catalog_template(locale, "hover.reference_resolved");
            assert!(reference.contains("{value}") && reference.contains("{stored}"));
            assert!(reference.contains("{file}") && reference.contains("{column}"));
            assert!(!reference.contains("{source}"));
        }
    }

    #[test]
    fn plugin_hovers_omit_source_provenance_and_blank_lines() {
        const SOURCE_FREE_KEYS: &[&str] = &[
            "plugin.cube-input.hover",
            "plugin.cube-output.hover",
            "plugin.item-code.hover",
            "plugin.treasure-class.hover",
        ];
        let hover_args = args([
            ("base", json!("gld")),
            ("code", json!("gld")),
            ("name", json!("Gold")),
            ("kind", json!("item")),
            ("input", json!("")),
            ("modifiers", json!("mul=1024")),
            ("modifierStorage", json!("mul=1024")),
            ("ignoredSuffix", json!("")),
            ("slot", json!(1)),
            ("picks", json!(1)),
            ("probability", json!(1)),
            ("perRollChance", json!("100%")),
            ("atLeastOnceChance", json!("100%")),
            ("sourceFile", json!("SOURCE_FILE_SENTINEL")),
            ("sourceKind", json!("SOURCE_KIND_SENTINEL")),
            ("sourceVersion", json!("SOURCE_VERSION_SENTINEL")),
        ]);

        for locale in Locale::ALL
            .into_iter()
            .filter(|locale| *locale != Locale::EnUs)
        {
            for key in SOURCE_FREE_KEYS {
                let text = localize(locale, key, &hover_args);
                assert!(
                    !text.contains("SOURCE_"),
                    "{locale:?} {key} exposes source data"
                );
                assert!(
                    !text.contains("\n\n"),
                    "{locale:?} {key} contains a blank line"
                );
            }
        }

        let english = localized_plugin_diagnostic(
            Locale::EnUs,
            "plugin.treasure-class.hover",
            hover_args,
            Some("gld\n\nGold\n\nChance: 100%".to_string()),
            Diagnostic::default(),
        );
        assert_eq!(english.message, "gld\nGold\nChance: 100%");
    }

    #[test]
    fn bundled_plugin_sources_do_not_build_source_provenance_lines() {
        const SOURCES: &[&str] = &[
            include_str!("../contrib/d2rdoc/plugins/cubeInputCheck.ts"),
            include_str!("../contrib/d2rdoc/plugins/cubeOutputCheck.ts"),
            include_str!("../contrib/d2rdoc/plugins/itemCodeCheck.ts"),
            include_str!("../contrib/d2rdoc/plugins/propCodeCheck.ts"),
            include_str!("../contrib/d2rdoc/plugins/tcItemCheck.ts"),
        ];
        for source in SOURCES {
            assert!(!source.contains("Source: "));
            assert!(!source.contains("sourceDescription"));
            assert!(!source.contains("sourceKind"));
            assert!(!source.contains("sourceVersion"));
        }
    }
}
