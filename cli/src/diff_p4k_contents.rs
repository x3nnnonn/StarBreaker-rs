use std::path::Path;
use std::sync::Arc;

use rayon::prelude::*;
use starbreaker_3d::included_objects::IncludedObjects;
use starbreaker_chunks::chunk_file::ChunkFile;
use starbreaker_chunks::known_types::crch;
use starbreaker_p4k::{MappedP4k, P4kArchive};

use crate::error::Result;

struct XmlTask {
    output_rel: String,
    bytes: Vec<u8>,
}

struct SocTask {
    output_rel: String,
    bytes: Vec<u8>,
}

pub fn extract_p4k_xml_files(p4k: &MappedP4k, output: &Path) -> Result<()> {
    std::fs::create_dir_all(output)?;

    let mut xml_tasks = Vec::new();
    let mut soc_tasks = Vec::new();

    for entry in p4k.entries() {
        if ends_with_ci(&entry.name, ".xml") {
            let bytes = p4k.read(entry)?;
            xml_tasks.push(XmlTask {
                output_rel: normalize_relative_path(&entry.name),
                bytes,
            });
        } else if ends_with_ci(&entry.name, ".soc") {
            let bytes = p4k.read(entry)?;
            soc_tasks.push(SocTask {
                output_rel: normalize_relative_path(&entry.name),
                bytes,
            });
        }
    }

    let main_xml = xml_tasks.len();
    let main_soc = soc_tasks.len();
    let mut socpak_xml = 0usize;
    let mut socpak_soc = 0usize;

    for entry in p4k.entries() {
        if !is_socpak_entry(&entry.name) {
            continue;
        }
        let socpak_rel = normalize_relative_path(&entry.name);
        let socpak_data = match p4k.read(entry) {
            Ok(data) => data,
            Err(e) => {
                eprintln!("[WARN] could not read socpak {}: {e}", entry.name);
                continue;
            }
        };
        let inner = match P4kArchive::from_bytes(&socpak_data) {
            Ok(archive) => archive,
            Err(e) => {
                eprintln!("[WARN] could not parse socpak {}: {e}", entry.name);
                continue;
            }
        };

        for inner_entry in inner.entries() {
            let nested_rel = socpak_nested_output_path(&socpak_rel, &inner_entry.name);
            if ends_with_ci(&inner_entry.name, ".xml") {
                let bytes = match inner.read(inner_entry) {
                    Ok(data) => data,
                    Err(e) => {
                        eprintln!(
                            "[WARN] could not read {} in {}: {e}",
                            inner_entry.name, entry.name
                        );
                        continue;
                    }
                };
                xml_tasks.push(XmlTask {
                    output_rel: nested_rel,
                    bytes,
                });
                socpak_xml += 1;
            } else if ends_with_ci(&inner_entry.name, ".soc") {
                let bytes = match inner.read(inner_entry) {
                    Ok(data) => data,
                    Err(e) => {
                        eprintln!(
                            "[WARN] could not read {} in {}: {e}",
                            inner_entry.name, entry.name
                        );
                        continue;
                    }
                };
                soc_tasks.push(SocTask {
                    output_rel: nested_rel,
                    bytes,
                });
                socpak_soc += 1;
            }
        }
    }

    if xml_tasks.is_empty() && soc_tasks.is_empty() {
        eprintln!("[WARN] No XML or SOC files found in P4K or SOCPAKs");
        return Ok(());
    }

    let output = Arc::new(output.to_path_buf());
    xml_tasks
        .par_iter()
        .try_for_each(|task| extract_xml_bytes(&output, &task.output_rel, &task.bytes))?;
    soc_tasks
        .par_iter()
        .try_for_each(|task| extract_soc_bytes(&output, &task.output_rel, &task.bytes))?;

    eprintln!(
        "Extracted {main_xml} XML + {socpak_xml} SOCPAK XML, {main_soc} SOC + {socpak_soc} SOCPAK SOC"
    );
    Ok(())
}

fn extract_xml_bytes(output: &Path, relative_path: &str, bytes: &[u8]) -> Result<()> {
    let out_path = output.join(relative_path);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let output_bytes = if starbreaker_cryxml::is_cryxmlb(bytes) {
        match starbreaker_cryxml::from_bytes(bytes) {
            Ok(xml) => text_to_crlf_bytes(format!("{xml}").as_bytes()),
            Err(_) => text_to_crlf_bytes(bytes),
        }
    } else {
        text_to_crlf_bytes(bytes)
    };
    std::fs::write(&out_path, &output_bytes)?;
    Ok(())
}

