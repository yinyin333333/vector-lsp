//! Locale-aware descriptions for bundled D2RDoc schema fields.
//!
//! The upstream schema documentation is English-only. Product UI must not mix
//! that English prose into a non-English session. Carefully reviewed semantic
//! overlays are used where available; every other bundled field receives a
//! localized description derived from its declared type/reference contract.
//! Explicit user-provided schemas remain author-owned content and are shown
//! verbatim.

use crate::i18n::Locale;
use crate::schema::{FieldType, FieldTypeName, SchemaField, format_description};

pub fn localized_field_description(
    locale: Locale,
    file_stem: &str,
    field: &SchemaField,
    bundled_schema: bool,
) -> Option<String> {
    let original = field.description.as_deref()?;
    if locale == Locale::EnUs || !bundled_schema {
        return Some(format_description(original));
    }

    if file_stem.eq_ignore_ascii_case("actinfo")
        && field.name.eq_ignore_ascii_case("maxnpcitemlevel")
    {
        return Some(max_npc_item_level(locale).to_string());
    }

    field
        .field_type
        .as_ref()
        .map(|field_type| structural_description(locale, field_type))
}

fn max_npc_item_level(locale: Locale) -> &'static str {
    match locale {
        Locale::EnUs => unreachable!(),
        Locale::ZhTw => "設定此 Act 中 NPC 販售物品的最高物品等級。",
        Locale::DeDe => {
            "Legt die maximale Gegenstandsstufe für Gegenstände fest, die NPCs in diesem Akt verkaufen."
        }
        Locale::EsEs | Locale::EsMx => {
            "Define el nivel máximo de objeto de los objetos que venden los PNJ en este acto."
        }
        Locale::FrFr => {
            "Définit le niveau d’objet maximal des objets vendus par les PNJ dans cet acte."
        }
        Locale::ItIt => {
            "Imposta il livello oggetto massimo degli oggetti venduti dai PNG in questo atto."
        }
        Locale::KoKr => "해당 액트에서 NPC가 판매하는 아이템의 최대 아이템 레벨을 설정합니다.",
        Locale::PlPl => {
            "Określa maksymalny poziom przedmiotów sprzedawanych przez NPC w tym akcie."
        }
        Locale::JaJp => "この Act で NPC が販売するアイテムの最大アイテムレベルを設定します。",
        Locale::PtBr => "Define o nível máximo dos itens vendidos pelos NPCs neste ato.",
        Locale::RuRu => "Задаёт максимальный уровень предметов, продаваемых NPC в этом акте.",
        Locale::ZhCn => "设置此 Act 中 NPC 出售物品的最高物品等级。",
    }
}

