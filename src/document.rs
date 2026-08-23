use tower_lsp::lsp_types::Range;

/// Return the length of `text` in LSP's default UTF-16 code units.
pub fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

/// Convert an LSP UTF-16 offset to a UTF-8 byte index.
///
/// Offsets beyond the line clamp to its end. An invalid offset between the two
/// code units of a supplementary character clamps to that character's start,
/// so malformed client positions never split a UTF-8 code point or panic.
pub fn utf16_offset_to_byte_index(text: &str, offset: u32) -> usize {
    let mut utf16_offset = 0u32;
    for (byte_index, character) in text.char_indices() {
        let next_offset = utf16_offset + character.len_utf16() as u32;
        if offset < next_offset {
            return byte_index;
        }
        if offset == next_offset {
            return byte_index + character.len_utf8();
        }
        utf16_offset = next_offset;
    }
    text.len()
}

/// A single cell in a delimited row.
/// `col_start` is the UTF-16 character offset of the cell's value within its line,
/// used as the anchor for LSP diagnostic ranges. Sub-cell parsers add their own
/// relative offsets to this value to produce precise positions.
#[derive(PartialEq, Eq)]
pub struct Cell {
    pub value: String,
    pub col_start: u32,
}

#[derive(PartialEq, Eq)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// 0-based line number within the file.
    pub line: u32,
}

#[derive(PartialEq, Eq)]
pub struct DocumentData {
    pub headers: Vec<String>,
    pub rows: Vec<Row>,
    delimiter_utf16_len: u32,
}

impl DocumentData {
    /// Parse a delimited text document into rows and cells.
    /// Handles LF, CRLF, and bare CR line endings used by the editor's text
    /// codec.
    pub fn parse(text: &str, delimiter: char) -> Self {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut line_iter = normalized.lines().enumerate();
        let delimiter_utf16_len = delimiter.len_utf16() as u32;

        let headers = match line_iter.next() {
            Some((_, header_line)) => header_line
                .split(delimiter)
                .map(|s| s.to_string())
                .collect(),
            None => {
                return Self {
                    headers: vec![],
                    rows: vec![],
                    delimiter_utf16_len,
                };
            }
        };

        let rows = line_iter
            .map(|(line_num, line)| {
                let mut cells = Vec::new();
                let mut col_start: u32 = 0;
                for field in line.split(delimiter) {
                    cells.push(Cell {
                        value: field.to_string(),
                        col_start,
                    });
                    // LSP positions use UTF-16 code units for both field content and
                    // the delimiter that separates it from the following field.
                    col_start += utf16_len(field) + delimiter_utf16_len;
                }
                Row {
                    cells,
                    line: line_num as u32,
                }
            })
            .collect();

        Self {
            headers,
            rows,
            delimiter_utf16_len,
        }
    }

    /// Return the column index whose header starts at or before `character` in line 0,
    /// using the same "last winner" rule as `cell_at`. Returns None if headers is empty.
    pub fn header_at(&self, character: u32) -> Option<usize> {
        let mut col_start = 0u32;
        let mut found = None;
        for (i, header) in self.headers.iter().enumerate() {
            if col_start <= character {
                found = Some(i);
            } else {
                break;
            }
            col_start += utf16_len(header) + self.delimiter_utf16_len;
        }
        found
    }

    /// Return the UTF-16 start and end offsets for a header cell.
    pub fn header_span(&self, index: usize) -> Option<(u32, u32)> {
        let header = self.headers.get(index)?;
        let start = self.headers[..index]
            .iter()
            .map(|value| utf16_len(value) + self.delimiter_utf16_len)
            .sum();
        Some((start, start + utf16_len(header)))
    }

    /// Return the data row at the given 0-based document line. Parsed rows are
    /// contiguous from line 1; verify the stored line to reject malformed data.
    pub fn row_at(&self, line: u32) -> Option<&Row> {
        let row = self.rows.get(line.checked_sub(1)? as usize)?;
        (row.line == line).then_some(row)
    }

    /// Return the (column_index, &Cell) for the given cursor position, or None if
    /// the position is not within any data row (e.g. cursor is on the header line).
    ///
    /// Finds the last cell whose `col_start` is ≤ `character`, so a cursor sitting
    /// on a trailing delimiter is attributed to the preceding cell.
    pub fn cell_at(&self, line: u32, character: u32) -> Option<(usize, &Cell)> {
        let row = self.row_at(line)?;
        let mut found = None;
        for (i, cell) in row.cells.iter().enumerate() {
            if cell.col_start <= character {
                found = Some((i, cell));
            } else {
                break;
            }
        }
        found
    }
}

