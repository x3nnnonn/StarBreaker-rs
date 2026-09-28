use std::collections::HashMap;
use std::path::{Path, PathBuf};

use quick_xml::{Reader, events::Event};
use rayon::prelude::*;
use starbreaker_p4k::{MappedP4k, P4kArchive, P4kEntry};

use crate::error::{CliError, Result};

pub type FileIndex = HashMap<String, (u32, u64)>;

pub fn key(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

pub fn add_entry(index: &mut FileIndex, prefix: &str, entry: &P4kEntry) {
    let path = if prefix.is_empty() { entry.name.clone() } else { format!("{prefix}/{}", entry.name) };
    index.insert(key(&path), (entry.crc32, entry.uncompressed_size));
}

pub fn add_archive(index: &mut FileIndex, prefix: &str, archive: &P4kArchive<'_>) -> Result<()> {
    for entry in archive.entries() {
        add_entry(index, prefix, entry);
        if crate::p4k_compare::is_socpak_path(&entry.name) {
            let bytes = archive.read(entry)?;
            let nested = P4kArchive::from_bytes(&bytes)?;
            add_archive(index, &format!("{prefix}/{}", entry.name), &nested)?;
        }
    }
    Ok(())
}

pub fn current(p4k: &MappedP4k) -> Result<FileIndex> {
    let mut index = FileIndex::with_capacity(p4k.entries().len());
    for entry in p4k.entries() { add_entry(&mut index, "", entry); }
    let nested: Result<Vec<_>> = p4k.entries().par_iter()
        .filter(|entry| crate::p4k_compare::is_socpak_path(&entry.name))
        .map(|entry| {
            let bytes = p4k.read(entry)?;
            let archive = P4kArchive::from_bytes(&bytes)?;
            let mut children = FileIndex::new();
            add_archive(&mut children, &entry.name, &archive)?;
            Ok(children)
        }).collect();
    for children in nested? { index.extend(children); }
    Ok(index)
}

pub fn exists(root: &Path) -> bool {
    root.join("P4k").is_dir()
}

pub fn load(root: &Path) -> Result<FileIndex> {
    let manifests = root.join("P4k");
    let manifests = if manifests.is_dir() { manifests } else if root.file_name().is_some_and(|name| name.eq_ignore_ascii_case("P4k")) { root.to_path_buf() } else {
        return Err(CliError::InvalidInput(format!("baseline must contain the dump's P4k manifest directory: {}", root.display())));
    };
    if !manifests.is_dir() {
        return Err(CliError::InvalidInput(format!("baseline must be an existing diff directory: {}", root.display())));
    }
    let mut files = Vec::new();
    let mut index = FileIndex::new();
    collect_manifests(&manifests, &manifests, &mut files, &mut index)?;
    if files.is_empty() {
        return Err(CliError::InvalidInput(format!("no P4k XML/JSON manifests in {}", manifests.display())));
    }
    let parsed: Result<Vec<_>> = files.par_iter().map(|path| read_manifest(&manifests, path)).collect();
    for entries in parsed? { index.extend(entries); }
    Ok(index)
}

fn collect_manifests(root: &Path, dir: &Path, files: &mut Vec<PathBuf>, index: &mut FileIndex) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            let relative = path.strip_prefix(root).unwrap().to_string_lossy();
            if crate::p4k_compare::is_socpak_path(&relative) { index.insert(key(&relative), (0, 0)); }
            collect_manifests(root, &path, files, index)?;
        } else if kind.is_file() && path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("xml") || ext.eq_ignore_ascii_case("json")) {
            files.push(path);
        }
    }
    Ok(())
}

fn insert_manifest_entry(index: &mut FileIndex, dir: &str, name: &str, crc: &str, size: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', ':']) {
        return Err(CliError::InvalidInput(format!("invalid manifest file name {name:?}")));
    }
    let crc = u32::from_str_radix(crc.trim_start_matches("0x").trim_start_matches("0X"), 16)?;
    let size = size.parse()?;
    let path = if dir.is_empty() { name.to_owned() } else { format!("{dir}/{name}") };
    index.insert(key(&path), (crc, size));
    Ok(())
}

fn read_manifest(root: &Path, path: &Path) -> Result<FileIndex> {
    let dir = path.parent().unwrap().strip_prefix(root).unwrap().to_string_lossy();
    let bytes = std::fs::read(path)?;
    let mut index = FileIndex::new();
    let invalid = || CliError::InvalidInput(format!("invalid P4k manifest {}", path.display()));
    if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("json")) {
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        for file in value.get("Files").and_then(|value| value.as_array()).ok_or_else(invalid)? {
            let field = |name| file.get(name).and_then(|value| value.as_str()).ok_or_else(invalid);
            insert_manifest_entry(&mut index, &dir, field("Name")?, field("CRC32")?, field("Size")?)?;
        }
    } else {
        let mut reader = Reader::from_reader(bytes.as_slice());
        let mut directory = false;
        let mut depth = 0usize;
        loop {
            let event = reader.read_event().map_err(|_| invalid())?;
            let opens = matches!(&event, Event::Start(_));
            match event {
                Event::Start(node) | Event::Empty(node) => {
                    if node.name().as_ref() == b"Directory" && !directory { directory = true; }
                    else if node.name().as_ref() == b"File" && directory && depth == 1 {
                        let attributes: Result<HashMap<_, _>> = node.attributes().map(|attribute| {
                            let attribute = attribute.map_err(|_| invalid())?;
                            Ok((attribute.key.as_ref().to_vec(), attribute.unescape_value().map_err(|_| invalid())?.into_owned()))
                        }).collect();
                        let attributes = attributes?;
                        let field = |name: &[u8]| attributes.get(name).map(String::as_str).ok_or_else(invalid);
                        insert_manifest_entry(&mut index, &dir, field(b"Name")?, field(b"CRC32")?, field(b"Size")?)?;
                    } else { return Err(invalid()); }
                    if opens { depth += 1; }
                }
                Event::End(_) => { depth = depth.checked_sub(1).ok_or_else(invalid)?; }
                Event::Eof => break,
                _ => {}
            }
        }
        if !directory || depth != 0 { return Err(invalid()); }
    }
    Ok(index)
}

pub fn unchanged(previous: Option<&FileIndex>, path: &str, crc: u32, size: u64) -> bool {
    previous.is_some_and(|index| index.get(&key(path)) == Some(&(crc, size)))
}

pub fn added_containers(current: &FileIndex, previous: &FileIndex) -> Vec<String> {
    let mut paths: Vec<_> = current.keys().filter(|path| crate::p4k_compare::is_socpak_path(path) && !previous.contains_key(*path)).cloned().collect();
    paths.sort_unstable();
    paths
}

#[cfg(test)]
#[path = "diff_index_tests.rs"]
mod tests;
