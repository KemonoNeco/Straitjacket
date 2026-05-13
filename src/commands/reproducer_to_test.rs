use crate::common::Stack;
use std::path::PathBuf;

#[derive(clap::Args, Debug)]
pub struct Args {
    #[arg(long)]
    pub repro_path: PathBuf,
    #[arg(long)]
    pub target_file: String,
    #[arg(long)]
    pub target_function: String,
    #[arg(long, value_enum)]
    pub stack: Stack,
    #[arg(long)]
    pub repo_root: PathBuf,
    #[arg(long)]
    pub work_units_file: PathBuf,
    #[arg(long)]
    pub output_test_file: Option<PathBuf>,
}

pub fn run(_args: Args) -> anyhow::Result<()> {
    unimplemented!("reproducer-to-test — TDD work unit pending implementation")
}
