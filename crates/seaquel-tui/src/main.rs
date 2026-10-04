//! The `seaquel-tui` binary. Everything lives in the library (`lib.rs`).

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    seaquel_tui::run(seaquel_tui::TuiArgs::parse())
}