fn structural_description(locale: Locale, field_type: &FieldType) -> String {
    if field_type.type_name == FieldTypeName::Reference
        && let (Some(file), Some(column)) = (&field_type.file, &field_type.field)
    {
        return match locale {
            Locale::EnUs => unreachable!(),
            Locale::ZhTw => format!("參照 `{file}.{column}` 中的值。"),
            Locale::DeDe => format!("Verweist auf einen Wert in `{file}.{column}`."),
            Locale::EsEs | Locale::EsMx => {
                format!("Hace referencia a un valor de `{file}.{column}`.")
            }
            Locale::FrFr => format!("Référence une valeur de `{file}.{column}`."),
            Locale::ItIt => format!("Fa riferimento a un valore di `{file}.{column}`."),
            Locale::KoKr => format!("`{file}.{column}`의 값을 참조하는 열입니다."),
            Locale::PlPl => format!("Odwołuje się do wartości z `{file}.{column}`."),
            Locale::JaJp => format!("`{file}.{column}` の値を参照します。"),
            Locale::PtBr => format!("Referencia um valor de `{file}.{column}`."),
            Locale::RuRu => format!("Ссылается на значение из `{file}.{column}`."),
            Locale::ZhCn => format!("引用 `{file}.{column}` 中的值。"),
        };
    }

    let kind = match field_type.type_name {
        FieldTypeName::Int => 0,
        FieldTypeName::Float => 1,
        FieldTypeName::Boolean => 2,
        FieldTypeName::Parse => 3,
        FieldTypeName::Text | FieldTypeName::String => 4,
        FieldTypeName::Array | FieldTypeName::Object => 5,
        FieldTypeName::Reference | FieldTypeName::Comment | FieldTypeName::Unknown => 6,
    };
    match locale {
        Locale::EnUs => unreachable!(),
        Locale::ZhTw => [
            "此欄使用整數值。",
            "此欄使用數值。",
            "此欄使用布林值。",
            "此欄使用 calc 運算式。",
            "此欄使用文字值。",
            "此欄使用結構化值。",
            "此欄包含遊戲資料值。",
        ][kind]
            .into(),
        Locale::DeDe => [
            "Diese Spalte verwendet einen Ganzzahlwert.",
            "Diese Spalte verwendet einen numerischen Wert.",
            "Diese Spalte verwendet einen booleschen Wert.",
            "Diese Spalte verwendet einen calc-Ausdruck.",
            "Diese Spalte verwendet einen Textwert.",
            "Diese Spalte verwendet einen strukturierten Wert.",
            "Diese Spalte enthält einen Spieldatenwert.",
        ][kind]
            .into(),
        Locale::EsEs | Locale::EsMx => [
            "Esta columna utiliza un valor entero.",
            "Esta columna utiliza un valor numérico.",
            "Esta columna utiliza un valor booleano.",
            "Esta columna utiliza una expresión calc.",
            "Esta columna utiliza un valor de texto.",
            "Esta columna utiliza un valor estructurado.",
            "Esta columna contiene un valor de datos del juego.",
        ][kind]
            .into(),
        Locale::FrFr => [
            "Cette colonne utilise une valeur entière.",
            "Cette colonne utilise une valeur numérique.",
            "Cette colonne utilise une valeur booléenne.",
            "Cette colonne utilise une expression calc.",
            "Cette colonne utilise une valeur textuelle.",
            "Cette colonne utilise une valeur structurée.",
            "Cette colonne contient une valeur de données du jeu.",
        ][kind]
            .into(),
        Locale::ItIt => [
            "Questa colonna usa un valore intero.",
            "Questa colonna usa un valore numerico.",
            "Questa colonna usa un valore booleano.",
            "Questa colonna usa un'espressione calc.",
            "Questa colonna usa un valore di testo.",
            "Questa colonna usa un valore strutturato.",
            "Questa colonna contiene un valore dei dati di gioco.",
        ][kind]
            .into(),
        Locale::KoKr => [
            "정수 값을 입력하는 열입니다.",
            "숫자 값을 입력하는 열입니다.",
            "불리언 값을 입력하는 열입니다.",
            "calc 표현식을 입력하는 열입니다.",
            "텍스트 값을 입력하는 열입니다.",
            "구조화된 값을 입력하는 열입니다.",
            "게임 데이터 값을 입력하는 열입니다.",
        ][kind]
            .into(),
        Locale::PlPl => [
            "Ta kolumna używa wartości całkowitej.",
            "Ta kolumna używa wartości liczbowej.",
            "Ta kolumna używa wartości logicznej.",
            "Ta kolumna używa wyrażenia calc.",
            "Ta kolumna używa wartości tekstowej.",
            "Ta kolumna używa wartości strukturalnej.",
            "Ta kolumna zawiera wartość danych gry.",
        ][kind]
            .into(),
        Locale::JaJp => [
            "この列には整数値を入力します。",
            "この列には数値を入力します。",
            "この列にはブール値を入力します。",
            "この列には calc 式を入力します。",
            "この列にはテキスト値を入力します。",
            "この列には構造化された値を入力します。",
            "この列にはゲームデータの値を入力します。",
        ][kind]
            .into(),
        Locale::PtBr => [
            "Esta coluna usa um valor inteiro.",
            "Esta coluna usa um valor numérico.",
            "Esta coluna usa um valor booleano.",
            "Esta coluna usa uma expressão calc.",
            "Esta coluna usa um valor de texto.",
            "Esta coluna usa um valor estruturado.",
            "Esta coluna contém um valor de dados do jogo.",
        ][kind]
            .into(),
        Locale::RuRu => [
            "В этом столбце используется целое число.",
            "В этом столбце используется числовое значение.",
            "В этом столбце используется логическое значение.",
            "В этом столбце используется выражение calc.",
            "В этом столбце используется текстовое значение.",
            "В этом столбце используется структурированное значение.",
            "В этом столбце содержится значение игровых данных.",
        ][kind]
            .into(),
        Locale::ZhCn => [
            "此列使用整数值。",
            "此列使用数值。",
            "此列使用布尔值。",
            "此列使用 calc 表达式。",
            "此列使用文本值。",
            "此列使用结构化值。",
            "此列包含游戏数据值。",
        ][kind]
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, field_type: FieldType) -> SchemaField {
        SchemaField {
            name: name.to_string(),
            description: Some("English schema documentation".to_string()),
            field_type: Some(field_type),
            alt_names: Vec::new(),
            append_field: None,
            table: None,
            unique: false,
        }
    }

    #[test]
    fn bundled_non_english_headers_never_fall_back_to_english_schema_prose() {
        let int_field = field(
            "maxnpcitemlevel",
            FieldType {
                type_name: FieldTypeName::Int,
                data_length: 0,
                mem_size: 32,
                file: None,
                field: None,
                resolver: Default::default(),
                unknown_policy: Default::default(),
            },
        );
        for locale in Locale::ALL
            .into_iter()
            .filter(|locale| *locale != Locale::EnUs)
        {
            let description = localized_field_description(locale, "actinfo", &int_field, true)
                .expect("bundled field description");
            assert!(!description.contains("English schema documentation"));
            assert!(!description.is_empty());
        }
    }

    #[test]
    fn external_schema_descriptions_remain_author_owned_content() {
        let int_field = field(
            "custom",
            FieldType {
                type_name: FieldTypeName::Int,
                data_length: 0,
                mem_size: 32,
                file: None,
                field: None,
                resolver: Default::default(),
                unknown_policy: Default::default(),
            },
        );
        assert_eq!(
            localized_field_description(Locale::KoKr, "custom", &int_field, false).as_deref(),
            Some("English schema documentation")
        );
    }
}
