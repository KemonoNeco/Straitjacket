#[derive(clap::Args, Debug)]
pub struct Args {
    /// Which workflow stage script to emit (e.g. "adversarial", "fanout").
    pub stage: String,
}

/// Returns the embedded workflow stage script for `stage`, or `None` if the stage is unknown.
/// The scripts are compiled in via `include_str!` of `workflows/<stage>.js`.
pub fn workflow_script(stage: &str) -> Option<&'static str> {
    match stage {
        "adversarial" => Some(include_str!("../../workflows/adversarial.js")),
        "audit" => Some(include_str!("../../workflows/audit.js")),
        "fanout" => Some(include_str!("../../workflows/fanout.js")),
        "tdd-cycle" => Some(include_str!("../../workflows/tdd-cycle.js")),
        _ => None,
    }
}

pub fn run(args: Args) -> anyhow::Result<()> {
    match workflow_script(&args.stage) {
        Some(s) => {
            print!("{s}");
            Ok(())
        }
        None => {
            eprintln!(
                "unknown workflow stage {:?}; known stages are: adversarial, audit, fanout, tdd-cycle",
                args.stage
            );
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workflow_script_adversarial_is_some_with_meta() {
        let result = workflow_script("adversarial");
        assert!(result.is_some());
        let s = result.unwrap();
        assert!(s.contains("export const meta"));
        assert!(s.contains("adversarial"));
    }

    #[test]
    fn test_workflow_script_fanout_is_some_with_meta() {
        let result = workflow_script("fanout");
        assert!(result.is_some());
        let s = result.unwrap();
        assert!(s.contains("export const meta"));
    }

    #[test]
    fn test_workflow_script_unknown_stage_is_none() {
        let result = workflow_script("bogus");
        assert!(result.is_none());
    }

    #[test]
    fn test_workflow_script_known_stages_nonempty_and_have_meta() {
        for stage in &["adversarial", "audit", "fanout", "tdd-cycle"] {
            let result = workflow_script(stage);
            assert!(result.is_some(), "stage {stage:?} should be Some");
            let s = result.unwrap();
            assert!(!s.is_empty(), "stage {stage:?} content should be non-empty");
            assert!(
                s.contains("export const meta"),
                "stage {stage:?} content should contain 'export const meta'"
            );
        }
    }

    /// Embed-freshness gate (issue #49, mechanism 2): the COMMITTED `bin/straitjacket-<triple>` binary must
    /// embed the CURRENT on-disk `workflows/*.js`. The scripts are `include_str!`'d at build time, so a
    /// source-level hardening fix does NOT ship until the binary is rebuilt + re-committed — and nothing
    /// else gated that the committed binary was fresh (empirically: at commit 61145a5 the committed binary
    /// emitted stale pre-fix JS for two commits). This execs the COMMITTED host-triple binary and asserts
    /// its `workflow-script <stage>` output matches what THIS build embeds, for every stage. It goes RED
    /// exactly when the committed binary is stale relative to the source.
    ///
    /// It must EXEC THE COMMITTED ARTIFACT — comparing `workflow_script(stage)` (the `include_str!`
    /// constant) to the on-disk file would be tautological (both come from this build) and could never
    /// catch a stale *committed* binary. EOL is normalized (`\r\n`→`\n`) on both sides so a CRLF/LF
    /// checkout difference between the binary's build machine and the test's checkout reads as a content
    /// match, not a spurious "stale" signal — `workflows/*.js` is not `.gitattributes`-EOL-locked.
    ///
    /// Skips gracefully when the host-triple binary is not committed (only some triples ship; CI runs the
    /// suite on Windows, where `straitjacket-x86_64-pc-windows-msvc.exe` IS committed, so the gate fires there).
    #[test]
    fn committed_binary_embeds_current_workflow_scripts() {
        let arch = std::env::consts::ARCH; // "x86_64" | "aarch64"
        let (suffix, ext) = if cfg!(target_os = "windows") {
            ("pc-windows-msvc", ".exe")
        } else if cfg!(target_os = "macos") {
            ("apple-darwin", "")
        } else {
            ("unknown-linux-gnu", "")
        };
        let triple = format!("{arch}-{suffix}");
        let bin = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("bin")
            .join(format!("straitjacket-{triple}{ext}"));
        if !bin.exists() {
            eprintln!(
                "SKIP committed_binary_embeds_current_workflow_scripts: no committed binary for host triple \
                 {triple} at {} (only some triples are committed; CI gates this on Windows)",
                bin.display()
            );
            return;
        }
        let normalize = |s: &str| s.replace("\r\n", "\n");
        for stage in &["adversarial", "audit", "fanout", "tdd-cycle"] {
            let out = std::process::Command::new(&bin)
                .args(["workflow-script", stage])
                .output()
                .unwrap_or_else(|e| panic!("failed to exec committed binary {}: {e}", bin.display()));
            assert!(
                out.status.success(),
                "committed binary `workflow-script {stage}` exited non-zero: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let emitted = String::from_utf8_lossy(&out.stdout);
            let on_disk = workflow_script(stage).expect("known stage is Some");
            assert_eq!(
                normalize(&emitted),
                normalize(on_disk),
                "STALE COMMITTED BINARY: bin/straitjacket-{triple}{ext} does not embed the current \
                 workflows/{stage}.js — rebuild the binary (scripts\\cargo-msvc.cmd build --release) and \
                 re-commit bin/. workflows/*.js are include_str!'d at build time, so a source edit does not \
                 ship until the binary is rebuilt."
            );
        }
    }
}
