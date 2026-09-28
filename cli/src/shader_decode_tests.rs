use super::*;

pub(crate) fn binary(tokens: &[u32], dictionary: &[(u32, &str)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for value in [0x31425846, 11674302, 123, 28 + tokens.len() as u32 * 4, 0, tokens.len() as u32, 456] {
        bytes.extend(value.to_le_bytes());
    }
    for token in tokens { bytes.extend(token.to_le_bytes()); }
    for (id, text) in dictionary {
        bytes.extend(id.to_le_bytes());
        bytes.extend(text.as_bytes());
        bytes.push(0);
    }
    let size = bytes.len() as u32;
    bytes[16..20].copy_from_slice(&size.to_le_bytes());
    bytes
}

#[test]
fn shader_dictionary_order_and_unused_entries_do_not_change_readable_output() {
    let a = binary(&[600, 41, 601, 36], &[(600, "Exposure"), (601, "0.18")]);
    let b = binary(&[600, 41, 601, 36], &[(601, "0.18"), (777, "unused"), (600, "Exposure")]);
    assert_eq!(decode(&a).unwrap(), decode(&b).unwrap());
    assert_eq!(decode(&a).unwrap().0, "Exposure = 0.18;\n");
    let changed = binary(&[600, 41, 601, 36], &[(600, "Exposure"), (601, "0.25")]);
    assert_ne!(decode(&a).unwrap(), decode(&changed).unwrap());
}

#[test]
fn shader_builtin_domain_cannot_be_overridden_by_dictionary() {
    let (text, missing) = decode(&binary(
        &[1, 496, 497, 0, 416],
        &[(1, "wrong"), (496, "wrong"), (497, "custom"), (0, "wrong"), (416, "wrong")],
    )).unwrap();
    assert!(text.starts_with("#include PROSPERO\ncustom"));
    assert!(!text.contains("wrong"));
    assert_eq!(missing, BTreeSet::from([0, 416]));
}

#[test]
fn shader_additional_native_tokens_are_readable() {
    let (text, missing) = decode(&binary(
        &[3, 600, 0, 4, 600, 25, 601, 26, 601, 0, 5, 600, 10, 600, 11, 600, 17, 601, 18, 19, 20, 21, 22, 600, 23, 24, 451, 452, 453, 454],
        &[(600, "FLAG"), (601, "value")],
    )).unwrap();
    assert_eq!(text, "#define FLAG\n#define FLAG(value) value\n#undefine FLAG\n#ifdef FLAG\n#ifndef FLAG\n#warning value\n#register_env\n#ifcvar\n#ifncvar\n#elifcvar\n#skip FLAG\n#skip_(\n#skip_)\ns0 s1 s2 s3\n");
    assert!(missing.is_empty());
}

#[test]
fn shader_unknown_tokens_remain_visible_and_distinct() {
    let (text, missing) = decode(&binary(&[0, 9000, 9001], &[])).unwrap();
    assert!(text.contains("<TOKEN_0x00000000>"));
    assert!(text.contains("<TOKEN_0x00002328>"));
    assert!(text.contains("<TOKEN_0x00002329>"));
    assert_eq!(missing, BTreeSet::from([0, 9000, 9001]));
}

#[test]
fn shader_rejects_invalid_bounds_versions_and_dictionary() {
    let valid = binary(&[600], &[(600, "test")]);
    for length in 0..valid.len() { assert!(decode(&valid[..length]).is_err()); }
    for (offset, value) in [(0, 0), (4, 1), (12, 0), (16, 0), (20, u32::MAX)] {
        let mut broken = valid.clone();
        broken[offset..offset + 4].copy_from_slice(&u32::to_le_bytes(value));
        assert!(decode(&broken).is_err());
    }
    assert!(decode(&binary(&[600], &[(600, "a"), (600, "b")])).is_err());
    let mut broken = valid.clone();
    *broken.last_mut().unwrap() = 1;
    assert!(decode(&broken).is_err());
}
