use std::collections::{BTreeSet, HashMap};

use crate::error::{CliError, Result};

fn invalid(message: &str) -> CliError {
    CliError::InvalidInput(format!("Invalid FXB1 shader: {message}"))
}

fn word(data: &[u8], offset: usize) -> Result<u32> {
    let bytes = data.get(offset..offset + 4).ok_or_else(|| invalid("truncated word"))?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

pub fn decode(data: &[u8]) -> Result<(String, BTreeSet<u32>)> {
    if data.len() < 28 || word(data, 0)? != 0x31425846 {
        return Err(invalid("missing 28-byte FXB1 header"));
    }
    if word(data, 4)? != 11674302 {
        return Err(invalid("unsupported token format version"));
    }
    let dictionary_offset = word(data, 12)? as usize;
    let token_count = word(data, 20)? as usize;
    if word(data, 16)? as usize != data.len()
        || token_count.checked_mul(4).and_then(|size| size.checked_add(28)) != Some(dictionary_offset)
        || dictionary_offset > data.len()
    {
        return Err(invalid("invalid token or dictionary bounds"));
    }
    let mut dictionary = HashMap::new();
    let mut offset = dictionary_offset;
    while offset < data.len() {
        let id = word(data, offset)?;
        offset += 4;
        let length = data[offset..].iter().position(|byte| *byte == 0)
            .ok_or_else(|| invalid("unterminated dictionary string"))?;
        let value = std::str::from_utf8(&data[offset..offset + length])
            .map_err(|_| invalid("non-UTF-8 dictionary string"))?;
        if dictionary.insert(id, value).is_some() {
            return Err(invalid("duplicate dictionary identifier"));
        }
        offset += length + 1;
    }
    let mut tokens = Vec::with_capacity(token_count);
    let mut unresolved = BTreeSet::new();
    for bytes in data[28..dictionary_offset].chunks_exact(4) {
        let id = u32::from_le_bytes(bytes.try_into().unwrap());
        if id == 0 {
            tokens.push((id, String::new()));
            continue;
        }
        let resolved = if id < 497 {
            crate::shader_tokens::BUILTINS[id as usize]
        } else {
            dictionary.get(&id).copied()
        };
        let token = if let Some(value) = resolved {
            value
        } else {
            unresolved.insert(id);
            tokens.push((id, format!("<TOKEN_0x{id:08X}>")));
            continue;
        };
        tokens.push((id, token.to_owned()));
    }
    let output = crate::shader_format::format(tokens, &mut unresolved)?;
    Ok((output, unresolved))
}

#[cfg(test)]
#[path = "shader_decode_tests.rs"]
pub(crate) mod tests;
