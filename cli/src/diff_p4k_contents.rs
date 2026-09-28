use std::path::Path;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;
use starbreaker_3d::included_objects::IncludedObjects;
use starbreaker_chunks::chunk_file::ChunkFile;
use starbreaker_chunks::known_types::{crch, ivo};
use starbreaker_p4k::{MappedP4k, P4kArchive};

use crate::error::Result;

#[cfg(test)]
#[path = "diff_dba_tests.rs"]
mod dba_tests;

#[cfg(test)]
#[path = "diff_entdata_tests.rs"]
mod entdata_tests;

pub fn extract_p4k_contents(p4k: &MappedP4k, output: &Path) -> Result<()> {
    std::fs::create_dir_all(output)?;
    let main_count = AtomicUsize::new(0);
    let nested_count = AtomicUsize::new(0);
    p4k.entries().par_iter().try_for_each(|entry| -> Result<()> {
        if ends_with_ci(&entry.name, ".dba") { return Ok(()); }
        if is_diff_content(&entry.name) {
            extract_content_bytes(output, &normalize_relative_path(&entry.name), &p4k.read(entry)?)?;
            main_count.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        if !is_socpak_entry(&entry.name) { return Ok(()); }
        let socpak_rel = normalize_relative_path(&entry.name);
        let socpak_data = match p4k.read(entry) {
            Ok(data) => data,
            Err(e) => {
                eprintln!("[WARN] could not read socpak {}: {e}", entry.name);
                return Ok(());
            }
        };
        let inner = match P4kArchive::from_bytes(&socpak_data) {
            Ok(archive) => archive,
            Err(e) => {
                eprintln!("[WARN] could not parse socpak {}: {e}", entry.name);
                return Ok(());
            }
        };

        for inner_entry in inner.entries() {
            if !is_diff_content(&inner_entry.name) { continue; }
            let nested_rel = socpak_nested_output_path(&socpak_rel, &inner_entry.name);
            let bytes = inner.read(inner_entry)?;
            extract_content_bytes(output, &nested_rel, &bytes)?;
            nested_count.fetch_add(1, Ordering::Relaxed);
        }
        return Ok(());
    })?;
    for entry in p4k.entries().iter().filter(|entry| ends_with_ci(&entry.name, ".dba")) {
        extract_content_bytes(output, &normalize_relative_path(&entry.name), &p4k.read(entry)?)?;
        main_count.fetch_add(1, Ordering::Relaxed);
    }
    eprintln!(
        "Extracted {} P4K + {} SOCPAK content files (XML assets, SOC and readable DBA)",
        main_count.load(Ordering::Relaxed), nested_count.load(Ordering::Relaxed)
    );
    Ok(())
}

fn is_xml_asset(name: &str) -> bool {
    [".xml", ".adb", ".animevents", ".chrparams", ".cdf", ".mtl", ".bspace", ".comb", ".entdata", ".entxml", ".animsettings"]
        .iter().any(|extension| ends_with_ci(name, extension))
}

fn is_diff_content(name: &str) -> bool {
    is_xml_asset(name) || ends_with_ci(name, ".soc") || ends_with_ci(name, ".dba")
}

pub(crate) fn extract_content_bytes(output: &Path, relative_path: &str, bytes: &[u8]) -> Result<()> {
    if ends_with_ci(relative_path, ".soc") {
        extract_soc_bytes(output, relative_path, bytes)
    } else if ends_with_ci(relative_path, ".dba") {
        let mut summary = animation_database_summary(bytes);
        let path = output.join(format!("{relative_path}.json"));
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        let mut writer = BufWriter::new(std::fs::File::create(path)?);
        let header = serde_json::to_string_pretty(&summary)?;
        writer.write_all(&text_to_crlf_bytes(header[..header.len() - 1].as_bytes()))?;
        writer.write_all(b",\r\n\"animation_database\":")?;
        match starbreaker_3d::animation::dba::write_readable(bytes, &mut writer) {
            Ok(errors) => {
                writer.write_all(b"\r\n}\r\n")?;
                if errors != 0 { eprintln!("[WARN] {relative_path}: {errors} animation tracks could not be decoded; see decode_error fields"); }
            }
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                eprintln!("[WARN] {relative_path}: readable DBA export failed: {error}");
                summary["animation_database_error"] = error.to_string().into();
                writer.flush()?;
                writer.get_mut().set_len(0)?;
                writer.seek(SeekFrom::Start(0))?;
                writer.write_all(&text_to_crlf_bytes(serde_json::to_string_pretty(&summary)?.as_bytes()))?;
            }
            Err(error) => return Err(error.into()),
        }
        writer.flush()?;
        Ok(())
    } else {
        extract_xml_bytes(output, relative_path, bytes)
    }
}

