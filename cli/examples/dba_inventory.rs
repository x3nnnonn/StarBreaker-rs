use std::{collections::BTreeMap, path::Path};
use starbreaker_chunks::ChunkFile;
use starbreaker_p4k::MappedP4k;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("P4K path");
    let audit = std::env::args().any(|arg| arg == "--audit");
    let p4k = MappedP4k::open(Path::new(&path))?;
    let mut inventory = BTreeMap::new();
    let mut formats = BTreeMap::new();
    let mut clips = 0u64;
    let mut keys = 0u64;
    let mut audited = 0usize;
    let mut audit_failures = 0usize;
    for entry in p4k.entries().iter().filter(|e| e.name.to_ascii_lowercase().ends_with(".dba")) {
        let bytes = p4k.read(entry)?;
        if audit {
            audited += 1;
            match starbreaker_3d::animation::dba::write_readable(&bytes, &mut std::io::sink()) {
                Ok(0) => {},
                result => { audit_failures += 1; println!("AUDIT {}: {result:?}",entry.name); },
            }
            continue;
        }
        let ChunkFile::Ivo(file) = ChunkFile::from_bytes(&bytes)? else { continue };
        for chunk in file.chunks() {
            *inventory.entry((format!("{:08x}",chunk.chunk_type),chunk.version)).or_insert(0u64) += 1;
            let data = file.chunk_data(chunk);
            if chunk.chunk_type == 0xF7351608 {
                let count = u32::from_le_bytes(data[..4].try_into()?);
                clips += count as u64;
                if entry.name.to_ascii_lowercase().contains("scorpius") || entry.uncompressed_size < 400 {
                    println!("{} meta={:02x?}",entry.name,&data[..data.len().min(72)]);
                }
            }
            if chunk.chunk_type != 0x194FBC50 { continue }
            let mut pos = 4;
            while data.get(pos..pos+4) == Some(b"#dba") {
                let count = u16::from_le_bytes(data[pos+4..pos+6].try_into()?) as usize;
                let controllers = pos+12+count*4;
                for index in 0..count {
                    for channel in [0,12] {
                        let start=controllers+index*24+channel;
                        let n=u16::from_le_bytes(data[start..start+2].try_into()?);
                        if n==0 {continue}
                        keys+=u64::from(n);
                        let flags=u16::from_le_bytes(data[start+2..start+4].try_into()?);
                        *formats.entry(format!("{}:{flags:04x}",if channel==0 {"rot"}else{"pos"})).or_insert(0u64)+=1;
                    }
                }
                pos=controllers+count*24;
            }
        }
    }
    if audit {
        println!("audited={audited} failures={audit_failures}");
        anyhow::ensure!(audited != 0 && audit_failures == 0, "DBA audit failed");
    } else {
        println!("chunks={inventory:?}\nformats={formats:?}\nclips={clips} keys={keys}");
    }
    Ok(())
}
