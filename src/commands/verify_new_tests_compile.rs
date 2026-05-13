use crate::common::Stack;
use std::path::PathBuf;

#[derive(clap::Args, Debug)]
pub struct Args {
    #[arg(long)]
    pub repo_root: PathBuf,
    #[arg(long)]
    pub work_units_file: PathBuf,
    #[arg(long, value_enum)]
    pub stack: Stack,
    #[arg(long)]
    pub log_dir: PathBuf,
}

pub fn run(_args: Args) -> anyhow::Result<()> {
    unimplemented!("verify-new-tests-compile — TDD work unit pending implementation")
}
