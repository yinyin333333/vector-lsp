/// Format a schema description string for display in LSP hover output.
///
/// Handles two transformations:
/// - `<br>` / `<br/>` / `<br />` HTML line-break tags → Markdown double newline
/// - `$!file#field!$` cross-reference syntax → Markdown emphasis/code formatting
///   - `$!enums#EARMORTYPE!$`  →  `` `EARMORTYPE` (in *enums*) ``
///   - `$!monstats!$`          →  `*monstats*`
///   - `$!#Id!$`               →  `` `Id` ``
pub fn format_description(text: &str) -> String {
    let text = text
        .replace("<br />", "\n\n")
        .replace("<br/>", "\n\n")
        .replace("<br>", "\n\n");

    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();

    while let Some(start) = rest.find("$!") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("!$") {
            Some(end) => {
                push_cross_ref(&after[..end], &mut out);
                rest = &after[end + 2..];
            }
            None => {
                out.push_str("$!");
                rest = after;
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn push_cross_ref(content: &str, out: &mut String) {
    match content.find('#') {
        Some(i) => {
            let file = &content[..i];
            let field = &content[i + 1..];
            match (file.is_empty(), field.is_empty()) {
                (false, false) => out.push_str(&format!("`{}` (in *{}*)", field, file)),
                (false, true) => out.push_str(&format!("*{}*", file)),
                (true, false) => out.push_str(&format!("`{}`", field)),
                (true, true) => {}
            }
        }
        None if !content.is_empty() => out.push_str(&format!("*{}*", content)),
        _ => {}
    }
}
