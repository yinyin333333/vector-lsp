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
    /// Handles both LF and CRLF line endings (Rust's `str::lines` strips both).
    pub fn parse(text: &str, delimiter: char) -> Self {
        let mut line_iter = text.lines().enumerate();
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

    /// Return the (column_index, &Cell) for the given cursor position, or None if
    /// the position is not within any data row (e.g. cursor is on the header line).
    ///
    /// Finds the last cell whose `col_start` is ≤ `character`, so a cursor sitting
    /// on a trailing delimiter is attributed to the preceding cell.
    pub fn cell_at(&self, line: u32, character: u32) -> Option<(usize, &Cell)> {
        let row = self.rows.iter().find(|r| r.line == line)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_and_header_offsets_use_utf16_code_units() {
        let document = DocumentData::parse("🙂\tsecond\n🙂\tvalue", '\t');

        assert_eq!(document.rows[0].cells[1].col_start, 3);
        assert_eq!(document.header_at(2), Some(0));
        assert_eq!(document.header_at(3), Some(1));
        assert_eq!(document.header_span(1), Some((3, 9)));
        assert_eq!(document.cell_at(1, 2).map(|(index, _)| index), Some(0));
        assert_eq!(document.cell_at(1, 3).map(|(index, _)| index), Some(1));
    }
}
