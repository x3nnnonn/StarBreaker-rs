use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Args, ValueEnum};
use rayon::prelude::*;
use starbreaker_p4k::{MappedP4k, P4kArchive, P4kEntry};

use crate::common::load_p4k;
use crate::dcb::DcbFormat;
use crate::error::{CliError, Result};

#[derive(Clone, Copy, ValueEnum)]
pub enum ManifestFormat {
    Xml,
    Json,
}

impl ManifestFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Xml => "xml",
            Self::Json => "json",
        }
    }
}

#[derive(Args)]
pub struct DiffArgs {
    #[arg(long, short = 'g', env = "GAME_FOLDER")]
    pub game: PathBuf,
    #[arg(long, short = 'o', env = "OUTPUT_FOLDER")]
    pub output: PathBuf,
    #[arg(long, short = 'k', env = "KEEP_OLD")]
    pub keep: bool,
    #[arg(long, short = 'f', value_enum, default_value = "xml", env = "TEXT_FORMAT")]
    pub format: ManifestFormat,
}

impl DiffArgs {
    pub fn run(self) -> Result<()> {
        let DiffArgs { game, output, keep, format } = self;

        if !keep {
            timed("Clear old output", || clear_old(&output))?;
        }
        let total = Instant::now();

        let p4k_path = game.join("Data.p4k");
        let exe_path = game.join("Bin64").join("StarCitizen.exe");
        let p4k = load_p4k(Some(&p4k_path))?;
        let p4kcontents_dir = output.join("P4kContents");

        timed("P4k manifest", || dump_p4k_manifest(&p4k, &output.join("P4k"), format))?;
        timed("Localization", || extract_localization(&p4k, &p4kcontents_dir))?;
        timed("TagDatabase", || extract_tag_database(&p4k, &p4kcontents_dir))?;
        timed("DataCore records", || {
            crate::dcb::extract(
                Some(p4k_path.clone()),
                None,
                output.join("DataCore"),
                match format {
                    ManifestFormat::Xml => DcbFormat::DataForge,
                    ManifestFormat::Json => DcbFormat::Json,
                },
                None,
            )
        })?;

        let (_p4k_for_dcb, dcb_bytes) = crate::common::load_dcb_bytes(Some(&p4k_path), None)?;
        let db = starbreaker_datacore::database::Database::from_bytes(&dcb_bytes)?;
        timed("DataCore types", || {
            dump_data_core_types(&db, &output.join("DataCoreTypes"))
        })?;
        timed("DataCore enums", || {
            dump_data_core_enums(&db, &output.join("DataCoreEnums"))
        })?;

        timed("DataCore.dcb.zst", || {
            extract_dcb_to_zst(&p4k, &output.join("DataCore.dcb.zst"))
        })?;
        if exe_path.exists() {
            timed("StarCitizen.exe.zst", || {
                compress_file_to_zst(&exe_path, &output.join("StarCitizen.exe.zst"))
            })?;
        } else {
            eprintln!("[SKIP] {} not found — skipping exe.zst", exe_path.display());
        }
        timed("build_manifest.json", || {
            copy_build_manifest(&game.join("build_manifest.id"), &output.join("build_manifest.json"))
        })?;

        eprintln!("[DONE] total: {:.1}s", total.elapsed().as_secs_f64());
        Ok(())
    }
}

fn timed<F: FnOnce() -> Result<()>>(label: &str, f: F) -> Result<()> {
    let start = Instant::now();
    f()?;
    eprintln!("[OK]  {label}: {:.1}s", start.elapsed().as_secs_f64());
    Ok(())
}

