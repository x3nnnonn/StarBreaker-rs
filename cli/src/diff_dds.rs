use std::path::Path;
use std::sync::Arc;

use rayon::prelude::*;
use starbreaker_dds::DdsFile;
use starbreaker_p4k::{MappedP4k, P4kArchive};

use crate::error::{CliError, Result};
use crate::p4k_compare::{self, ComparisonStatus};

pub fn extract_dds_files(
    p4k: &MappedP4k,
    output: &Path,
    diff_against: Option<&Path>,
) -> Result<()> {
    std::fs::create_dir_all(output)?;

    let entries_to_extract: Vec<String> = if let Some(against) = diff_against {
        let previous_path = p4k_compare::resolve_previous_p4k_path(against);
        if previous_path.is_file() {
            let previous = MappedP4k::open(&previous_path)?;
            let left = p4k_compare::build_file_index(&previous);
            let right = p4k_compare::build_file_index(p4k);
            let paths = p4k_compare::compare_indexes(&left, &right)
                .into_iter()
                .filter(|(path, status)| {
                    matches!(status, ComparisonStatus::Added | ComparisonStatus::Modified)
                        && is_base_dds_entry(path)
                        && !is_mipmap_filename_filtered(path)
                })
                .map(|(path, _)| path)
                .collect::<Vec<_>>();
            eprintln!("Found {} new/modified DDS files to extract.", paths.len());
            paths
        } else {
            eprintln!(
                "[WARN] Previous P4K not found at {}. Extracting all DDS files.",
                previous_path.display()
            );
            base_dds_paths(p4k)
        }
    } else {
        base_dds_paths(p4k)
    };

    let p4k = Arc::new(p4k);
    let output = Arc::new(output.to_path_buf());
    let (processed, failed) = entries_to_extract
        .par_iter()
        .map(|path| extract_one_dds(&p4k, &output, path))
        .fold(
            || (0usize, 0usize),
            |(ok, err), result| match result {
                Ok(()) => (ok + 1, err),
                Err(e) => {
                    eprintln!("[WARN] Failed to extract DDS: {e}");
                    (ok, err + 1)
                }
            },
        )
        .reduce(|| (0, 0), |(a, b), (c, d)| (a + c, b + d));

    eprintln!("Extracted {processed} DDS files ({failed} failed).");
    Ok(())
}

fn extract_one_dds(p4k: &MappedP4k, output: &Path, path: &str) -> Result<()> {
    let load_path = dds_merge_base_path(path);
    let data = read_entry_at_path(p4k, &load_path)?;
    let reader = P4kSiblingReader {
        p4k,
        base_path: load_path,
    };
    let dds = DdsFile::from_split(&data, &reader).or_else(|_| DdsFile::from_bytes(&data))?;
    let png_name = dds_png_output_name(path);
    let out_path = output.join(png_name);
    dds.save_png(&out_path, 0)?;
    Ok(())
}

fn dds_merge_base_path(path: &str) -> String {
    let normalized = path.replace('/', "\\");
    let lower = normalized.to_ascii_lowercase();
    if lower.ends_with(".dds.a") {
        normalized[..normalized.len() - 2].to_string()
    } else {
        normalized
    }
}

fn base_dds_paths(p4k: &MappedP4k) -> Vec<String> {
    p4k
        .entries()
        .iter()
        .filter(|e| is_base_dds_entry(&e.name))
        .map(|e| e.name.replace('/', "\\"))
        .collect()
}

fn is_base_dds_entry(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if !(lower.ends_with(".dds") || lower.ends_with(".dds.a")) {
        return false;
    }
    if lower.ends_with(".ddna.dds") || lower.ends_with(".ddna.dds.n") {
        return false;
    }
    name.chars().last().is_some_and(|c| !c.is_ascii_digit())
}

fn is_mipmap_filename_filtered(path: &str) -> bool {
    let file_name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    file_name.chars().last().is_some_and(|c| c.is_ascii_digit())
}

fn dds_png_output_name(entry_path: &str) -> String {
    let file_name = entry_path.rsplit(['\\', '/']).next().unwrap_or(entry_path);
    let stem = file_name
        .rsplit_once('.')
        .map(|(base, _)| base)
        .unwrap_or(file_name);
    let stem = if stem.to_ascii_lowercase().ends_with(".dds") {
        &stem[..stem.len() - 4]
    } else {
        stem
    };
    format!("{stem}.png")
}

