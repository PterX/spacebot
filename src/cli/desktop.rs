//! `spacebot desktop` — launch the installed desktop app.
//!
//! Launches an installed build only; building the desktop app from source
//! is a development task and lives in the justfile.

#[cfg(target_os = "macos")]
pub async fn run(_ctx: &super::Context) -> anyhow::Result<()> {
    use anyhow::Context as _;

    let status = std::process::Command::new("open")
        .args(["-b", "sh.spacebot.desktop"])
        .status()
        .context("failed to run `open`")?;
    if !status.success() {
        anyhow::bail!(
            "Spacebot desktop is not installed — download it from https://spacebot.sh or build it with `just desktop-build`"
        );
    }
    eprintln!("Launched Spacebot desktop.");
    Ok(())
}

#[cfg(target_os = "linux")]
pub async fn run(_ctx: &super::Context) -> anyhow::Result<()> {
    // The Tauri bundle installs the binary under the product name.
    for candidate in ["spacebot-desktop", "Spacebot"] {
        if std::process::Command::new(candidate).spawn().is_ok() {
            eprintln!("Launched Spacebot desktop.");
            return Ok(());
        }
    }
    anyhow::bail!(
        "Spacebot desktop is not installed — download it from https://spacebot.sh or build it with `just desktop-build`"
    )
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub async fn run(_ctx: &super::Context) -> anyhow::Result<()> {
    anyhow::bail!("`spacebot desktop` is not supported on this platform yet")
}