fn parallel_remove_dir_all(path: &Path) -> Result<()> {
    let entries: Vec<_> = std::fs::read_dir(path)
        .map_err(|e| CliError::IoPath { source: e, path: path.display().to_string() })?
        .filter_map(|r| r.ok())
        .collect();

    entries.par_iter().try_for_each(|entry| -> Result<()> {
        let p = entry.path();
        let ft = entry
            .file_type()
            .map_err(|e| CliError::IoPath { source: e, path: p.display().to_string() })?;
        if ft.is_dir() {
            parallel_remove_dir_all(&p)?;
        } else {
            std::fs::remove_file(&p)
                .map_err(|e| CliError::IoPath { source: e, path: p.display().to_string() })?;
        }
        Ok(())
    })?;

    std::fs::remove_dir(path)
        .map_err(|e| CliError::IoPath { source: e, path: path.display().to_string() })?;
    Ok(())
}

fn clear_old(output: &Path) -> Result<()> {
    let folders = ["DataCore", "DataCoreTypes", "DataCoreEnums", "P4k", "P4kContents"];
    let files = ["build_manifest.json", "DataCore.dcb.zst", "StarCitizen.exe.zst"];

    folders.par_iter().try_for_each(|name| -> Result<()> {
        let p = output.join(name);
        if p.exists() {
            parallel_remove_dir_all(&p)?;
        }
        Ok(())
    })?;
    for name in files {
        let p = output.join(name);
        if p.exists() {
            std::fs::remove_file(&p)
                .map_err(|e| CliError::IoPath { source: e, path: p.display().to_string() })?;
        }
    }
    Ok(())
}

fn dump_p4k_manifest(p4k: &MappedP4k, output: &Path, format: ManifestFormat) -> Result<()> {
    std::fs::create_dir_all(output)?;
    let ext = format.extension();

    let mut by_dir: BTreeMap<String, Vec<&P4kEntry>> = BTreeMap::new();
    let mut socpak_refs: Vec<&P4kEntry> = Vec::new();
    for entry in p4k.entries() {
        let lower = entry.name.to_ascii_lowercase();
        let is_archive = lower.ends_with(".socpak") || lower.ends_with(".pak");
        let is_shadercache = lower.contains("shadercache_");
        if is_archive && !is_shadercache {
            socpak_refs.push(entry);
            continue;
        }
        let norm = entry.name.replace('\\', "/");
        let dir = norm.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
        by_dir.entry(dir).or_default().push(entry);
    }

    by_dir.par_iter().try_for_each(|(dir, _)| -> Result<()> {
        if !dir.is_empty() {
            std::fs::create_dir_all(output.join(dir))?;
        }
        Ok(())
    })?;
    socpak_refs.par_iter().try_for_each(|socpak| -> Result<()> {
        let rel = socpak.name.replace('\\', "/");
        std::fs::create_dir_all(output.join(&rel))?;
        Ok(())
    })?;

    by_dir
        .par_iter()
        .filter(|(_, children)| !children.is_empty())
        .try_for_each(|(dir, children)| -> Result<()> {
            let (manifest_path, dir_name) = if dir.is_empty() {
                (output.join(format!("root.{ext}")), String::new())
            } else {
                let dir_path = output.join(dir);
                let leaf = Path::new(dir)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "root".to_string());
                (dir_path.join(format!("{leaf}.{ext}")), leaf)
            };
            let mut sorted: Vec<&P4kEntry> = children.iter().copied().collect();
            sorted.sort_by(|a, b| compare_dictionary_order(&a.name, &b.name));
            match format {
                ManifestFormat::Xml => write_manifest_xml(&manifest_path, &dir_name, &sorted)?,
                ManifestFormat::Json => write_manifest_json(&manifest_path, &dir_name, &sorted)?,
            }
            Ok(())
        })?;

    socpak_refs.par_iter().try_for_each(|socpak_entry| -> Result<()> {
        let data = match p4k.read(socpak_entry) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("[WARN] could not read socpak {}: {e}", socpak_entry.name);
                return Ok(());
            }
        };
        let inner = match P4kArchive::from_bytes(&data) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("[WARN] could not parse socpak {}: {e}", socpak_entry.name);
                return Ok(());
            }
        };
        let socpak_rel = socpak_entry.name.replace('\\', "/");
        dump_archive_manifests(output, &socpak_rel, &inner, ext, format)
    })?;

    Ok(())
}

