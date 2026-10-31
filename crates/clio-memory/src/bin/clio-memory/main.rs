//! `clio-memory`: inspect and maintain a memory file without writing code.
//! The commands that only read open the file read-only, so they work while another program
//! has it open and never change it; the ones that change something open it read-write and
//! fail with a clear message if another program holds its write lock.

mod args;
mod commands;
mod output;
mod write;

use std::process::ExitCode;

fn main() -> ExitCode {
    match args::parse(std::env::args().skip(1)) {
        Ok(args::Parsed::Help) => {
            output::line(args::USAGE);
            ExitCode::SUCCESS
        }
        Ok(args::Parsed::Run(cli)) => match commands::run(&cli) {
            Ok(code) => code,
            Err(message) => {
                output::error(&format!("error: {message}"));
                ExitCode::from(1)
            }
        },
        Err(args::UsageError(message)) => {
            // The full text is ~14 KB; a caller (often an agent) pays for it on every typo.
            output::error(&format!(
                "error: {message}\nhelp: `clio-memory --help` prints the usage"
            ));
            ExitCode::from(2)
        }
    }
}