fn extract_soc_bytes(output: &Path, relative_path: &str, soc_bytes: &[u8]) -> Result<()> {
    let adjusted_rel = normalize_soc_relative_path(relative_path);
    let entry_path = output.join(&adjusted_rel);
    let object_container_dir = entry_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| output.to_path_buf());
    let base_name = entry_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "soc".to_string());

    std::fs::create_dir_all(&object_container_dir)?;

    let chunk_file = match ChunkFile::from_bytes(soc_bytes) {
        Ok(ChunkFile::CrCh(crch_file)) => crch_file,
        Ok(_) | Err(_) => {
            let raw_path = object_container_dir.join(entry_path.file_name().unwrap_or_default());
            std::fs::write(raw_path, soc_bytes)?;
            return Ok(());
        }
    };

    for (i, chunk) in chunk_file.chunks().iter().enumerate() {
        let chunk_data = chunk_file.chunk_data(chunk);
        let chunk_type_name = crch::name(chunk.chunk_type)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Unknown_0x{:04X}", chunk.chunk_type));

        if starbreaker_cryxml::is_cryxmlb(chunk_data) {
            match starbreaker_cryxml::from_bytes(chunk_data) {
                Ok(xml) => {
                    let xml_path = object_container_dir
                        .join(format!("{base_name}_{i}_{chunk_type_name}.xml"));
                    let bytes = text_to_crlf_bytes(format!("{xml}").as_bytes());
                    std::fs::write(xml_path, &bytes)?;
                    continue;
                }
                Err(_) => {}
            }
        }

        if chunk.chunk_type == crch::INCLUDED_OBJECTS {
            match IncludedObjects::from_bytes(chunk_data) {
                Ok(included) => {
                    let txt_path = object_container_dir
                        .join(format!("{base_name}_{i}_{chunk_type_name}.txt"));
                    let bytes = text_to_crlf_bytes(included.format_text().as_bytes());
                    std::fs::write(txt_path, &bytes)?;
                    continue;
                }
                Err(_) => {}
            }
        }

        let bin_path = object_container_dir.join(format!("{base_name}_{i}_{chunk_type_name}.bin"));
        std::fs::write(bin_path, chunk_data)?;
    }

    Ok(())
}

pub(crate) fn text_to_crlf_bytes(data: &[u8]) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(data) else {
        return data.to_vec();
    };
    text_to_crlf(text).into_bytes()
}

fn text_to_crlf(text: &str) -> String {
    let unified = text.replace("\r\n", "\n").replace('\r', "\n");
    unified.replace('\n', "\r\n")
}

fn ends_with_ci(name: &str, suffix: &str) -> bool {
    name.len() >= suffix.len()
        && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

fn is_socpak_entry(name: &str) -> bool {
    ends_with_ci(name, ".socpak")
}

fn normalize_relative_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn normalize_soc_relative_path(relative_path: &str) -> String {
    relative_path
        .trim_start_matches(['/', '\\'])
        .replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn socpak_nested_output_path(socpak_rel: &str, inner_rel: &str) -> String {
    let socpak_rel = socpak_rel.replace('\\', "/");
    let inner_rel = normalize_soc_relative_path(inner_rel);
    let socpak_dir = socpak_rel.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    let socpak_name = Path::new(&socpak_rel)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "socpak".to_string());

    if socpak_dir.is_empty() {
        format!("{socpak_name}/{inner_rel}")
    } else {
        format!("{socpak_dir}/{socpak_name}/{inner_rel}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_to_crlf_normalizes_lf_and_preserves_crlf() {
        assert_eq!(text_to_crlf("a\nb"), "a\r\nb");
        assert_eq!(text_to_crlf("a\r\nb"), "a\r\nb");
        assert_eq!(text_to_crlf("a\r\nb\n"), "a\r\nb\r\n");
    }

    #[test]
    fn text_to_crlf_bytes_leaves_non_utf8_unchanged() {
        let bin = [0xFF, 0xFE, 0x00];
        assert_eq!(text_to_crlf_bytes(&bin), bin);
    }
}
