use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const EDITOR_EXTENSIONS: &[&str] = &["txt", "tsv", "tbl", "csv"];

#[derive(Clone, Debug)]
pub struct ScanPolicy {
    extensions: Vec<String>,
    case_insensitive: bool,
}

impl ScanPolicy {
    pub fn standalone(extension: &str) -> Self {
        Self {
            extensions: vec![extension.to_string()],
            case_insensitive: false,
        }
    }

    pub fn editor() -> Self {
        Self {
            extensions: EDITOR_EXTENSIONS
                .iter()
                .map(|value| value.to_string())
                .collect(),
            case_insensitive: true,
        }
    }

    fn includes(&self, path: &Path) -> bool {
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            return false;
        };
        self.extensions.iter().any(|candidate| {
            if self.case_insensitive {
                extension.eq_ignore_ascii_case(candidate)
            } else {
                extension == candidate
            }
        })
    }
}

#[derive(Clone, Debug)]
pub struct ScanFailure {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug)]
pub struct ScanDiscovery {
    pub paths: Vec<PathBuf>,
    pub failures: Vec<ScanFailure>,
}

pub fn collect_data_files(root: &Path, policy: &ScanPolicy) -> io::Result<ScanDiscovery> {
    let canonical_root = fs::canonicalize(root)?;
    let mut paths = Vec::new();
    let mut failures = Vec::new();
    let mut stack = vec![canonical_root.clone()];
    let mut visited_directories = HashSet::new();
    let mut visited_files = HashSet::new();

    while let Some(directory) = stack.pop() {
        let canonical_directory = match fs::canonicalize(&directory) {
            Ok(path) => path,
            Err(error) => {
                failures.push(ScanFailure {
                    path: directory,
                    reason: error.to_string(),
                });
                continue;
            }
        };
        if !canonical_directory.starts_with(&canonical_root) {
            failures.push(ScanFailure {
                path: canonical_directory,
                reason: "directory link resolves outside the workspace root".to_string(),
            });
            continue;
        }
        if !visited_directories.insert(canonical_directory.clone()) {
            continue;
        }
        let entries = match fs::read_dir(&canonical_directory) {
            Ok(entries) => entries,
            Err(error) => {
                failures.push(ScanFailure {
                    path: canonical_directory,
                    reason: error.to_string(),
                });
                continue;
            }
        };
        let mut entries = entries
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry),
                Err(error) => {
                    failures.push(ScanFailure {
                        path: canonical_directory.clone(),
                        reason: error.to_string(),
                    });
                    None
                }
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(fs::DirEntry::path);

        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if !policy.includes(&path) {
                continue;
            }
            let canonical_file = match fs::canonicalize(&path) {
                Ok(path) => path,
                Err(error) => {
                    failures.push(ScanFailure {
                        path,
                        reason: error.to_string(),
                    });
                    continue;
                }
            };
            if !canonical_file.starts_with(&canonical_root) {
                failures.push(ScanFailure {
                    path: canonical_file,
                    reason: "file link resolves outside the workspace root".to_string(),
                });
                continue;
            }
            if visited_files.insert(canonical_file.clone()) {
                paths.push(canonical_file);
            }
        }
    }

    paths.sort();
    failures.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(ScanDiscovery { paths, failures })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_policy_matches_the_editor_extension_set_case_insensitively() {
        let policy = ScanPolicy::editor();
        for path in ["a.txt", "b.TXT", "c.tsv", "d.TBL", "e.csv"] {
            assert!(policy.includes(Path::new(path)), "{path}");
        }
        assert!(!policy.includes(Path::new("ignored.md")));
    }
}
