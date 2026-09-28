use std::io::{Result, Write};
use serde_json::{Value, json};
use starbreaker_chunks::{ChunkFile, known_types::ivo};
use super::dba_tracks::{bytes, floats, invalid, track, u16_at, u32_at};

fn catalog(data: &[u8]) -> Result<(Vec<Value>, &[u8])> {
    let count = u32_at(data, 0)? as usize;
    let mask_size = u32_at(data, 4)? as usize;
    let entries = bytes(data, 8, count * 48)?;
    let masks = bytes(data, 8 + entries.len(), mask_size)?;
    let mut names = 8 + entries.len() + mask_size;
    let mut clips = Vec::with_capacity(count);
    for index in 0..count {
        let entry = &entries[index * 48..(index + 1) * 48];
        let remaining = data.get(names..).ok_or_else(|| invalid("name table exceeds metadata"))?;
        let length = remaining.iter().position(|&b| b == 0).ok_or_else(|| invalid("unterminated clip name"))?;
        let name = std::str::from_utf8(&remaining[..length]).map_err(|e| invalid(e.to_string()))?;
        names += length + 1;
        clips.push(json!({
            "index": index, "name": name, "flags": format!("0x{:08x}", u32_at(entry, 0)?),
            "frames_per_second": u16_at(entry, 4)?, "joint_track_count": u16_at(entry, 6)?,
            "cdik_target_count": u16_at(entry, 8)?, "compression": u16_at(entry, 10)?,
            "subhierarchy_mask_size": u16_at(entry, 12)?, "reserved": u16_at(entry, 14)?,
            "end_key": u32_at(entry, 16)? as i32,
            "start_rotation_xyzw": floats::<4>(entry, 20)?,
            "start_position_xyz": floats::<3>(entry, 36)?,
        }));
    }
    Ok((clips, masks))
}

