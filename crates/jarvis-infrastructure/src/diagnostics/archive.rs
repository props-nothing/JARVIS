//! A minimal, dependency-free ZIP writer for stored (uncompressed) members.
//!
//! The support bundle is the only archive JARVIS produces, and `Cargo.toml` has
//! no reviewed archive dependency. Rather than add one for a single use, this
//! module writes the small subset of the ZIP format the bundle needs:
//!
//! - method `0` (stored) only, so no compression library is involved and the
//!   bytes in the archive are exactly the bytes that were redacted;
//! - no ZIP64, which is safe only because [`MAX_ARCHIVE_BYTES`] bounds the total
//!   and every size is checked against the 32-bit field before it is written;
//! - a fixed DOS timestamp, so the same input produces byte-identical output.
//!
//! Determinism is a property worth having here: it makes "the previewed bundle
//! equals the exported bundle" checkable by comparing bytes, and it keeps a
//! digest meaningful. A build timestamp would make every bundle differ.

/// The largest archive this module will produce.
///
/// A bundle is a bounded diagnostic snapshot. The bound exists so a bug in the
/// collector cannot turn into an unbounded in-memory allocation, and it is
/// checked before any bytes are emitted.
pub const MAX_ARCHIVE_BYTES: usize = 16 * 1024 * 1024;

/// The largest number of members in one archive.
pub const MAX_ARCHIVE_ENTRIES: usize = 64;

/// The largest name a non-ZIP64 archive can encode.
const MAX_NAME_BYTES: usize = u16::MAX as usize;

/// Signature of a local file header.
const LOCAL_HEADER_SIGNATURE: u32 = 0x0403_4b50;
/// Signature of a central directory header.
const CENTRAL_HEADER_SIGNATURE: u32 = 0x0201_4b50;
/// Signature of the end-of-central-directory record.
const END_OF_CENTRAL_SIGNATURE: u32 = 0x0605_4b50;

/// The general-purpose flag that marks member names as UTF-8.
const FLAG_UTF8: u16 = 0x0800;

/// The DOS date for 1980-01-01, the earliest value the format can encode.
///
/// It is fixed rather than taken from the clock so archives are reproducible.
const FIXED_DOS_DATE: u16 = 0x0021;
/// The DOS time for 00:00:00, paired with [`FIXED_DOS_DATE`].
const FIXED_DOS_TIME: u16 = 0x0000;

/// An error raised while building an archive.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ArchiveError {
    /// The archive would exceed the size bound.
    #[error("the archive would exceed the {limit}-byte bound")]
    TooLarge {
        /// The bound that was exceeded.
        limit: usize,
    },
    /// The archive would contain more members than the bound allows.
    #[error("the archive would exceed the {limit}-member bound")]
    TooManyEntries {
        /// The bound that was exceeded.
        limit: usize,
    },
    /// A member name is empty, too long, or contains a path escape.
    #[error("an archive member name is empty, too long, or contains a path escape")]
    InvalidName,
    /// Two members would share one name.
    #[error("two archive members would share the name {name}")]
    DuplicateName {
        /// The duplicated name.
        name: String,
    },
}

impl ArchiveError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::TooLarge { .. } => "jarvis.archive_too_large",
            Self::TooManyEntries { .. } => "jarvis.archive_too_many_entries",
            Self::InvalidName => "jarvis.archive_invalid_name",
            Self::DuplicateName { .. } => "jarvis.archive_duplicate_name",
        }
    }
}

/// A finished archive plus the metadata a caller may want to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipArchive {
    bytes: Vec<u8>,
    entries: usize,
}

impl ZipArchive {
    /// Returns the archive bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consumes the archive and returns its bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Returns how many members the archive holds.
    #[must_use]
    pub const fn entries(&self) -> usize {
        self.entries
    }
}

/// CRC-32 (IEEE, reflected) lookup table, built at compile time.
///
/// ZIP uses the reflected IEEE polynomial. The table is `const` so the cost is
/// paid once at compile time and no initialization check can fail at runtime.
const CRC_TABLE: [u32; 256] = build_crc_table();

const fn build_crc_table() -> [u32; 256] {
    let mut table = [0_u32; 256];
    // The index is kept as `u32` so no narrowing conversion is involved: a
    // `u32` to `usize` conversion is widening on every supported target, while
    // the reverse would be a truncating cast.
    let mut index = 0_u32;
    while index < 256 {
        let mut value = index;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 {
                // The reflected IEEE polynomial, as ZIP defines it.
                0xedb8_8320 ^ (value >> 1)
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index as usize] = value;
        index += 1;
    }
    table
}

/// Computes the CRC-32 of `bytes`.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in bytes {
        let index = (crc ^ u32::from(*byte)) & 0xff;
        crc = CRC_TABLE[index as usize] ^ (crc >> 8);
    }
    crc ^ 0xffff_ffff
}

