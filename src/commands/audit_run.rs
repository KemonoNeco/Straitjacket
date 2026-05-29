use crate::common::Stack;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ─── Public types ────────────────────────────────────────────────────────────

#[derive(clap::ValueEnum, Debug, Clone, PartialEq, Eq)]
#[clap(rename_all = "kebab-case")]
pub enum AuditTool {
    ClippyDeadCode,
    CargoAudit,
    CargoDeny,
    CargoGeiger,
    CargoUdeps,
    DotnetVulnerable,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct AuditInvocation {
    pub program: String,
    pub args: Vec<String>,
}

/// A single finding emitted by an audit tool. Field names match schemas/audit-finding.schema.json.
#[derive(Debug, Serialize, Deserialize)]
pub struct Finding {
    pub lens: String,
    pub source: String,
    pub severity: String,
    pub title: String,
    pub summary: String,
    pub suspect_files: Vec<String>,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub evidence: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AuditRunResult {
    pub tool: String,
    pub available: bool,
    pub nothing_scanned: bool,
    pub findings: Vec<Finding>,
}

#[derive(clap::Args, Debug)]
pub struct Args {
    #[arg(long, value_enum)]
    pub tool: AuditTool,
    #[arg(long, value_enum)]
    pub stack: Stack,
    #[arg(long)]
    pub repo_root: PathBuf,
}

// ─── Pure helpers (stubs) ─────────────────────────────────────────────────────

/// Returns the cargo/dotnet invocation to run `tool` against `stack`, or `None`
/// when the tool does not apply to the given stack (e.g. a cargo-* tool against C#).
pub fn audit_command(tool: AuditTool, stack: Stack) -> Option<AuditInvocation> {
    // cargo-* tools scan Rust; DotnetVulnerable scans C#. Match the stack
    // applicability positively so Stack::None (and any future variant) yields
    // None rather than being swept in by a `!= Csharp`-style negative test.
    let applies_to_rust = matches!(stack, Stack::Rust | Stack::Both);
    let applies_to_csharp = matches!(stack, Stack::Csharp | Stack::Both);

    let (program, args): (&str, &[&str]) = match tool {
        AuditTool::ClippyDeadCode => (
            "cargo",
            &["clippy", "--message-format=json", "--", "-W", "dead_code"],
        ),
        AuditTool::CargoAudit => ("cargo", &["audit", "--json"]),
        AuditTool::CargoDeny => ("cargo", &["deny", "check", "advisories"]),
        AuditTool::CargoGeiger => ("cargo", &["geiger", "--output-format", "Json"]),
        AuditTool::CargoUdeps => ("cargo", &["+nightly", "udeps", "--output", "json"]),
        AuditTool::DotnetVulnerable => ("dotnet", &["list", "package", "--vulnerable"]),
    };

    let applicable = match tool {
        AuditTool::DotnetVulnerable => applies_to_csharp,
        _ => applies_to_rust,
    };

    if !applicable {
        return None;
    }

    Some(AuditInvocation {
        program: program.to_string(),
        args: args.iter().map(|a| a.to_string()).collect(),
    })
}

/// Parses one-JSON-object-per-line output from `cargo clippy --message-format=json`
/// and returns a `Finding` for every compiler-message whose `message.code.code` is
/// `"dead_code"`. Lines that are not valid JSON, not compiler-messages, or whose
/// code is anything other than `dead_code` are silently skipped.
pub fn parse_clippy_dead_code(json_lines: &str) -> Vec<Finding> {
    let mut findings = Vec::new();

    for line in json_lines.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Lines that are not valid JSON are silently skipped.
        let value: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Keep only compiler-messages whose diagnostic code is dead_code.
        // serde navigation is null-safe: missing/null fields yield None, no panic.
        if value["reason"].as_str() != Some("compiler-message") {
            continue;
        }
        let message = &value["message"];
        if message["code"]["code"].as_str() != Some("dead_code") {
            continue;
        }

        // Prefer the primary span; fall back to the first span.
        let span = match message["spans"].as_array() {
            Some(spans) if !spans.is_empty() => spans
                .iter()
                .find(|s| s["is_primary"].as_bool() == Some(true))
                .unwrap_or(&spans[0]),
            _ => continue,
        };

        let file = span["file_name"].as_str().unwrap_or_default().to_string();
        let line_start = span["line_start"].as_u64().unwrap_or(0) as u32;
        let message_text = message["message"].as_str().unwrap_or_default().to_string();

        findings.push(Finding {
            lens: "clippy-dead-code".to_string(),
            source: "mechanical".to_string(),
            severity: "low".to_string(),
            title: format!("dead code: {}:{}", file, line_start),
            summary: message_text.clone(),
            suspect_files: vec![file.clone()],
            file: Some(file),
            line: Some(line_start),
            evidence: Some(message_text),
        });
    }

