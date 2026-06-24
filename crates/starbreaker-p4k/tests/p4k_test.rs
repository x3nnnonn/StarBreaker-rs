use starbreaker_p4k::{DirEntry, MappedP4k, P4kArchive};

/// Helper: auto-discover and open the P4k, or skip if not installed.
fn open_p4k_or_skip() -> Option<MappedP4k> {
    match starbreaker_p4k::open_p4k() {
        Ok(p4k) => Some(p4k),
        Err(e) => {
            eprintln!("SKIP: {e}");
            None
        }
    }
}

#[test]
fn open_real_p4k() {
    let Some(p4k) = open_p4k_or_skip() else {
        return;
    };

    println!("entries: {}", p4k.len());
    assert!(p4k.len() > 100_000);
}

#[test]
fn lookup_known_entry() {
    let Some(p4k) = open_p4k_or_skip() else {
        return;
    };

    let path = "Data\\Libs\\CharacterCustomizer\\MasculineDefault.xml";
    let entry = p4k.entry(path);
    assert!(entry.is_some(), "Entry not found: {path}");
    let entry = entry.unwrap();
    println!(
        "Found entry: {} (compressed={}, uncompressed={}, encrypted={}, method={})",
        entry.name,
        entry.compressed_size,
        entry.uncompressed_size,
        entry.is_encrypted,
        entry.compression_method
    );
}

#[test]
fn read_entry_from_p4k() {
    let Some(p4k) = open_p4k_or_skip() else {
        return;
    };

    let path = "Data\\Libs\\CharacterCustomizer\\MasculineDefault.xml";
    let entry = p4k.entry(path).expect("Entry not found");

    let p4k_data = p4k.read(entry).expect("Failed to read entry from P4k");
    println!("Read {} bytes from P4k", p4k_data.len());
    assert!(!p4k_data.is_empty(), "Entry data should not be empty");
}

#[test]
fn read_encrypted_entry() {
    let Some(p4k) = open_p4k_or_skip() else {
        return;
    };

    let encrypted = p4k.entries().iter().find(|e| e.is_encrypted);
    assert!(encrypted.is_some(), "No encrypted entries found");

    let entry = encrypted.unwrap();
    println!(
        "Reading encrypted entry: {} (compressed={}, uncompressed={}, method={})",
        entry.name, entry.compressed_size, entry.uncompressed_size, entry.compression_method
    );

    let data = p4k.read(entry).expect("Failed to read encrypted entry");
    assert!(!data.is_empty(), "Decrypted data is empty");
    println!(
        "Successfully read {} bytes from encrypted entry",
        data.len()
    );
}

#[test]
fn read_socpak_as_zip() {
    let Some(p4k) = open_p4k_or_skip() else {
        return;
    };

    // Find a .socpak file
    let socpak = p4k.entries().iter().find(|e| {
        e.name
            .to_lowercase()
            .contains("charactercustomizer_pu.socpak")
    });

    if socpak.is_none() {
        eprintln!("SKIP: No charactercustomizer_pu.socpak found");
        return;
    }

    let entry = socpak.unwrap();
    println!(
        "Reading socpak: {} ({} bytes compressed)",
        entry.name, entry.compressed_size
    );

    let socpak_data = p4k.read(entry).expect("Failed to read socpak");
    println!("Extracted socpak: {} bytes", socpak_data.len());

    // Parse the socpak as a ZIP/P4k archive
    let inner = P4kArchive::from_bytes(&socpak_data).expect("Failed to parse socpak as ZIP");
    println!("Socpak contains {} entries", inner.len());
    assert!(!inner.is_empty(), "Socpak has no entries");

    // Print first few entries
    for entry in inner.entries().iter().take(10) {
        println!("  - {}", entry.name);
    }
}

#[test]
fn entry_stats() {
    let Some(p4k) = open_p4k_or_skip() else {
        return;
    };

    let total = p4k.len();
    let encrypted = p4k.entries().iter().filter(|e| e.is_encrypted).count();
    let zstd = p4k
        .entries()
        .iter()
        .filter(|e| e.compression_method == 100)
        .count();
    let deflate = p4k
        .entries()
        .iter()
        .filter(|e| e.compression_method == 8)
        .count();
    let stored = p4k
        .entries()
        .iter()
        .filter(|e| e.compression_method == 0)
        .count();
    let other = total - zstd - deflate - stored;

    println!("P4k Entry Statistics:");
    println!("  Total:     {total}");
    println!("  Encrypted: {encrypted}");
    println!("  Zstd:      {zstd}");
    println!("  Deflate:   {deflate}");
    println!("  Stored:    {stored}");
    println!("  Other:     {other}");
}

