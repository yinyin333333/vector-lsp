/// A single cell in a delimited row.
/// `col_start` is the UTF-16 character offset of the cell's value within its line,
/// used as the anchor for LSP diagnostic ranges. Sub-cell parsers add their own
/// relative offsets to this value to produce precise positions.
pub struct Cell {
    pub value: String,
    pub col_start: u32,
}

pub struct Row {
    pub cells: Vec<Cell>,
    /// 0-based line number within the file.
    pub line: u32,
}

pub struct DocumentData {
    pub headers: Vec<String>,
    pub rows: Vec<Row>,
}

impl DocumentData {
    /// Parse a delimited text document into rows and cells.
    /// Handles both LF and CRLF line endings (Rust's `str::lines` strips both).
    pub fn parse(text: &str, delimiter: char) -> Self {
        let mut line_iter = text.lines().enumerate();

        let headers = match line_iter.next() {
            Some((_, header_line)) => header_line
                .split(delimiter)
                .map(|s| s.to_string())
                .collect(),
            None => {
                return Self {
                    headers: vec![],
                    rows: vec![],
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
                    // Advance by the field length plus one delimiter character.
                    // For ASCII/UTF-8 sources this equals byte length; for non-ASCII
                    // sources the caller is responsible for ensuring col_start is in
                    // UTF-16 code units as required by the LSP spec.
                    col_start += field.chars().count() as u32 + 1;
                }
                Row {
                    cells,
                    line: line_num as u32,
                }
            })
            .collect();

        Self { headers, rows }
    }

    /// Return the cell at (row_index, col_name), or None if out of bounds.
    pub fn get_cell(&self, row_index: usize, col_name: &str) -> Option<&Cell> {
        let col_index = self.headers.iter().position(|h| h == col_name)?;
        self.rows.get(row_index)?.cells.get(col_index)
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
            col_start += header.chars().count() as u32 + 1;
        }
        found
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
