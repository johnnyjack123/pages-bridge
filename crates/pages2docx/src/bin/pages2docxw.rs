//! `pages2docxw`: the same program without a console window, for double-clicks on `.pages`
//! files. Errors appear as message boxes.

#![windows_subsystem = "windows"]

fn main() -> std::process::ExitCode {
    pages2docx::cli::run(true)
}
