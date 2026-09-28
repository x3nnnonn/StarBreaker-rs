use crate::error::{CliError, Result};

fn invalid(message: &str) -> CliError {
    CliError::InvalidInput(format!("Invalid FXB1 shader formatting: {message}"))
}

fn native_separator(byte: u8) -> bool {
    byte <= 32 || matches!(byte, 33..=34 | 38..=47 | 58..=63 | 91 | 93 | 123..=125)
}

pub fn native_join(tokens: &[(u32, String)]) -> String {
    let mut output = String::new();
    for (_, text) in tokens {
        if let (Some(last), Some(first)) = (output.as_bytes().last(), text.as_bytes().first()) {
            if !native_separator(*last) && !native_separator(*first) { output.push(' '); }
        }
        output.push_str(text);
    }
    output
}

pub fn atoms(tokens: Vec<(u32, String)>) -> Result<Vec<(u32, String)>> {
    let mut result = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        let (id, text) = &tokens[i];
        if matches!(id, 38 | 39) {
            let start = i;
            i += 1;
            while i < tokens.len() && tokens[i].0 != *id {
                if tokens[i].0 == 0 { return Err(invalid("unterminated quoted text")); }
                i += 1;
            }
            if i == tokens.len() { return Err(invalid("unterminated quoted text")); }
            result.push((*id, format!("{text}{}{text}", native_join(&tokens[start + 1..i]))));
            i += 1;
            continue;
        }
        if text.as_bytes().first().is_some_and(u8::is_ascii_digit)
            || (text == "." && tokens.get(i + 1).is_some_and(|t| t.1.starts_with(|c: char| c.is_ascii_digit())))
        {
            let mut number = text.clone();
            i += 1;
            if !number.contains('.') && tokens.get(i).is_some_and(|t| t.1 == ".") {
                number.push('.');
                i += 1;
                if let Some((_, part)) = tokens.get(i) {
                    if part.starts_with(|c: char| c.is_ascii_digit()) {
                        number.push_str(part);
                        i += 1;
                    }
                }
            } else if number == "." {
                number.push_str(&tokens[i].1);
                i += 1;
            }
            if number.ends_with(['e', 'E']) {
                if tokens.get(i).is_some_and(|t| matches!(t.1.as_str(), "+" | "-")) {
                    number.push_str(&tokens[i].1);
                    i += 1;
                }
                if let Some((_, part)) = tokens.get(i) {
                    if part.starts_with(|c: char| c.is_ascii_digit()) {
                        number.push_str(part);
                        i += 1;
                    }
                }
            }
            result.push((*id, number));
            continue;
        }
        let mut combined = None;
        if matches!(text.as_str(), "+" | "-" | "=" | "!" | "<" | ">" | "&" | "|" | "*" | "/" | "%" | "^" | ":" | "#") {
            for size in [3, 2] {
                if let Some(parts) = tokens.get(i..i + size) {
                    let joined: String = parts.iter().map(|t| t.1.as_str()).collect();
                    if matches!(joined.as_str(), "<<=" | ">>=" | "++" | "--" | "==" | "!=" | "<=" | ">=" | "&&" | "||" | "<<" | ">>" | "+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^=" | "::" | "->" | "##") {
                        combined = Some((size, joined));
                        break;
                    }
                }
            }
        }
        if let Some((size, text)) = combined {
            result.push((*id, text));
            i += size;
        } else {
            result.push(tokens[i].clone());
            i += 1;
        }
    }
    Ok(result)
}

fn condition_end(tokens: &[(u32, String)], start: usize) -> Result<usize> {
    let mut i = start;
    loop {
        if tokens.get(i).is_some_and(|t| t.1 == "!") { i += 1; }
        let Some((id, text)) = tokens.get(i) else { return Err(invalid("missing directive condition")); };
        if *id == 0 || (1..=24).contains(id) && !matches!(id, 15 | 16) {
            return Err(invalid("missing directive condition"));
        }
        if text == "(" {
            let mut depth = 1;
            i += 1;
            while depth != 0 {
                let Some((id, text)) = tokens.get(i) else { return Err(invalid("unclosed directive condition")); };
                if *id == 0 { return Err(invalid("unclosed directive condition")); }
                if text == "(" { depth += 1; }
                if text == ")" { depth -= 1; }
                i += 1;
            }
        } else {
            i += 1;
        }
        if tokens.get(i).is_some_and(|t| matches!(t.1.as_str(), "&" | "|" | "&&" | "||")) {
            i += 1;
        } else {
            return Ok(i);
        }
    }
}

pub fn directive_end(tokens: &[(u32, String)], start: usize) -> Result<usize> {
    match tokens[start].0 {
        2..=4 => tokens[start + 1..].iter().position(|t| t.0 == 0)
            .map(|n| start + 1 + n).ok_or_else(|| invalid("unterminated macro definition")),
        6..=12 => condition_end(tokens, start + 1),
        1 | 5 | 17 | 22 => {
            if tokens.get(start + 1).is_none_or(|t| t.0 == 0 || (1..=24).contains(&t.0)) {
                return Err(invalid("missing directive argument"));
            }
            Ok(start + 2)
        }
        _ => Ok(start + 1),
    }
}
