//! The `seaquel-cli` binary. Everything lives in the library (`lib.rs`).

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    seaquel_cli::run(seaquel_cli::Cli::parse())
}
