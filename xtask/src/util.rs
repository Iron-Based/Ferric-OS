use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub fn repo_root() -> PathBuf {
    // The xtask lives at the monorepo root. Derive it from CARGO_MANIFEST_DIR
    // so it resolves the same from any cwd.
    let xtask_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = xtask_dir
        .parent()
        .expect("xtask must live one level under the monorepo root?");
    if root.join("ferric-k/crates/ferric-kernel").is_dir() {
        return root.to_path_buf();
    }
    panic!(
        "could not locate the monorepo root at {} (expected ferric-k/crates/ferric-kernel)",
        root.display()
    );
}

/// Returns an error mentioning the executable when it cannot be spawned, which
/// covers the "not on PATH" case (NotFound).
pub fn find(program: &str) -> Result<PathBuf, String> {
    let probe = if cfg!(windows) {
        program.to_string() + ".exe"
    } else {
        program.to_string()
    };
    if let Ok(paths) = env::var("PATH") {
        for dir in env::split_paths(&paths) {
            let cand = dir.join(&probe);
            if cand.is_file() {
                return Ok(cand);
            }
        }
    }
    Err(format!(
        "'{}' not found on PATH. Run: cargo xtask bootstrap",
        program
    ))
}

pub fn run(program: &str, args: &[&str]) -> std::io::Result<Output> {
    Command::new(program).args(args).output()
}

/// Like `run`, but executes with `dir` as the working directory. Cargo
/// subcommands must run from the monorepo root so the workspace,
/// rust-toolchain.toml, and per-target cfg pick up. `RUSTUP_TOOLCHAIN` is
/// stripped so rustup re-resolves the pin from `dir`.
pub fn run_in(dir: &Path, program: &str, args: &[&str]) -> std::io::Result<Output> {
    Command::new(program)
        .args(args)
        .current_dir(dir)
        .env_remove("RUSTUP_TOOLCHAIN")
        .output()
}

pub fn checked(program: &str, args: &[&str], what: &str) -> Result<(), String> {
    let out = run(program, args).map_err(|e| format!("failed to run {program}: {e}"))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        let tail = if msg.trim().is_empty() {
            String::from_utf8_lossy(&out.stdout).to_string()
        } else {
            msg.to_string()
        };
        return Err(format!("{what} failed ({program})\n{}", tail.trim_end()));
    }
    Ok(())
}

/// Like `checked`, but executes with `dir` as the working directory.
pub fn checked_in(dir: &Path, program: &str, args: &[&str], what: &str) -> Result<(), String> {
    let out = run_in(dir, program, args).map_err(|e| format!("failed to run {program}: {e}"))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        let tail = if msg.trim().is_empty() {
            String::from_utf8_lossy(&out.stdout).to_string()
        } else {
            msg.to_string()
        };
        return Err(format!("{what} failed ({program})\n{}", tail.trim_end()));
    }
    Ok(())
}
