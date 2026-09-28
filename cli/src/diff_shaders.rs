use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use starbreaker_p4k::{MappedP4k, P4kArchive, P4kEntry};

use crate::diff_index::{FileIndex, key};
use crate::error::{CliError, Result};

pub fn is_shader_archive(name: &str) -> bool {
    let leaf = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let Some((stem, extension)) = leaf.rsplit_once('.') else { return false; };
    extension.eq_ignore_ascii_case("pak")
        && stem.get(.."shadercache_".len()).is_some_and(|prefix| prefix.eq_ignore_ascii_case("shadercache_"))
}

pub fn is_source(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        ["cfxb", "cfib", "ext"].iter().any(|suffix| extension.eq_ignore_ascii_case(suffix))
    })
}

fn safe_path(name: &str) -> Result<String> {
    let normalized = name.replace('\\', "/");
    if normalized.is_empty() || normalized.contains(':')
        || normalized.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
        return Err(CliError::InvalidInput(format!("Invalid shader member path {name:?}")));
    }
    Ok(normalized)
}

pub fn with_archive<T>(p4k: &MappedP4k, entry: &P4kEntry, action: impl FnOnce(&P4kArchive<'_>) -> Result<T>) -> Result<T> {
    if entry.compression_method != 0 || entry.is_encrypted {
        let bytes = p4k.read(entry)?;
        return action(&P4kArchive::from_bytes(&bytes)?);
    }
    if entry.compressed_size != entry.uncompressed_size || entry.compressed_size < 22 {
        return Err(CliError::InvalidInput(format!("Invalid stored shader archive {}", entry.name)));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let mut file = options.open(p4k.path())?;
    let mut offset = entry.offset;
    if entry.has_local_header {
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0u8; 30];
        file.read_exact(&mut header)?;
        let signature = u32::from_le_bytes(header[..4].try_into().unwrap());
        if !matches!(signature, starbreaker_p4k::types::LOCAL_FILE_SIGNATURE | starbreaker_p4k::types::LOCAL_FILE_CIG_SIGNATURE) {
            return Err(CliError::InvalidInput("Invalid shader archive local header".into()));
        }
        let skip = 30 + u16::from_le_bytes(header[26..28].try_into().unwrap()) as u64
            + u16::from_le_bytes(header[28..30].try_into().unwrap()) as u64;
        offset = offset.checked_add(skip).ok_or_else(|| CliError::InvalidInput("Shader archive offset overflow".into()))?;
    }
    let end = offset.checked_add(entry.compressed_size)
        .ok_or_else(|| CliError::InvalidInput("Shader archive length overflow".into()))?;
    if end > file.metadata()?.len() {
        return Err(CliError::InvalidInput("Truncated shader archive".into()));
    }
    let length = usize::try_from(entry.compressed_size)
        .map_err(|_| CliError::InvalidInput("Shader archive exceeds address space".into()))?;
    let mapped = unsafe { memmap2::MmapOptions::new().offset(offset).len(length).map(&file)? };
    action(&P4kArchive::from_bytes(&mapped)?)
}

pub fn add_inventory(index: &mut FileIndex, prefix: &str, archive: &P4kArchive<'_>) -> Result<()> {
    for entry in archive.entries() {
        if entry.name.ends_with(['/', '\\']) { continue; }
        let name = safe_path(&entry.name)?;
        let path = key(&format!("{prefix}/{name}"));
        if index.insert(path.clone(), (entry.crc32, entry.uncompressed_size)).is_some() {
            return Err(CliError::InvalidInput(format!("Duplicate shader member {path}")));
        }
    }
    Ok(())
}

fn source_path(root: &Path, source: &str) -> Result<std::path::PathBuf> {
    Ok(root.join(format!("{}.txt", safe_path(source)?)))
}

pub fn export_archive(root: &Path, prefix: &str, archive: &P4kArchive<'_>) -> Result<(usize, BTreeSet<u32>)> {
    let mut prepared = Vec::new();
    let mut unresolved = BTreeSet::new();
    for entry in archive.entries().iter().filter(|entry| is_source(&entry.name)) {
        let bytes = archive.read(entry)?;
        if bytes.len() as u64 != entry.uncompressed_size || crc32fast::hash(&bytes) != entry.crc32 {
            return Err(CliError::InvalidInput(format!("Shader member CRC32 or size mismatch: {prefix}/{}", entry.name)));
        }
        let text = if key(&entry.name).ends_with(".ext") {
            std::str::from_utf8(&bytes).map_err(|_| CliError::InvalidInput(format!("Non-UTF-8 shader extension {}", entry.name)))?
                .trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n")
        } else {
            let (text, missing) = crate::shader_decode::decode(&bytes)?;
            unresolved.extend(missing);
            text
        };
        prepared.push((source_path(root, &key(&format!("{prefix}/{}", entry.name)))?, text));
    }
    for (path, text) in &prepared { crate::diff_outputs::write_changed(path, text.as_bytes())?; }
    Ok((prepared.len(), unresolved))
}

pub fn change_report(current: &FileIndex, previous: Option<&FileIndex>) -> serde_json::Value {
    let mut archives = BTreeMap::new();
    let mut prefixes = BTreeSet::new();
    let has_baseline = previous.into_iter().flat_map(|index| index.keys())
        .any(|path| !is_shader_archive(path) && path.split('/').any(is_shader_archive));
    for path in current.keys().chain(previous.into_iter().flat_map(|index| index.keys())) {
        if is_shader_archive(path) { prefixes.insert(path.clone()); }
    }
    for archive in prefixes {
        let prefix = format!("{archive}/");
        let before: BTreeMap<_, _> = previous.into_iter().flat_map(|index| index.iter())
            .filter_map(|(path, value)| path.strip_prefix(&prefix).map(|name| (name, value))).collect();
        let after: BTreeMap<_, _> = current.iter()
            .filter_map(|(path, value)| path.strip_prefix(&prefix).map(|name| (name, value))).collect();
        let initialized = has_baseline;
        let mut changes = Vec::new();
        let mut added = 0;
        let mut modified = 0;
        let mut removed = 0;
        if initialized {
            for (name, value) in &after {
                let old = before.get(name);
                if old.copied() == Some(*value) { continue; }
                let status = if old.is_none() { added += 1; "added" } else { modified += 1; "modified" };
                changes.push(serde_json::json!({"path": name, "status": status,
                    "crc32": format!("{:08X}", value.0), "size": value.1,
                    "previous_crc32": old.map(|value| format!("{:08X}", value.0)), "previous_size": old.map(|value| value.1)}));
            }
            for (name, value) in &before {
                if after.contains_key(name) { continue; }
                removed += 1;
                changes.push(serde_json::json!({"path": name, "status": "removed",
                    "previous_crc32": format!("{:08X}", value.0), "previous_size": value.1}));
            }
        }
        archives.insert(archive, serde_json::json!({
            "baseline_initialized": initialized, "members": after.len(),
            "readable_sources": after.keys().filter(|name| is_source(name)).count(),
            "added": added, "modified": modified, "removed": removed, "changes": changes
        }));
    }
    serde_json::json!({"format_version": 1, "archives": archives})
}

pub fn export(p4k: &MappedP4k, output: &Path, previous: Option<&FileIndex>, current: &FileIndex, keep: bool) -> Result<()> {
    let root = output.join("ShaderSources");
    let mut coverage = BTreeMap::new();
    for entry in p4k.entries().iter().filter(|entry| is_shader_archive(&entry.name)) {
        let (count, unresolved) = with_archive(p4k, entry, |archive| export_archive(&root, &entry.name, archive))?;
        eprintln!("[SHADERS] {}: {count} readable files, {} unresolved token IDs", entry.name, unresolved.len());
        coverage.insert(key(&entry.name), serde_json::json!({"readable_files": count, "unresolved_token_ids": unresolved}));
    }
    if !keep {
        for source in previous.into_iter().flat_map(|index| index.keys()) {
            if source.split('/').any(is_shader_archive) && is_source(source) && !current.contains_key(source) {
                let path = source_path(&root, source)?;
                if path.is_file() { std::fs::remove_file(path)?; }
            }
        }
    }
    let mut report = change_report(current, previous);
    report["decoder"] = serde_json::json!({"format": "FXB1", "version": 11674302, "formatter_version": 2, "coverage": coverage,
        "unresolved_tokens": "Preserved as explicit TOKEN markers. Output uses normalized shader formatting. Original whitespace and comments are not recoverable."});
    crate::diff_outputs::write_changed(&output.join("ShaderChanges.json"), &serde_json::to_vec_pretty(&report)?)?;
    Ok(())
}

#[cfg(test)]
#[path = "diff_shaders_tests.rs"]
mod tests;