    findings
}

/// Returns the canonical kebab-case wire name for `tool` — the form used in the
/// schema, LLM lens names, and the audit workflow's corroboration/join logic.
/// e.g. `AuditTool::ClippyDeadCode` → `"clippy-dead-code"`.
pub fn tool_wire_name(tool: &AuditTool) -> &'static str {
    match tool {
        AuditTool::ClippyDeadCode => "clippy-dead-code",
        AuditTool::CargoAudit => "cargo-audit",
        AuditTool::CargoDeny => "cargo-deny",
        AuditTool::CargoGeiger => "cargo-geiger",
        AuditTool::CargoUdeps => "cargo-udeps",
        AuditTool::DotnetVulnerable => "dotnet-vulnerable",
    }
}

/// CLI entry-point: runs `audit_command`, executes the tool, parses output, and writes
/// results as JSON to stdout.
pub fn run(args: Args) -> anyhow::Result<()> {
    use crate::common::subprocess::run_with_timeout;
    use std::time::Duration;

    // `AuditTool` is Clone but not Copy; capture what we need by reference
    // before moving `args.tool` into `audit_command`.
    let is_clippy = args.tool == AuditTool::ClippyDeadCode;
    let tool_label = tool_wire_name(&args.tool).to_string();

    let result = match audit_command(args.tool, args.stack) {
        // Tool does not apply to this stack: nothing was scanned.
        None => AuditRunResult {
            tool: tool_label,
            available: false,
            nothing_scanned: true,
            findings: vec![],
        },
        Some(inv) => {
            let arg_refs: Vec<&str> = inv.args.iter().map(String::as_str).collect();
            match run_with_timeout(
                &inv.program,
                &arg_refs,
                &args.repo_root,
                Duration::from_secs(300),
            ) {
                // Spawn failure (e.g. tool not installed): treat as unavailable.
                Err(_) => AuditRunResult {
                    tool: tool_label,
                    available: false,
                    nothing_scanned: true,
                    findings: vec![],
                },
                Ok(run_result) => {
                    let output = run_result.combined_output;
                    let (findings, nothing_scanned) = if is_clippy {
                        let findings = parse_clippy_dead_code(&output);
                        // nothing_scanned is orthogonal to findings being empty: it
                        // reflects whether the run produced any analyzable compiler
                        // messages at all, not whether any were dead_code.
                        let scanned_anything = output.lines().any(|line| {
                            let trimmed = line.trim();
                            if trimmed.is_empty() {
                                return false;
                            }
                            serde_json::from_str::<serde_json::Value>(trimmed)
                                .map(|v| v["reason"].as_str() == Some("compiler-message"))
                                .unwrap_or(false)
                        });
                        (findings, !scanned_anything)
                    } else {
                        // No dedicated parser yet for the other tools; the run still
                        // scanned if it produced any output.
                        (vec![], output.trim().is_empty())
                    };
                    AuditRunResult {
                        tool: tool_label,
                        available: true,
                        nothing_scanned,
                        findings,
                    }
                }
            }
        }
    };

    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: a realistic cargo --message-format=json line for a dead_code warning.
    fn dead_code_json_line(file_name: &str, line_start: u32) -> String {
        serde_json::json!({
            "reason": "compiler-message",
            "message": {
                "code": { "code": "dead_code", "explanation": null },
                "level": "warning",
                "message": "function `unused_fn` is never used",
                "spans": [
                    {
                        "file_name": file_name,
                        "line_start": line_start,
                        "line_end": line_start,
                        "column_start": 1,
                        "column_end": 2,
                        "is_primary": true,
                        "label": null
                    }
                ],
                "children": []
            },
            "package_id": "straightjacket 0.1.0 (path+file:///repo)"
        })
        .to_string()
    }

    // Helper: a cargo --message-format=json line for a non-dead_code warning.
    fn unused_variables_json_line() -> String {
        serde_json::json!({
            "reason": "compiler-message",
            "message": {
                "code": { "code": "unused_variables", "explanation": null },
                "level": "warning",
                "message": "unused variable: `x`",
                "spans": [
                    {
                        "file_name": "src/main.rs",
                        "line_start": 5,
                        "line_end": 5,
                        "column_start": 9,
                        "column_end": 10,
                        "is_primary": true,
                        "label": null
                    }
                ],
                "children": []
            },
            "package_id": "straightjacket 0.1.0 (path+file:///repo)"
        })
        .to_string()
    }

    // ─── audit_command: stack-applicability contracts ────────────────────────

    #[test]
    fn cargo_audit_does_not_apply_to_csharp_stack() {
        // A cargo-* tool must return None for a C# stack — it cannot scan C# projects.
        let result = audit_command(AuditTool::CargoAudit, Stack::Csharp);
        assert!(
            result.is_none(),
            "expected None for CargoAudit on Csharp stack, got: {:?}",
            result
        );
    }

    #[test]
    fn dotnet_vulnerable_does_not_apply_to_rust_stack() {
        // A dotnet-* tool must return None for a Rust stack.
        let result = audit_command(AuditTool::DotnetVulnerable, Stack::Rust);
        assert!(
            result.is_none(),
            "expected None for DotnetVulnerable on Rust stack, got: {:?}",
            result
        );
    }

    #[test]
    fn clippy_dead_code_applies_to_rust_stack_with_cargo_and_clippy_args() {
        // ClippyDeadCode on Rust must return Some with program=="cargo" and args
        // containing both "clippy" and "--message-format=json".
        let result = audit_command(AuditTool::ClippyDeadCode, Stack::Rust);
        let inv = result.expect("expected Some(AuditInvocation) for ClippyDeadCode on Rust stack");
        assert_eq!(
            inv.program, "cargo",
            "program must be \"cargo\", got: {:?}",
            inv.program
        );
        assert!(
            inv.args.iter().any(|a| a == "clippy"),
            "args must contain \"clippy\", got: {:?}",
            inv.args
        );
        assert!(
            inv.args.iter().any(|a| a == "--message-format=json"),
            "args must contain \"--message-format=json\", got: {:?}",
            inv.args
        );
    }

    // ─── parse_clippy_dead_code contracts ────────────────────────────────────

    #[test]
    fn parse_clippy_dead_code_extracts_finding_from_dead_code_message() {
        // A compiler-message JSON line with code=="dead_code" and a primary span
        // must produce exactly one Finding with the correct lens, source, file, and line.
        let line = dead_code_json_line("src/foo.rs", 12);
        let findings = parse_clippy_dead_code(&line);
        assert_eq!(
            findings.len(),
            1,
            "expected exactly one Finding from dead_code message, got: {:?}",
            findings.len()
        );
        let f = &findings[0];
        assert_eq!(f.lens, "clippy-dead-code", "lens must be \"clippy-dead-code\"");
        assert_eq!(f.source, "mechanical", "source must be \"mechanical\"");
        assert_eq!(
            f.file.as_deref(),
            Some("src/foo.rs"),
            "file must match span's file_name"
        );
        assert_eq!(
            f.line,
            Some(12),
            "line must match span's line_start"
        );
    }

    #[test]
    fn parse_clippy_dead_code_ignores_non_dead_code_warnings() {
        // A compiler-message with code != "dead_code" must be silently skipped.
        let line = unused_variables_json_line();
        let findings = parse_clippy_dead_code(&line);
        assert!(
            findings.is_empty(),
            "expected empty Vec for unused_variables code, got: {:?} findings",
            findings.len()
        );
    }

    #[test]
    fn parse_clippy_dead_code_returns_empty_vec_for_empty_input() {
        let findings = parse_clippy_dead_code("");
        assert!(
            findings.is_empty(),
            "expected empty Vec for empty input, got: {:?} findings",
            findings.len()
        );
    }

    // ─── tool_wire_name: kebab-case wire-name contract ───────────────────────

    #[test]
    fn tool_wire_name_is_kebab_case_for_every_variant() {
        // The wire name must be the kebab-case form used by the schema, LLM lens
        // names, and the audit workflow's corroboration/join logic — NOT the
        // camel-case Debug representation (e.g. "ClippyDeadCode").
        assert_eq!(tool_wire_name(&AuditTool::ClippyDeadCode), "clippy-dead-code");
        assert_eq!(tool_wire_name(&AuditTool::CargoAudit), "cargo-audit");
        assert_eq!(tool_wire_name(&AuditTool::CargoDeny), "cargo-deny");
        assert_eq!(tool_wire_name(&AuditTool::CargoGeiger), "cargo-geiger");
        assert_eq!(tool_wire_name(&AuditTool::CargoUdeps), "cargo-udeps");
        assert_eq!(tool_wire_name(&AuditTool::DotnetVulnerable), "dotnet-vulnerable");
    }
}