pub fn write_readable(data: &[u8], writer: &mut impl Write) -> Result<usize> {
    let file = ChunkFile::from_bytes(data).map_err(|e| invalid(e.to_string()))?;
    let ChunkFile::Ivo(file) = file else { return Err(invalid("readable DBA requires IVO")); };
    let metadata = file.chunks().iter().find(|c| c.chunk_type == ivo::DBA)
        .ok_or_else(|| invalid("missing DBA metadata"))?;
    if metadata.version != 0x902 { return Err(invalid(format!("unsupported DBA metadata version {:#x}", metadata.version))); }
    let (clips, masks) = catalog(file.chunk_data(metadata))?;
    let storage = if let Some(chunk) = file.chunks().iter().find(|c| c.chunk_type == ivo::DB_DATA) {
        if chunk.version != 0x900 { return Err(invalid(format!("unsupported DBA data version {:#x}", chunk.version))); }
        let payload = file.chunk_data(chunk);
        bytes(payload, 4, u32_at(payload, 0)? as usize)?
    } else { &[] };
    let mut blocks = Vec::with_capacity(clips.len());
    let mut offset = 0;
    for clip in &clips {
        let count = clip["joint_track_count"].as_u64().unwrap() as usize;
        blocks.push(offset);
        if count == 0 { continue; }
        if bytes(storage, offset, 4)? != b"#dba" { return Err(invalid(format!("invalid joint block signature for clip {}", clip["index"]))); }
        if usize::from(u16_at(storage, offset + 4)?) != count { return Err(invalid(format!("joint count mismatch for clip {}", clip["index"]))); }
        bytes(storage, offset, 12 + count * 28)?;
        offset += 12 + count * 28;
    }
    let cdik = if let Some(chunk) = file.chunks().iter().find(|c| c.chunk_type == 0x570fccba) {
        if chunk.version != 0x901 { return Err(invalid(format!("unsupported CDIK version {:#x}", chunk.version))); }
        let payload = file.chunk_data(chunk);
        bytes(payload, 4, u32_at(payload, 0)? as usize)?
    } else { &[] };
    let mut cdik_blocks = Vec::with_capacity(clips.len());
    let mut cdik_offset = 0;
    for clip in &clips {
        let count = clip["cdik_target_count"].as_u64().unwrap() as usize;
        cdik_blocks.push(cdik_offset);
        if count == 0 { continue; }
        if bytes(cdik, cdik_offset, 4)? != b"#dba" || usize::from(u16_at(cdik, cdik_offset + 4)?) != count {
            return Err(invalid(format!("CDIK block mismatch for clip {}", clip["index"])));
        }
        bytes(cdik, cdik_offset, 12 + count * 108)?;
        cdik_offset += 12 + count * 108;
    }
    let additional: Vec<_> = file.chunks().iter().filter(|c| ![ivo::DBA, ivo::DB_DATA, 0x570fccba].contains(&c.chunk_type)).collect();
    write!(writer, "{{\r\n\"schema_version\":1,\r\n\"coordinate_space\":\"source\",\r\n\"rotation_key_layout\":[\"frame\",\"x\",\"y\",\"z\",\"w\"],\r\n\"position_key_layout\":[\"frame\",\"x\",\"y\",\"z\"],\r\n\"clip_count\":{},\r\n\"clips\":[\r\n", clips.len())?;
    let mut failures = 0;
    for (index, (clip, block)) in clips.iter().zip(blocks).enumerate() {
        if index != 0 { write!(writer, ",\r\n")?; }
        write!(writer, "{{\"metadata\":")?;
        serde_json::to_writer(&mut *writer, clip)?;
        write!(writer, ",\r\n\"joint_tracks\":[\r\n")?;
        let count = clip["joint_track_count"].as_u64().unwrap() as usize;
        for joint in 0..count {
            if joint != 0 { write!(writer, ",\r\n")?; }
            let controller = block + 12 + count * 4 + joint * 24;
            let rotation = track(storage, controller, 0)?;
            let position = track(storage, controller, 12)?;
            failures += usize::from(rotation.get("decode_error").is_some()) + usize::from(position.get("decode_error").is_some());
            write!(writer, "{{\"bone_hash\":\"0x{:08x}\",\"rotation\":", u32_at(storage, block + 12 + joint * 4)?)?;
            write_track(writer, rotation)?;
            write!(writer, ",\"position\":")?;
            write_track(writer, position)?;
            write!(writer, "}}")?;
        }
        write!(writer, "\r\n],\r\n\"cdik_targets\":[")?;
        let count = clip["cdik_target_count"].as_u64().unwrap() as usize;
        let block = cdik_blocks[index];
        for target in 0..count {
            if target != 0 { write!(writer, ",")?; }
            let descriptor = block + 12 + target * 72;
            let name_bytes = bytes(cdik, descriptor, 64)?;
            let name_end = name_bytes.iter().position(|&b| b == 0).unwrap_or(64);
            let mut info = json!({"parent_controller_id":format!("0x{:08x}",u32_at(cdik,descriptor+64)?),
                "flags":u16_at(cdik,descriptor+68)?, "reserved":u16_at(cdik,descriptor+70)?});
            match std::str::from_utf8(&name_bytes[..name_end]) {
                Ok(name) => info["name"] = name.into(),
                Err(error) => { info["name_error"] = error.to_string().into(); info["name_bytes"] = json!(name_bytes); failures += 1; }
            }
            write!(writer, "\r\n{{\"metadata\":")?;
            serde_json::to_writer(&mut *writer, &info)?;
            let controller = block + 12 + count * 72 + target * 36;
            for (channel, name) in [(0,"rotation"),(12,"position"),(24,"weight")] {
                let value = track(cdik,controller,channel)?;
                failures += usize::from(value.get("decode_error").is_some());
                write!(writer, ",\"{name}\":")?;
                write_track(writer,value)?;
            }
            write!(writer, "}}")?;
        }
        write!(writer, "\r\n]}}")?;
    }
    write!(writer, "\r\n],\r\n\"track_decode_errors\":{failures},\r\n\"subhierarchy_masks_hex\":")?;
    write_hex(writer, masks)?;
    write!(writer, ",\r\n\"additional_chunks\":[")?;
    for (i, chunk) in additional.iter().enumerate() {
        if i != 0 { write!(writer, ",")?; }
        write!(writer, "\r\n{{\"type\":\"0x{:08x}\",\"version\":{},\"decode_status\":\"raw_preserved\",\"data_hex\":", chunk.chunk_type, chunk.version)?;
        write_hex(writer, file.chunk_data(chunk))?;
        write!(writer, "}}")?;
    }
    write!(writer, "\r\n],\r\n\"tracks_complete\":{},\r\n\"auxiliary_data_decoded\":{}\r\n}}", failures == 0, additional.is_empty() && masks.is_empty())?;
    Ok(failures)
}

fn write_track(writer: &mut impl Write, mut value: Value) -> Result<()> {
    let keys = value.as_object_mut().unwrap().remove("keys");
    let header = serde_json::to_string(&value)?;
    writer.write_all(header[..header.len() - 1].as_bytes())?;
    if let Some(Value::Array(keys)) = keys {
        write!(writer, ",\"keys\":[")?;
        for (i, key) in keys.iter().enumerate() {
            if i != 0 { write!(writer, ",")?; }
            write!(writer, "\r\n")?;
            serde_json::to_writer(&mut *writer, key)?;
        }
        write!(writer, "\r\n]")?;
    }
    write!(writer, "}}")
}

fn write_hex(writer: &mut impl Write, data: &[u8]) -> Result<()> {
    writer.write_all(b"\"")?;
    for byte in data { write!(writer, "{byte:02x}")?; }
    writer.write_all(b"\"")
}

#[cfg(test)]
#[path = "dba_tests.rs"]
mod tests;
