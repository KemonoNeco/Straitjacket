use std::path::PathBuf;

#[derive(clap::Args, Debug)]
pub struct Args {
    #[arg(long)]
    pub repo_root: PathBuf,
    #[arg(long)]
    pub log_dir: PathBuf,
}

pub fn run(_args: Args) -> anyhow::Result<()> {
    unimplemented!("preflight — TDD work unit pending implementation")
}
