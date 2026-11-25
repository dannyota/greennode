pub mod render;
use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Parser)]
#[command(
    name = "greennode",
    version = VERSION,
    about = "Command-line access to GreenNode"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(about = "Print the greennode version")]
    Version,
}

pub fn run<I, T>(args: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let rendered = error.render();
            let _ = if error.use_stderr() {
                write!(stderr, "{rendered}")
            } else {
                write!(stdout, "{rendered}")
            };
            return ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(1));
        }
    };
    match cli.command {
        Some(Command::Version) => {
            let _ = writeln!(stdout, "greennode {VERSION}");
        }
        None => {
            let _ = write!(stdout, "{}", Cli::command().render_help());
        }
    }
    ExitCode::SUCCESS
}
