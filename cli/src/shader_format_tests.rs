use crate::shader_decode::{decode, tests::binary};
use std::collections::{BTreeSet, HashMap};

pub(crate) fn assert_token_contents_preserved(bytes: &[u8], formatted: &str) {
    let mut dictionary: HashMap<u32, String> = crate::shader_tokens::BUILTINS.iter().enumerate()
        .filter_map(|(id, text)| text.map(|text| (id as u32, text.to_owned()))).collect();
    let boundary = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let mut cursor = boundary;
    while cursor < bytes.len() {
        let id = u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().unwrap());
        cursor += 4;
        let length = bytes[cursor..].iter().position(|b| *b == 0).unwrap();
        if id >= 497 { dictionary.insert(id, String::from_utf8(bytes[cursor..cursor + length].to_vec()).unwrap()); }
        cursor += length + 1;
    }
    let mut expected = String::new();
    for word in bytes[28..boundary].chunks_exact(4) {
        let id = u32::from_le_bytes(word.try_into().unwrap());
        if id != 0 { expected.extend(dictionary[&id].chars().filter(|c| !c.is_whitespace())); }
    }
    let actual: String = formatted.chars().filter(|c| !c.is_whitespace()).collect();
    assert_eq!(actual, expected);
}

fn encoded(words: &[&str]) -> Vec<u8> {
    let builtins = &crate::shader_tokens::BUILTINS;
    let mut dictionary = Vec::new();
    let tokens: Vec<_> = words.iter().map(|word| {
        if *word == "<END>" { return 0; }
        if *word == "#function" { return 4; }
        if *word == "#define" { return 2; }
        if let Some(id) = builtins.iter().position(|text| *text == Some(*word)) { return id as u32; }
        let id = 1000 + dictionary.len() as u32;
        dictionary.push((id, *word));
        id
    }).collect();
    binary(&tokens, &dictionary)
}

fn formatted(words: &[&str]) -> String {
    let (text, missing) = decode(&encoded(words)).unwrap();
    assert_eq!(missing, BTreeSet::new());
    text
}

#[test]
fn shader_format_directives_macros_and_conditions() {
    let text = formatted(&[
        "#include", "Common", "#define", "EPSILON", "(", "0", ".", "05", ")", "<END>",
        "#function", "select", "(", "condition", ",", "lhs", ",", "rhs", ")", "(", "condition", "?", "lhs", ":", "rhs", ")", "<END>",
        "#if", "(", "VULKAN", "&", "&", "!", "%_VS", ")", "|", "|", "PROSPERO",
        "float", "x", "=", "1e", "-", "3f", ";", "#else", "float", "x", "=", ".", "5", ";", "#endif",
        "#warning", "Parameter was ignored", "#undefine", "EPSILON",
    ]);
    assert_eq!(text, "#include Common\n#define EPSILON (0.05)\n#define select(condition, lhs, rhs) (condition ? lhs : rhs)\n#if (VULKAN && !%_VS) || PROSPERO\nfloat x = 1e-3f;\n#else\nfloat x = .5;\n#endif\n#warning Parameter was ignored\n#undefine EPSILON\n");
}

#[test]
fn shader_format_quotes_annotations_and_member_access() {
    let text = formatted(&[
        "float", "Script", ":", "STANDARDSGLOBAL", "<", "string", "Script", "=",
        "\"", "Public", ";", "\"", "\"", "ShaderType", "=", "General", ";", "\"", ";", ">", ";",
        "string", "UIName", "=", "\"", "Direction", "Map", "\"", ";",
        "string", "UIHelp", "=", "\"", "two  spaces\\nremain", "\"", ";",
        "float", "v", "=", "input", ".", "xyz", "[", "0", "]", ".", "x", ";",
    ]);
    assert_eq!(text, "float Script : STANDARDSGLOBAL <\n    string Script = \"Public;\" \"ShaderType=General;\";\n>;\nstring UIName = \"Direction Map\";\nstring UIHelp = \"two  spaces\\nremain\";\nfloat v = input.xyz[0].x;\n");
}

#[test]
fn shader_format_blocks_loops_and_operators() {
    let text = formatted(&[
        "struct", "Input", "{", "float3", "position", ";", "}", ";",
        "float", "run", "(", "int", "n", ")", "{",
        "for", "(", "int", "i", "=", "0", ";", "i", "<", "n", ";", "+", "+", "i", ")", "{",
        "if", "(", "i", "!", "=", "0", ")", "{", "n", "+", "=", "i", ";", "}", "else", "{", "n", "-", "-", ";", "}", "}",
        "return", "n", ";", "}",
    ]);
    assert_eq!(text, "struct Input {\n    float3 position;\n};\nfloat run(int n) {\n    for (int i = 0; i < n; ++i) {\n        if (i != 0) {\n            n += i;\n        } else {\n            n--;\n        }\n    }\n    return n;\n}\n");
}

#[test]
fn shader_format_rejects_truncated_macro_and_quote() {
    assert!(decode(&encoded(&["#define", "VALUE", "1"])).is_err());
    assert!(decode(&encoded(&["string", "name", "=", "\"", "missing"])).is_err());
    assert!(decode(&encoded(&["#include"])).is_err());
    assert!(decode(&encoded(&["#if", "(", "VULKAN"])).is_err());
}

#[test]
fn shader_format_annotation_close_is_not_a_comparison_operator() {
    let text = formatted(&[
        "float", "Power", "<", "string", "UIName", "=", "\"", "Power", "\"", ";",
        "float", "UIMin", "=", "0", ";", ">", "=", "1", ";",
        "float", "other", "=", "0", ";",
        "bool", "enabled", "=", "other", ">", "=", "Power", ";",
    ]);
    assert_eq!(text, "float Power <\n    string UIName = \"Power\";\n    float UIMin = 0;\n> = 1;\nfloat other = 0;\nbool enabled = other >= Power;\n");
}

#[test]
#[ignore]
fn shader_format_hair_archive_reference() {
    let bytes = std::fs::read(std::env::var("SC_SHADER_FORMAT_FIXTURE").unwrap()).unwrap();
    let (text, missing) = decode(&bytes).unwrap();
    assert!(missing.is_empty());
    assert!(text.starts_with("#include Common\nfloat Script : STANDARDSGLOBAL <\n"));
    assert!(text.contains("#define NON_THIN_HAIR_THRESHOLD (0.05)\n"));
    assert!(text.contains("\"Public;\" \"Hair;\""));
    assert!(text.contains("#include vertexLib\n"));
    assert!(!text.contains("TOKEN_"));
    assert!(text.contains("\n> = 0.0;\nfloat AlphaBlendMultiplier <\n"));
    assert!(text.lines().map(|line| line.len() - line.trim_start().len()).max().unwrap() < 64);
    std::fs::write(std::env::var("SC_SHADER_FORMAT_OUTPUT").unwrap(), text).unwrap();
}
