//! Thin binary wrapper over the `type-bridge` CLI library.

use std::process::ExitCode;

fn main() -> ExitCode {
    match u8::try_from(type_bridge_cli::run_cli(std::env::args_os())) {
        Ok(code) => ExitCode::from(code),
        Err(_) => ExitCode::FAILURE,
    }
}
