use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::document::DocumentData;
use crate::schema::Schema;

/// Cross-file symbol index.
///
/// Key: `(file_stem, column_name, cell_value)` — all three components lowercased.
/// Value: the LSP Location of that cell in the workspace.
///
/// Only columns that are `reference` targets in the schema are stored, keeping
/// memory usage proportional to what go-to-definition actually needs.
#[derive(Clone)]
pub struct SymbolIndex {
    entries: HashMap<(String, String, String), Location>,
    columns: HashSet<(String, String)>,
    files: HashSet<String>,
}

impl SymbolIndex {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            columns: HashSet::new(),
            files: HashSet::new(),
        }
    }

    /// Index the cells of `doc` that belong to columns listed in `ref_targets`.
    /// Call `remove_file` first if re-indexing an already-indexed file.
    pub fn index_document(
        &mut self,
        uri: &Url,
        file_stem: &str,
        doc: &DocumentData,
        ref_targets: &HashSet<(String, String)>,
    ) {
        let stem = file_stem.to_lowercase();
        self.files.insert(stem.clone());
        for header in &doc.headers {
            if !header.is_empty() {
                self.columns.insert((stem.clone(), header.to_lowercase()));
            }
        }
        for row in &doc.rows {
            if row
                .cells
                .first()
                .map(|cell| cell.value.trim_start().starts_with('*'))
                .unwrap_or(false)
            {
                continue;
            }
            for (col_idx, cell) in row.cells.iter().enumerate() {
                if cell.value.trim().is_empty() {
                    continue;
                }
                let col_name = match doc.headers.get(col_idx) {
                    Some(h) => h.as_str(),
                    None => continue,
                };
                let col_lower = col_name.to_lowercase();
                if !ref_targets.contains(&(stem.clone(), col_lower.clone())) {
                    continue;
                }
                let end_char = cell.col_start + cell.value.chars().count() as u32;
                self.entries.insert(
                    (stem.clone(), col_lower, cell.value.to_lowercase()),
                    Location {
                        uri: uri.clone(),
                        range: Range {
                            start: Position {
                                line: row.line,
                                character: cell.col_start,
                            },
                            end: Position {
                                line: row.line,
                                character: end_char,
                            },
                        },
                    },
                );
            }
        }
    }

    /// Remove all index entries for a file stem. Call before re-indexing after a change.
    pub fn remove_file(&mut self, file_stem: &str) {
        let stem = file_stem.to_lowercase();
        self.entries.retain(|(f, _, _), _| *f != stem);
        self.columns.retain(|(f, _)| *f != stem);
        self.files.remove(&stem);
    }

    /// Look up the location of a specific value in a specific column of a specific file.
    pub fn lookup(&self, file_stem: &str, column: &str, value: &str) -> Option<&Location> {
        self.entries.get(&(
            file_stem.to_lowercase(),
            column.to_lowercase(),
            value.to_lowercase(),
        ))
    }

    pub fn has_file(&self, file_stem: &str) -> bool {
        self.files.contains(&file_stem.to_lowercase())
    }

    pub fn has_column(&self, file_stem: &str, column: &str) -> bool {
        self.columns
            .contains(&(file_stem.to_lowercase(), column.to_lowercase()))
    }
}

pub struct Workspace {
    pub root_uri: Option<Url>,
    /// Documents currently open in the editor (managed via didOpen/didChange).
    pub open_documents: HashMap<Url, Arc<DocumentData>>,
    /// All other workspace files parsed from disk on startup.
    pub file_cache: HashMap<PathBuf, Arc<DocumentData>>,
    pub symbols: SymbolIndex,
    /// Schema loaded from the configured schema directory, if any.
    /// Stored as `Arc` so it can be shared cheaply with the plugin host.
    pub schema: Option<Arc<Schema>>,
    /// Cached set of `(file_stem, column_name)` reference targets derived from the schema.
    /// Drives what SymbolIndex stores — populated once when the schema loads.
    pub ref_targets: HashSet<(String, String)>,
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            root_uri: None,
            open_documents: HashMap::new(),
            file_cache: HashMap::new(),
            symbols: SymbolIndex::new(),
            schema: None,
            ref_targets: HashSet::new(),
        }
    }
}