fn read_entry_at_path(p4k: &MappedP4k, full_path: &str) -> Result<Vec<u8>> {
    let path = full_path.replace('/', "\\");
    if let Some(entry) = p4k.entry_case_insensitive(&path) {
        return p4k.read(entry).map_err(CliError::from);
    }

    let lower = path.to_ascii_lowercase();
    for marker in [".socpak\\", ".pak\\"] {
        let Some(pos) = lower.find(marker) else {
            continue;
        };
        let archive_path = &path[..pos + marker.len() - 1];
        let remainder = &path[pos + marker.len()..];
        let archive_entry = p4k
            .entry_case_insensitive(archive_path)
            .ok_or_else(|| CliError::NotFound(format!("archive not found: {archive_path}")))?;
        let archive_data = p4k.read(archive_entry).map_err(CliError::from)?;
        let archive = P4kArchive::from_bytes(&archive_data).map_err(CliError::from)?;
        return read_entry_in_archive(&archive, remainder);
    }

    Err(CliError::NotFound(format!("P4k entry not found: {path}")))
}

fn read_entry_in_archive(archive: &P4kArchive<'_>, path: &str) -> Result<Vec<u8>> {
    let path = path.replace('/', "\\");
    let lower = path.to_ascii_lowercase();
    for marker in [".socpak\\", ".pak\\"] {
        let Some(pos) = lower.find(marker) else {
            continue;
        };
        let archive_name = &path[..pos + marker.len() - 1];
        let remainder = &path[pos + marker.len()..];
        let inner_entry = archive
            .entries()
            .iter()
            .find(|e| paths_equal(&e.name, archive_name))
            .ok_or_else(|| CliError::NotFound(format!("{archive_name} in archive")))?;
        let inner_data = archive.read(inner_entry).map_err(CliError::from)?;
        let inner = P4kArchive::from_bytes(&inner_data).map_err(CliError::from)?;
        return read_entry_in_archive(&inner, remainder);
    }

    let entry = archive
        .entries()
        .iter()
        .find(|e| paths_equal(&e.name, &path))
        .ok_or_else(|| CliError::NotFound(format!("{path} in archive")))?;
    archive.read(entry).map_err(CliError::from)
}

fn paths_equal(a: &str, b: &str) -> bool {
    a.replace('/', "\\").eq_ignore_ascii_case(&b.replace('/', "\\"))
}

struct P4kSiblingReader<'a> {
    p4k: &'a MappedP4k,
    base_path: String,
}

impl starbreaker_dds::ReadSibling for P4kSiblingReader<'_> {
    fn read_sibling(&self, suffix: &str) -> Option<Vec<u8>> {
        let path = format!("{}{suffix}", self.base_path);
        read_entry_at_path(self.p4k, &path).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_dds_entry_filters_match_csharp() {
        assert!(is_base_dds_entry(r"Data\foo\bar.dds"));
        assert!(is_base_dds_entry(r"Data\foo\bar.dds.a"));
        assert!(!is_base_dds_entry(r"Data\foo\bar.dds.1"));
        assert!(!is_base_dds_entry(r"Data\foo\bar.ddna.dds"));
        assert!(!is_base_dds_entry(r"Data\foo\bar.ddna.dds.n"));
    }

    #[test]
    fn png_output_name_strips_dds_suffix() {
        assert_eq!(
            dds_png_output_name(r"Data\Textures\ship_hull.dds"),
            "ship_hull.png"
        );
        assert_eq!(
            dds_png_output_name(r"Data\Textures\ship_hull.dds.a"),
            "ship_hull.png"
        );
    }

    #[test]
    fn dds_merge_base_path_strips_gloss_suffix() {
        assert_eq!(
            dds_merge_base_path(r"Data\Textures\ship_hull.dds.a"),
            r"Data\Textures\ship_hull.dds"
        );
        assert_eq!(
            dds_merge_base_path(r"Data\Textures\ship_hull.dds"),
            r"Data\Textures\ship_hull.dds"
        );
    }

    #[test]
    fn compare_indexes_detects_modified_crc() {
        use std::collections::HashMap;

        use crate::p4k_compare::{self, EntryMeta};

        let mut left = HashMap::new();
        let mut right = HashMap::new();
        left.insert(
            r"Data\a.dds".to_string(),
            EntryMeta {
                crc32: 1,
                uncompressed_size: 100,
            },
        );
        right.insert(
            r"Data\a.dds".to_string(),
            EntryMeta {
                crc32: 2,
                uncompressed_size: 100,
            },
        );
        let result = p4k_compare::compare_indexes(&left, &right);
        assert_eq!(result[0].1, ComparisonStatus::Modified);
    }
}
