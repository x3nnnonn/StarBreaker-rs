use std::path::{Path, PathBuf};

use crate::error::Result;

pub fn write_changed(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.is_file() && std::fs::metadata(path)?.len() == bytes.len() as u64 {
        let existing = std::fs::read(path)?;
        if crc32fast::hash(&existing) == crc32fast::hash(bytes) { return Ok(()); }
    }
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    std::fs::write(path, bytes)?;
    Ok(())
}

pub fn files_under(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !root.is_dir() { return Ok(files); }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() { files.extend(files_under(&entry.path())?); }
        else if entry.file_type()?.is_file() { files.push(entry.path()); }
    }
    Ok(files)
}

pub fn content_files(root: &Path, relative: &str) -> Result<Vec<PathBuf>> {
    let path = root.join(relative);
    let lower = relative.to_ascii_lowercase();
    if !lower.ends_with(".soc") {
        let path = if lower.ends_with(".dba") { root.join(format!("{relative}.json")) } else { path };
        return Ok(if path.is_file() { vec![path] } else { Vec::new() });
    }
    let parent = path.parent().unwrap();
    if !parent.is_dir() { return Ok(Vec::new()); }
    let stem = path.file_stem().unwrap().to_string_lossy().to_ascii_lowercase();
    let prefix = format!("{stem}_");
    let raw = path.file_name().unwrap().to_string_lossy().to_ascii_lowercase();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() { continue; }
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        let chunk = name.strip_prefix(&prefix).and_then(|rest| rest.split_once('_')).is_some_and(|(index, tail)| {
            !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit()) && is_chunk_output(tail)
        });
        if name == raw || chunk { files.push(entry.path()); }
    }
    Ok(files)
}

fn is_chunk_output(tail: &str) -> bool {
    static NAMES: std::sync::OnceLock<std::collections::HashSet<String>> = std::sync::OnceLock::new();
    let Some((name, extension)) = tail.rsplit_once('.') else { return false; };
    if !matches!(extension, "xml" | "txt" | "bin") { return false; }
    let names = NAMES.get_or_init(|| (0..=u16::MAX).filter_map(starbreaker_chunks::known_types::crch::name).map(str::to_ascii_lowercase).collect());
    names.contains(name) || name.strip_prefix("unknown_0x").is_some_and(|hex| hex.len() == 4 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

pub fn remove_content(root: &Path, relative: &str) -> Result<()> {
    for path in content_files(root, relative)? { std::fs::remove_file(path)?; }
    Ok(())
}

pub fn content_path(source: &str) -> String {
    source.replace('\\', "/").split('/').map(|part| {
        let lower = part.to_ascii_lowercase();
        if lower.ends_with(".socpak") { &part[..part.len() - 7] } else { part }
    }).collect::<Vec<_>>().join("/")
}

pub fn has_extension(root: &Path, extension: &str) -> Result<bool> {
    if !root.is_dir() { return Ok(false); }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if has_extension(&entry.path(), extension)? { return Ok(true); }
        } else if entry.path().extension().is_some_and(|value| value.eq_ignore_ascii_case(extension)) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn crc32(mut input: impl std::io::Read) -> Result<(u64, u32)> {
    let mut hasher = crc32fast::Hasher::new();
    let mut length = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 { break; }
        length += count as u64;
        hasher.update(&buffer[..count]);
    }
    Ok((length, hasher.finalize()))
}

pub fn backup_matches(input: &Path, backup: &Path) -> Result<bool> {
    let source = crc32(std::fs::File::open(input)?)?;
    let decoded = zstd::stream::read::Decoder::new(std::fs::File::open(backup)?)?;
    Ok(source == crc32(decoded)?)
}

pub fn prune_empty_directories(root: &Path) -> Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() { continue; }
        let path = entry.path();
        prune_empty_directories(&path)?;
        if std::fs::read_dir(&path)?.next().is_none() { std::fs::remove_dir(&path)?; }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soc_cleanup_does_not_claim_other_mesh_prefixes() {
        let root = std::env::temp_dir().join(format!("starbreaker-soc-ownership-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for name in ["a_0_Mesh.bin", "a_2_Unknown_0xABCD.xml", "a_1_0_Mesh.bin", "a_1_Mesh_0_Node.bin", "a_0_custom.txt"] {
            std::fs::write(root.join(name), b"test").unwrap();
        }
        remove_content(&root, "a.soc").unwrap();
        assert!(!root.join("a_0_Mesh.bin").exists());
        assert!(!root.join("a_2_Unknown_0xABCD.xml").exists());
        for name in ["a_1_0_Mesh.bin", "a_1_Mesh_0_Node.bin", "a_0_custom.txt"] { assert!(root.join(name).exists()); }
        std::fs::remove_dir_all(root).unwrap();
    }
}
