use super::*;
use quick_xml::{Reader, events::Event};

#[test]
#[ignore = "requires STARBREAKER_DIFF_TEST_P4K and STARBREAKER_DIFF_TEST_OUTPUT"]
fn live_objectcontainer_entdata_exports() {
    let archive=std::env::var_os("STARBREAKER_DIFF_TEST_P4K").expect("archive path required");
    let output=std::env::var_os("STARBREAKER_DIFF_TEST_OUTPUT").expect("output path required");
    let p4k=MappedP4k::open(Path::new(&archive)).unwrap();
    let mut candidates:Vec<_>=p4k.entries().iter().filter(|entry| {
        ends_with_ci(&entry.name,".socpak") && entry.uncompressed_size < 8 * 1024 * 1024
    }).collect();
    candidates.sort_by_key(|entry| entry.uncompressed_size);
    let mut containers=0;
    let mut entities=0;
    let mut compiled=0;
    for entry in candidates {
        let bytes=p4k.read(entry).unwrap();
        let container=P4kArchive::from_bytes(&bytes).unwrap();
        let members:Vec<_>=container.entries().iter().filter(|member| ends_with_ci(&member.name,".entxml")).collect();
        if members.is_empty() { continue; }
        for member in members {
            let bytes=container.read(member).unwrap();
            let relative=socpak_nested_output_path(&entry.name,&member.name);
            assert!(is_diff_content(&member.name));
            extract_content_bytes(Path::new(&output),&relative,&bytes).unwrap();
            let text=std::fs::read_to_string(Path::new(&output).join(&relative)).unwrap();
            let mut reader=Reader::from_str(&text);
            let mut actual=Vec::new();
            loop {
                match reader.read_event().unwrap() {
                    Event::Start(node) | Event::Empty(node) => {
                        let attributes=node.attributes().map(|attribute| {
                            let attribute=attribute.unwrap();
                            (String::from_utf8(attribute.key.as_ref().to_vec()).unwrap(),attribute.unescape_value().unwrap().into_owned())
                        }).collect::<Vec<_>>();
                        actual.push((String::from_utf8(node.name().as_ref().to_vec()).unwrap(),attributes));
                    }
                    Event::Eof => break,
                    _ => {}
                }
            }
            assert!(!actual.is_empty(),"{relative}");
            if starbreaker_cryxml::is_cryxmlb(&bytes) {
                compiled+=1;
                let source=starbreaker_cryxml::from_bytes(&bytes).unwrap();
                let mut pending=vec![source.root()];
                let mut expected=Vec::new();
                while let Some(node)=pending.pop() {
                    expected.push((source.node_tag(node).to_owned(),source.node_attributes(node)
                        .map(|(key,value)|(key.to_owned(),value.to_owned())).collect::<Vec<_>>()));
                    pending.extend(source.node_children(node).collect::<Vec<_>>().into_iter().rev());
                }
                assert_eq!(actual,expected,"{relative}");
            }
            entities+=1;
        }
        containers+=1;
        println!("Verified entdata in {}",entry.name);
        if containers==8 { break; }
    }
    assert_eq!(containers,8);
    assert!(entities>=8);
    assert!(compiled>0);
    println!("Verified {entities} entity exports across {containers} object containers ({compiled} CryXML records)");
}
