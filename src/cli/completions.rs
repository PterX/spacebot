//! `spacebot completions` — shell completion scripts.

use clap::CommandFactory as _;

pub fn run(shell: clap_complete::Shell) -> anyhow::Result<()> {
    let mut command = super::Cli::command();
    clap_complete::generate(shell, &mut command, "spacebot", &mut std::io::stdout());
    Ok(())
}