fn dump_archive_manifests(
    output: &Path,
    base_rel: &str,
    archive: &P4kArchive<'_>,
    ext: &str,
    format: ManifestFormat,
) -> Result<()> {
    let base_leaf = Path::new(base_rel)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "root".into());

    let mut by_dir: BTreeMap<String, Vec<&P4kEntry>> = BTreeMap::new();
    for entry in archive.entries() {
        let norm = entry.name.replace('\\', "/");
        let dir = norm.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
        by_dir.entry(dir).or_default().push(entry);
    }

    for (dir, children) in &by_dir {
        if children.is_empty() {
            continue;
        }
        let (manifest_path, dir_name) = if dir.is_empty() {
            let dir_path = output.join(base_rel);
            std::fs::create_dir_all(&dir_path)?;
            (dir_path.join(format!("{base_leaf}.{ext}")), base_leaf.clone())
        } else {
            let dir_path = output.join(base_rel).join(dir);
            std::fs::create_dir_all(&dir_path)?;
            let leaf = Path::new(dir)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "root".into());
            (dir_path.join(format!("{leaf}.{ext}")), leaf)
        };

        let mut sorted: Vec<&P4kEntry> = children.iter().copied().collect();
        sorted.sort_by(|a, b| compare_dictionary_order(&a.name, &b.name));

        match format {
            ManifestFormat::Xml => write_manifest_xml(&manifest_path, &dir_name, &sorted)?,
            ManifestFormat::Json => write_manifest_json(&manifest_path, &dir_name, &sorted)?,
        }
    }
    Ok(())
}

fn write_manifest_xml(path: &Path, dir_name: &str, entries: &[&P4kEntry]) -> Result<()> {
    let mut s = String::new();
    s.push_str("\u{FEFF}<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n");
    s.push_str(&format!("<Directory Name=\"{}\">\r\n", xml_attr(dir_name)));
    for e in entries {
        let leaf = e.name.rsplit(['\\', '/']).next().unwrap_or(&e.name);
        s.push_str(&format!(
            "  <File Name=\"{}\" CRC32=\"0x{:08X}\" Size=\"{}\" CompressionType=\"{}\" Encrypted=\"{}\" />\r\n",
            xml_attr(leaf),
            e.crc32,
            e.uncompressed_size,
            e.compression_method,
            if e.is_encrypted { "True" } else { "False" },
        ));
    }
    s.push_str("</Directory>");
    std::fs::write(path, s)?;
    Ok(())
}

fn write_manifest_json(path: &Path, dir_name: &str, entries: &[&P4kEntry]) -> Result<()> {
    let files: Vec<_> = entries
        .iter()
        .map(|e| {
            let leaf = e.name.rsplit(['\\', '/']).next().unwrap_or(&e.name);
            serde_json::json!({
                "Name": leaf,
                "CRC32": format!("0x{:08X}", e.crc32),
                "Size": e.uncompressed_size.to_string(),
                "CompressionType": e.compression_method.to_string(),
                "Encrypted": e.is_encrypted.to_string(),
            })
        })
        .collect();
    let obj = serde_json::json!({ "Name": dir_name, "Files": files });
    let bytes = serde_json::to_vec_pretty(&obj)?;
    std::fs::write(path, &bytes)?;
    Ok(())
}

fn compare_dictionary_order(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut ai = a.bytes();
    let mut bi = b.bytes();
    let mut tiebreak = Ordering::Equal;
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return tiebreak,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) => match sort_weight(x).cmp(&sort_weight(y)) {
                Ordering::Equal => {
                    if tiebreak == Ordering::Equal {
                        tiebreak = x.cmp(&y);
                    }
                    continue;
                }
                other => return other,
            },
        }
    }
}

