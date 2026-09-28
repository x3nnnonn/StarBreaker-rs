use crate::error::Result;
use crate::shader_lex::{atoms, directive_end};

#[cfg(test)]
#[path = "shader_format_tests.rs"]
pub(crate) mod tests;

fn flush(output: &mut String, line: &mut String, indent: usize) {
    if !line.is_empty() {
        output.push_str(&"    ".repeat(indent));
        output.push_str(line.trim_end());
        output.push('\n');
        line.clear();
    }
}

fn append(line: &mut String, previous: &str, text: &str) {
    let tight = matches!(text, ";" | "," | ")" | "]" | "." | "::" | "->")
        || matches!(previous, "(" | "[" | "." | "::" | "->" | "!" | "~")
        || (text == "(" && !previous.starts_with('#') && !matches!(previous, "if" | "while" | "for" | "switch" | "catch" | "=" | "+" | "-" | "*" | "/" | "&&" | "||" | "return"))
        || text == "["
        || (matches!(text, "++" | "--") && previous != ";")
        || matches!(previous, "++" | "--");
    if !line.is_empty() && !tight { line.push(' '); }
    line.push_str(text);
}

fn inline(tokens: &[(u32, String)], function_macro: bool) -> String {
    let mut line = String::new();
    let mut previous = "";
    let mut parameters = 0;
    let mut body_start = false;
    for (i, (_, text)) in tokens.iter().enumerate() {
        if body_start || i == 2 && text == "(" && !function_macro {
            line.push(' ');
            line.push_str(text);
            body_start = false;
        } else {
            append(&mut line, previous, text);
        }
        if function_macro && i >= 2 {
            if text == "(" { parameters += 1; }
            if text == ")" && parameters != 0 {
                parameters -= 1;
                if parameters == 0 { body_start = true; }
            }
        }
        previous = text;
    }
    line
}

pub fn format(tokens: Vec<(u32, String)>, unresolved: &mut std::collections::BTreeSet<u32>) -> Result<String> {
    let tokens = atoms(tokens)?;
    let mut output = String::new();
    let mut line = String::new();
    let mut indent: usize = 0;
    let mut parens: usize = 0;
    let mut brackets: usize = 0;
    let mut annotations: Vec<(usize, usize)> = Vec::new();
    let mut conditionals: Vec<(usize, usize, usize)> = Vec::new();
    let mut previous = "";
    let mut i = 0;
    while i < tokens.len() {
        let (id, text) = &tokens[i];
        if *id == 0 {
            flush(&mut output, &mut line, indent);
            output.push_str("<TOKEN_0x00000000>\n");
            unresolved.insert(0);
            previous = "";
            i += 1;
            continue;
        }
        if (1..=24).contains(id) && !matches!(id, 15 | 16) {
            flush(&mut output, &mut line, indent);
            let end = directive_end(&tokens, i)?;
            if matches!(id, 6..=11) { conditionals.push((indent, parens, brackets)); }
            if matches!(id, 12 | 14) {
                if let Some(&(level, round, square)) = conditionals.last() {
                    indent = level;
                    parens = round;
                    brackets = square;
                }
            }
            if *id == 13 { conditionals.pop(); }
            output.push_str(&inline(&tokens[i..end], *id == 4));
            output.push('\n');
            i = end + usize::from(matches!(id, 2..=4));
            previous = "";
            continue;
        }
        let next = tokens.get(i + 1).map(|t| t.1.as_str()).unwrap_or("");
        match text.as_str() {
            "(" => {
                append(&mut line, previous, text);
                parens += 1;
            }
            ")" => {
                append(&mut line, previous, text);
                parens = parens.saturating_sub(1);
            }
            "[" => {
                append(&mut line, previous, text);
                brackets += 1;
            }
            "]" => {
                append(&mut line, previous, text);
                brackets = brackets.saturating_sub(1);
            }
            "<" if parens == 0 && brackets == 0 && next == "string" => {
                append(&mut line, previous, text);
                flush(&mut output, &mut line, indent);
                annotations.push((indent, parens));
                indent += 1;
            }
            ">" | ">=" if annotations.last().is_some_and(|(_, round)| *round == parens)
                && (line.is_empty() || previous.starts_with('"')) => {
                flush(&mut output, &mut line, indent);
                indent = annotations.pop().unwrap().0;
                line.push('>');
                if text == ">=" { line.push_str(" ="); }
            }
            "{" => {
                append(&mut line, previous, text);
                flush(&mut output, &mut line, indent);
                indent += 1;
            }
            "}" => {
                flush(&mut output, &mut line, indent);
                indent = indent.saturating_sub(1);
                line.push('}');
                if !matches!(next, ";" | "," | ")" | "]" | "else" | "while") {
                    flush(&mut output, &mut line, indent);
                }
            }
            ";" => {
                append(&mut line, previous, text);
                if parens == 0 { flush(&mut output, &mut line, indent); }
            }
            ":" if line == "default" || line.starts_with("case ") => {
                line.push(':');
                flush(&mut output, &mut line, indent);
            }
            _ => append(&mut line, previous, text),
        }
        previous = text;
        i += 1;
    }
    flush(&mut output, &mut line, indent);
    Ok(output)
}
