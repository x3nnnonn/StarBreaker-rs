use zerocopy::{FromBytes, Immutable, KnownLayout};

// ── Signature constants ──────────────────────────────────────────────────────

/// Standard End of Central Directory signature.
pub const EOCD_SIGNATURE: u32 = 0x06054B50;
/// ZIP64 End of Central Directory Locator signature.
pub const ZIP64_LOCATOR_SIGNATURE: u32 = 0x07064B50;
/// ZIP64 End of Central Directory Record signature.
pub const EOCD64_SIGNATURE: u32 = 0x06064B50;
/// Central Directory File Header signature.
pub const CENTRAL_DIR_SIGNATURE: u32 = 0x02014B50;
/// Standard Local File Header signature.
pub const LOCAL_FILE_SIGNATURE: u32 = 0x04034B50;
/// CIG-specific Local File Header signature.
pub const LOCAL_FILE_CIG_SIGNATURE: u32 = 0x14034B50;

/// EOCD signature as little-endian bytes for byte scanning.
pub const EOCD_MAGIC: [u8; 4] = EOCD_SIGNATURE.to_le_bytes();

// ── Packed binary structs ────────────────────────────────────────────────────

/// End of Central Directory Record (22 bytes).
#[derive(Debug, Clone, Copy, FromBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct EocdRecord {
    pub signature: u32,
    pub disk_number: u16,
    pub start_disk_number: u16,
    pub entries_on_disk: u16,
    pub total_entries: u16,
    pub central_directory_size: u32,
    pub central_directory_offset: u32,
    pub comment_length: u16,
}

const _: () = assert!(size_of::<EocdRecord>() == 22);

impl EocdRecord {
    /// Returns true if any field indicates ZIP64 is needed.
    pub fn is_zip64(&self) -> bool {
        self.disk_number == 0xFFFF
            || self.start_disk_number == 0xFFFF
            || self.entries_on_disk == 0xFFFF
            || self.total_entries == 0xFFFF
            || self.central_directory_size == 0xFFFFFFFF
            || self.central_directory_offset == 0xFFFFFFFF
    }
}

/// ZIP64 End of Central Directory Locator (20 bytes).
#[derive(Debug, Clone, Copy, FromBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct Zip64Locator {
    pub signature: u32,
    pub disk_with_eocd64: u32,
    pub eocd64_offset: u64,
    pub total_disks: u32,
}

const _: () = assert!(size_of::<Zip64Locator>() == 20);

/// ZIP64 End of Central Directory Record (56 bytes).
#[derive(Debug, Clone, Copy, FromBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct Eocd64Record {
    pub signature: u32,
    pub size_of_record: u64,
    pub version_made_by: u16,
    pub version_needed: u16,
    pub disk_number: u32,
    pub start_disk_number: u32,
    pub entries_on_disk: u64,
    pub total_entries: u64,
    pub central_directory_size: u64,
    pub central_directory_offset: u64,
}

const _: () = assert!(size_of::<Eocd64Record>() == 56);

/// Central Directory File Header (46 bytes).
#[derive(Debug, Clone, Copy, FromBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct CentralDirHeader {
    pub signature: u32,
    pub version_made_by: u16,
    pub version_needed: u16,
    pub flags: u16,
    pub compression_method: u16,
    pub last_modified: u32,
    pub crc32: u32,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub file_name_length: u16,
    pub extra_field_length: u16,
    pub file_comment_length: u16,
    pub disk_number_start: u16,
    pub internal_file_attributes: u16,
    pub external_file_attributes: u32,
    pub local_header_offset: u32,
}

const _: () = assert!(size_of::<CentralDirHeader>() == 46);

/// Local File Header (30 bytes, including the 4-byte signature).
#[derive(Debug, Clone, Copy, FromBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct LocalFileHeader {
    pub signature: u32,
    pub version_needed: u16,
    pub flags: u16,
    pub compression_method: u16,
    pub last_mod_time: u16,
    pub last_mod_date: u16,
    pub crc32: u32,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub file_name_length: u16,
    pub extra_field_length: u16,
}

const _: () = assert!(size_of::<LocalFileHeader>() == 30);

pub const EOCD_V2_MAGIC: u32 = 0x696A694A;
pub const EOCD_V2_VERSION: u16 = 2;
pub const EOCD_V2_SIZE: usize = 175;
pub const CDR_V2_ENTRY_SIZE: usize = 204;

#[derive(Debug, Clone, Copy, FromBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct Eocd2Record {
    pub number_of_entries: u64,
    pub number_of_extension_entries: u64,
    pub central_directory_offset: u64,
    pub central_directory_size: u64,
    pub central_directory_extension_size: u64,
    pub name_table_offset: u64,
    pub name_table_size: u64,
    pub extension_text_size: u64,
    pub end_of_payload: u64,
    pub num_freelist_blocks: u64,
    pub trie_cache_offset: u64,
    pub trie_cache_size: u64,
    pub sector_size: u64,
    pub status_flags: u8,
    pub manifest_sha256: [u8; 64],
    pub version: u16,
    pub magic: u32,
}

const _: () = assert!(size_of::<Eocd2Record>() == EOCD_V2_SIZE);

#[derive(Debug, Clone, Copy, FromBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct CentralDirHeaderV2 {
    pub compression_method: u16,
    pub last_mod_time: u16,
    pub last_mod_date: u16,
    pub crc32: u32,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub offset_to_file_data: u64,
    pub offset_to_filename: u64,
    pub signature: [u8; 128],
    pub encryption_flag: u16,
    pub sha256: [u8; 32],
}

const _: () = assert!(size_of::<CentralDirHeaderV2>() == CDR_V2_ENTRY_SIZE);
