use super::*;

fn central(extra: &[u8], extended: bool) -> Vec<u8> {
    let mut bytes = vec![0u8; 46];
    bytes[..4].copy_from_slice(&CENTRAL_DIR_SIGNATURE.to_le_bytes());
    let size = if extended { u32::MAX } else { 12 };
    bytes[20..24].copy_from_slice(&size.to_le_bytes());
    bytes[24..28].copy_from_slice(&size.to_le_bytes());
    bytes[28..30].copy_from_slice(&5u16.to_le_bytes());
    bytes[30..32].copy_from_slice(&(extra.len() as u16).to_le_bytes());
    bytes[42..46].copy_from_slice(&size.to_le_bytes());
    bytes.extend(b"a.bin");
    bytes.extend(extra);
    bytes
}

#[test]
fn zip64_archive_accepts_ordinary_members_without_extras() {
    let bytes = central(&[], false);
    let entry = read_entry(&mut SpanReader::new(&bytes), true).unwrap();
    assert_eq!(entry.uncompressed_size, 12);
    assert_eq!(entry.offset, 12);
    assert!(!entry.is_encrypted);
}

#[test]
fn standard_zip64_member_sizes_do_not_require_vendor_fields() {
    let mut extra = vec![1, 0, 24, 0];
    for value in [100u64, 80, 4_500_000_000] { extra.extend(value.to_le_bytes()); }
    let bytes = central(&extra, true);
    let entry = read_entry(&mut SpanReader::new(&bytes), true).unwrap();
    assert_eq!(entry.uncompressed_size, 100);
    assert_eq!(entry.compressed_size, 80);
    assert_eq!(entry.offset, 4_500_000_000);
    assert!(!entry.is_encrypted);
    assert!(read_entry(&mut SpanReader::new(&central(&[], true)), true).is_err());
    extra.truncate(12);
    assert!(read_entry(&mut SpanReader::new(&central(&extra, true)), true).is_err());
}

#[test]
fn cig_zip64_extra_fields_keep_encryption_and_sizes() {
    let mut extra = vec![1, 0, 24, 0];
    for value in [100u64, 80, 4_500_000_000] { extra.extend(value.to_le_bytes()); }
    extra.extend([0, 0x50, 4, 0, 2, 0x50, 6, 0, 1, 0, 3, 0x50, 4, 0]);
    let bytes = central(&extra, true);
    let entry = read_entry(&mut SpanReader::new(&bytes), true).unwrap();
    assert_eq!(entry.uncompressed_size, 100);
    assert_eq!(entry.compressed_size, 80);
    assert_eq!(entry.offset, 4_500_000_000);
    assert!(entry.is_encrypted);
}
