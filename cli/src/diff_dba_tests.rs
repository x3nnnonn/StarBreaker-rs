use super::*;

#[test]
#[ignore = "requires STARBREAKER_DIFF_TEST_P4K and STARBREAKER_DIFF_TEST_OUTPUT"]
fn live_readable_dba_sample() {
    let archive=std::env::var_os("STARBREAKER_DIFF_TEST_P4K").expect("archive path required");
    let root=std::env::var_os("STARBREAKER_DIFF_TEST_OUTPUT").expect("output path required");
    let p4k=MappedP4k::open(Path::new(&archive)).unwrap();
    let entry=p4k.entries().iter().find(|e| e.name.to_ascii_lowercase().ends_with("ships\\rsi\\scorpius.dba")).unwrap();
    let data=p4k.read(entry).unwrap();
    extract_content_bytes(Path::new(&root),"Scorpius.dba",&data).unwrap();
    let output=std::fs::read(Path::new(&root).join("Scorpius.dba.json")).unwrap();
    let value:serde_json::Value=serde_json::from_slice(&output).unwrap();
    assert!(value.get("animation_database_error").is_none(),"{value}");
    assert_eq!(value["animation_database"]["tracks_complete"],true);
    assert_eq!(value["animation_database"]["clip_count"],55);
    let clips=value["animation_database"]["clips"].as_array().unwrap();
    assert_eq!(clips.len(),55);
    for clip in clips {
        assert_eq!(clip["metadata"]["joint_track_count"].as_u64().unwrap() as usize,clip["joint_tracks"].as_array().unwrap().len());
        assert_eq!(clip["metadata"]["frames_per_second"],30);
    }
    println!("Exported {} bytes, {} readable clips to {}",output.len(),clips.len(),Path::new(&root).join("Scorpius.dba.json").display());
}
