use super::*;

fn sample() -> Vec<u8> {
    let mut file = vec![0u8; 224];
    file[..4].copy_from_slice(b"#ivo");
    for (offset, value) in [(4,0x900u32),(8,2),(12,16),(16,ivo::DB_DATA),(20,0x900),
        (32,ivo::DBA),(36,0x902),(48,64),(120,1),(124,0),(128,0x12),(144,12)] {
        file[offset..offset+4].copy_from_slice(&value.to_le_bytes());
    }
    file[24..32].copy_from_slice(&48u64.to_le_bytes());
    file[40..48].copy_from_slice(&120u64.to_le_bytes());
    file[52..56].copy_from_slice(b"#dba");
    file[56..58].copy_from_slice(&1u16.to_le_bytes());
    file[58..60].copy_from_slice(&0xaa55u16.to_le_bytes());
    file[64..68].copy_from_slice(&0x12345678u32.to_le_bytes());
    file[68..70].copy_from_slice(&1u16.to_le_bytes());
    file[70]=0x40; file[71]=0x80;
    file[72..76].copy_from_slice(&24u32.to_le_bytes());
    file[76..80].copy_from_slice(&28u32.to_le_bytes());
    file[92]=7;
    file[108..112].copy_from_slice(&1f32.to_le_bytes());
    file[132..134].copy_from_slice(&30u16.to_le_bytes());
    file[134..136].copy_from_slice(&1u16.to_le_bytes());
    file[138..140].copy_from_slice(&9u16.to_le_bytes());
    file[160..164].copy_from_slice(&1f32.to_le_bytes());
    file[172..176].copy_from_slice(&42f32.to_le_bytes());
    file[176..191].copy_from_slice(b"animations/test");
    file
}

#[test]
fn readable_preserves_metadata_and_exact_key_times() {
    let mut output = Vec::new();
    assert_eq!(write_readable(&sample(), &mut output).unwrap(), 0);
    let value: Value = serde_json::from_slice(&output).unwrap();
    let clip=&value["clips"][0];
    assert_eq!(clip["metadata"]["name"], "animations/test");
    assert_eq!(clip["metadata"]["frames_per_second"],30);
    assert_eq!(clip["metadata"]["compression"],9);
    assert_eq!(clip["metadata"]["start_position_xyz"][2],42.0);
    assert_eq!(clip["joint_tracks"][0]["rotation"]["keys"][0],json!([7,0.0,0.0,0.0,1.0]));
}

#[test]
fn unknown_encoding_is_an_explicit_track_error() {
    let mut data=sample(); data[71]=0x99;
    let mut output=Vec::new();
    assert_eq!(write_readable(&data,&mut output).unwrap(),1);
    let value: Value=serde_json::from_slice(&output).unwrap();
    let track=&value["clips"][0]["joint_tracks"][0]["rotation"];
    assert!(track.get("keys").is_none());
    assert!(track["decode_error"].as_str().unwrap().contains("0x99"));
    assert_eq!(value["tracks_complete"],false);
}

#[test]
fn malformed_catalog_does_not_write_partial_json() {
    let mut data=sample(); data[120..124].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut output=Vec::new();
    assert!(write_readable(&data,&mut output).is_err());
    assert!(output.is_empty());
    for length in 0..sample().len() {
        let _=write_readable(&sample()[..length],&mut std::io::sink());
    }
}

#[test]
fn follows_declared_chunk_table_position() {
    let mut data=sample();
    let table=data[16..48].to_vec();
    let table_offset=data.len() as u32;
    data[12..16].copy_from_slice(&table_offset.to_le_bytes());
    data.extend(table);
    data[16..48].fill(0);
    let mut output=Vec::new();
    assert_eq!(write_readable(&data,&mut output).unwrap(),0);
    let value:Value=serde_json::from_slice(&output).unwrap();
    assert_eq!(value["clip_count"],1);
}

#[test]
fn declared_storage_cannot_cross_into_metadata() {
    let mut data=sample();
    data[48..52].copy_from_slice(&100u32.to_le_bytes());
    let mut output=Vec::new();
    assert!(write_readable(&data,&mut output).is_err());
    assert!(output.is_empty());
}

