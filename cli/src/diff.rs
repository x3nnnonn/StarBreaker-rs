use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Args, ValueEnum};
use rayon::prelude::*;
use starbreaker_p4k::{MappedP4k, P4kArchive, P4kEntry};

use crate::common::load_p4k;
use crate::dcb::DcbFormat;
use crate::error::{CliError, Result};

#[cfg(test)]
#[path = "diff_container_cache_tests.rs"]
mod container_cache_tests;

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
#[command(about = "Export a diff snapshot including DataCore, XML assets, animation databases, object containers and shaders")]
pub struct DiffArgs {
    #[arg(long, short = 'g', env = "GAME_FOLDER")]
    pub game: PathBuf,
    #[arg(long, short = 'o', env = "OUTPUT_FOLDER")]
    pub output: PathBuf,
    #[arg(long, short = 'k', env = "KEEP_OLD")]
    pub keep: bool,
    #[arg(long, short = 'f', value_enum, default_value = "xml", env = "TEXT_FORMAT")]
    pub format: ManifestFormat,
    #[arg(long, env = "EXTRACT_DDS")]
    pub extract_dds: bool,
    #[arg(
        long,
        env = "DIFF_AGAINST",
        alias = "base",
        help = "Deprecated and ignored. CRC32 comparison always uses the existing output dump"
    )]
    pub diff_against: Option<PathBuf>,
    #[arg(long)]
    pub dds_only: bool,
    #[arg(long, help = "Regenerate all outputs instead of reusing validated unchanged exports")]
    pub rebuild: bool,
}

impl DiffArgs {
    pub fn run(self) -> Result<()> {
        let DiffArgs {
            game,
            output,
            keep,
            format,
            extract_dds,
            diff_against,
            dds_only,
            rebuild,
        } = self;

        if dds_only && !extract_dds {
            return Err(CliError::InvalidInput(
                "--dds-only requires --extract-dds".into(),
            ));
        }

        let total = Instant::now();
        if diff_against.is_some() { eprintln!("[INFO] --diff-against is ignored. Comparing with the existing dump in {}", output.display()); }
        let baseline_path = crate::diff_index::exists(&output).then_some(output.as_path());
        let previous = baseline_path.map(|path| {
            let start = Instant::now();
            let index = crate::diff_index::load(path)?;
            eprintln!("[DONE] Baseline: {} entries from {} in {:.1}s", index.len(), path.display(), start.elapsed().as_secs_f64());
            Ok::<_, CliError>(index)
        }).transpose()?;
        let p4k_path = game.join("Data.p4k");
        let exe_path = game.join("Bin64").join("StarCitizen.exe");
        let p4k = load_p4k(Some(&p4k_path))?;
        std::fs::create_dir_all(&output)?;
        let reuse = if rebuild { None } else { previous.as_ref() };
        if dds_only {
            let current = crate::diff_index::current(&p4k)?;
            timed("DDS extraction", || {
                crate::diff_dds::extract_dds_files(
                    &p4k,
                    &output.join("DDS_Files"),
                    &current,
                    reuse,
                )
            })?;
            eprintln!("[DONE] total: {:.1}s", total.elapsed().as_secs_f64());
            return Ok(());
        }

        let p4kcontents_dir = output.join("P4kContents");

        let mut current = crate::diff_index::FileIndex::new();
        timed("Current archive CRC32 inventory", || {
            current = crate::diff_index::current(&p4k)?;
            Ok(())
        })?;
        timed("Localization", || extract_localization(&p4k, &p4kcontents_dir, reuse))?;
        let dcb_entry = p4k.entry_case_insensitive("Data\\Game2.dcb").or_else(|| p4k.entry_case_insensitive("Data\\Game.dcb"))
            .ok_or_else(|| CliError::NotFound("DataCore in current P4K".into()))?;
        timed("DataCore records, types, enums and backup", || {
            if crate::diff_index::unchanged(reuse, &dcb_entry.name, dcb_entry.crc32, dcb_entry.uncompressed_size)
                && output.join("DataCore.dcb.zst").is_file()
                && crate::diff_outputs::has_extension(&output.join("DataCore"), format.extension())?
                && crate::diff_outputs::has_extension(&output.join("DataCoreTypes"), "xml")?
                && crate::diff_outputs::has_extension(&output.join("DataCoreEnums"), "xml")? {
                eprintln!("[SKIP] DataCore CRC32 and size unchanged");
                return Ok(());
            }
            let dcb_bytes = p4k.read(dcb_entry)?;
            let db = starbreaker_datacore::database::Database::from_bytes(&dcb_bytes)?;
            for directory in ["DataCore", "DataCoreTypes", "DataCoreEnums"] {
                let path = output.join(directory);
                if !keep && path.is_dir() { parallel_remove_dir_all(&path)?; }
            }
            crate::dcb::extract_database(
                &db,
                &output.join("DataCore"),
                match format {
                    ManifestFormat::Xml => DcbFormat::DataForge,
                    ManifestFormat::Json => DcbFormat::Json,
                },
                None,
            )?;
            dump_data_core_types(&db, &output.join("DataCoreTypes"))?;
            dump_data_core_enums(&db, &output.join("DataCoreEnums"))?;
            let backup = output.join("DataCore.dcb.zst");
            let mut writer = BufWriter::new(File::create(&backup)?);
            zstd::stream::copy_encode(dcb_bytes.as_slice(), &mut writer, 3)?;
            writer.flush()?;
            Ok(())
        })?;

        timed("P4k XML/animation/SOC contents", || {
            crate::diff_p4k_contents::extract_incremental_contents(&p4k, &p4kcontents_dir, reuse)
        })?;

        timed("Readable shaders and cache changes", || {
            crate::diff_shaders::export(&p4k, &output, previous.as_ref(), &current, keep)
        })?;

        if extract_dds {
            timed("DDS extraction", || {
                crate::diff_dds::extract_dds_files(
                    &p4k,
                    &output.join("DDS_Files"),
                    &current,
                    reuse,
                )
            })?;
        }

        if let Some(previous) = previous.as_ref() {
            timed("New SOCPAK list", || write_new_socpak_list(&current, previous, &output))?;
        }

        if exe_path.exists() {
            timed("StarCitizen.exe.zst", || {
                let backup = output.join("StarCitizen.exe.zst");
                if backup.is_file() && crate::diff_outputs::backup_matches(&exe_path, &backup)? {
                    return Ok(());
                }
                compress_file_to_zst(&exe_path, &backup)
            })?;
        } else {
            eprintln!("[SKIP] {} not found — skipping exe.zst", exe_path.display());
        }
        timed("build_manifest.json", || {
            copy_build_manifest(&game.join("build_manifest.id"), &output.join("build_manifest.json"))
        })?;
        if !keep {
            timed("Remove obsolete exported content", || crate::diff_p4k_contents::remove_obsolete_contents(&p4kcontents_dir, previous.as_ref(), &current))?;
        }
        timed("Update dump manifests", || {
            dump_p4k_manifest(&p4k, &output.join("P4k"), format)?;
            Ok(())
        })?;
        for name in ["P4k.index.json", "Diff.cache.json"] {
            let path = output.join(name);
            if path.is_file() { std::fs::remove_file(path)?; }
        }

        eprintln!("[DONE] total: {:.1}s", total.elapsed().as_secs_f64());
        Ok(())
    }
}

