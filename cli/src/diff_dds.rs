use std::path::Path;
use std::sync::Arc;

use rayon::prelude::*;
use starbreaker_dds::DdsFile;
use starbreaker_p4k::{MappedP4k, P4kArchive};

use crate::error::{CliError, Result};
use crate::diff_index::{FileIndex, key};

pub fn extract_dds_files(
    p4k: &MappedP4k,
    output: &Path,
    current: &FileIndex,
    previous: Option<&FileIndex>,
) -> Result<()> {
    std::fs::create_dir_all(output)?;

    let entries_to_extract = changed_dds_paths(current, previous);
    eprintln!("Found {} new/modified DDS files to extract.", entries_to_extract.len());
    let mut names = std::collections::HashMap::new();
    for path in current.keys().filter(|path| path.ends_with(".dds") && is_base_dds_entry(path)) {
        *names.entry(dds_png_output_name(path)).or_insert(0usize) += 1;
    }

    let p4k = Arc::new(p4k);
    let output = Arc::new(output.to_path_buf());
    let (processed, failed) = entries_to_extract
        .par_iter()
        .map(|path| {
            let name = dds_png_output_name(path);
            let relative = if names[&name] > 1 { std::path::PathBuf::from(path).with_extension("png") } else { name.into() };
            let destination = output.join(relative);
            extract_one_dds(&p4k, &destination, path)
        })
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
    if failed != 0 { return Err(CliError::InvalidInput(format!("{failed} DDS exports failed; snapshot index was not advanced"))); }
    Ok(())
}

fn extract_one_dds(p4k: &MappedP4k, destination: &Path, path: &str) -> Result<()> {
    let load_path = dds_merge_base_path(path);
    let data = read_entry_at_path(p4k, &load_path)?;
    let reader = P4kSiblingReader {
        p4k,
        base_path: load_path,
    };
    let dds = DdsFile::from_split(&data, &reader).or_else(|_| DdsFile::from_bytes(&data))?;
    if let Some(parent) = destination.parent() { std::fs::create_dir_all(parent)?; }
    use image::ImageEncoder;
    let pixels = dds.decode_rgba(0)?;
    let (width, height) = dds.dimensions(0);
    let mut output = std::io::BufWriter::new(std::fs::File::create(destination)?);
    image::codecs::png::PngEncoder::new_with_quality(&mut output,
        image::codecs::png::CompressionType::Fast, image::codecs::png::FilterType::Adaptive)
        .write_image(&pixels, width, height, image::ExtendedColorType::Rgba8)?;
    std::io::Write::flush(&mut output)?;
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

pub(crate) fn changed_dds_paths(current: &FileIndex, previous: Option<&FileIndex>) -> Vec<String> {
    let mut changed = std::collections::HashSet::new();
    for (path, meta) in current {
        if previous.is_some_and(|previous| previous.get(path) == Some(meta)) { continue; }
        if let Some(base) = dds_family_base(path) {
            if current.contains_key(&base) && is_base_dds_entry(&base) { changed.insert(base); }
        }
    }
    if let Some(previous) = previous {
        for path in previous.keys().filter(|path| !current.contains_key(*path)) {
            if let Some(base) = dds_family_base(path) {
                if current.contains_key(&base) && is_base_dds_entry(&base) { changed.insert(base); }
            }
        }
    }
    let mut paths: Vec<_> = changed.into_iter().collect();
    paths.sort_unstable();
    paths
}

fn dds_family_base(path: &str) -> Option<String> {
    let path = key(path);
    let start = path.rfind(".dds")?;
    let suffix = &path[start + 4..];
    if suffix.is_empty() || suffix == ".a" || suffix.strip_prefix('.').is_some_and(|suffix| {
        let digits = suffix.strip_suffix('a').unwrap_or(suffix);
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    }) {
        Some(path[..start + 4].to_owned())
    } else { None }
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
        let left = FileIndex::from([(key(r"Data\a.dds"), (1, 100))]);
        let right = FileIndex::from([(key(r"Data\a.dds"), (2, 100))]);
        assert_eq!(changed_dds_paths(&right, Some(&left)), vec!["data/a.dds"]);
    }

    #[test]
    #[ignore = "requires STARBREAKER_DIFF_TEST_P4K and STARBREAKER_DIFF_TEST_OUTPUT"]
    fn live_png_pixels_and_encoding_time() {
        let p4k = MappedP4k::open(Path::new(&std::env::var_os("STARBREAKER_DIFF_TEST_P4K").unwrap())).unwrap();
        let output = std::path::PathBuf::from(std::env::var_os("STARBREAKER_DIFF_TEST_OUTPUT").unwrap());
        std::fs::create_dir_all(&output).unwrap();
        let mut count = 0;
        let mut old_time = std::time::Duration::ZERO;
        let mut new_time = std::time::Duration::ZERO;
        for entry in p4k.entries().iter().filter(|entry| entry.name.to_ascii_lowercase().ends_with(".dds") && is_base_dds_entry(&entry.name)) {
            let data = p4k.read(entry).unwrap();
            let reader = P4kSiblingReader { p4k: &p4k, base_path: entry.name.clone() };
            let Ok(dds) = DdsFile::from_split(&data, &reader).or_else(|_| DdsFile::from_bytes(&data)) else { continue; };
            let (width, height) = dds.dimensions(0);
            if width < 128 || height < 128 || width > 2048 || height > 2048 { continue; }
            if dds.decode_rgba(0).is_err() { continue; }
            let old_path = output.join(format!("{count}-old.png"));
            let new_path = output.join(format!("{count}-new.png"));
            let start = std::time::Instant::now();
            dds.save_png(&old_path, 0).unwrap();
            old_time += start.elapsed();
            let start = std::time::Instant::now();
            extract_one_dds(&p4k, &new_path, &entry.name).unwrap();
            new_time += start.elapsed();
            assert_eq!(image::open(&old_path).unwrap().into_rgba8(), image::open(&new_path).unwrap().into_rgba8(), "{}", entry.name);
            count += 1;
            if count == 24 { break; }
        }
        assert_eq!(count, 24);
        eprintln!("24 PNGs: old decode/encode {:.3}s; new read/decode/encode {:.3}s; identical pixels", old_time.as_secs_f64(), new_time.as_secs_f64());
    }
}