/// Builds a stored ZIP archive from `entries`.
///
/// # Errors
///
/// Returns [`ArchiveError::TooManyEntries`], [`ArchiveError::TooLarge`],
/// [`ArchiveError::InvalidName`], or [`ArchiveError::DuplicateName`] before any
/// bytes are returned, so a caller never receives a partially valid archive.
pub fn build_stored_archive(entries: &[(String, &[u8])]) -> Result<ZipArchive, ArchiveError> {
    if entries.len() > MAX_ARCHIVE_ENTRIES {
        return Err(ArchiveError::TooManyEntries {
            limit: MAX_ARCHIVE_ENTRIES,
        });
    }

    // Size is summed before writing so the bound is checked against the whole
    // archive rather than discovered when the allocation fails.
    let payload: usize = entries.iter().map(|(_, bytes)| bytes.len()).sum();
    let overhead = entries.len() * 128;
    if payload.saturating_add(overhead) > MAX_ARCHIVE_BYTES {
        return Err(ArchiveError::TooLarge {
            limit: MAX_ARCHIVE_BYTES,
        });
    }

    let mut seen: Vec<&str> = Vec::with_capacity(entries.len());
    let mut local = Vec::with_capacity(payload + overhead);
    let mut central: Vec<CentralEntry> = Vec::with_capacity(entries.len());

    for (name, bytes) in entries {
        validate_name(name)?;
        if seen.contains(&name.as_str()) {
            return Err(ArchiveError::DuplicateName { name: name.clone() });
        }
        seen.push(name.as_str());

        // Non-ZIP64 fields are 32-bit. The total-size bound makes an overflow
        // impossible, but the check stays because correctness must not depend on
        // a bound defined in another constant staying small.
        let Ok(size) = u32::try_from(bytes.len()) else {
            return Err(ArchiveError::TooLarge {
                limit: MAX_ARCHIVE_BYTES,
            });
        };

        let offset = u32::try_from(local.len()).map_err(|_| ArchiveError::TooLarge {
            limit: MAX_ARCHIVE_BYTES,
        })?;
        let crc = crc32(bytes);
        let name_bytes = name.as_bytes();

        let mut header = Vec::with_capacity(30 + name_bytes.len());
        push_u32(&mut header, LOCAL_HEADER_SIGNATURE);
        push_u16(&mut header, 20); // Version needed to extract: 2.0.
        push_u16(&mut header, FLAG_UTF8);
        push_u16(&mut header, 0); // Method: stored.
        push_u16(&mut header, FIXED_DOS_TIME);
        push_u16(&mut header, FIXED_DOS_DATE);
        push_u32(&mut header, crc);
        push_u32(&mut header, size);
        push_u32(&mut header, size);
        push_u16(
            &mut header,
            u16::try_from(name_bytes.len()).map_err(|_| ArchiveError::InvalidName)?,
        );
        push_u16(&mut header, 0); // Extra field length.
        header.extend_from_slice(name_bytes);
        local.extend_from_slice(&header);
        local.extend_from_slice(bytes);

        central.push(CentralEntry {
            name: name.clone(),
            crc,
            size,
            offset,
        });
    }

    let central_offset = u32::try_from(local.len()).map_err(|_| ArchiveError::TooLarge {
        limit: MAX_ARCHIVE_BYTES,
    })?;
    for entry in &central {
        local.extend_from_slice(&entry.bytes()?);
    }

    let central_size = u32::try_from(local.len()).map_err(|_| ArchiveError::TooLarge {
        limit: MAX_ARCHIVE_BYTES,
    })? - central_offset;
    let count = u16::try_from(central.len()).map_err(|_| ArchiveError::TooManyEntries {
        limit: MAX_ARCHIVE_ENTRIES,
    })?;

    push_u32(&mut local, END_OF_CENTRAL_SIGNATURE);
    push_u16(&mut local, 0); // This disk.
    push_u16(&mut local, 0); // Disk with the central directory.
    push_u16(&mut local, count);
    push_u16(&mut local, count);
    push_u32(&mut local, central_size);
    push_u32(&mut local, central_offset);
    push_u16(&mut local, 0); // Comment length.

    if local.len() > MAX_ARCHIVE_BYTES {
        return Err(ArchiveError::TooLarge {
            limit: MAX_ARCHIVE_BYTES,
        });
    }

    Ok(ZipArchive {
        bytes: local,
        entries: central.len(),
    })
}

/// One central-directory record, kept until its offset is known.
struct CentralEntry {
    name: String,
    crc: u32,
    size: u32,
    offset: u32,
}

