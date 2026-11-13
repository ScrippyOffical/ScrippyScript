//! Prints the syntax tree of a `.scp` file, or its errors with a caret.
//!
//! ```sh
//! cargo run --example parse -- .scp/demo.scp
//! ```

use std::env;
use std::fs;
use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(path) = env::args().nth(1) else {
        eprintln!("usage: parse <file.scp>");
        return ExitCode::from(2);
    };

    let src = match fs::read_to_string(&path) {
        Ok(src) => src,
        Err(err) => {
            eprintln!("{path}: {err}");
            return ExitCode::FAILURE;
        }
    };

    match scrippy_script_core::parse(&src) {
        Ok(script) => {
            println!("{script:#?}");
            ExitCode::SUCCESS
        }
        Err(errors) => {
            for err in errors {
                eprintln!("{}", err.render(&src));
            }
            ExitCode::FAILURE
        }
    }
}