#[test]
fn list_dir_root() {
    let Some(p4k) = open_p4k_or_skip() else {
        return;
    };

    let root = p4k.list_dir("");
    let dirs: Vec<_> = root
        .iter()
        .filter_map(|e| match e {
            DirEntry::Directory(name) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let files: Vec<_> = root
        .iter()
        .filter_map(|e| match e {
            DirEntry::File(entry) => Some(entry.name.as_str()),
            _ => None,
        })
        .collect();

    println!("Root: {} dirs, {} files", dirs.len(), files.len());
    for d in &dirs {
        println!("  [DIR] {d}");
    }
    for f in files.iter().take(5) {
        println!("  [FILE] {f}");
    }

    assert!(dirs.contains(&"Data"), "expected 'Data' directory at root");
}

#[test]
fn list_dir_character_customizer() {
    let Some(p4k) = open_p4k_or_skip() else {
        return;
    };

    let items = p4k.list_dir("Data\\Libs\\CharacterCustomizer");
    let dirs: Vec<_> = items
        .iter()
        .filter_map(|e| match e {
            DirEntry::Directory(name) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let files: Vec<_> = items
        .iter()
        .filter_map(|e| match e {
            DirEntry::File(entry) => Some(entry.name.as_str()),
            _ => None,
        })
        .collect();

    println!(
        "CharacterCustomizer: {} dirs, {} files",
        dirs.len(),
        files.len()
    );
    for d in &dirs {
        println!("  [DIR] {d}");
    }
    for f in &files {
        println!("  [FILE] {f}");
    }

    assert!(dirs.contains(&"PU"), "expected 'PU' subdirectory");
    assert!(
        files.iter().any(|f| f.contains("MasculineDefault")),
        "expected MasculineDefault"
    );
}

fn build_v2_archive(name: &str, content: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(content);
    let data_offset = 0u64;
    let cdr_offset = buf.len() as u64;

    let mut rec = [0u8; 204];
    rec[0x0A..0x12].copy_from_slice(&(content.len() as u64).to_le_bytes());
    rec[0x12..0x1A].copy_from_slice(&(content.len() as u64).to_le_bytes());
    rec[0x1A..0x22].copy_from_slice(&data_offset.to_le_bytes());
    rec[0x22..0x2A].copy_from_slice(&0u64.to_le_bytes());
    buf.extend_from_slice(&rec);

    let name_offset = buf.len() as u64;
    let mut names = name.as_bytes().to_vec();
    names.push(0);
    buf.extend_from_slice(&names);

    let mut eocd = [0u8; 175];
    eocd[0x00..0x08].copy_from_slice(&1u64.to_le_bytes());
    eocd[0x10..0x18].copy_from_slice(&cdr_offset.to_le_bytes());
    eocd[0x18..0x20].copy_from_slice(&204u64.to_le_bytes());
    eocd[0x28..0x30].copy_from_slice(&name_offset.to_le_bytes());
    eocd[0x30..0x38].copy_from_slice(&(names.len() as u64).to_le_bytes());
    eocd[0x60..0x68].copy_from_slice(&4096u64.to_le_bytes());
    eocd[0xA9..0xAB].copy_from_slice(&2u16.to_le_bytes());
    eocd[0xAB..0xAF].copy_from_slice(&0x696A694Au32.to_le_bytes());
    buf.extend_from_slice(&eocd);

    buf
}

#[test]
fn parse_synthetic_v2() {
    let content = b"hello v2 format, no local header here";
    let data = build_v2_archive("Data/test.txt", content);

    let archive = P4kArchive::from_bytes(&data).unwrap();
    assert_eq!(archive.len(), 1);

    let entry = archive.entry("Data\\test.txt").expect("entry by normalized name");
    assert_eq!(entry.uncompressed_size, content.len() as u64);
    assert_eq!(entry.compressed_size, content.len() as u64);
    assert!(!entry.has_local_header);

    let read = archive.read(entry).unwrap();
    assert_eq!(read, content);
}
