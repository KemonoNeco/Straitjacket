use crate::common::json_io::{read_json_file, write_json_file};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(clap::ValueEnum, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BugStatus {
    Open,
    Mirrored,
    Fixed,
    Wontfix,
    Duplicate,
}

impl BugStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            BugStatus::Open => "open",
            BugStatus::Mirrored => "mirrored",
            BugStatus::Fixed => "fixed",
            BugStatus::Wontfix => "wontfix",
            BugStatus::Duplicate => "duplicate",
        }
    }
}

#[derive(clap::Args, Debug)]
pub struct Args {
    #[arg(long)]
    pub repo_root: PathBuf,
    #[arg(long)]
    pub id: String,
    #[arg(long, value_enum)]
    pub set: BugStatus,
    #[arg(long)]
    pub note: Option<String>,
}

/// Finds the bug entry whose `id` matches `id` in `ledger`
/// (`{"bugs":[{...,"id":..,"status":..}]}`), sets its `status` to `status`,
/// optionally appends `note` to its `notes` field as a STRING (per
/// `schemas/bug-record.schema.json`, where `notes` is `type: string`), and
/// returns the updated ledger.
///
/// Note-append semantics:
/// - When `note` is `None`, only `status` is changed; `notes` is left
///   completely untouched.
/// - The prior `notes` content is coalesced to a string first: a string is
///   preserved as-is; a legacy array (written by older buggy code) has its
///   string elements joined; an absent key, JSON `null`, or any other scalar/
///   object is treated as empty.
/// - When the coalesced prior content is empty, the result is exactly the new
///   note (no leading separator). When it is non-empty, the new note is
///   appended after a newline separator, so sequential appends accumulate in
///   order. The result is always a JSON string.
///
/// Returns an error if no bug with the given id is found. All other bugs are
/// preserved unchanged.
pub fn set_bug_status(
    ledger: &serde_json::Value,
    id: &str,
    status: &str,
    note: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    let mut updated = ledger.clone();
    let bugs = updated
        .get_mut("bugs")
        .and_then(|b| b.as_array_mut())
        .ok_or_else(|| anyhow::anyhow!("ledger has no `bugs` array"))?;

    let bug = bugs
        .iter_mut()
        .find(|b| b.get("id").and_then(|v| v.as_str()) == Some(id))
        .ok_or_else(|| anyhow::anyhow!("no bug with id {id}"))?;

    bug["status"] = Value::String(status.to_string());

    if let Some(n) = note {
        // Coalesce any prior `notes` value into an owned string. Absent, null,
        // and any non-string/non-array scalar or object collapse to empty so we
        // never emit the literal "null" or a debug array representation.
        let prior = match bug.get("notes") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(arr)) => arr
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        let combined = if prior.is_empty() {
            n.to_string()
        } else {
            format!("{prior}\n{n}")
        };
        bug["notes"] = Value::String(combined);
    }

    Ok(updated)
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let path = args.repo_root.join(".straitjacket").join("bugs.json");
    let ledger: Value = read_json_file(&path)
        .with_context(|| format!("failed to read bug ledger at {}", path.display()))?;
    let updated = set_bug_status(&ledger, &args.id, args.set.as_str(), args.note.as_deref())?;
    write_json_file(&path, &updated)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "id": args.id,
            "status": args.set.as_str(),
        }))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn two_bug_ledger() -> serde_json::Value {
        json!({
            "bugs": [
                {"id": "bug-001", "title": "crash on startup", "status": "open"},
                {"id": "bug-002", "title": "missing icon",     "status": "open"}
            ]
        })
    }

    #[test]
    fn set_bug_status_flips_status_of_matching_id_and_leaves_other_bug_untouched() {
        let ledger = two_bug_ledger();
        let updated = set_bug_status(&ledger, "bug-001", "fixed", None).unwrap();
        assert_eq!(updated["bugs"][0]["status"], "fixed");
        // Second bug must be untouched.
        assert_eq!(updated["bugs"][1]["status"], "open");
        assert_eq!(updated["bugs"][1]["id"], "bug-002");
    }

    #[test]
    fn test_set_bug_status_initializes_absent_notes_as_string_not_array() {
        // Starting ledger has no `notes` field on the bug.
        let ledger = two_bug_ledger();
        let updated =
            set_bug_status(&ledger, "bug-001", "fixed", Some("confirmed in v1.2")).unwrap();
        assert!(updated["bugs"][0]["notes"].is_string());
        assert!(!updated["bugs"][0]["notes"].is_array());
        assert_eq!(updated["bugs"][0]["notes"], "confirmed in v1.2");
    }

    #[test]
    fn test_set_bug_status_appends_note_to_existing_string_notes_preserving_prior_content() {
        let ledger = json!({
            "bugs": [
                {"id": "bug-001", "title": "crash on startup", "status": "open",
                 "notes": "reproduced on Windows see thread"}
            ]
        });
        let updated =
            set_bug_status(&ledger, "bug-001", "fixed", Some("appended note text")).unwrap();
        assert_eq!(updated["bugs"][0]["status"], "fixed");
        assert!(updated["bugs"][0]["notes"].is_string());
        let notes_str = updated["bugs"][0]["notes"].as_str().unwrap();
        assert!(notes_str.contains("reproduced on Windows see thread"));
        assert!(notes_str.contains("appended note text"));
    }

    #[test]
    fn test_set_bug_status_coalesces_legacy_array_notes_into_string_without_dropping_content() {
        let ledger = json!({
            "bugs": [
                {"id": "bug-001", "title": "crash on startup", "status": "open",
                 "notes": ["first legacy note", "second legacy note"]}
            ]
        });
        let updated =
            set_bug_status(&ledger, "bug-001", "fixed", Some("new note after migration")).unwrap();
        assert!(updated["bugs"][0]["notes"].is_string());
        assert!(!updated["bugs"][0]["notes"].is_array());
        let notes_str = updated["bugs"][0]["notes"].as_str().unwrap();
        assert!(notes_str.contains("first legacy note"));
        assert!(notes_str.contains("second legacy note"));
        assert!(notes_str.contains("new note after migration"));
    }

    #[test]
    fn test_set_bug_status_with_no_note_flips_status_and_leaves_existing_string_notes_untouched() {
        let ledger = json!({
            "bugs": [
                {"id": "bug-001", "title": "crash on startup", "status": "open",
                 "notes": "untouched freeform note"}
            ]
        });
        let updated = set_bug_status(&ledger, "bug-001", "wontfix", None).unwrap();
        assert_eq!(updated["bugs"][0]["status"], "wontfix");
        assert!(updated["bugs"][0]["notes"].is_string());
        assert_eq!(updated["bugs"][0]["notes"], "untouched freeform note");
    }

    #[test]
    fn set_bug_status_returns_err_for_unknown_id() {
        let ledger = two_bug_ledger();
        let result = set_bug_status(&ledger, "bug-999", "fixed", None);
        assert!(result.is_err());
    }

    #[test]
    fn set_bug_status_preserves_other_fields_on_the_matched_bug() {
        let ledger = two_bug_ledger();
        let updated = set_bug_status(&ledger, "bug-001", "wontfix", None).unwrap();
        // `title` must survive the status update.
        assert_eq!(updated["bugs"][0]["title"], "crash on startup");
    }

    #[test]
    fn test_set_bug_status_sequential_appends_accumulate_in_order() {
        let ledger = json!({
            "bugs": [
                {"id": "bug-001", "title": "crash on startup", "status": "open",
                 "notes": "original"}
            ]
        });
        let after_first =
            set_bug_status(&ledger, "bug-001", "open", Some("first append")).unwrap();
        let after_second =
            set_bug_status(&after_first, "bug-001", "open", Some("second append")).unwrap();
        let notes = &after_second["bugs"][0]["notes"];
        assert!(notes.is_string());
        let s = notes.as_str().unwrap();
        assert!(s.contains("original"));
        assert!(s.contains("first append"));
        assert!(s.contains("second append"));
        let pos_original = s.find("original").unwrap();
        let pos_first = s.find("first append").unwrap();
        let pos_second = s.find("second append").unwrap();
        assert!(pos_original < pos_first);
        assert!(pos_first < pos_second);
    }

    #[test]
    fn test_set_bug_status_coalesces_legacy_array_into_readable_string_not_debug_format() {
        let ledger = json!({
            "bugs": [
                {"id": "bug-001", "title": "crash on startup", "status": "open",
                 "notes": ["alpha note", "beta note"]}
            ]
        });
        let updated =
            set_bug_status(&ledger, "bug-001", "fixed", Some("gamma note")).unwrap();
        let notes = &updated["bugs"][0]["notes"];
        assert!(notes.is_string());
        let s = notes.as_str().unwrap();
        assert!(s.contains("alpha note"));
        assert!(s.contains("beta note"));
        assert!(s.contains("gamma note"));
        assert!(!s.contains("[\""));
        assert!(!s.contains("\"]"));
    }

    #[test]
    fn test_set_bug_status_appends_to_empty_string_notes_without_leading_separator() {
        let ledger = json!({
            "bugs": [
                {"id": "bug-001", "title": "crash on startup", "status": "open",
                 "notes": ""}
            ]
        });
        let updated =
            set_bug_status(&ledger, "bug-001", "fixed", Some("the note")).unwrap();
        let notes = &updated["bugs"][0]["notes"];
        assert!(notes.is_string());
        assert_eq!(notes, "the note");
    }

    #[test]
    fn test_set_bug_status_coalesces_empty_legacy_array_to_just_the_note() {
        let ledger = json!({
            "bugs": [
                {"id": "bug-001", "title": "crash on startup", "status": "open",
                 "notes": []}
            ]
        });
        let updated =
            set_bug_status(&ledger, "bug-001", "fixed", Some("the note")).unwrap();
        let notes = &updated["bugs"][0]["notes"];
        assert!(notes.is_string());
        assert!(!notes.is_array());
        assert_eq!(notes, "the note");
    }

    #[test]
    fn test_set_bug_status_treats_null_notes_as_absent_initializing_string() {
        let ledger = json!({
            "bugs": [
                {"id": "bug-001", "title": "crash on startup", "status": "open",
                 "notes": null}
            ]
        });
        let updated =
            set_bug_status(&ledger, "bug-001", "fixed", Some("the note")).unwrap();
        let notes = &updated["bugs"][0]["notes"];
        assert!(notes.is_string());
        assert!(!notes.is_array());
        assert_eq!(notes, "the note");
        assert_ne!(notes, "null");
    }
}