fn animation_database_summary(bytes: &[u8]) -> serde_json::Value {
    let mut summary = serde_json::json!({
        "size": bytes.len(),
        "crc32c": format!("{:08x}", crc32c::crc32c(bytes)),
    });
    match ChunkFile::from_bytes(bytes) {
        Ok(ChunkFile::Ivo(file)) => {
            summary["container"] = "IVO".into();
            summary["chunks"] = file.chunks().iter().map(|chunk| serde_json::json!({
                "type": format!("0x{:08X}", chunk.chunk_type),
                "name": ivo::name(chunk.chunk_type),
                "version": chunk.version,
                "offset": chunk.offset,
                "size": chunk.size,
                "crc32c": format!("{:08x}", crc32c::crc32c(file.chunk_data(chunk))),
            })).collect();
        }
        Ok(ChunkFile::CrCh(file)) => {
            summary["container"] = "CrCh".into();
            summary["chunks"] = file.chunks().iter().map(|chunk| serde_json::json!({
                "type": format!("0x{:04X}", chunk.chunk_type),
                "name": crch::name(chunk.chunk_type),
                "version": chunk.version,
                "id": chunk.id,
                "big_endian": chunk.big_endian,
                "offset": chunk.offset,
                "size": chunk.size,
                "crc32c": format!("{:08x}", crc32c::crc32c(file.chunk_data(chunk))),
            })).collect();
        }
        Err(error) => { summary["parse_error"] = error.to_string().into(); }
    }
    summary
}

