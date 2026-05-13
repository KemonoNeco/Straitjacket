use crate::common::Stack;
use std::path::PathBuf;

#[derive(clap::Args, Debug)]
pub struct Args {
    #[arg(long)]
    pub repo_root: PathBuf,
    #[arg(long, value_enum)]
    pub stack: Stack,
}

pub fn run(_args: Args) -> anyhow::Result<()> {
    unimplemented!("fuzz-setup — TDD work unit pending implementation")
}
