use std::io::{Error, ErrorKind, Result};
use serde_json::{Value, json};

pub(super) fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidData, message.into())
}

pub(super) fn bytes(data: &[u8], offset: usize, size: usize) -> Result<&[u8]> {
    data.get(offset..offset.checked_add(size).ok_or_else(|| invalid("range overflow"))?)
        .ok_or_else(|| invalid(format!("range {offset:#x}+{size:#x} exceeds {} bytes", data.len())))
}

pub(super) fn u16_at(data: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(bytes(data, offset, 2)?.try_into().unwrap()))
}

pub(super) fn u32_at(data: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(bytes(data, offset, 4)?.try_into().unwrap()))
}

pub(super) fn f32_at(data: &[u8], offset: usize) -> Result<f32> {
    let value = f32::from_bits(u32_at(data, offset)?);
    if !value.is_finite() { return Err(invalid(format!("nonfinite float at {offset:#x}"))); }
    Ok(value)
}

pub(super) fn floats<const N: usize>(data: &[u8], offset: usize) -> Result<[f32; N]> {
    let mut values = [0.0; N];
    for (i, value) in values.iter_mut().enumerate() { *value = f32_at(data, offset + i * 4)?; }
    Ok(values)
}

fn times(data: &[u8], offset: usize, count: usize, format: u8) -> Result<Vec<u16>> {
    let times = match format {
        0x40 => bytes(data, offset, count)?.iter().map(|&v| u16::from(v)).collect(),
        0x41 => (0..count).map(|i| u16_at(data, offset + i * 2)).collect::<Result<Vec<_>>>()?,
        0x42 => {
            let start = u16_at(data, offset)?;
            let end = u16_at(data, offset + 2)?;
            if end < start { return Err(invalid("bitmap end precedes start")); }
            let bit_count = usize::from(end - start) + 1;
            let bitmap = bytes(data, offset + 4, bit_count.div_ceil(16) * 2)?;
            (0..bit_count).filter(|&bit| bitmap[bit / 8] & (1 << (bit % 8)) != 0)
                .map(|bit| start + bit as u16).collect()
        }
        _ => return Err(invalid(format!("unsupported time encoding {format:#04x}"))),
    };
    if times.len() != count { return Err(invalid(format!("time table has {} keys; expected {count}", times.len()))); }
    if times.windows(2).any(|pair| pair[0] >= pair[1]) { return Err(invalid("key times are not strictly increasing")); }
    Ok(times)
}

fn quaternion(data: &[u8], offset: usize, format: u8) -> Result<[f32; 4]> {
    let (largest_index, components) = match format {
        0x80 => return floats(data, offset),
        0x81 => {
            let bits = u32_at(data, offset)?;
            let scale = f32::from_bits(0x3ab54a01);
            ((bits >> 30) as usize, [bits & 1023, (bits >> 10) & 1023, (bits >> 20) & 1023].map(|v| v as f32 * scale))
        }
        0x82 => {
            let a = u32::from(u16_at(data, offset)?);
            let b = u32::from(u16_at(data, offset + 2)?);
            let c = u32::from(u16_at(data, offset + 4)?);
            let scale = f32::from_bits(0x383505e6);
            ((c >> 14) as usize, [a & 32767, ((b << 1) + (a >> 15)) & 32767,
                ((c << 2) + (b >> 14)) & 32767].map(|v| v as f32 * scale))
        }
        0x83 => {
            let a = u32_at(data, offset)?;
            let b = u32_at(data, offset + 4)?;
            ((b >> 30) as usize, [
                (a & 0x1fffff) as f32 * f32::from_bits(0x353504fe),
                (((a >> 21) | (b << 11)) & 0x1fffff) as f32 * f32::from_bits(0x353504fe),
                ((b >> 10) & 0xfffff) as f32 * f32::from_bits(0x35b50506),
            ])
        }
        _ => return Err(invalid(format!("unsupported rotation encoding {format:#04x}"))),
    };
    let components = components.map(|v| v - std::f32::consts::FRAC_1_SQRT_2);
    let squared = 1.0 - components[0] * components[0] - components[1] * components[1] - components[2] * components[2];
    if squared < 0.0 { return Err(invalid("compressed quaternion has a negative remaining squared component")); }
    let mut result = [0.0; 4];
    let mut source = components.into_iter();
    for (i, value) in result.iter_mut().enumerate() {
        *value = if i == largest_index { squared.sqrt() } else { source.next().unwrap() };
    }
    Ok(result)
}

