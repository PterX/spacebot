use std::process::Command;

fn main() {
    emit_version();
    if std::env::var("SPACEBOT_SKIP_FRONTEND_BUILD").is_ok() {
        return;
    }
    // Re-run if interface source files change
    println!("cargo:rerun-if-changed=interface/src/");
    println!("cargo:rerun-if-changed=interface/index.html");
    println!("cargo:rerun-if-changed=interface/package.json");
    println!("cargo:rerun-if-changed=interface/vite.config.ts");
    println!("cargo:rerun-if-changed=interface/tailwind.config.ts");

    let interface_dir = std::path::Path::new("interface");

    // Skip if bun isn't installed or node_modules is missing (CI without frontend deps)
    if !interface_dir.join("node_modules").exists() {
        eprintln!(
            "cargo:warning=interface/node_modules not found, skipping frontend build. Run `bun install` in interface/"
        );
        ensure_dist_dir();
        return;
    }

    let status = Command::new("bun")
        .args(["run", "build"])
        .current_dir(interface_dir)
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!(
                "cargo:warning=frontend build exited with {s}, the binary will serve a stale or empty UI"
            );
        }
        Err(e) => {
            eprintln!(
                "cargo:warning=failed to run `bun run build`: {e}. Install bun to build the frontend."
            );
            ensure_dist_dir();
        }
    }
}

/// Emit `SPACEBOT_VERSION` for `--version` and the status API: the plain
/// crate version for release builds (HEAD exactly on the matching
/// `v{version}` tag, or no git repo at all), otherwise
/// `{version} ({branch}@{commit})`.
fn emit_version() {
    let pkg = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let full = dev_version(&pkg).unwrap_or(pkg);
    println!("cargo:rustc-env=SPACEBOT_VERSION={full}");
}

fn dev_version(pkg: &str) -> Option<String> {
    let commit = git(&["rev-parse", "--short=8", "HEAD"])?;

    // Rebuild when HEAD moves: branch switches rewrite HEAD itself, new
    // commits rewrite the branch's loose ref (or packed-refs after a gc).
    // Only existing paths are emitted — a missing rerun path makes cargo
    // re-run the script on every build.
    let mut rerun_paths = vec!["HEAD".to_string(), "packed-refs".to_string()];
    if let Some(head_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
        rerun_paths.push(head_ref);
    }
    for path in rerun_paths {
        if let Some(resolved) = git(&["rev-parse", "--git-path", &path])
            && std::path::Path::new(&resolved).exists()
        {
            println!("cargo:rerun-if-changed={resolved}");
        }
    }

    let release_tag = format!("v{pkg}");
    if git(&["describe", "--tags", "--exact-match", "HEAD"]).is_some_and(|tag| tag == release_tag) {
        return None;
    }

    let detail = match git(&["rev-parse", "--abbrev-ref", "HEAD"]).filter(|b| b != "HEAD") {
        Some(branch) => format!("{branch}@{commit}"),
        None => commit,
    };
    Some(format!("{pkg} ({detail})"))
}

/// Run a git command, returning trimmed stdout on success (None on failure
/// or empty output).
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// rust-embed requires the folder to exist even if empty.
fn ensure_dist_dir() {
    let dist = std::path::Path::new("interface/dist");
    if !dist.exists() {
        std::fs::create_dir_all(dist).ok();
    }
}
