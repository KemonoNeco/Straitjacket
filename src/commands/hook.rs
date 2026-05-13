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

pub fn run(_args: Args) -> anyhow::Result<()> {
    unimplemented!("hook — TDD work unit pending implementation")
}