pub(super) fn track(data: &[u8], controller: usize, channel: usize) -> Result<Value> {
    let header = controller + channel;
    let count = usize::from(u16_at(data, header)?);
    let time_format = bytes(data, header + 2, 1)?[0];
    let value_format = bytes(data, header + 3, 1)?[0];
    let time_offset = u32_at(data, header + 4)? as usize;
    let value_offset = u32_at(data, header + 8)? as usize;
    let mut output = json!({"key_count": count, "time_format": format!("0x{time_format:02x}"),
        "value_format": format!("0x{value_format:02x}")});
    if count == 0 { output["keys"] = json!([]); return Ok(output); }
    let decoded = (|| -> Result<Value> {
        let times = times(data, controller + time_offset, count, time_format)?;
        let start = controller + value_offset;
        let mut keys = Vec::with_capacity(count);
        if channel == 0 {
            let stride = match value_format { 0x80 => 16, 0x81 => 4, 0x82 => 6, 0x83 => 8,
                _ => return Err(invalid(format!("unsupported rotation encoding {value_format:#04x}"))) };
            bytes(data, start, count * stride)?;
            for (i, time) in times.into_iter().enumerate() {
                let value = quaternion(data, start + i * stride, value_format)?;
                keys.push(json!([time, value[0], value[1], value[2], value[3]]));
            }
        } else if channel == 24 {
            for (i, time) in times.into_iter().enumerate() {
                let value = match value_format {
                    0xf0 => f32_at(data, start + i * 4)?,
                    0xf1 => f32::from(u16_at(data, start + 8 + i * 2)?) * f32_at(data, start)? + f32_at(data, start + 4)?,
                    _ => return Err(invalid(format!("unsupported weight encoding {value_format:#04x}"))),
                };
                if !value.is_finite() { return Err(invalid("nonfinite decoded weight")); }
                keys.push(json!([time, value]));
            }
        } else {
            let mut scale = [0.0; 3];
            let mut origin = [0.0; 3];
            let mut axis_starts = [start + 24; 3];
            if value_format == 0xc1 || value_format == 0xc2 {
                scale = floats(data, start)?;
                origin = floats(data, start + 12)?;
                let mut next = start + 24;
                for axis in 0..3 {
                    axis_starts[axis] = next;
                    if scale[axis] != f32::MAX { next += count * 2; }
                }
                bytes(data, start, if value_format == 0xc1 { 24 + count * 6 } else { next - start })?;
            } else if value_format != 0xc0 {
                return Err(invalid(format!("unsupported position encoding {value_format:#04x}")));
            }
            for (i, time) in times.into_iter().enumerate() {
                let mut value = origin;
                for axis in 0..3 {
                    value[axis] = match value_format {
                        0xc0 => f32_at(data, start + i * 12 + axis * 4)?,
                        0xc1 => f32::from(u16_at(data, start + 24 + i * 6 + axis * 2)?) * scale[axis] + origin[axis],
                        _ if scale[axis] == f32::MAX => origin[axis],
                        _ => f32::from(u16_at(data, axis_starts[axis] + i * 2)?) * scale[axis] + origin[axis],
                    };
                    if !value[axis].is_finite() { return Err(invalid("nonfinite decoded position")); }
                }
                keys.push(json!([time, value[0], value[1], value[2]]));
            }
        }
        Ok(Value::Array(keys))
    })();
    match decoded {
        Ok(keys) => output["keys"] = keys,
        Err(error) => {
            output["decode_error"] = error.to_string().into();
            output["time_offset"] = time_offset.into();
            output["data_offset"] = value_offset.into();
        }
    }
    Ok(output)
}
