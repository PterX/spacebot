//! `spacebot dashboard` — open the web UI, starting the daemon if needed.

use anyhow::Context as _;
use clap::Args;

#[derive(Args)]
pub struct DashboardArgs {
    /// Print the dashboard URL without opening a browser
    #[arg(long)]
    no_open: bool,
}

pub async fn run(ctx: &super::Context, args: DashboardArgs) -> anyhow::Result<()> {
    let config = super::load_config(&ctx.config_path)?;
    let paths = spacebot::daemon::DaemonPaths::new(&config.instance_dir);

    if spacebot::daemon::is_running(&paths).is_none() {
        eprintln!("spacebot is not running — starting it...");
        let exe = std::env::current_exe().context("failed to resolve the spacebot executable")?;
        let mut command = std::process::Command::new(exe);
        if let Some(path) = &ctx.config_path {
            command.arg("--config").arg(path);
        }
        command.arg("start");
        let status = command.status().context("failed to run `spacebot start`")?;
        if !status.success() {
            anyhow::bail!("`spacebot start` failed");
        }
    }

    let url = format!(
        "http://{}:{}",
        display_host(&config.api.bind),
        config.api.port
    );

    if !wait_for_api(&config.api.bind, config.api.port).await {
        anyhow::bail!("the daemon started but the API at {url} did not come up within 15s");
    }

    println!("{url}");
    if !args.no_open
        && let Err(error) = open::that(&url)
    {
        eprintln!("failed to open browser: {error}");
    }
    Ok(())
}

/// Wildcard binds aren't connectable addresses — browsers need localhost.
/// IPv6 literals need brackets to form a valid URL host.
fn display_host(bind: &str) -> String {
    match bind {
        "0.0.0.0" | "::" | "[::]" => "localhost".to_string(),
        host if host.contains(':') && !host.starts_with('[') => format!("[{host}]"),
        host => host.to_string(),
    }
}

async fn wait_for_api(bind: &str, port: u16) -> bool {
    // Bare address for the socket connect — brackets are URL syntax only.
    let host = match bind {
        "0.0.0.0" | "::" | "[::]" => "localhost",
        host => host.trim_start_matches('[').trim_end_matches(']'),
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if tokio::net::TcpStream::connect((host, port)).await.is_ok() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
}