impl CentralEntry {
    fn bytes(&self) -> Result<Vec<u8>, ArchiveError> {
        let name_bytes = self.name.as_bytes();
        let mut out = Vec::with_capacity(46 + name_bytes.len());
        push_u32(&mut out, CENTRAL_HEADER_SIGNATURE);
        push_u16(&mut out, 20); // Version made by.
        push_u16(&mut out, 20); // Version needed.
        push_u16(&mut out, FLAG_UTF8);
        push_u16(&mut out, 0); // Method: stored.
        push_u16(&mut out, FIXED_DOS_TIME);
        push_u16(&mut out, FIXED_DOS_DATE);
        push_u32(&mut out, self.crc);
        push_u32(&mut out, self.size);
        push_u32(&mut out, self.size);
        push_u16(
            &mut out,
            u16::try_from(name_bytes.len()).map_err(|_| ArchiveError::InvalidName)?,
        );
        push_u16(&mut out, 0); // Extra field length.
        push_u16(&mut out, 0); // Comment length.
        push_u16(&mut out, 0); // Disk number start.
        push_u16(&mut out, 0); // Internal attributes.
        push_u32(&mut out, 0); // External attributes.
        push_u32(&mut out, self.offset);
        out.extend_from_slice(name_bytes);
        Ok(out)
    }
}

/// Rejects a name that is empty, too long, or a path escape.
fn validate_name(name: &str) -> Result<(), ArchiveError> {
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return Err(ArchiveError::InvalidName);
    }
    // A member name is written into a file that a user will extract. Absolute
    // paths, drive letters, and `..` components are refused here rather than
    // sanitized, because a namespaced path is the caller's decision, not this
    // module's.
    if name.starts_with('/') || name.starts_with('\\') || name.contains(':') {
        return Err(ArchiveError::InvalidName);
    }
    for component in name.split(['/', '\\']) {
        if component == ".." {
            return Err(ArchiveError::InvalidName);
        }
    }
    if name.split(['/', '\\']).any(str::is_empty) {
        return Err(ArchiveError::InvalidName);
    }
    Ok(())
}

/// Writes a little-endian `u16`.
fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Writes a little-endian `u32`.
fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::{ArchiveError, MAX_ARCHIVE_ENTRIES, build_stored_archive, crc32, validate_name};

    #[test]
    fn crc32_matches_known_vectors() {
        // The canonical check value for the IEEE CRC-32 of "123456789".
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"a"), 0xe8b7_be43);
    }

    #[test]
    fn an_archive_starts_with_a_local_header_and_ends_with_the_central_record() {
        let entries = vec![("manifest.json".to_owned(), b"{}".as_slice())];
        let archive = build_stored_archive(&entries).expect("archive builds");
        let bytes = archive.bytes();
        assert_eq!(&bytes[0..4], &[0x50, 0x4b, 0x03, 0x04]);
        // The end-of-central-directory signature is somewhere near the end.
        assert!(
            bytes
                .windows(4)
                .any(|window| window == [0x50, 0x4b, 0x05, 0x06]),
            "the central directory record must exist"
        );
        assert_eq!(archive.entries(), 1);
        // The payload appears verbatim, because the member is stored.
        assert!(bytes.windows(2).any(|window| window == b"{}"));
    }

    #[test]
    fn identical_input_produces_identical_bytes() {
        let entries = vec![("a.txt".to_owned(), b"one".as_slice())];
        let first = build_stored_archive(&entries).expect("first");
        let second = build_stored_archive(&entries).expect("second");
        assert_eq!(first.bytes(), second.bytes());
    }

    #[test]
    fn path_escapes_and_drive_letters_are_refused() {
        assert_eq!(validate_name("../escape"), Err(ArchiveError::InvalidName));
        assert_eq!(validate_name("a/../../b"), Err(ArchiveError::InvalidName));
        assert_eq!(validate_name("C:/windows"), Err(ArchiveError::InvalidName));
        assert_eq!(validate_name("/absolute"), Err(ArchiveError::InvalidName));
        assert_eq!(validate_name("a//b"), Err(ArchiveError::InvalidName));
        assert_eq!(validate_name(""), Err(ArchiveError::InvalidName));
        assert_eq!(validate_name("logs/jarvis.log"), Ok(()));
    }

    #[test]
    fn duplicate_and_excess_entries_fail_closed() {
        let duplicate = vec![
            ("a".to_owned(), b"one".as_slice()),
            ("a".to_owned(), b"two".as_slice()),
        ];
        assert!(matches!(
            build_stored_archive(&duplicate),
            Err(ArchiveError::DuplicateName { .. })
        ));

        let many: Vec<(String, &[u8])> = (0..=MAX_ARCHIVE_ENTRIES)
            .map(|index| (format!("item-{index}"), b"x".as_slice()))
            .collect();
        assert!(matches!(
            build_stored_archive(&many),
            Err(ArchiveError::TooManyEntries { .. })
        ));
    }
}