/// Rebuild normalized delimited text from a parsed document so incremental
/// changes can be applied when the original source buffer is unavailable.
pub(crate) fn reconstruct_text(document: &DocumentData, delimiter: char) -> String {
    let delimiter = delimiter.to_string();
    let header = document.headers.join(&delimiter);
    let rows = document.rows.iter().map(|row| {
        row.cells
            .iter()
            .map(|cell| cell.value.as_str())
            .collect::<Vec<_>>()
            .join(&delimiter)
    });
    std::iter::once(header)
        .chain(rows)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Normalize the line-ending forms accepted by the editor into an LSP change
/// buffer without discarding a final empty line.
pub(crate) fn split_text_lines(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(str::to_owned)
        .collect()
}

/// Apply one LSP incremental content change using UTF-16 positions.
pub(crate) fn apply_change(lines: &mut Vec<String>, range: Range, new_text: &str) {
    let start_line = range.start.line as usize;
    let start_character = range.start.character;
    let end_line = range.end.line as usize;
    let end_character = range.end.character;

    let prefix = lines
        .get(start_line)
        .map(|line| &line[..utf16_offset_to_byte_index(line, start_character)])
        .unwrap_or_default();
    let suffix = lines
        .get(end_line)
        .map(|line| &line[utf16_offset_to_byte_index(line, end_character)..])
        .unwrap_or_default();

    let normalized_new_text = new_text.replace("\r\n", "\n").replace('\r', "\n");
    let new_lines: Vec<&str> = normalized_new_text.split('\n').collect();
    let replacement = match new_lines.as_slice() {
        [] | [""] => vec![format!("{prefix}{suffix}")],
        [only] => vec![format!("{prefix}{}{suffix}", only.trim_end_matches('\r'))],
        [first, rest @ ..] => {
            let mut replacement = vec![format!("{prefix}{}", first.trim_end_matches('\r'))];
            for middle in &rest[..rest.len() - 1] {
                replacement.push(middle.trim_end_matches('\r').to_string());
            }
            replacement.push(format!(
                "{}{suffix}",
                rest.last().unwrap().trim_end_matches('\r')
            ));
            replacement
        }
    };

    while lines.len() <= end_line {
        lines.push(String::new());
    }
    lines.splice(start_line..=end_line, replacement);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::Position;

    fn apply(text: &str, start: u32, end: u32, replacement: &str) -> String {
        let mut lines = vec![text.to_string()];
        apply_change(
            &mut lines,
            Range::new(Position::new(0, start), Position::new(0, end)),
            replacement,
        );
        lines.join("\n")
    }

    #[test]
    fn cell_and_header_offsets_use_utf16_code_units() {
        let document = DocumentData::parse("🙂\tsecond\n🙂\tvalue", '\t');

        assert_eq!(document.rows[0].cells[1].col_start, 3);
        assert_eq!(document.header_at(2), Some(0));
        assert_eq!(document.header_at(3), Some(1));
        assert_eq!(document.header_span(1), Some((3, 9)));
        assert!(document.row_at(0).is_none());
        assert_eq!(document.row_at(1).map(|row| row.line), Some(1));
        assert!(document.row_at(2).is_none());
        assert_eq!(document.cell_at(1, 2).map(|(index, _)| index), Some(0));
        assert_eq!(document.cell_at(1, 3).map(|(index, _)| index), Some(1));
    }

    #[test]
    fn parse_accepts_bare_carriage_return_line_endings() {
        let document = DocumentData::parse("code\rvalue\rnext", '\t');

        assert_eq!(document.headers, vec!["code"]);
        assert_eq!(document.rows.len(), 2);
        assert_eq!(document.rows[0].line, 1);
        assert_eq!(document.rows[0].cells[0].value, "value");
        assert_eq!(document.rows[1].line, 2);
        assert_eq!(document.rows[1].cells[0].value, "next");
    }

    #[test]
    fn reconstruct_text_preserves_parsed_headers_rows_and_delimiter() {
        let document = DocumentData::parse("name|value\nfirst|one\nsecond|two", '|');

        assert_eq!(
            reconstruct_text(&document, '|'),
            "name|value\nfirst|one\nsecond|two"
        );
    }

    #[test]
    fn incremental_changes_use_utf16_offsets_around_supplementary_characters() {
        assert_eq!(apply("A🙂B", 1, 1, "X"), "AX🙂B");
        assert_eq!(apply("A🙂B", 3, 3, "X"), "A🙂XB");
        assert_eq!(apply("A🙂B", 1, 3, ""), "AB");
        assert_eq!(apply("A🙂B\told", 5, 8, "new"), "A🙂B\tnew");
    }

    #[test]
    fn incremental_changes_keep_bare_carriage_return_lines() {
        let mut lines = split_text_lines("[\r  {\"id\":1}\r]");
        apply_change(
            &mut lines,
            Range::new(Position::new(1, 2), Position::new(1, 2)),
            "X",
        );

        assert_eq!(lines.join("\n"), "[\n  X{\"id\":1}\n]");
    }

    #[test]
    fn invalid_half_surrogate_offsets_clamp_to_the_code_point_start() {
        assert_eq!(apply("A🙂B", 2, 2, "X"), "AX🙂B");
    }
}
