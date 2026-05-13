use std::path::PathBuf;

#[derive(clap::Args, Debug)]
pub struct Args {
    #[arg(long)]
    pub repo_root: PathBuf,
    #[arg(long)]
    pub out_file: PathBuf,
}

pub fn run(_args: Args) -> anyhow::Result<()> {
    unimplemented!("snapshot-tests — TDD work unit pending implementation")
}