#[inline]
fn sort_weight(b: u8) -> u8 {
    match b {
        b' ' => 0x01,
        b'_' => 0x02,
        b'-' => 0x03,
        b'A'..=b'Z' => b + 0x20,
        _ => b,
    }
}

fn xml_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn xml_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn extract_localization(p4k: &MappedP4k, output: &Path) -> Result<()> {
    let candidates = [
        "Data/Localization/english/global.ini",
        "Data\\Localization\\english\\global.ini",
    ];
    for path in candidates {
        if let Some(entry) = p4k.entry_case_insensitive(path) {
            let data = p4k.read(entry)?;
            let rel = entry.name.replace('\\', "/");
            let out_path = output.join(&rel);
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&out_path, &data)?;
            return Ok(());
        }
    }
    eprintln!("[WARN] Localization file not found in P4k");
    Ok(())
}

fn extract_tag_database(p4k: &MappedP4k, output: &Path) -> Result<()> {
    let needle = "tagdatabase.tagdatabase.xml";
    let entry = p4k
        .entries()
        .iter()
        .find(|e| e.name.to_ascii_lowercase().contains(needle));
    let Some(entry) = entry else {
        eprintln!("[WARN] TagDatabase not found in P4k");
        return Ok(());
    };

    let bytes = p4k.read(entry)?;
    let rel = entry.name.replace('\\', "/");
    let out_path = output.join(&rel);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if starbreaker_cryxml::is_cryxmlb(&bytes) {
        match starbreaker_cryxml::from_bytes(&bytes) {
            Ok(xml) => std::fs::write(&out_path, format!("{xml}").as_bytes())?,
            Err(_) => std::fs::write(&out_path, &bytes)?,
        }
    } else {
        std::fs::write(&out_path, &bytes)?;
    }
    Ok(())
}

fn dump_data_core_types(
    db: &starbreaker_datacore::database::Database,
    output: &Path,
) -> Result<()> {
    std::fs::create_dir_all(output)?;
    let struct_defs = db.struct_defs();
    let property_defs = db.property_defs();

    struct_defs.par_iter().try_for_each(|def| -> Result<()> {
        let name = db.resolve_string2(def.name_offset);
        let parent_idx = def.parent_type_index;
        let mut s = String::with_capacity(512);
        s.push_str("\u{FEFF}<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n");
        s.push_str(&format!("<Struct Name=\"{}\"", xml_attr(name)));
        if parent_idx != -1 {
            let parent = db.resolve_string2(struct_defs[parent_idx as usize].name_offset);
            s.push_str(&format!(" Parent=\"{}\"", xml_attr(parent)));
        }

        let first = def.first_attribute_index as usize;
        let count = def.attribute_count as usize;
        if count == 0 {
            s.push_str(" />");
        } else {
            s.push_str(">\r\n");
            for prop in &property_defs[first..first + count] {
                let prop_name = db.resolve_string2(prop.name_offset);
                let type_str = property_type_string(db, prop);
                s.push_str(&format!(
                    "  <Property Name=\"{}\" Type=\"{}\" />\r\n",
                    xml_attr(prop_name),
                    xml_attr(&type_str),
                ));
            }
            s.push_str("</Struct>");
        }
        let out_path = output.join(format!("{name}.xml"));
        std::fs::write(&out_path, s)
            .map_err(|e| CliError::IoPath { source: e, path: out_path.display().to_string() })?;
        Ok(())
    })?;
    Ok(())
}