fn write_new_socpak_list(
    current: &crate::diff_index::FileIndex,
    previous: &crate::diff_index::FileIndex,
    output: &Path,
) -> Result<()> {
    let added = crate::diff_index::added_containers(current, previous);
    let content = if added.is_empty() { String::new() } else { format!("{}\n", added.join("\n")) };
    std::fs::write(output.join("New_SOCPAK_Files.txt"), content)?;
    eprintln!("Found {} new SOCPAK / object-container files.", added.len());
    Ok(())
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

pub(crate) fn dump_p4k_manifest(p4k: &MappedP4k, output: &Path, format: ManifestFormat) -> Result<crate::diff_index::FileIndex> {
    std::fs::create_dir_all(output)?;
    let ext = format.extension();
    let old_manifests = crate::diff_outputs::files_under(output)?;

    let mut by_dir: BTreeMap<String, Vec<&P4kEntry>> = BTreeMap::new();
    let mut socpak_refs: Vec<&P4kEntry> = Vec::new();
    for entry in p4k.entries() {
        let lower = entry.name.to_ascii_lowercase();
        let is_archive = lower.ends_with(".socpak") || lower.ends_with(".pak");
        if is_archive && (!lower.contains("shadercache_") || crate::diff_shaders::is_shader_archive(&entry.name)) {
            socpak_refs.push(entry);
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

    let written: Result<Vec<_>> = by_dir
        .par_iter()
        .filter(|(_, children)| !children.is_empty())
        .map(|(dir, children)| -> Result<PathBuf> {
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
            Ok(manifest_path)
        }).collect();
    let mut written = written?;

    let nested: Result<Vec<_>> = socpak_refs.par_iter().map(|socpak_entry| {
        if crate::diff_shaders::is_shader_archive(&socpak_entry.name) {
            return crate::diff_shaders::with_archive(p4k, socpak_entry, |inner| {
                let relative = socpak_entry.name.replace('\\', "/");
                let files = dump_archive_manifests(output, &relative, inner, ext, format)?;
                let mut index = crate::diff_index::FileIndex::new();
                crate::diff_shaders::add_inventory(&mut index, &relative, inner)?;
                Ok((index, files))
            });
        }
        let data = match p4k.read(socpak_entry) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("[WARN] could not read socpak {}: {e}", socpak_entry.name);
                return Err(e.into());
            }
        };
        let inner = match P4kArchive::from_bytes(&data) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("[WARN] could not parse socpak {}: {e}", socpak_entry.name);
                return Err(e.into());
            }
        };
        let socpak_rel = socpak_entry.name.replace('\\', "/");
        let files = dump_archive_manifests(output, &socpak_rel, &inner, ext, format)?;
        let mut index = crate::diff_index::FileIndex::new();
        crate::diff_index::add_archive(&mut index, &socpak_rel, &inner)?;
        Ok((index, files))
    }).collect();

    let mut index = crate::diff_index::FileIndex::with_capacity(p4k.entries().len());
    for entry in p4k.entries() { crate::diff_index::add_entry(&mut index, "", entry); }
    for (children, files) in nested? { index.extend(children); written.extend(files); }
    let retained: std::collections::HashSet<_> = written.iter().map(|path| crate::diff_index::key(&path.to_string_lossy())).collect();
    for path in old_manifests {
        if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("xml") || ext.eq_ignore_ascii_case("json"))
            && !retained.contains(&crate::diff_index::key(&path.to_string_lossy())) { std::fs::remove_file(path)?; }
    }
    crate::diff_outputs::prune_empty_directories(output)?;
    Ok(index)
}

