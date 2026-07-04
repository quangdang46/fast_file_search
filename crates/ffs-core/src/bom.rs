/// UTF-8 BOM (byte order mark) detection and stripping.
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// Read a file and strip the UTF-8 BOM prefix if present.
pub fn read_file(path: impl AsRef<std::path::Path>) -> std::io::Result<String> {
    let mut bytes = std::fs::read(path)?;
    strip_bom_bytes(&mut bytes);
    String::from_utf8(bytes).map_err(|e| std::io::Error::new(
        std::io::ErrorKind::InvalidData, e,
    ))
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
    let content = String::from_utf8(bytes).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e)
    })?;
    Ok((content, mtime))
}

/// Strip a UTF-8 BOM prefix from a byte vector in place.
fn strip_bom_bytes(bytes: &mut Vec<u8>) {
    if bytes.starts_with(UTF8_BOM) {
        bytes.drain(..3);
    }
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