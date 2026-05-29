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
/// optionally appends `note` to its `notes` array (creating the array if
/// absent), and returns the updated ledger. Returns an error if no bug with
/// the given id is found. All other bugs are preserved unchanged.
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
        let needs_init = !bug.get("notes").is_some_and(Value::is_array);
        if needs_init {
            bug["notes"] = Value::Array(Vec::new());
        }
        if let Some(notes) = bug.get_mut("notes").and_then(Value::as_array_mut) {
            notes.push(Value::String(n.to_string()));
        }
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
    fn set_bug_status_appends_note_and_creates_notes_array_when_absent() {
        // Starting ledger has no `notes` field on the bug.
        let ledger = two_bug_ledger();
        let updated =
            set_bug_status(&ledger, "bug-001", "fixed", Some("confirmed in v1.2")).unwrap();
        assert_eq!(updated["bugs"][0]["notes"], json!(["confirmed in v1.2"]));
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
}
