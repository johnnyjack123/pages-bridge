//! `pages-bridgew`: the program without a console window. It handles double-clicks on `.pages`
//! files, shows the settings, and as the downloadable setup installs itself; errors appear as
//! message boxes.

#![windows_subsystem = "windows"]

/// The console program, when this is the setup (see `build.rs`); empty otherwise.
static EMBEDDED_CLI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/embedded-cli.exe"));

fn main() -> std::process::ExitCode {
    pages_bridge::cli::run(true, EMBEDDED_CLI)
}
