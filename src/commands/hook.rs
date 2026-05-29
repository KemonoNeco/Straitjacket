use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::io::Read;

#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum HookEvent {
    Preflight,
    PreAdversarial,
    PostAgent,
}

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Hook event entry point. Reads JSON payload from stdin.
    #[arg(value_enum)]
    pub event: HookEvent,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum HookDecision {
    /// Allow the tool call / continue. Emits no blocking output.
    Allow,
    /// Block the tool call with the given reason.
    Deny(String),
    /// Post-tool: validation checks should be run by the host orchestrator (since
    /// only the orchestrator knows the work-units path). The hook surfaces which
    /// checks are appropriate for the subagent_type.
    RunChecks(Vec<CheckKind>),
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckKind {
    VerifyNoTestMutation,
    VerifyNewTestsCompile,
    RunNewTests,
    Preflight,
}

/// Forbidden substrings that must not leak into an adversarial agent's prompt.
/// The agent's tool restriction (no Bash/PowerShell) already prevents it from
/// running `git diff` itself; this scan is defense-in-depth on prompt construction.
pub const FORBIDDEN_ADVERSARIAL_STRINGS: &[&str] = &["--- a/", "+++ b/", "git diff"];

/// Decides what to do with a PreToolUse on the Agent tool when the spawned
/// agent's subagent_type is one of the adversarial specialists.
///
/// Non-adversarial subagent_types are not this function's concern; the caller
/// should short-circuit Allow before invoking this.
pub fn decide_pre_adversarial(prompt: &str) -> HookDecision {
    for forbidden in FORBIDDEN_ADVERSARIAL_STRINGS {
        if prompt.contains(forbidden) {
            return HookDecision::Deny(format!(
                "adversarial-specialist prompt contains forbidden string: {:?}. The adversarial \
                 agents must operate in isolation from the diff — strip and rebuild the prompt.",
                forbidden
            ));
        }
    }
    HookDecision::Allow
}

/// Decides what post-tool checks to run after an Agent of the given subagent_type returns.
///
/// Note: `VerifyNoTestMutation` is intentionally NOT dispatched here. The snapshot
/// file is consumed by author prompts as documentation ("do not modify these files"),
/// and the orchestrator runs `verify-no-test-mutation` once at end-of-phase as an
/// audit. The adversarial-vacuousness and adversarial-misalignment specialists are
/// the primary defense against test-mutation cheats. Per-author SHA enforcement was
/// removed because it produced false positives on idiomatic Rust source files
/// (which embed `#[cfg(test)] mod tests`) without adding catch-power beyond the
/// adversarial trio.
pub fn decide_post_agent(subagent_type: &str) -> HookDecision {
    match subagent_type {
        "unit-test-author" | "integration-test-author" => {
            HookDecision::RunChecks(vec![CheckKind::VerifyNewTestsCompile])
        }
        "implementation-author" => HookDecision::RunChecks(vec![
            CheckKind::VerifyNewTestsCompile,
            CheckKind::RunNewTests,
        ]),
        _ => HookDecision::Allow,
    }
}

/// Returns true if the given slash-command name should trigger the green-baseline preflight
/// on UserPromptExpansion. Only skills that REQUIRE a green/buildable tree to be meaningful
/// are listed: `tdd` (red/green discipline), `mutation` + `fuzz` (need a building target),
/// and `debug` (operates "from a green state"). Deliberately EXCLUDED: `audit` (read-only
/// analysis — you often audit *because* the tree is unhealthy), `triage` (a router; the
/// `debug`/`tdd` skills it invokes carry their own gate), and `report-bug` (capture-fast).
/// `regression` was retired as a command.
pub fn is_plugin_skill_invocation(command_name: &str) -> bool {
    // The exact form Claude Code uses for plugin-scoped commands is verified at install time.
    // We accept both `plugin:skill` form and bare `skill` form for robustness.
    matches!(
        command_name,
        "straitjacket:tdd"
            | "straitjacket:mutation"
            | "straitjacket:fuzz"
            | "straitjacket:debug"
            | "tdd"
            | "mutation"
            | "fuzz"
            | "debug"
    )
}

/// Extracts `tool_input.prompt` from a hook stdin payload. Returns empty string
/// if absent (caller should treat absence as "no scan needed").
pub fn extract_prompt(payload: &serde_json::Value) -> &str {
    payload
        .get("tool_input")
        .and_then(|t| t.get("prompt"))
        .and_then(|p| p.as_str())
        .unwrap_or("")
}

/// Extracts `tool_input.subagent_type` from a hook stdin payload. Returns empty string if absent.
pub fn extract_subagent_type(payload: &serde_json::Value) -> &str {
    payload
        .get("tool_input")
        .and_then(|t| t.get("subagent_type"))
        .and_then(|s| s.as_str())
        .unwrap_or("")
}

/// Extracts `prompt.command_name` for UserPromptExpansion events. Returns empty
/// string if absent.
pub fn extract_command_name(payload: &serde_json::Value) -> &str {
    payload
        .get("prompt")
        .and_then(|p| p.get("command_name"))
        .and_then(|s| s.as_str())
        .or_else(|| payload.get("command_name").and_then(|s| s.as_str()))
        .unwrap_or("")
}

/// Renders a HookDecision into the JSON shape Claude Code expects for a given event.
pub fn render_decision(event: HookEvent, decision: &HookDecision) -> serde_json::Value {
    match (event, decision) {
        (HookEvent::PreAdversarial, HookDecision::Deny(reason)) => serde_json::json!({
            "hookSpecificOutput": {
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        }),
        (HookEvent::Preflight, HookDecision::Deny(reason)) => serde_json::json!({
            "decision": "block",
            "reason": reason,
        }),
        (HookEvent::PostAgent, HookDecision::Deny(reason)) => serde_json::json!({
            "decision": "block",
            "reason": reason,
        }),
        (_, HookDecision::Allow) => serde_json::json!({}),
        (_, HookDecision::RunChecks(checks)) => serde_json::json!({
            "checks_to_run": checks,
        }),
    }
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("read hook stdin payload")?;

    let payload: serde_json::Value = if buf.trim().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&buf).unwrap_or(serde_json::Value::Null)
    };

    let decision = match args.event {
        HookEvent::Preflight => {
            let cmd = extract_command_name(&payload);
            if !cmd.is_empty() && !is_plugin_skill_invocation(cmd) {
                HookDecision::Allow
            } else {
                // Caller (orchestrator script) handles the actual preflight check; we just
                // signal allow here since no static decision is possible without running
                // baseline_check / lint_check, which the SKILL.md Phase 1 already does.
                HookDecision::Allow
            }
        }
        HookEvent::PreAdversarial => {
            let st = extract_subagent_type(&payload);
            if st.starts_with("adversarial-") {
                decide_pre_adversarial(extract_prompt(&payload))
            } else {
                HookDecision::Allow
            }
        }
        HookEvent::PostAgent => {
            let st = extract_subagent_type(&payload);
            decide_post_agent(st)
        }
    };

    let response = render_decision(args.event, &decision);
    if !response.as_object().map(|m| m.is_empty()).unwrap_or(true) {
        println!("{}", serde_json::to_string(&response)?);
    }

    // Exit code semantics: 0 = allow / no-op; 2 = block with stderr message. We use 0 + JSON
    // here per the documented decision/permissionDecision pattern.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_adversarial_clean_prompt_allows() {
        let d = decide_pre_adversarial("Review the following tests for vacuousness...");
        assert_eq!(d, HookDecision::Allow);
    }

    #[test]
    fn pre_adversarial_diff_marker_denies() {
        let d = decide_pre_adversarial("--- a/foo.rs\n+++ b/foo.rs\n@@ ...");
        match d {
            HookDecision::Deny(reason) => {
                assert!(reason.contains("--- a/") || reason.contains("forbidden"));
            }
            other => panic!("expected Deny, got {:?}", other),
        }
    }

    #[test]
    fn pre_adversarial_git_diff_string_denies() {
        let d = decide_pre_adversarial("The orchestrator ran git diff before this...");
        assert!(matches!(d, HookDecision::Deny(_)));
    }

    #[test]
    fn post_agent_unit_test_author_runs_compile_only() {
        let d = decide_post_agent("unit-test-author");
        assert_eq!(
            d,
            HookDecision::RunChecks(vec![CheckKind::VerifyNewTestsCompile])
        );
    }

    #[test]
    fn post_agent_integration_test_author_runs_compile_only() {
        let d = decide_post_agent("integration-test-author");
        assert_eq!(
            d,
            HookDecision::RunChecks(vec![CheckKind::VerifyNewTestsCompile])
        );
    }

    #[test]
    fn post_agent_never_dispatches_verify_no_test_mutation() {
        // Regression guard: per-author test-mutation enforcement was removed in
        // favor of an end-of-phase audit + adversarial-trio coverage. If a future
        // change re-introduces it here, this test breaks loudly.
        for st in [
            "unit-test-author",
            "integration-test-author",
            "implementation-author",
        ] {
            let d = decide_post_agent(st);
            if let HookDecision::RunChecks(checks) = d {
                assert!(
                    !checks.contains(&CheckKind::VerifyNoTestMutation),
                    "{} should not dispatch VerifyNoTestMutation",
                    st
                );
            }
        }
    }

    #[test]
    fn post_agent_implementation_author_runs_compile_plus_run() {
        let d = decide_post_agent("implementation-author");
        assert_eq!(
            d,
            HookDecision::RunChecks(vec![
                CheckKind::VerifyNewTestsCompile,
                CheckKind::RunNewTests,
            ])
        );
    }

    #[test]
    fn post_agent_unknown_subagent_is_silent_noop() {
        assert_eq!(decide_post_agent("general-purpose"), HookDecision::Allow);
        assert_eq!(decide_post_agent(""), HookDecision::Allow);
        assert_eq!(decide_post_agent("Explore"), HookDecision::Allow);
    }

    #[test]
    fn plugin_skill_invocation_matcher_accepts_namespaced_and_bare() {
        for name in [
            "straitjacket:tdd",
            "straitjacket:mutation",
            "straitjacket:fuzz",
            "straitjacket:debug",
            "tdd",
            "mutation",
            "fuzz",
            "debug",
        ] {
            assert!(
                is_plugin_skill_invocation(name),
                "{name} should trigger the green-baseline preflight"
            );
        }
    }

    #[test]
    fn plugin_skill_invocation_matcher_rejects_unrelated_and_non_gated_skills() {
        assert!(!is_plugin_skill_invocation("review"));
        assert!(!is_plugin_skill_invocation(""));
        assert!(!is_plugin_skill_invocation("other-plugin:tdd"));
        // Retired skill must no longer trigger the preflight.
        assert!(!is_plugin_skill_invocation("straitjacket:regression"));
        // Read-only / router / capture skills deliberately skip the green-baseline gate.
        assert!(!is_plugin_skill_invocation("straitjacket:audit"));
        assert!(!is_plugin_skill_invocation("straitjacket:triage"));
        assert!(!is_plugin_skill_invocation("straitjacket:report-bug"));
    }

    #[test]
    fn extract_prompt_reads_tool_input_prompt() {
        let payload = serde_json::json!({ "tool_input": { "prompt": "hello" } });
        assert_eq!(extract_prompt(&payload), "hello");
    }

    #[test]
    fn extract_prompt_returns_empty_when_absent() {
        let payload = serde_json::json!({});
        assert_eq!(extract_prompt(&payload), "");
    }

    #[test]
    fn extract_subagent_type_reads_correct_field() {
        let payload = serde_json::json!({
            "tool_input": { "subagent_type": "adversarial-vacuousness" }
        });
        assert_eq!(extract_subagent_type(&payload), "adversarial-vacuousness");
    }

    #[test]
    fn render_deny_for_pre_adversarial_uses_permission_decision_shape() {
        let d = HookDecision::Deny("forbidden string".into());
        let rendered = render_decision(HookEvent::PreAdversarial, &d);
        let outer = rendered.get("hookSpecificOutput").unwrap();
        assert_eq!(outer.get("permissionDecision").unwrap(), "deny");
        assert!(outer.get("permissionDecisionReason").is_some());
    }

    #[test]
    fn render_deny_for_post_agent_uses_decision_block_shape() {
        let d = HookDecision::Deny("test mutation detected".into());
        let rendered = render_decision(HookEvent::PostAgent, &d);
        assert_eq!(rendered.get("decision").unwrap(), "block");
        assert!(rendered.get("reason").is_some());
    }

    #[test]
    fn render_allow_is_empty_object() {
        let rendered = render_decision(HookEvent::PreAdversarial, &HookDecision::Allow);
        assert_eq!(rendered, serde_json::json!({}));
    }

    #[test]
    fn render_run_checks_includes_kebab_case_names() {
        let d = HookDecision::RunChecks(vec![
            CheckKind::VerifyNoTestMutation,
            CheckKind::RunNewTests,
        ]);
        let rendered = render_decision(HookEvent::PostAgent, &d);
        let checks = rendered.get("checks_to_run").unwrap();
        let arr = checks.as_array().unwrap();
        assert_eq!(arr[0], "verify-no-test-mutation");
        assert_eq!(arr[1], "run-new-tests");
    }

    #[test]
    fn test_pre_adversarial_denies_forbidden_string_embedded_after_leading_context() {
        // Forbidden marker appears after non-empty leading context — not at position 0.
        // Exercises that the substring scan does not require the marker to be at the start.
        let prompt =
            "Please review these tests for vacuousness.\nHere is some background material.\n\ngit diff origin/main\n";
        let d = decide_pre_adversarial(prompt);
        match d {
            HookDecision::Deny(reason) => {
                assert!(
                    reason.contains("git diff"),
                    "deny reason should name the forbidden substring; got: {reason}"
                );
            }
            other => panic!("expected Deny, got {:?}", other),
        }
    }

    #[test]
    fn test_extract_command_name_falls_back_to_top_level_field() {
        // Top-level `command_name` field (no `prompt` wrapper) — exercises the or_else branch.
        let payload = serde_json::json!({ "command_name": "straitjacket:tdd" });
        assert_eq!(extract_command_name(&payload), "straitjacket:tdd");
    }

    #[test]
    fn test_render_deny_for_preflight_uses_decision_block_shape_not_permission_shape() {
        let d = HookDecision::Deny("preflight block reason".into());
        let rendered = render_decision(HookEvent::Preflight, &d);
        assert_eq!(
            rendered.get("decision").unwrap(),
            "block",
            "Preflight Deny must render decision=block"
        );
        assert!(
            rendered.get("reason").is_some(),
            "Preflight Deny must include a reason field"
        );
        assert!(
            rendered.get("hookSpecificOutput").is_none(),
            "Preflight Deny must NOT use the hookSpecificOutput/permissionDecision shape"
        );
    }
}
