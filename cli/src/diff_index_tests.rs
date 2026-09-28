use super::*;

fn fixture_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("starbreaker-index-{name}-{}", std::process::id()));
    if root.exists() { std::fs::remove_dir_all(&root).unwrap(); }
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn xml_and_json_manifests_are_authoritative_without_sidecars() {
    let root = fixture_root("legacy");
    let textures = root.join("P4k/Data/Textures");
    let nested = root.join("P4k/Data/Test.SOCPAK/entdata");
    std::fs::create_dir_all(&textures).unwrap();
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::create_dir_all(root.join("P4k/Data/empty.pak")).unwrap();
    std::fs::write(textures.join("Textures.xml"), "\u{feff}<?xml version=\"1.0\"?><Directory Name=\"Textures\"><File Name=\"A&amp;B.DDS\" CRC32=\"0x00001234\" Size=\"99\"/><File Name=\"A&amp;B.DDS.1\" CRC32=\"0x00005678\" Size=\"100\"/></Directory>").unwrap();
    std::fs::write(nested.join("entdata.json"), r#"{"Name":"entdata","Files":[{"Name":"one.entxml","CRC32":"0xAABBCCDD","Size":"42"}]}"#).unwrap();
    let legacy = load(&root).unwrap();
    assert_eq!(legacy["data/textures/a&b.dds"], (0x1234, 99));
    assert_eq!(legacy["data/test.socpak/entdata/one.entxml"], (0xaabbccdd, 42));
    assert!(legacy.contains_key("data/test.socpak"));
    assert!(legacy.contains_key("data/empty.pak"));
    assert_eq!(load(&root.join("P4k")).unwrap(), legacy);
    std::fs::write(root.join("P4k.index.json"), b"invalid obsolete index").unwrap();
    std::fs::write(root.join("Diff.cache.json"), b"invalid obsolete cache").unwrap();
    assert_eq!(load(&root).unwrap(), legacy);
    std::fs::remove_dir_all(root.join("P4k")).unwrap();
    assert!(load(&root).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn changed_textures_include_split_changes_and_deduplicate_alpha() {
    let before = FileIndex::from([
        (key("Data\\A.DDS"), (1, 12)),
        (key("data/a.dds.1"), (2, 64)),
        (key("data/a.dds.a"), (3, 16)),
        (key("data/unchanged.dds"), (4, 8)),
        (key("data/gone.dds"), (5, 8)),
    ]);
    let mut after = before.clone();
    after.insert(key("data/a.dds.1"), (6, 64));
    after.insert(key("data/new.socpak/texture.dds"), (7, 32));
    after.remove("data/gone.dds");
    assert_eq!(crate::diff_dds::changed_dds_paths(&after, Some(&before)), vec!["data/a.dds", "data/new.socpak/texture.dds"]);
    assert!(crate::diff_dds::changed_dds_paths(&before, Some(&before)).is_empty());
    after = before.clone();
    after.remove("data/a.dds.1");
    assert_eq!(crate::diff_dds::changed_dds_paths(&after, Some(&before)), vec!["data/a.dds"]);
    assert_eq!(crate::diff_dds::changed_dds_paths(&before, None).len(), 3);
}

#[test]
fn new_containers_use_normalized_membership() {
    let before = FileIndex::from([(key("Data\\Old.SOCPAK"), (0, 0))]);
    let after = FileIndex::from([(key("data/old.socpak"), (1, 99)), (key("data/new.socpak"), (2, 42)), (key("data/new.socpak/a.entxml"), (3, 12))]);
    assert_eq!(added_containers(&after, &before), vec!["data/new.socpak"]);
}

#[test]
fn invalid_baseline_is_an_error() {
    let root = fixture_root("invalid");
    assert!(load(&root).is_err());
    std::fs::create_dir_all(root.join("P4k")).unwrap();
    std::fs::write(root.join("P4k/bad.xml"), "<Directory><File Name=\"../escape\" CRC32=\"0x1\" Size=\"4\"/></Directory>").unwrap();
    assert!(load(&root).is_err());
    std::fs::write(root.join("P4k/bad.xml"), "<Directory><File Name=\"a.dds\"/></Directory>").unwrap();
    assert!(load(&root).is_err());
    std::fs::write(root.join("P4k/bad.xml"), "<Directory><File Name=\"a.dds\" CRC32=\"0x1\" Size=\"4\"/>").unwrap();
    assert!(load(&root).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_baseline_does_not_clear_existing_output() {
    let root = fixture_root("preserve");
    let output = root.join("output");
    let file = output.join("P4kContents/retained.xml");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "keep").unwrap();
    std::fs::create_dir_all(output.join("P4k")).unwrap();
    std::fs::write(output.join("P4k/bad.xml"), "<Invalid/>").unwrap();
    let result = crate::diff::DiffArgs {
        game: root.join("missing-game"), output, keep: false,
        format: crate::diff::ManifestFormat::Xml, extract_dds: true,
        diff_against: Some(root.join("missing-baseline")), dds_only: false, rebuild: false,
    }.run();
    assert!(result.is_err());
    assert_eq!(std::fs::read_to_string(file).unwrap(), "keep");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_game_argument_is_ignored_and_existing_dump_is_used() {
    let root = fixture_root("legacy-argument");
    let game = root.join("PTU");
    let live = root.join("LIVE");
    let output = root.join("dump");
    std::fs::create_dir_all(&game).unwrap();
    let profile = live.join("user/client/0/Profiles/default/SavedViews.xml");
    std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
    std::fs::write(&profile, "<SavedViews/>").unwrap();
    let archive = crate::diff_p4k_contents::tests::stored_archive(&[("Data/example.xml", b"<Example/>")]);
    std::fs::write(game.join("Data.p4k"), archive).unwrap();
    let p4k = MappedP4k::open(&game.join("Data.p4k")).unwrap();
    crate::diff::dump_p4k_manifest(&p4k, &output.join("P4k"), crate::diff::ManifestFormat::Xml).unwrap();
    assert!(load(&live).unwrap_err().to_string().contains("P4k manifest directory"));
    let retained_image = output.join("DDS_Files/retained.png");
    std::fs::create_dir_all(retained_image.parent().unwrap()).unwrap();
    std::fs::write(&retained_image, b"existing image").unwrap();
    crate::diff::DiffArgs {
        game, output: output.clone(), keep: false, format: crate::diff::ManifestFormat::Xml,
        extract_dds: true, diff_against: Some(live), dds_only: true, rebuild: false,
    }.run().unwrap();
    assert_eq!(std::fs::read(&retained_image).unwrap(), b"existing image");
    assert!(!output.join("P4k.index.json").exists());
    assert!(!output.join("Diff.cache.json").exists());
    drop(p4k);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "requires STARBREAKER_DIFF_BASELINE and STARBREAKER_DIFF_TEST_P4K"]
fn live_dump_baseline_and_selection() {
    let root = PathBuf::from(std::env::var_os("STARBREAKER_DIFF_BASELINE").unwrap());
    let archive = PathBuf::from(std::env::var_os("STARBREAKER_DIFF_TEST_P4K").unwrap());
    let start = std::time::Instant::now();
    let previous = load(&root).unwrap();
    eprintln!("Baseline: {} entries in {:.3}s", previous.len(), start.elapsed().as_secs_f64());
    let p4k = MappedP4k::open(&archive).unwrap();
    let current = current(&p4k).unwrap();
    let changed = crate::diff_dds::changed_dds_paths(&current, Some(&previous));
    eprintln!("Current: {} entries, {} changed images, {} new containers", current.len(), changed.len(), added_containers(&current, &previous).len());
    assert!(previous.len() > 1000);
    assert!(current.len() > 1000);
    assert!(crate::diff_dds::changed_dds_paths(&current, Some(&current)).is_empty());
    assert!(added_containers(&current, &current).is_empty());
}

#[test]
#[ignore = "requires STARBREAKER_DIFF_BEFORE and STARBREAKER_DIFF_AFTER"]
fn live_snapshot_output_equivalence() {
    let before = PathBuf::from(std::env::var_os("STARBREAKER_DIFF_BEFORE").unwrap());
    let after = PathBuf::from(std::env::var_os("STARBREAKER_DIFF_AFTER").unwrap());
    let selected = std::env::var("STARBREAKER_DIFF_DIRECTORIES").ok();
    for directory in ["P4k", "P4kContents", "DataCore", "DataCoreTypes", "DataCoreEnums"] {
        if selected.as_ref().is_some_and(|names| !names.split(',').any(|name| name == directory)) { continue; }
        let before_root = before.join(directory);
        let after_root = after.join(directory);
        let before_files = crate::diff_outputs::files_under(&before_root).unwrap();
        let after_files = crate::diff_outputs::files_under(&after_root).unwrap();
        let relative: std::collections::BTreeSet<_> = before_files.iter().map(|path| path.strip_prefix(&before_root).unwrap().to_path_buf()).collect();
        let actual: std::collections::BTreeSet<_> = after_files.iter().map(|path| path.strip_prefix(&after_root).unwrap().to_path_buf()).collect();
        assert_eq!(relative.difference(&actual).take(10).collect::<Vec<_>>(), Vec::<&PathBuf>::new(), "missing outputs in {directory}");
        assert_eq!(actual.difference(&relative).take(10).collect::<Vec<_>>(), Vec::<&PathBuf>::new(), "extra outputs in {directory}");
        before_files.par_iter().for_each(|path| {
            let destination = after_root.join(path.strip_prefix(&before_root).unwrap());
            assert_eq!(snapshot_crc32(std::fs::File::open(path).unwrap()), snapshot_crc32(std::fs::File::open(&destination).unwrap()), "different size or CRC32: {}", path.display());
        });
        eprintln!("{directory}: {} files match by size and CRC32", before_files.len());
    }
    for name in ["DataCore.dcb.zst", "StarCitizen.exe.zst"] {
        let old = zstd::stream::read::Decoder::new(std::fs::File::open(before.join(name)).unwrap()).unwrap();
        let new = zstd::stream::read::Decoder::new(std::fs::File::open(after.join(name)).unwrap()).unwrap();
        assert_eq!(snapshot_crc32(old), snapshot_crc32(new), "different decompressed backup: {name}");
    }
}

fn snapshot_crc32(mut reader: impl std::io::Read) -> (u64, u32) {
    let mut hash = crc32fast::Hasher::new();
    let mut length = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer).unwrap();
        if count == 0 { break; }
        hash.update(&buffer[..count]);
        length += count as u64;
    }
    (length, hash.finalize())
}

#[test]
fn snapshot_crc32_uses_ieee_polynomial_and_tracks_length() {
    assert_eq!(snapshot_crc32(&b"123456789"[..]), (9, 0xcbf43926));
    assert_eq!(snapshot_crc32(&b""[..]), (0, 0));
}
