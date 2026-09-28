use super::*;

#[test]
fn changed_archive_updates_snapshot_and_removes_obsolete_outputs() {
    let root = std::env::temp_dir().join(format!("starbreaker-incremental-{}", std::process::id()));
    if root.exists() { std::fs::remove_dir_all(&root).unwrap(); }
    std::fs::create_dir_all(&root).unwrap();
    let archive = root.join("Data.p4k");
    let output = root.join("snapshot");
    let contents = output.join("P4kContents");
    std::fs::write(&archive, super::tests::stored_archive(&[
        ("Data/changed.xml", b"<Value n=\"1\"/>"),
        ("Data/retained.xml", b"<Keep/>"),
        ("Data/removed.xml", b"<Remove/>"),
    ])).unwrap();
    let p4k = MappedP4k::open(&archive).unwrap();
    extract_incremental_contents(&p4k, &contents, None).unwrap();
    crate::diff::dump_p4k_manifest(&p4k, &output.join("P4k"), crate::diff::ManifestFormat::Xml).unwrap();
    let retained_stamp = std::fs::metadata(contents.join("Data/retained.xml")).unwrap().modified().unwrap();
    drop(p4k);
    std::fs::write(&archive, super::tests::stored_archive(&[
        ("Data/changed.xml", b"<Value n=\"2\"/>"),
        ("Data/retained.xml", b"<Keep/>"),
        ("Data/added.entxml", b"<Entity/>"),
    ])).unwrap();
    let p4k = MappedP4k::open(&archive).unwrap();
    let previous = crate::diff_index::load(&output).unwrap();
    let current = crate::diff_index::current(&p4k).unwrap();
    extract_incremental_contents(&p4k, &contents, Some(&previous)).unwrap();
    remove_obsolete_contents(&contents, Some(&previous), &current).unwrap();
    crate::diff::dump_p4k_manifest(&p4k, &output.join("P4k"), crate::diff::ManifestFormat::Xml).unwrap();
    assert_eq!(std::fs::read(contents.join("Data/changed.xml")).unwrap(), b"<Value n=\"2\"/>");
    assert_eq!(std::fs::metadata(contents.join("Data/retained.xml")).unwrap().modified().unwrap(), retained_stamp);
    assert_eq!(std::fs::read(contents.join("Data/added.entxml")).unwrap(), b"<Entity/>");
    assert!(!contents.join("Data/removed.xml").exists());
    std::fs::remove_file(contents.join("Data/retained.xml")).unwrap();
    let previous = crate::diff_index::load(&output).unwrap();
    extract_incremental_contents(&p4k, &contents, Some(&previous)).unwrap();
    assert_eq!(std::fs::read(contents.join("Data/retained.xml")).unwrap(), b"<Keep/>");
    assert!(!output.join("Diff.cache.json").exists());
    assert!(!output.join("P4k.index.json").exists());
    drop(p4k);
    std::fs::remove_dir_all(root).unwrap();
}
