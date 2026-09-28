use super::*;

fn shader_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut bytes = crate::diff_p4k_contents::tests::stored_archive(entries);
    let mut local = 0;
    let mut central: usize = entries.iter().map(|(name, data)| 30 + name.len() + data.len()).sum();
    for (name, data) in entries {
        let crc = crc32fast::hash(data).to_le_bytes();
        bytes[local + 14..local + 18].copy_from_slice(&crc);
        bytes[central + 16..central + 20].copy_from_slice(&crc);
        local += 30 + name.len() + data.len();
        central += 46 + name.len();
    }
    bytes
}

fn fixture(shader: &str, cache: &[u8], include_removed: bool) -> Vec<u8> {
    let bytes = crate::shader_decode::tests::binary(&[600, 36], &[(600, shader)]);
    let mut entries = vec![("Shaders/test.cfxb", bytes.as_slice()), ("Shaders/stable.ext", b"Text\r\n".as_slice()), ("Shaders/test.shcb", cache)];
    if include_removed { entries.push(("Shaders/remove.ext", b"Remove")); }
    let inner = shader_zip(&entries);
    shader_zip(&[("Engine/ShaderCache_D3D11.pak", inner.as_slice())])
}

#[test]
fn shader_incremental_pipeline_uses_dump_manifests_and_repairs_missing_sources() {
    let root = std::env::temp_dir().join(format!("starbreaker-shaders-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("Data.p4k");
    let output = root.join("snapshot");
    let source = output.join("ShaderSources/engine/shadercache_d3d11.pak/shaders/test.cfxb.txt");
    let stable = output.join("ShaderSources/engine/shadercache_d3d11.pak/shaders/stable.ext.txt");
    let removed = output.join("ShaderSources/engine/shadercache_d3d11.pak/shaders/remove.ext.txt");
    std::fs::write(&path, fixture("Original", b"cache1", true)).unwrap();
    let p4k = MappedP4k::open(&path).unwrap();
    let index = crate::diff_index::current(&p4k).unwrap();
    export(&p4k, &output, None, &index, false).unwrap();
    crate::diff::dump_p4k_manifest(&p4k, &output.join("P4k"), crate::diff::ManifestFormat::Xml).unwrap();
    let previous = crate::diff_index::load(&output).unwrap();
    assert_eq!(previous["engine/shadercache_d3d11.pak/shaders/test.shcb"], (crc32fast::hash(b"cache1"), 6));
    let stamp = std::fs::metadata(&stable).unwrap().modified().unwrap();
    let unchanged = change_report(&index, Some(&previous));
    assert_eq!(unchanged["archives"]["engine/shadercache_d3d11.pak"]["changes"].as_array().unwrap().len(), 0);
    std::fs::remove_file(&source).unwrap();
    export(&p4k, &output, Some(&previous), &index, false).unwrap();
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "Original;\n");
    assert_eq!(std::fs::metadata(&stable).unwrap().modified().unwrap(), stamp);
    drop(p4k);
    std::fs::write(&path, fixture("Modified", b"cache2", false)).unwrap();
    let p4k = MappedP4k::open(&path).unwrap();
    let current = crate::diff_index::current(&p4k).unwrap();
    export(&p4k, &output, Some(&previous), &current, false).unwrap();
    assert!(!removed.exists());
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "Modified;\n");
    assert_eq!(std::fs::metadata(&stable).unwrap().modified().unwrap(), stamp);
    let report = change_report(&current, Some(&previous));
    let summary = &report["archives"]["engine/shadercache_d3d11.pak"];
    assert_eq!(summary["modified"], 2);
    assert_eq!(summary["removed"], 1);
    crate::diff::dump_p4k_manifest(&p4k, &output.join("P4k"), crate::diff::ManifestFormat::Json).unwrap();
    let retained = crate::diff_index::load(&output).unwrap();
    assert_eq!(retained["engine/shadercache_d3d11.pak/shaders/test.shcb"], (crc32fast::hash(b"cache2"), 6));
    export(&p4k, &output, Some(&retained), &FileIndex::new(), false).unwrap();
    assert!(!source.exists());
    drop(p4k);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn shader_report_bootstrap_added_removed_and_outer_only_changes() {
    let outer = "engine/shadercache_vulkan.pak".to_owned();
    let a = format!("{outer}/shaders/a.shcb");
    let b = format!("{outer}/shaders/b.shcb");
    let previous = FileIndex::from([(outer.clone(), (1, 10)), (a.clone(), (2, 20))]);
    let current = FileIndex::from([(outer.clone(), (3, 40)), (a.clone(), (2, 20)), (b, (4, 50))]);
    assert_eq!(change_report(&current, None)["archives"][&outer]["added"], 0);
    assert_eq!(change_report(&current, None)["archives"][&outer]["baseline_initialized"], false);
    let report = change_report(&current, Some(&previous));
    assert_eq!(report["archives"][&outer]["added"], 1);
    assert_eq!(report["archives"][&outer]["modified"], 0);
    assert_eq!(change_report(&FileIndex::new(), Some(&previous))["archives"][&outer]["removed"], 1);
    let new_outer = "engine/shadercache_ext.pak".to_owned();
    let new_index = FileIndex::from([(new_outer.clone(), (7, 99)), (format!("{new_outer}/shaders/new.ext"), (8, 11))]);
    assert_eq!(change_report(&new_index, Some(&previous))["archives"][&new_outer]["added"], 1);
}

#[test]
fn shader_corruption_and_unsafe_paths_fail_before_output() {
    let root = std::env::temp_dir().join(format!("starbreaker-shader-invalid-{}", std::process::id()));
    let mut bytes = shader_zip(&[("safe.ext", b"valid"), ("bad.ext", b"corrupt")]);
    bytes[30 + "safe.ext".len()] ^= 1;
    let archive = P4kArchive::from_bytes(&bytes).unwrap();
    assert!(export_archive(&root, "engine/shadercache_ext.pak", &archive).is_err());
    assert!(!root.exists());
    let bytes = shader_zip(&[("../escape.ext", b"text")]);
    let archive = P4kArchive::from_bytes(&bytes).unwrap();
    assert!(add_inventory(&mut FileIndex::new(), "engine/shadercache_ext.pak", &archive).is_err());
    assert!(export_archive(&root, "engine/shadercache_ext.pak", &archive).is_err());
    assert!(!root.exists());
}

#[test]
#[ignore]
fn shader_actual_ptu_export_and_repeat() {
    let archive_path = std::env::var("SC_SHADER_TEST_P4K").expect("SC_SHADER_TEST_P4K");
    let output = std::path::PathBuf::from(std::env::var("SC_SHADER_TEST_OUTPUT").expect("SC_SHADER_TEST_OUTPUT"));
    let p4k = MappedP4k::open(archive_path).unwrap();
    let mut index = FileIndex::new();
    for entry in p4k.entries().iter().filter(|entry| is_shader_archive(&entry.name)) {
        crate::diff_index::add_entry(&mut index, "", entry);
        with_archive(&p4k, entry, |archive| add_inventory(&mut index, &entry.name, archive)).unwrap();
        with_archive(&p4k, entry, |archive| {
            for member in archive.entries().iter().filter(|member| member.name.ends_with(".cfxb") || member.name.ends_with(".cfib")) {
                let bytes = archive.read(member)?;
                let (text, missing) = crate::shader_decode::decode(&bytes)?;
                assert!(missing.is_empty(), "{}: {missing:?}", member.name);
                crate::shader_format::tests::assert_token_contents_preserved(&bytes, &text);
            }
            Ok(())
        }).unwrap();
    }
    let start = std::time::Instant::now();
    export(&p4k, &output, None, &index, false).unwrap();
    println!("First shader export: {:.2}s, {} index entries", start.elapsed().as_secs_f64(), index.len());
    let files = crate::diff_outputs::files_under(&output.join("ShaderSources")).unwrap();
    let before: Vec<_> = files.iter().map(|path| (path, std::fs::metadata(path).unwrap().modified().unwrap())).collect();
    let start = std::time::Instant::now();
    export(&p4k, &output, Some(&index), &index, false).unwrap();
    println!("Repeat shader export: {:.2}s, {} readable files", start.elapsed().as_secs_f64(), files.len());
    for (path, timestamp) in before { assert_eq!(std::fs::metadata(path).unwrap().modified().unwrap(), timestamp); }
    assert!(!files.is_empty());
}
