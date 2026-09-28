use super::*;

#[test]
fn equivalent_audio_xml_has_identical_bytes() {
    let a = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<ATLConfig atl_name=\"Helmet\">\n    <AudioTriggers />\n    <AudioPreloads><Bank path=\"Banks/\" /></AudioPreloads>\n</ATLConfig>\n";
    let b = b"\xef\xbb\xbf<?xml version='1.0' encoding='utf-16'?>\r\n<ATLConfig atl_name='Helmet'>\r\n  <AudioTriggers></AudioTriggers>\r\n  <AudioPreloads>\r\n    <Bank path='Banks/'/>\r\n  </AudioPreloads>\r\n</ATLConfig>";
    let normalized = normalize_audio_xml(a).unwrap().unwrap();
    assert_eq!(normalized, normalize_audio_xml(b).unwrap().unwrap());
    assert_eq!(normalized, normalize_audio_xml(&normalized).unwrap().unwrap());
    let text = std::str::from_utf8(&normalized).unwrap();
    assert!(text.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n"));
    assert!(text.contains("\r\n    <AudioTriggers />\r\n"));
}

#[test]
fn utf16_audio_matches_utf8() {
    let text = "<ATLConfig atl_name=\"音楽\"><AudioTriggers /></ATLConfig>";
    let expected = normalize_audio_xml(text.as_bytes()).unwrap();
    for little in [true, false] {
        let mut bytes = if little { vec![0xff,0xfe] } else { vec![0xfe,0xff] };
        for word in text.encode_utf16() {
            bytes.extend(if little { word.to_le_bytes() } else { word.to_be_bytes() });
        }
        assert_eq!(normalize_audio_xml(&bytes).unwrap(), expected);
        assert_eq!(normalize_audio_xml(&bytes[2..]).unwrap(), expected);
    }
}

#[test]
fn production_audio_export_matches_across_source_encodings() {
    let root=std::env::temp_dir().join(format!("starbreaker-audio-encoding-{}",std::process::id()));
    assert!(!root.exists());
    let text=b"<?xml version='1.0' encoding='utf-8'?><ATLConfig></ATLConfig>";
    let mut binary=b"CryXmlB\0".to_vec();
    for value in [0u32,44,1,72,0,72,0,72,10,0,0,0,u32::MAX,0,0,0] { binary.extend(value.to_le_bytes()); }
    binary.extend(b"ATLConfig\0");
    let utf16:Vec<_>="<ATLConfig />".encode_utf16().flat_map(u16::to_le_bytes).collect();
    let expected=normalize_audio_xml(text).unwrap().unwrap();
    for (name,input) in [("plain.xml",text.as_slice()),("compiled.xml",binary.as_slice()),("utf16.xml",utf16.as_slice())] {
        crate::diff_p4k_contents::extract_content_bytes(&root,name,input).unwrap();
        assert_eq!(std::fs::read(root.join(name)).unwrap(),expected,"{name}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn real_changes_order_comments_and_escaped_values_survive() {
    let a = b"<ATLConfig><!--note--><Bank name='a &amp; &quot;b&quot;' empty='' newline='&#10;' /><Bank name='second'/></ATLConfig>";
    let normalized = normalize_audio_xml(a).unwrap().unwrap();
    let text = std::str::from_utf8(&normalized).unwrap();
    assert!(text.contains("<!--note-->"));
    assert!(text.contains("empty=\"\" newline=\"&#10;\""));
    assert!(text.contains("name=\"a &amp; &quot;b&quot;\""));
    let changed = text.replace("second", "different");
    assert_ne!(Some(normalized), normalize_audio_xml(changed.as_bytes()).unwrap());
}

#[test]
fn text_preserving_and_unrelated_documents_are_not_reformatted() {
    for text in ["<ATLConfig><Label> text </Label></ATLConfig>",
        "<ATLConfig><Label>  </Label></ATLConfig>",
        "<ATLConfig xml:space='preserve'> <X/> </ATLConfig>",
        "<ATLConfig><![CDATA[value]]></ATLConfig>", "<Material><X /></Material>"] {
        assert!(normalize_audio_xml(text.as_bytes()).unwrap().is_none());
    }
    assert!(normalize_audio_xml(b"<ATLConfig><X></ATLConfig>").is_err());
    assert!(normalize_audio_xml(b"<ATLConfig>").is_err());
    assert!(normalize_audio_xml(b"<ATLConfig/><ATLConfig/>").is_err());
}

#[test]
#[ignore = "requires STARBREAKER_DIFF_TEST_P4K and STARBREAKER_DIFF_TEST_OUTPUT"]
fn live_audio_xml_exports() {
    use std::path::Path;
    use starbreaker_p4k::MappedP4k;
    let archive=std::env::var_os("STARBREAKER_DIFF_TEST_P4K").expect("archive path required");
    let root=std::env::var_os("STARBREAKER_DIFF_TEST_OUTPUT").expect("output path required");
    let p4k=MappedP4k::open(Path::new(&archive)).unwrap();
    let mut count=0;
    let mut unchanged=0;
    for entry in p4k.entries().iter().filter(|entry| {
        let path=entry.name.replace('\\',"/").to_ascii_lowercase();
        path.starts_with("data/libs/gameaudio/") && path.ends_with(".xml")
    }) {
        let bytes=p4k.read(entry).unwrap();
        let text=if starbreaker_cryxml::is_cryxmlb(&bytes) {
            starbreaker_cryxml::from_bytes(&bytes).unwrap().to_string().into_bytes()
        } else { bytes.clone() };
        let expected=if let Some(expected)=normalize_audio_xml(&text).unwrap() {
            assert_eq!(normalize_audio_xml(&expected).unwrap().unwrap(),expected,"{}",entry.name);
            count+=1;
            expected
        } else {
            let source=decode_text(&text).unwrap().unwrap();
            let mut reader=Reader::from_str(&source);
            loop {
                match reader.read_event().unwrap() {
                    Event::Start(root) | Event::Empty(root) => {
                        assert_ne!(root.name().as_ref(),b"ATLConfig","unexpected unformatted audio config: {}",entry.name);
                        break;
                    }
                    Event::Eof => panic!("missing root: {}",entry.name),
                    _ => {}
                }
            }
            unchanged+=1;
            crate::diff_p4k_contents::text_to_crlf_bytes(&text)
        };
        let relative=entry.name.replace('\\',"/");
        crate::diff_p4k_contents::extract_content_bytes(Path::new(&root),&relative,&bytes).unwrap();
        assert_eq!(std::fs::read(Path::new(&root).join(&relative)).unwrap(),expected);
    }
    assert!(count>0);
    println!("Verified {count} normalized audio configs and {unchanged} other GameAudio XML exports");
}
