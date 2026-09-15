use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub fn repo_root() -> PathBuf {
    // The xtask lives at the monorepo root; the Ferric-K workspace is the
    // sibling directory that owns the kernel crates and target specs. Derive
    // it from CARGO_MANIFEST_DIR so it resolves the same from any cwd.
    let xtask_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let monorepo_root = xtask_dir
        .parent()
        .expect("xtask must live one level under the monorepo root?");
    let ferric_k = monorepo_root.join("ferric-k");
    if ferric_k.join("Cargo.toml").is_file() && ferric_k.join("crates/ferric-kernel").is_dir() {
        return ferric_k;
    }
    panic!(
        "could not locate the Ferric-K workspace at {} (expected ferric-k/crates/ferric-kernel)",
        ferric_k.display()
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
/// subcommands must run from the Ferric-K workspace so their workspace,
/// rust-toolchain.toml, and per-target cfg pick up. `RUSTUP_TOOLCHAIN` is
/// stripped so rustup re-resolves the pin from `dir` instead of inheriting
/// whatever channel the outer `cargo xtask` invocation used (e.g. stable at
/// the monorepo root).
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
