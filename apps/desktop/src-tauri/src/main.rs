// Keep release builds from opening an extra console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Write as _;
use std::process::ExitCode;

use link2linux_desktop_lib::cli::{self, Command};

fn main() -> ExitCode {
    match cli::parse(std::env::args_os().skip(1)) {
        Command::Run => {
            link2linux_desktop_lib::run();
            ExitCode::SUCCESS
        }
        // A closed pipe, as in `--help | head -1`, is no reason to panic.
        Command::Print(text) => {
            let _ = std::io::stdout().write_all(text.as_bytes());
            ExitCode::SUCCESS
        }
        Command::Refuse(text) => {
            let _ = std::io::stderr().write_all(text.as_bytes());
            ExitCode::from(2)
        }
    }
}