fn extract_xml_bytes(output: &Path, relative_path: &str, bytes: &[u8]) -> Result<()> {
    let out_path = output.join(relative_path);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let output_bytes = if starbreaker_cryxml::is_cryxmlb(bytes) {
        match starbreaker_cryxml::from_bytes(bytes) {
            Ok(xml) => text_to_crlf_bytes(format!("{xml}").as_bytes()),
            Err(error) => {
                eprintln!("[WARN] could not decode CryXML {relative_path}: {error}; preserving original bytes");
                std::fs::write(&out_path, bytes)?;
                return Ok(());
            },
        }
    } else {
        bytes.to_vec()
    };
    let output_bytes = match crate::diff_xml::normalize_audio_xml(&output_bytes) {
        Ok(Some(normalized)) => normalized,
        Ok(None) => text_to_crlf_bytes(&output_bytes),
        Err(error) => {
            eprintln!("[WARN] could not normalize XML {relative_path}: {error}; preserving decoded output");
            text_to_crlf_bytes(&output_bytes)
        }
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
    name.get(name.len().saturating_sub(suffix.len())..)
        .is_some_and(|ending| ending.eq_ignore_ascii_case(suffix))
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
    fn selects_animation_and_related_xml_assets() {
        for extension in ["xml", "adb", "animevents", "chrparams", "cdf", "mtl", "bspace", "comb", "entdata", "entxml", "animsettings", "dba", "soc"] {
            assert!(is_diff_content(&format!("Data/Assets/test.{extension}")), "{extension}");
            assert!(is_diff_content(&format!("Data\\Assets\\test.{}", extension.to_uppercase())));
        }
        for name in ["file.dds", "file.dba.backup", "file.adb.tmp", "ééé", "", "file.png"] {
            assert!(!is_diff_content(name));
        }
    }

    fn synthetic_dba() -> Vec<u8> {
        let mut bytes = b"#ivo".to_vec();
        for value in [0x900u32, 1, 16, ivo::DB_DATA, 0x902] {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(32u64.to_le_bytes());
        bytes.extend(b"synthetic payload");
        bytes
    }

    #[test]
    fn dba_summary_is_stable_and_detects_payload_changes() {
        let mut bytes = synthetic_dba();
        let before = animation_database_summary(&bytes);
        assert_eq!(before, animation_database_summary(&bytes));
        assert_eq!(before["container"], "IVO");
        assert_eq!(before["chunks"][0]["name"], "DbData");
        assert_eq!(before["chunks"][0]["offset"], 32);
        *bytes.last_mut().unwrap() ^= 1;
        let after = animation_database_summary(&bytes);
        assert_ne!(before["crc32c"], after["crc32c"]);
        assert_ne!(before["chunks"][0]["crc32c"], after["chunks"][0]["crc32c"]);
        assert_eq!(before["chunks"][0]["size"], after["chunks"][0]["size"]);
    }

    #[test]
    fn malformed_dba_is_reported_without_panicking() {
        let mut bytes = synthetic_dba();
        bytes[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
        let summary = animation_database_summary(&bytes);
        assert!(summary["parse_error"].as_str().unwrap().contains("exceeds file length"));
        assert!(summary["crc32c"].is_string());
        assert!(animation_database_summary(b"invalid")["parse_error"].is_string());
        let mut legacy = b"CrCh".to_vec();
        for value in [0x746u32, 1, 16, 0x1000, 1, 32, u32::MAX] {
            legacy.extend(value.to_le_bytes());
        }
        assert!(animation_database_summary(&legacy)["parse_error"].is_string());
    }

    #[test]
    fn exports_nested_animation_assets_and_binary_summary() {
        let output = std::env::temp_dir().join(format!("starbreaker-diff-content-{}", std::process::id()));
        assert!(!output.exists());
        let relative = socpak_nested_output_path("Data/Containers/test.socpak", "Animations\\test.ADB");
        extract_content_bytes(&output, &relative, b"<AnimDB>\n</AnimDB>\n").unwrap();
        assert_eq!(std::fs::read(output.join(&relative)).unwrap(), b"<AnimDB>\r\n</AnimDB>\r\n");
        let dba_path = "Data/Animations/test.dba";
        extract_content_bytes(&output, dba_path, &synthetic_dba()).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(output.join(format!("{dba_path}.json"))).unwrap()).unwrap();
        assert_eq!(json["container"], "IVO");
        assert!(!output.join(dba_path).exists());
        let invalid = b"CryXmlB\0\ninvalid\r\ndata";
        extract_content_bytes(&output, "broken.adb", invalid).unwrap();
        assert_eq!(std::fs::read(output.join("broken.adb")).unwrap(), invalid);
        std::fs::remove_dir_all(&output).unwrap();
    }

    #[test]
    #[ignore = "requires STARBREAKER_DIFF_TEST_P4K pointing to a local game archive"]
    fn live_animation_and_xml_exports() {
        let archive = std::env::var_os("STARBREAKER_DIFF_TEST_P4K").expect("archive path required");
        let p4k = MappedP4k::open(Path::new(&archive)).unwrap();
        let output = std::env::temp_dir().join(format!("starbreaker-diff-live-{}", std::process::id()));
        assert!(!output.exists());
        for extension in [".adb", ".dba", ".animevents", ".chrparams", ".cdf", ".mtl", ".bspace", ".comb"] {
            let entry = p4k.entries().iter().filter(|entry| ends_with_ci(&entry.name, extension))
                .min_by_key(|entry| entry.uncompressed_size).expect(extension);
            let bytes = p4k.read(entry).unwrap();
            let relative = normalize_relative_path(&entry.name);
            extract_content_bytes(&output, &relative, &bytes).unwrap();
            if extension == ".dba" {
                let text = std::fs::read(output.join(format!("{relative}.json"))).unwrap();
                let summary: serde_json::Value = serde_json::from_slice(&text).unwrap();
                assert!(summary.get("parse_error").is_none(), "{}: {summary}", entry.name);
                assert!(summary.get("animation_database_error").is_none(), "{}: {summary}", entry.name);
                assert_eq!(summary["animation_database"]["tracks_complete"], true);
                assert!(!summary["chunks"].as_array().unwrap().is_empty());
            } else {
                let text = std::fs::read_to_string(output.join(&relative)).unwrap();
                assert!(text.trim_start_matches('\u{feff}').trim_start().starts_with('<'), "{}", entry.name);
                assert!(!text.starts_with("CryXmlB"));
            }
            println!("Verified {} ({} bytes)", entry.name, bytes.len());
        }
        std::fs::remove_dir_all(output).unwrap();
    }

    fn stored_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut directory = Vec::new();
        for &(name, payload) in entries {
            let offset = bytes.len() as u32;
            let mut local = vec![0u8; 30];
            local[..4].copy_from_slice(&0x04034b50u32.to_le_bytes());
            local[4..6].copy_from_slice(&20u16.to_le_bytes());
            local[14..18].copy_from_slice(&crc32c::crc32c(payload).to_le_bytes());
            local[18..22].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            local[22..26].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            local[26..28].copy_from_slice(&(name.len() as u16).to_le_bytes());
            bytes.extend(local);
            bytes.extend(name.as_bytes());
            bytes.extend(payload);
            let mut central = vec![0u8; 46];
            central[..4].copy_from_slice(&0x02014b50u32.to_le_bytes());
            central[4..6].copy_from_slice(&20u16.to_le_bytes());
            central[6..8].copy_from_slice(&20u16.to_le_bytes());
            central[16..20].copy_from_slice(&crc32c::crc32c(payload).to_le_bytes());
            central[20..24].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            central[24..28].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            central[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
            central[42..46].copy_from_slice(&offset.to_le_bytes());
            directory.extend(central);
            directory.extend(name.as_bytes());
        }
        let directory_start = bytes.len() as u32;
        let directory_size = directory.len() as u32;
        bytes.extend(directory);
        bytes.extend(0x06054b50u32.to_le_bytes());
        bytes.extend([0u8; 4]);
        bytes.extend((entries.len() as u16).to_le_bytes());
        bytes.extend((entries.len() as u16).to_le_bytes());
        bytes.extend(directory_size.to_le_bytes());
        bytes.extend(directory_start.to_le_bytes());
        bytes.extend([0u8; 2]);
        bytes
    }

    #[test]
    fn archive_pipeline_includes_main_and_socpak_animation_assets() {
        let root = std::env::temp_dir().join(format!("starbreaker-diff-archive-{}", std::process::id()));
        assert!(!root.exists());
        std::fs::create_dir_all(&root).unwrap();
        let entity=b"<Entity Id=\"123\" Name=\"placed entity\"><Properties value=\"42\" /></Entity>";
        let nested = stored_archive(&[("Animations/test.adb", b"<AnimDB/>"), ("props/test.mtl", b"<Material/>"),
            ("test/entdata/123.ENTXML",entity)]);
        let dba = synthetic_dba();
        let archive = stored_archive(&[("Data/Animations/main.ADB", b"<AnimDB/>"),
            ("Data/Animations/main.dba", &dba), ("Data/Containers/test.socpak", &nested),
            ("Data/Loose/entdata/456.entxml",entity), ("Data/skip.dds", b"ignored")]);
        let archive_path = root.join("Data.p4k");
        std::fs::write(&archive_path, archive).unwrap();
        let p4k = MappedP4k::open(&archive_path).unwrap();
        let output = root.join("contents");
        extract_p4k_contents(&p4k, &output).unwrap();
        for path in ["Data/Animations/main.ADB", "Data/Animations/main.dba.json",
            "Data/Containers/test/Animations/test.adb", "Data/Containers/test/props/test.mtl"] {
            assert!(output.join(path).is_file(), "{path}");
        }
        assert!(!output.join("Data/skip.dds").exists());
        for path in ["Data/Containers/test/test/entdata/123.ENTXML","Data/Loose/entdata/456.entxml"] {
            assert_eq!(std::fs::read(output.join(path)).unwrap(),entity);
        }
        drop(p4k);
        std::fs::remove_dir_all(root).unwrap();
    }

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
