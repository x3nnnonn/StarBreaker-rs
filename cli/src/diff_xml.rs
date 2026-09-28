use quick_xml::events::{BytesDecl, BytesStart, Event};
use quick_xml::{Reader, Writer};

pub(crate) fn normalize_audio_xml(data: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let Some(text) = decode_text(data)? else { return Ok(None); };
    let mut reader = Reader::from_str(&text);
    let mut events = Vec::new();
    let mut depth = 0usize;
    let mut element_content = Vec::<(bool, bool)>::new();
    let mut roots = 0usize;
    let mut standalone = None;
    loop {
        let event = reader.read_event().map_err(|e| e.to_string())?;
        match &event {
            Event::Eof => break,
            Event::Decl(declaration) => {
                if declaration.version().map_err(|e| e.to_string())?.as_ref() != b"1.0" {
                    return Ok(None);
                }
                standalone = declaration.standalone().transpose().map_err(|e| e.to_string())?
                    .map(|s| String::from_utf8(s.into_owned())).transpose().map_err(|e| e.to_string())?;
                continue;
            }
            Event::Start(start) | Event::Empty(start) => {
                if depth == 0 {
                    roots += 1;
                    if roots == 1 && start.name().as_ref() != b"ATLConfig" { return Ok(None); }
                }
                for attribute in start.attributes() {
                    let attribute = attribute.map_err(|e| e.to_string())?;
                    let value = attribute.unescape_value().map_err(|e| e.to_string())?;
                    if attribute.key.as_ref() == b"xml:space" && value == "preserve" {
                        return Ok(None);
                    }
                }
                if let Some(parent) = element_content.last_mut() { parent.0 = true; }
                if matches!(event, Event::Start(_)) {
                    depth += 1;
                    element_content.push((false, false));
                }
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or("unexpected closing element")?;
                if let Some((false, true)) = element_content.pop() { return Ok(None); }
            }
            Event::Text(text) => {
                if !text.unescape().map_err(|e| e.to_string())?.chars().all(|c| matches!(c, ' ' | '\t' | '\r' | '\n')) {
                    return Ok(None);
                }
                if let Some(parent) = element_content.last_mut() { parent.1 = true; }
                continue;
            }
            Event::CData(_) | Event::DocType(_) => return Ok(None),
            _ => {}
        }
        events.push(event.into_owned());
    }
    if roots != 1 || depth != 0 { return Err("XML must contain one complete root element".into()); }
    let mut writer = Writer::new_with_indent(Vec::new(), b' ', 4);
    writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("utf-8"), standalone.as_deref())))
        .map_err(|e| e.to_string())?;
    let mut index = 0;
    while index < events.len() {
        let event = match &events[index] {
            Event::Start(start) if matches!(events.get(index + 1), Some(Event::End(_))) => {
                index += 1;
                Event::Empty(normalize_start(start)?)
            }
            Event::Start(start) => Event::Start(normalize_start(start)?),
            Event::Empty(start) => Event::Empty(normalize_start(start)?),
            other => other.clone(),
        };
        let empty = matches!(event, Event::Empty(_));
        writer.write_event(event).map_err(|e| e.to_string())?;
        if empty {
            let output = writer.get_mut();
            output.insert(output.len() - 2, b' ');
        }
        index += 1;
    }
    Ok(Some(super::diff_p4k_contents::text_to_crlf_bytes(&writer.into_inner())))
}

fn normalize_start(start: &BytesStart<'_>) -> Result<BytesStart<'static>, String> {
    let name = String::from_utf8(start.name().as_ref().to_vec()).map_err(|e| e.to_string())?;
    let mut normalized = BytesStart::new(name);
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|e| e.to_string())?;
        let value = std::str::from_utf8(attribute.value.as_ref()).map_err(|e| e.to_string())?.replace('"', "&quot;");
        normalized.push_attribute((attribute.key.as_ref(), value.as_bytes()));
    }
    Ok(normalized)
}

fn decode_text(data: &[u8]) -> Result<Option<String>, String> {
    let (data, little_endian) = if let Some(data) = data.strip_prefix(&[0xff, 0xfe]) {
        (data, Some(true))
    } else if let Some(data) = data.strip_prefix(&[0xfe, 0xff]) {
        (data, Some(false))
    } else if data.starts_with(b"<\0") {
        (data, Some(true))
    } else if data.starts_with(b"\0<") {
        (data, Some(false))
    } else {
        (data.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(data), None)
    };
    if let Some(little) = little_endian {
        if data.len() % 2 != 0 { return Err("odd UTF-16 byte count".into()); }
        let words: Vec<_> = data.chunks_exact(2).map(|c| {
            if little { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }
        }).collect();
        return String::from_utf16(&words).map(Some).map_err(|e| e.to_string());
    }
    match std::str::from_utf8(data) {
        Ok(text) if text.trim_start().starts_with('<') => Ok(Some(text.to_owned())),
        _ => Ok(None),
    }
}

#[cfg(test)]
#[path = "diff_xml_tests.rs"]
mod tests;