fn property_type_string(
    db: &starbreaker_datacore::database::Database,
    prop: &starbreaker_datacore::types::PropertyDefinition,
) -> String {
    use starbreaker_datacore::enums::{ConversionType, DataType};

    let scalar = match DataType::try_from(prop.data_type) {
        Ok(DataType::Boolean) => "bool".to_string(),
        Ok(DataType::Byte) => "byte".to_string(),
        Ok(DataType::SByte) => "sbyte".to_string(),
        Ok(DataType::Int16) => "short".to_string(),
        Ok(DataType::UInt16) => "ushort".to_string(),
        Ok(DataType::Int32) => "int".to_string(),
        Ok(DataType::UInt32) => "uint".to_string(),
        Ok(DataType::Int64) => "long".to_string(),
        Ok(DataType::UInt64) => "ulong".to_string(),
        Ok(DataType::Single) => "float".to_string(),
        Ok(DataType::Double) => "double".to_string(),
        Ok(DataType::Guid) => "CigGuid".to_string(),
        Ok(DataType::Locale) | Ok(DataType::String) => "string".to_string(),
        Ok(DataType::EnumChoice) => db
            .resolve_string2(db.enum_defs()[prop.struct_index as usize].name_offset)
            .to_string(),
        Ok(DataType::Class)
        | Ok(DataType::Reference)
        | Ok(DataType::StrongPointer)
        | Ok(DataType::WeakPointer) => db
            .resolve_string2(db.struct_defs()[prop.struct_index as usize].name_offset)
            .to_string(),
        Err(_) => "unknown".to_string(),
    };

    match ConversionType::try_from(prop.conversion_type) {
        Ok(ConversionType::Attribute) => scalar,
        _ => format!("{scalar}[]"),
    }
}

fn dump_data_core_enums(
    db: &starbreaker_datacore::database::Database,
    output: &Path,
) -> Result<()> {
    std::fs::create_dir_all(output)?;
    let enum_defs = db.enum_defs();

    (0..enum_defs.len() as i32)
        .into_par_iter()
        .try_for_each(|enum_index| -> Result<()> {
            let def = &enum_defs[enum_index as usize];
            let name = db.resolve_string2(def.name_offset);
            let options = db.enum_options(enum_index);

            let mut s = String::with_capacity(256);
            s.push_str("\u{FEFF}<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n");
            s.push_str(&format!("<Enum Name=\"{}\"", xml_attr(name)));
            if options.is_empty() {
                s.push_str(" />");
            } else {
                s.push_str(">\r\n");
                for opt in options {
                    let value = db.resolve_string2(*opt);
                    s.push_str(&format!("  <Value>{}</Value>\r\n", xml_text(value)));
                }
                s.push_str("</Enum>");
            }
            let out_path = output.join(format!("{name}.xml"));
            std::fs::write(&out_path, s)
                .map_err(|e| CliError::IoPath { source: e, path: out_path.display().to_string() })?;
            Ok(())
        })?;
    Ok(())
}

fn extract_dcb_to_zst(p4k: &MappedP4k, zst_path: &Path) -> Result<()> {
    let dcb_bytes = p4k
        .read_file("Data\\Game2.dcb")
        .or_else(|_| p4k.read_file("Data\\Game.dcb"))?;
    if let Some(parent) = zst_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = File::create(zst_path)?;
    let compressed = zstd::stream::encode_all(&dcb_bytes[..], 3)?;
    out.write_all(&compressed)?;
    Ok(())
}

fn compress_file_to_zst(input: &Path, zst_path: &Path) -> Result<()> {
    if let Some(parent) = zst_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut input_file = File::open(input)
        .map_err(|e| CliError::IoPath { source: e, path: input.display().to_string() })?;
    let mut buf = Vec::new();
    input_file.read_to_end(&mut buf)?;
    let compressed = zstd::stream::encode_all(&buf[..], 3)?;
    std::fs::write(zst_path, &compressed)?;
    Ok(())
}

fn copy_build_manifest(input: &Path, output: &Path) -> Result<()> {
    if !input.exists() {
        eprintln!("[WARN] {} not found — skipping build manifest copy", input.display());
        return Ok(());
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(input, output)
        .map_err(|e| CliError::IoPath { source: e, path: output.display().to_string() })?;
    Ok(())
}
