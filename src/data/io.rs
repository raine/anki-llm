use std::fs;
use std::io;
use std::path::Path;

use indexmap::IndexMap;
use tempfile::NamedTempFile;

use super::csv_io;
use super::error::DataError;
use super::rows::{Row, require_note_id};
use super::yaml_io;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileFormat {
    Csv,
    Yaml,
}

/// Determine file format from extension.
pub fn file_format(path: &Path) -> Result<FileFormat, DataError> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("csv") => Ok(FileFormat::Csv),
        Some("yaml" | "yml") => Ok(FileFormat::Yaml),
        Some(ext) => Err(DataError::UnsupportedFormat(format!(".{ext}"))),
        None => Err(DataError::UnsupportedFormat("(no extension)".into())),
    }
}

/// Read and parse a data file (CSV or YAML).
pub fn parse_data_file(path: &Path) -> Result<Vec<Row>, DataError> {
    let content = fs::read_to_string(path)?;
    match file_format(path)? {
        FileFormat::Csv => csv_io::parse_csv(&content),
        FileFormat::Yaml => yaml_io::parse_yaml(&content),
    }
}

/// Serialize rows to a string in the format matching the file extension.
pub fn serialize_rows(rows: &[Row], path: &Path) -> Result<String, DataError> {
    match file_format(path)? {
        FileFormat::Csv => csv_io::serialize_csv(rows),
        FileFormat::Yaml => yaml_io::serialize_yaml(rows),
    }
}

/// Atomically write content to a file via temp-file-then-rename.
pub fn atomic_write_file(path: &Path, content: &str) -> Result<(), DataError> {
    use std::io::Write;
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut tmp = NamedTempFile::new_in(dir)?;
    tmp.write_all(content.as_bytes())?;
    tmp.persist(path).map_err(|e| DataError::Io(e.error))?;
    Ok(())
}

/// Load an existing output file as a map keyed by note ID.
///
/// Returns an empty map if the file does not exist. Returns an error for any
/// other failure (permission denied, parse error, etc.) so the caller can bail
/// rather than silently overwriting prior progress.
pub fn load_existing_output(path: &Path) -> anyhow::Result<IndexMap<String, Row>> {
    let rows = match parse_data_file(path) {
        Ok(r) => r,
        Err(DataError::Io(e)) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(IndexMap::new());
        }
        Err(e) => return Err(anyhow::anyhow!(e)),
    };

    let mut map = IndexMap::new();
    for (i, row) in rows.into_iter().enumerate() {
        let id = require_note_id(&row).map_err(|e| {
            anyhow::anyhow!(
                "row {} in existing output {} has no note id: {e}",
                i + 1,
                path.display()
            )
        })?;
        if map.contains_key(&id) {
            anyhow::bail!(
                "duplicate note id '{id}' in existing output {} — refusing to silently overwrite prior progress",
                path.display()
            );
        }
        map.insert(id, row);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn file_format_csv() {
        assert_eq!(
            file_format(&PathBuf::from("foo.csv")).unwrap(),
            FileFormat::Csv
        );
    }

    #[test]
    fn file_format_yaml() {
        assert_eq!(
            file_format(&PathBuf::from("foo.yaml")).unwrap(),
            FileFormat::Yaml
        );
    }

    #[test]
    fn file_format_yml() {
        assert_eq!(
            file_format(&PathBuf::from("foo.yml")).unwrap(),
            FileFormat::Yaml
        );
    }

    #[test]
    fn file_format_uppercase() {
        assert_eq!(
            file_format(&PathBuf::from("foo.CSV")).unwrap(),
            FileFormat::Csv
        );
    }

    #[test]
    fn file_format_unsupported() {
        assert!(file_format(&PathBuf::from("foo.txt")).is_err());
    }

    #[test]
    fn file_format_no_extension() {
        assert!(file_format(&PathBuf::from("foo")).is_err());
    }

    #[test]
    fn atomic_write_and_read_back() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("out.txt");
        atomic_write_file(&path, "hello world").unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "hello world");
    }

    #[test]
    fn load_existing_output_nonexistent() {
        let map = load_existing_output(&PathBuf::from("/nonexistent/path/file.yaml")).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn load_existing_output_builds_map() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("data.yaml");
        let content = "- noteId: 123\n  front: hello\n- noteId: 456\n  front: world\n";
        fs::write(&path, content).unwrap();
        let map = load_existing_output(&path).unwrap();
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("123"));
        assert!(map.contains_key("456"));
    }

    #[test]
    fn load_existing_output_corrupt_file_errors() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("data.yaml");
        fs::write(&path, "not: valid: yaml: [\n").unwrap();
        assert!(load_existing_output(&path).is_err());
    }
}
