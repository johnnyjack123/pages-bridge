//! `pages-bridge`: the console program.

fn main() -> std::process::ExitCode {
    pages_bridge::cli::run(false, &[])
}
