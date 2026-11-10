/// UTF-8 BOM (byte order mark) detection and stripping.
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// Read a file and strip the UTF-8 BOM prefix if present.
pub fn read_file(path: impl AsRef<std::path::Path>) -> std::io::Result<String> {
    let mut bytes = std::fs::read(path)?;
    strip_bom_bytes(&mut bytes);
    String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Read a file, strip the UTF-8 BOM, and return content + mtime.
pub fn read_file_with_mtime(
    path: impl AsRef<std::path::Path>,
) -> std::io::Result<(String, std::time::SystemTime)> {
    let path = path.as_ref();
    let mut bytes = std::fs::read(path)?;
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    strip_bom_bytes(&mut bytes);
    let content = String::from_utf8(bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok((content, mtime))
}

/// Strip a UTF-8 BOM prefix from a byte vector in place.
fn strip_bom_bytes(bytes: &mut Vec<u8>) {
    if bytes.starts_with(UTF8_BOM) {
        bytes.drain(..3);
    }
}

/// Decode bytes that are UTF-16 into a `String`, or return `None` when the
/// bytes are not confidently UTF-16.
///
/// Detection: an explicit BOM (`FF FE` = LE, `FE FF` = BE) always wins. With
/// no BOM we fall back to a NUL-parity heuristic on the first 256 bytes —
/// UTF-16LE ASCII text has `0x00` at every odd offset, UTF-16BE at every
/// even offset — and only accept when the decoded sample is mostly printable
/// so random binary can't false-positive.
pub fn decode_utf16(bytes: &[u8]) -> Option<String> {
    // FF FE 00 00 is the UTF-32LE BOM, not UTF-16 — don't misdecode it.
    let (little_endian, skip) = if bytes.starts_with(&[0xFE, 0xFF]) {
        (false, 2usize)
    } else if bytes.starts_with(&[0xFF, 0xFE]) && !bytes.starts_with(&[0xFF, 0xFE, 0x00, 0x00]) {
        (true, 2usize)
    } else {
        let sample = &bytes[..bytes.len().min(256)];
        if sample.len() < 8 {
            return None;
        }
        let pairs = sample.len() / 2;
        let even_zeros = sample.iter().step_by(2).filter(|&&b| b == 0).count();
        let odd_zeros = sample
            .iter()
            .skip(1)
            .step_by(2)
            .filter(|&&b| b == 0)
            .count();
        // Require ≥75% of one parity to be NUL and the other parity to be
        // almost entirely non-NUL — the signature of ASCII-range UTF-16.
        if odd_zeros * 4 >= pairs * 3 && even_zeros * 4 <= pairs {
            (true, 0usize) // LE: [char, 0x00] pairs
        } else if even_zeros * 4 >= pairs * 3 && odd_zeros * 4 <= pairs {
            (false, 0usize) // BE: [0x00, char] pairs
        } else {
            return None;
        }
    };

    let body = &bytes[skip..];
    if body.len() < 2 {
        return Some(String::new());
    }
    let units: Vec<u16> = body
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| {
            if little_endian {
                u16::from_le_bytes(*c)
            } else {
                u16::from_be_bytes(*c)
            }
        })
        .collect();
    let text = String::from_utf16_lossy(&units);

    // Sanity check for the heuristic path (BOM is trusted outright): reject
    // decodes that are mostly replacement chars or C0 control noise.
    if skip == 0 {
        let sample: Vec<char> = text.chars().take(256).collect();
        if !sample.is_empty() {
            let bad = sample
                .iter()
                .filter(|&&c| {
                    c == '\u{FFFD}' || (c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
                })
                .count();
            if bad * 2 > sample.len() {
                return None;
            }
        }
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn strips_bom_from_file() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"\xEF\xBB\xBFhello world").unwrap();
        let content = read_file(f.path()).unwrap();
        assert_eq!(content, "hello world");
    }

    #[test]
    fn no_bom_unchanged() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"hello world").unwrap();
        let content = read_file(f.path()).unwrap();
        assert_eq!(content, "hello world");
    }

    #[test]
    fn only_bom_returns_empty() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"\xEF\xBB\xBF").unwrap();
        let content = read_file(f.path()).unwrap();
        assert_eq!(content, "");
    }

    #[test]
    fn with_mtime_strips_bom() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"\xEF\xBB\xBFtest").unwrap();
        let (content, mtime) = read_file_with_mtime(f.path()).unwrap();
        assert_eq!(content, "test");
        assert!(mtime > std::time::SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn accepts_string_path() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"\xEF\xBB\xBFok").unwrap();
        let path_str = f.path().to_string_lossy().to_string();
        let content = read_file(&path_str).unwrap();
        assert_eq!(content, "ok");
    }
}