fn dump_archive_manifests(
    output: &Path,
    base_rel: &str,
    archive: &P4kArchive<'_>,
    ext: &str,
    format: ManifestFormat,
) -> Result<Vec<PathBuf>> {
    let base_leaf = Path::new(base_rel)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "root".into());

    let mut by_dir: BTreeMap<String, Vec<&P4kEntry>> = BTreeMap::new();
    for entry in archive.entries().iter().filter(|entry| !entry.name.ends_with(['/', '\\'])) {
        let norm = entry.name.replace('\\', "/");
        let dir = norm.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
        by_dir.entry(dir).or_default().push(entry);
    }

    let mut files = Vec::new();
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
        files.push(manifest_path);
    }
    for entry in archive.entries().iter().filter(|entry| crate::p4k_compare::is_socpak_path(&entry.name)) {
        let bytes = archive.read(entry)?;
        let child = P4kArchive::from_bytes(&bytes)?;
        files.extend(dump_archive_manifests(output, &format!("{base_rel}/{}", entry.name.replace('\\', "/")), &child, ext, format)?);
    }
    Ok(files)
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
    crate::diff_outputs::write_changed(path, s.as_bytes())?;
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
    crate::diff_outputs::write_changed(path, &bytes)?;
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

fn extract_localization(p4k: &MappedP4k, output: &Path, previous: Option<&crate::diff_index::FileIndex>) -> Result<()> {
    let candidates = [
        "Data/Localization/english/global.ini",
        "Data\\Localization\\english\\global.ini",
    ];
    for path in candidates {
        if let Some(entry) = p4k.entry_case_insensitive(path) {
            let rel = entry.name.replace('\\', "/");
            let out_path = output.join(&rel);
            if out_path.is_file() && crate::diff_index::unchanged(previous, &entry.name, entry.crc32, entry.uncompressed_size) { return Ok(()); }
            let data = p4k.read(entry)?;
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

fn dump_data_core_types(
    db: &starbreaker_datacore::database::Database,
    output: &Path,
) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(output)?;
    let struct_defs = db.struct_defs();
    let property_defs = db.property_defs();

    struct_defs.par_iter().map(|def| -> Result<PathBuf> {
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
        Ok(out_path)
    }).collect()
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
) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(output)?;
    let enum_defs = db.enum_defs();

    (0..enum_defs.len() as i32)
        .into_par_iter()
        .map(|enum_index| -> Result<PathBuf> {
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
            Ok(out_path)
        }).collect()
}

fn compress_file_to_zst(input: &Path, zst_path: &Path) -> Result<()> {
    if let Some(parent) = zst_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let input_file = File::open(input)
        .map_err(|e| CliError::IoPath { source: e, path: input.display().to_string() })?;
    let mut output = BufWriter::new(File::create(zst_path)?);
    zstd::stream::copy_encode(BufReader::new(input_file), &mut output, 3)?;
    output.flush()?;
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
    crate::diff_outputs::write_changed(output, &std::fs::read(input)?)?;
    Ok(())
}
