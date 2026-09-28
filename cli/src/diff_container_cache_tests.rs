use super::*;
use crate::diff_p4k_contents::tests::stored_archive;

#[test]
fn parent_container_updates_and_removal_preserve_child_outputs() {
    let root = std::env::temp_dir().join(format!("starbreaker-container-cache-{}", std::process::id()));
    if root.exists() { std::fs::remove_dir_all(&root).unwrap(); }
    std::fs::create_dir_all(&root).unwrap();
    let archive = root.join("Data.p4k");
    let output = root.join("snapshot");
    let parent = stored_archive(&[("parent.entxml", b"<Parent version=\"1\"/>"), ("stable.entxml", b"<Stable/>")]);
    let changed_parent = stored_archive(&[("parent.entxml", b"<Parent version=\"2\"/>"), ("stable.entxml", b"<Stable/>")]);
    let child = stored_archive(&[("child.entxml", b"<Child/>")]);
    let generations = [
        stored_archive(&[("Data/parent.socpak", &parent), ("Data/parent/child.socpak", &child)]),
        stored_archive(&[("Data/parent.socpak", &changed_parent), ("Data/parent/child.socpak", &child)]),
        stored_archive(&[("Data/parent/child.socpak", &child)]),
    ];
    let child_path = output.join("P4kContents/Data/parent/child/child.entxml");
    let mut child_stamp = None;
    let mut stable_stamp = None;
    for (generation, bytes) in generations.into_iter().enumerate() {
        std::fs::write(&archive, bytes).unwrap();
        let p4k = MappedP4k::open(&archive).unwrap();
        let previous = crate::diff_index::exists(&output).then(|| crate::diff_index::load(&output).unwrap());
        let current = crate::diff_index::current(&p4k).unwrap();
        crate::diff_p4k_contents::extract_incremental_contents(&p4k, &output.join("P4kContents"), previous.as_ref()).unwrap();
        crate::diff_p4k_contents::remove_obsolete_contents(&output.join("P4kContents"), previous.as_ref(), &current).unwrap();
        dump_p4k_manifest(&p4k, &output.join("P4k"), ManifestFormat::Xml).unwrap();
        assert_eq!(std::fs::read(&child_path).unwrap(), b"<Child/>");
        let stamp = std::fs::metadata(&child_path).unwrap().modified().unwrap();
        if let Some(previous) = child_stamp { assert_eq!(stamp, previous); }
        child_stamp = Some(stamp);
        if generation == 0 { stable_stamp = Some(std::fs::metadata(output.join("P4kContents/Data/parent/stable.entxml")).unwrap().modified().unwrap()); }
        if generation == 1 {
            assert_eq!(Some(std::fs::metadata(output.join("P4kContents/Data/parent/stable.entxml")).unwrap().modified().unwrap()), stable_stamp);
            assert_eq!(std::fs::read(output.join("P4kContents/Data/parent/parent.entxml")).unwrap(), b"<Parent version=\"2\"/>");
        }
        if generation == 2 {
            assert!(!output.join("P4kContents/Data/parent/parent.entxml").exists());
            assert!(!output.join("P4k/Data/parent.socpak/parent.socpak.xml").exists());
            assert!(!crate::diff_index::load(&output).unwrap().contains_key("data/parent.socpak"));
        }
        drop(p4k);
    }
    std::fs::remove_dir_all(root).unwrap();
}
