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
        "fanout" => Some(include_str!("../../workflows/fanout.js")),
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
                "unknown workflow stage {:?}; known stages are: adversarial, fanout",
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
        for stage in &["adversarial", "fanout"] {
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
}
