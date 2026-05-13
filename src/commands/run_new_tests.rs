use crate::common::Stack;
use std::path::PathBuf;

#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Expect {
    Pass,
    Fail,
}

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
    #[arg(long, default_value_t = 3)]
    pub runs: u32,
    #[arg(long, value_enum, default_value_t = Expect::Pass)]
    pub expect: Expect,
}

pub fn run(_args: Args) -> anyhow::Result<()> {
    unimplemented!("run-new-tests — TDD work unit pending implementation")
}
