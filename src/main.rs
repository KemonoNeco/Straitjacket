use clap::{Parser, Subcommand};
use straightjacket::commands;

#[derive(Parser)]
#[command(
    name = "straightjacket",
    version,
    about = "Multi-agent test workflow CLI for the straightjacket Claude Code plugin"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(name = "audit-run")]
    AuditRun(commands::audit_run::Args),
    #[command(name = "bug-status")]
    BugStatus(commands::bug_status::Args),
    #[command(name = "detect-stack")]
    DetectStack(commands::detect_stack::Args),
    #[command(name = "baseline-check")]
    BaselineCheck(commands::baseline_check::Args),
    #[command(name = "lint-check")]
    LintCheck(commands::lint_check::Args),
    #[command(name = "snapshot-tests")]
    SnapshotTests(commands::snapshot_tests::Args),
    #[command(name = "verify-no-test-mutation")]
    VerifyNoTestMutation(commands::verify_no_test_mutation::Args),
    #[command(name = "verify-new-tests-compile")]
    VerifyNewTestsCompile(commands::verify_new_tests_compile::Args),
    #[command(name = "verify-tree-clean")]
    VerifyTreeClean(commands::verify_tree_clean::Args),
    #[command(name = "fuzz-setup")]
    FuzzSetup(commands::fuzz_setup::Args),
    #[command(name = "reproducer-to-test")]
    ReproducerToTest(commands::reproducer_to_test::Args),
    #[command(name = "run-new-tests")]
    RunNewTests(commands::run_new_tests::Args),
    #[command(name = "preflight")]
    Preflight(commands::preflight::Args),
    #[command(name = "hook")]
    Hook(commands::hook::Args),
    #[command(name = "workflow-script")]
    WorkflowScript(commands::workflow_script::Args),
    #[command(name = "validate-work-units")]
    ValidateWorkUnits(commands::validate_work_units::Args),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::AuditRun(a) => commands::audit_run::run(a),
        Commands::BugStatus(a) => commands::bug_status::run(a),
        Commands::DetectStack(a) => commands::detect_stack::run(a),
        Commands::BaselineCheck(a) => commands::baseline_check::run(a),
        Commands::LintCheck(a) => commands::lint_check::run(a),
        Commands::SnapshotTests(a) => commands::snapshot_tests::run(a),
        Commands::VerifyNoTestMutation(a) => commands::verify_no_test_mutation::run(a),
        Commands::VerifyNewTestsCompile(a) => commands::verify_new_tests_compile::run(a),
        Commands::VerifyTreeClean(a) => commands::verify_tree_clean::run(a),
        Commands::FuzzSetup(a) => commands::fuzz_setup::run(a),
        Commands::ReproducerToTest(a) => commands::reproducer_to_test::run(a),
        Commands::RunNewTests(a) => commands::run_new_tests::run(a),
        Commands::Preflight(a) => commands::preflight::run(a),
        Commands::Hook(a) => commands::hook::run(a),
        Commands::WorkflowScript(a) => commands::workflow_script::run(a),
        Commands::ValidateWorkUnits(a) => commands::validate_work_units::run(a),
    }
}
