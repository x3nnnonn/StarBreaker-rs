use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use starbreaker_p4k::{MappedP4k, P4kArchive};

use crate::error::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComparisonStatus {
    Added,
    Removed,
    Modified,
    Unchanged,
}

#[derive(Clone, Copy)]
pub struct EntryMeta {
    pub crc32: u32,
    pub uncompressed_size: u64,
}

pub fn resolve_previous_p4k_path(diff_against: &Path) -> PathBuf {
    if diff_against.is_file() {
        return diff_against.to_path_buf();
    }
    if diff_against.is_dir() {
        if let Ok(entries) = std::fs::read_dir(diff_against) {
            if let Some(p4k) = entries
                .flatten()
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("p4k")))
            {
                return p4k;
            }
        }

        let p4k_dump_dir = diff_against.join("P4k");
        if p4k_dump_dir.is_dir() {
            if let Some(latest) = find_latest_p4k_recursive(&p4k_dump_dir) {
                return latest;
            }
        }
    }
    diff_against.to_path_buf()
}

fn find_latest_p4k_recursive(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(PathBuf, std::time::SystemTime)> = None;
    find_latest_p4k_recursive_inner(dir, &mut best);
    best.map(|(path, _)| path)
}

fn find_latest_p4k_recursive_inner(dir: &Path, best: &mut Option<(PathBuf, std::time::SystemTime)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            find_latest_p4k_recursive_inner(&path, best);
        } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("p4k")) {
            if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                let replace = best
                    .as_ref()
                    .map(|(_, t)| modified > *t)
                    .unwrap_or(true);
                if replace {
                    *best = Some((path, modified));
                }
            }
        }
    }
}

pub fn build_file_index(p4k: &MappedP4k) -> HashMap<String, EntryMeta> {
    let mut index = HashMap::new();
    add_entries_to_index(p4k, "", &mut index);
    index
}

fn add_entries_to_index(p4k: &MappedP4k, prefix: &str, index: &mut HashMap<String, EntryMeta>) {
    for entry in p4k.entries() {
        let path = if prefix.is_empty() {
            entry.name.clone()
        } else {
            format!("{prefix}\\{}", entry.name)
        };
        let lower = entry.name.to_ascii_lowercase();
        let is_archive = lower.ends_with(".socpak") || lower.ends_with(".pak");
        let is_shadercache = lower.contains("shadercache_");

        if !is_archive || is_shadercache {
            index.insert(
                path.clone(),
                EntryMeta {
                    crc32: entry.crc32,
                    uncompressed_size: entry.uncompressed_size,
                },
            );
        }

        if is_archive && !is_shadercache {
            let Ok(data) = p4k.read(entry) else { continue };
            let Ok(inner) = P4kArchive::from_bytes(&data) else { continue };
            add_archive_entries_to_index(&inner, &path, index);
        }
    }
}

fn add_archive_entries_to_index(
    archive: &P4kArchive<'_>,
    prefix: &str,
    index: &mut HashMap<String, EntryMeta>,
) {
    for entry in archive.entries() {
        let path = format!("{prefix}\\{}", entry.name);
        let lower = entry.name.to_ascii_lowercase();
        let is_archive = lower.ends_with(".socpak") || lower.ends_with(".pak");
        let is_shadercache = lower.contains("shadercache_");

        if !is_archive || is_shadercache {
            index.insert(
                path.clone(),
                EntryMeta {
                    crc32: entry.crc32,
                    uncompressed_size: entry.uncompressed_size,
                },
            );
        }

        if is_archive && !is_shadercache {
            let Ok(data) = archive.read(entry) else { continue };
            let Ok(inner) = P4kArchive::from_bytes(&data) else { continue };
            add_archive_entries_to_index(&inner, &path, index);
        }
    }
}

pub fn compare_indexes(
    left: &HashMap<String, EntryMeta>,
    right: &HashMap<String, EntryMeta>,
) -> Vec<(String, ComparisonStatus)> {
    let paths: BTreeSet<_> = left.keys().chain(right.keys()).cloned().collect();
    paths
        .into_iter()
        .map(|path| {
            let status = match (left.get(&path), right.get(&path)) {
                (None, Some(_)) => ComparisonStatus::Added,
                (Some(_), None) => ComparisonStatus::Removed,
                (Some(l), Some(r)) => {
                    if l.crc32 != r.crc32 || l.uncompressed_size != r.uncompressed_size {
                        ComparisonStatus::Modified
                    } else {
                        ComparisonStatus::Unchanged
                    }
                }
                (None, None) => ComparisonStatus::Unchanged,
            };
            (path, status)
        })
        .collect()
}

pub fn list_added_socpak_paths(current: &MappedP4k, previous: &MappedP4k) -> Vec<String> {
    let previous_paths: Vec<String> = previous
        .entries()
        .iter()
        .filter(|e| is_socpak_path(&e.name))
        .map(|e| e.name.replace('/', "\\"))
        .collect();

    let mut added: Vec<String> = current
        .entries()
        .iter()
        .filter(|e| is_socpak_path(&e.name))
        .map(|e| e.name.replace('/', "\\"))
        .filter(|path| {
            !previous_paths
                .iter()
                .any(|prev| prev.eq_ignore_ascii_case(path))
        })
        .collect();

    added.sort_by(|a, b| a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()));
    added
}

pub fn write_added_socpak_list(
    current: &MappedP4k,
    previous: &MappedP4k,
    output: &Path,
) -> Result<usize> {
    let added = list_added_socpak_paths(current, previous);
    let out_file = output.join("New_SOCPAK_Files.txt");
    if added.is_empty() {
        std::fs::write(&out_file, "")?;
        eprintln!("No new SOCPAK / object-container files found.");
        return Ok(0);
    }
    let mut content = added.join("\n");
    content.push('\n');
    std::fs::write(&out_file, &content)?;
    eprintln!(
        "Found {} new SOCPAK / object-container files. Written to {}",
        added.len(),
        out_file.display()
    );
    Ok(added.len())
}

pub fn is_socpak_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.contains("shadercache_") {
        return false;
    }
    let file_name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let name = file_name.to_ascii_lowercase();
    name.ends_with(".socpak") || name.ends_with(".pak")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_socpak_path_matches_csharp() {
        assert!(is_socpak_path(r"Data\ObjectContainers\ships\foo.socpak"));
        assert!(is_socpak_path(r"Data\Levels\bar.pak"));
        assert!(!is_socpak_path(r"Data\shadercache_foo.socpak"));
        assert!(!is_socpak_path(r"Data\Textures\baz.dds"));
    }
}
