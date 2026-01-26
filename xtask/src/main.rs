//! Workspace automation tasks.

use std::process::{Command, ExitCode, Stdio};

use clap::{Parser, Subcommand};

/// Workspace automation tasks.
#[derive(Parser)]
#[command(name = "xtask")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Format code and run clippy with auto-fix.
    Tidy,
    /// Run all tests using nextest.
    Test,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Commands::Tidy => tidy(),
        Commands::Test => test(),
    }
}

fn tidy() -> ExitCode {
    println!("Formatting code...");
    let fmt_status = Command::new("cargo")
        .args([
            "+nightly",
            "fmt",
            "--all",
            "--",
            "--config-path",
            "./rustfmt-nightly.toml",
        ])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();

    match fmt_status {
        Ok(status) if !status.success() => {
            eprintln!("cargo fmt failed");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("Failed to run cargo fmt: {e}");
            return ExitCode::FAILURE;
        }
        _ => {}
    }

    println!("\nRunning clippy with auto-fix...");
    let clippy_status = Command::new("cargo")
        .args([
            "clippy",
            "-q",
            "--fix",
            "--all",
            "--all-targets",
            "--all-features",
            "--allow-dirty",
            "--tests",
            "--examples",
        ])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();

    match clippy_status {
        Ok(status) if status.success() => {
            println!("\nTidy complete!");
            ExitCode::SUCCESS
        }
        Ok(_) => {
            eprintln!("clippy found issues");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("Failed to run clippy: {e}");
            ExitCode::FAILURE
        }
    }
}

fn test() -> ExitCode {
    println!("Running tests with nextest...");
    let status = Command::new("cargo")
        .args(["nextest", "run", "--all"])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();

    match status {
        Ok(status) if status.success() => {
            println!("\nAll tests passed!");
            ExitCode::SUCCESS
        }
        Ok(_) => {
            eprintln!("Tests failed");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("Failed to run nextest: {e}");
            ExitCode::FAILURE
        }
    }
}
