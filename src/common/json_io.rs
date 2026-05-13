use anyhow::Context;
use serde::{de::DeserializeOwned, Serialize};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

/// Reads a JSON file at `path` and deserializes into `T`. UTF-8 encoding assumed.
/// Returns an error if the file is missing or contains invalid JSON.
pub fn read_json_file<T: DeserializeOwned>(path: &Path) -> anyhow::Result<T> {
    let file = File::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let reader = BufReader::new(file);
    let value: T = serde_json::from_reader(reader)
        .with_context(|| format!("failed to parse JSON from {}", path.display()))?;
    Ok(value)
}

/// Writes `value` as pretty-printed JSON to `path`, UTF-8 encoded, terminated by a newline.
/// Creates parent directories if missing. Overwrites if the file exists.
pub fn write_json_file<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create parent dir of {}", path.display()))?;
        }
    }
    let file = File::create(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, value)
        .with_context(|| format!("failed to serialize JSON to {}", path.display()))?;
    writer.write_all(b"\n").context("failed to write trailing newline")?;
    writer.flush().context("failed to flush")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::fs;
    use tempfile::TempDir;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Sample {
        name: String,
        value: u32,
        tags: Vec<String>,
    }

    fn sample() -> Sample {
        Sample {
            name: "alice".to_string(),
            value: 42,
            tags: vec!["one".into(), "two".into()],
        }
    }

    #[test]
    fn write_then_read_round_trips_value() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("data.json");
        let original = sample();
        write_json_file(&path, &original).unwrap();
        let restored: Sample = read_json_file(&path).unwrap();
        assert_eq!(original, restored);
    }

    #[test]
    fn write_creates_missing_parent_directories() {
        let td = TempDir::new().unwrap();
        let nested = td.path().join("a").join("b").join("c").join("data.json");
        write_json_file(&nested, &sample()).unwrap();
        assert!(nested.exists(), "nested file not created");
    }

    #[test]
    fn write_emits_trailing_newline() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("data.json");
        write_json_file(&path, &sample()).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(
            content.ends_with('\n'),
            "expected trailing newline; ends with: {:?}",
            content.chars().last()
        );
    }

    #[test]
    fn write_overwrites_existing_file() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("data.json");
        write_json_file(&path, &sample()).unwrap();
        let replacement = Sample {
            name: "bob".into(),
            value: 99,
            tags: vec![],
        };
        write_json_file(&path, &replacement).unwrap();
        let restored: Sample = read_json_file(&path).unwrap();
        assert_eq!(restored, replacement);
    }

    #[test]
    fn read_returns_err_on_missing_file() {
        let td = TempDir::new().unwrap();
        let missing = td.path().join("absent.json");
        let result: anyhow::Result<Sample> = read_json_file(&missing);
        assert!(result.is_err(), "expected Err for missing file");
    }

    #[test]
    fn read_returns_err_on_invalid_json() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("bad.json");
        fs::write(&path, "{this is not json").unwrap();
        let result: anyhow::Result<Sample> = read_json_file(&path);
        assert!(result.is_err(), "expected Err for invalid JSON");
    }

    /// Adversarial-review #wu-jsonio-002 (MEDIUM): contract says "pretty-printed".
    /// Round-trip alone passes for compact JSON. Verify multi-line indented output.
    #[test]
    fn output_is_pretty_printed_not_compact() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("data.json");
        write_json_file(&path, &sample()).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        let newline_count = content.matches('\n').count();
        assert!(
            newline_count > 2,
            "expected pretty-printed multi-line JSON; got {} newlines in: {:?}",
            newline_count,
            content
        );
        assert!(
            content.contains("  \"name\"") || content.contains("  \"value\""),
            "expected indented field, got: {:?}",
            content
        );
    }

    /// Adversarial-review #wu-jsonio-001 (LOW): UTF-8 multibyte round-trip.
    #[test]
    fn utf8_multibyte_content_round_trips_losslessly() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("unicode.json");
        let original = Sample {
            name: "おはよう 🌸 émojí ünïcôdé".into(),
            value: 0,
            tags: vec!["café".into(), "日本語".into()],
        };
        write_json_file(&path, &original).unwrap();
        let restored: Sample = read_json_file(&path).unwrap();
        assert_eq!(original, restored);
    }
}