#[test]
fn source_quaternion_64bit_and_bitmap_times_decode() {
    let mut data=vec![0u8;80];
    data[..2].copy_from_slice(&2u16.to_le_bytes());
    data[2]=0x42; data[3]=0x83;
    data[4..8].copy_from_slice(&24u32.to_le_bytes());
    data[8..12].copy_from_slice(&32u32.to_le_bytes());
    data[24..26].copy_from_slice(&3u16.to_le_bytes());
    data[26..28].copy_from_slice(&12u16.to_le_bytes());
    data[28..30].copy_from_slice(&0x201u16.to_le_bytes());
    let packed=1048575u64 | (1048575u64<<21) | (524287u64<<42) | (3u64<<62);
    data[32..40].copy_from_slice(&packed.to_le_bytes());
    data[40..48].copy_from_slice(&packed.to_le_bytes());
    let value=track(&data,0,0).unwrap();
    assert!(value.get("decode_error").is_none(),"{value}");
    assert_eq!(value["keys"][0][0],3); assert_eq!(value["keys"][1][0],12);
    assert!(value["keys"][0][4].as_f64().unwrap()>0.99999);
    data[28]=0;
    assert!(track(&data,0,0).unwrap()["decode_error"].as_str().unwrap().contains("expected 2"));
}

#[test]
fn zero_track_clips_and_nonzero_mask_storage_keep_name_alignment() {
    let original=sample();
    let mut data=original[..120].to_vec();
    data.extend(2u32.to_le_bytes());
    data.extend(4u32.to_le_bytes());
    data.extend([0u8;48]);
    data.extend(&original[128..176]);
    data.extend([1,2,3,4]);
    data.extend(b"empty\0actual\0");
    let mut output=Vec::new();
    assert_eq!(write_readable(&data,&mut output).unwrap(),0);
    let value: Value=serde_json::from_slice(&output).unwrap();
    assert_eq!(value["clips"][0]["metadata"]["name"],"empty");
    assert_eq!(value["clips"][0]["joint_tracks"],json!([]));
    assert_eq!(value["clips"][1]["metadata"]["name"],"actual");
    assert_eq!(value["clips"][1]["joint_tracks"][0]["bone_hash"],"0x12345678");
    assert_eq!(value["subhierarchy_masks_hex"],"01020304");
}

#[test]
fn packed_positions_and_weights_keep_source_values() {
    let mut data=vec![0u8;96];
    data[12..14].copy_from_slice(&2u16.to_le_bytes());
    data[14]=0x40; data[15]=0xc2;
    data[16..20].copy_from_slice(&36u32.to_le_bytes());
    data[20..24].copy_from_slice(&40u32.to_le_bytes());
    data[36]=0;data[37]=9;
    for (i,value) in [2.0,f32::MAX,0.5,1.0,3.0,-1.0].into_iter().enumerate() {
        data[40+i*4..44+i*4].copy_from_slice(&value.to_le_bytes());
    }
    for (i,value) in [2u16,4,6,8].into_iter().enumerate() {
        data[64+i*2..66+i*2].copy_from_slice(&value.to_le_bytes());
    }
    assert_eq!(track(&data,0,12).unwrap()["keys"],json!([[0,5.0,3.0,2.0],[9,9.0,3.0,3.0]]));
    data[24..26].copy_from_slice(&2u16.to_le_bytes());
    data[26]=0x40;data[27]=0xf1;
    data[28..32].copy_from_slice(&36u32.to_le_bytes());
    data[32..36].copy_from_slice(&72u32.to_le_bytes());
    data[72..76].copy_from_slice(&0.25f32.to_le_bytes());
    data[76..80].copy_from_slice(&0.5f32.to_le_bytes());
    data[80..82].copy_from_slice(&2u16.to_le_bytes());
    data[82..84].copy_from_slice(&4u16.to_le_bytes());
    assert_eq!(track(&data,0,24).unwrap()["keys"],json!([[0,1.0],[9,1.5]]));
}
