use anyhow::Context;
use serde::{de::DeserializeOwned, Serialize};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

/// Reads a JSON file at `path` and deserializes into `T`. UTF-8 encoding assumed.
/// Returns an error if the file is missing or contains invalid JSON.
///
/// A single leading UTF-8 BOM (`EF BB BF` / `U+FEFF`) is stripped before parsing, since
/// `serde_json` does not treat it as whitespace and would otherwise reject the file. Only
/// the 3-byte UTF-8 BOM is handled; a non-UTF-8 file still errors at the read-to-string step.
pub fn read_json_file<T: DeserializeOwned>(path: &Path) -> anyhow::Result<T> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let value: T = serde_json::from_str(text)
        .with_context(|| format!("failed to parse JSON from {}", path.display()))?;
    Ok(value)
}

/// Extracts a borrowed slice of work-units from a parsed JSON value, accepting either a
/// bare top-level array OR the orchestrator's `{"work_units": [...], ...}` wrapper shape.
/// Returns `None` for any other shape (including an Object whose `work_units` field is
/// missing or not an array, scalars, etc.) so callers can choose: map `None` to `&[]` for
/// silent-accept, or map `None` to an error for strict-accept.
pub fn parse_work_units_array(value: &serde_json::Value) -> Option<&[serde_json::Value]> {
    match value {
        serde_json::Value::Array(a) => Some(a.as_slice()),
        serde_json::Value::Object(o) => o
            .get("work_units")
            .and_then(|x| x.as_array())
            .map(|a| a.as_slice()),
        _ => None,
    }
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

    #[test]
    fn parse_work_units_array_accepts_bare_array_shape() {
        let v = serde_json::json!([
            {"id": "a", "status": "written"},
            {"id": "b", "status": "pending"}
        ]);
        let arr = parse_work_units_array(&v).expect("bare array must yield Some");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["id"], "a");
        assert_eq!(arr[1]["id"], "b");
    }

    #[test]
    fn parse_work_units_array_accepts_wrapper_object_shape() {
        // The coverage-reviewer agent's documented response shape.
        let v = serde_json::json!({
            "work_units": [
                {"id": "inner-a", "status": "written"},
                {"id": "inner-b", "status": "pending"}
            ],
            "scope_summary": "wrapper-level metadata that must not be treated as a unit"
        });
        let arr = parse_work_units_array(&v).expect("wrapper object must yield Some");
        assert_eq!(
            arr.len(),
            2,
            "wrapper object's `work_units` inner array must be extracted; got: {arr:?}"
        );
        assert_eq!(arr[0]["id"], "inner-a");
        assert_eq!(arr[1]["id"], "inner-b");
    }

    #[test]
    fn parse_work_units_array_returns_none_for_object_without_work_units_key() {
        // Object shape but no `work_units` key — caller must not see the whole object
        // treated as a single status-less unit. None lets strict callers raise.
        let v = serde_json::json!({"unrelated": "object", "scope_summary": "x"});
        let arr = parse_work_units_array(&v);
        assert!(
            arr.is_none(),
            "object without `work_units` key must yield None, got: {arr:?}"
        );
    }

    #[test]
    fn parse_work_units_array_returns_none_for_unrecognized_scalar_shape() {
        // A bare scalar (string/number/null/bool) must not blow up; just None.
        for v in [
            serde_json::json!("not a structure"),
            serde_json::json!(42),
            serde_json::json!(null),
            serde_json::json!(true),
        ] {
            let arr = parse_work_units_array(&v);
            assert!(arr.is_none(), "scalar {v:?} must yield None, got: {arr:?}");
        }
    }

    /// Adversarial-review #wu-jsonio-001 (LOW): UTF-8 multibyte round-trip.
    /// Fix #bug-2026-06-02-11: a leading UTF-8 BOM (EF BB BF) must be tolerated.
    /// Currently `serde_json::from_reader` over raw bytes fails with
    /// "expected value at line 1 column 1". The BOM-prefixed file must parse to
    /// the SAME value as the BOM-free file.
    #[test]
    fn read_strips_leading_utf8_bom_and_parses() {
        let td = TempDir::new().unwrap();

        let json = br#"{"name":"alice","value":42,"tags":["one","two"]}"#;

        // BOM-free file: the reference value.
        let plain_path = td.path().join("plain.json");
        fs::write(&plain_path, json).unwrap();
        let from_plain: Sample = read_json_file(&plain_path).unwrap();

        // BOM-prefixed file: 3 raw BOM bytes followed by the identical JSON.
        let bom_path = td.path().join("bom.json");
        let mut bytes = Vec::with_capacity(3 + json.len());
        bytes.extend_from_slice(b"\xEF\xBB\xBF");
        bytes.extend_from_slice(json);
        fs::write(&bom_path, &bytes).unwrap();

        let from_bom: Sample = read_json_file(&bom_path)
            .expect("BOM-prefixed but otherwise-valid JSON must parse");

        assert_eq!(
            from_bom, from_plain,
            "BOM-prefixed file must deserialize to the same value as the BOM-free file"
        );
        assert_eq!(from_bom, sample());
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
