//! Command line and double-click entry points.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::config::{self, Config, ExistingDocx};
use crate::messages;
use crate::system::{self, Launcher};

#[derive(Parser)]
#[command(name = "pages2docx", version, about = messages::ABOUT, args_conflicts_with_subcommands = true, arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
    #[arg(help = messages::HELP_FILES)]
    files: Vec<PathBuf>,
    #[arg(short, long, help = messages::HELP_OUTPUT)]
    output: Option<PathBuf>,
    #[arg(long, help = messages::HELP_OVERWRITE)]
    overwrite: bool,
}

#[derive(Subcommand)]
enum Cmd {
    #[command(about = messages::HELP_OPEN)]
    Open {
        #[arg(help = messages::HELP_OPEN_FILE)]
        file: PathBuf,
    },
    #[command(about = messages::HELP_INSTALL)]
    Install,
    #[command(about = messages::HELP_UNINSTALL)]
    Uninstall,
    #[command(about = messages::HELP_CONFIG)]
    Config,
    #[command(about = messages::HELP_WRITEBACK)]
    Writeback {
        #[arg(help = messages::HELP_WRITEBACK_DOCX)]
        docx: PathBuf,
        #[arg(long, help = messages::HELP_WRITEBACK_PAGES)]
        pages: Option<PathBuf>,
        #[arg(short, long, help = messages::HELP_OUTPUT_PAGES)]
        output: Option<PathBuf>,
        /// Leave Pages' cached layout and selection as they are (for testing).
        #[arg(long, hide = true)]
        keep_view_state: bool,
        /// Re-encode the text even without edits (for testing the writer on its own).
        #[arg(long, hide = true)]
        rewrite_unchanged: bool,
    },
}

/// Where messages go: the console, or message boxes for the windowed launcher.
struct Ui {
    gui: bool,
}

impl Ui {
    fn info(&self, text: &str) {
        if !self.gui {
            println!("{text}");
        }
    }

    fn error(&self, text: &str) {
        if self.gui {
            system::message_box(text, true);
        } else {
            eprintln!("{text}");
        }
    }
}

/// Runs the program; `gui` is set for `pages2docxw.exe`, which has no console.
pub fn run(gui: bool) -> ExitCode {
    let ui = Ui { gui };
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) if gui => {
            ui.error(&e.to_string());
            return ExitCode::from(2);
        }
        Err(e) => e.exit(),
    };
    let result = match &cli.command {
        Some(Cmd::Open { file }) => open(file, &ui),
        Some(Cmd::Install) => install(&ui),
        Some(Cmd::Uninstall) => system::uninstall().map(|()| ui.info(messages::UNINSTALLED)).map_err(|e| e.to_string()),
        Some(Cmd::Config) => show_config(&ui),
        Some(Cmd::Writeback { docx, pages, output, keep_view_state, rewrite_unchanged }) => {
            let options = crate::writeback::Options { keep_view_state: *keep_view_state, rewrite_unchanged: *rewrite_unchanged };
            write_back(docx, pages.as_deref(), output.as_deref(), options, &ui)
        }
        // Files dropped onto the windowed program: open them, as a double-click would.
        None if gui => cli.files.iter().map(|f| open(f, &ui)).find(Result::is_err).unwrap_or(Ok(())),
        None => convert_files(&cli, &ui),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(text) => {
            if !text.is_empty() {
                ui.error(&text);
            }
            ExitCode::FAILURE
        }
    }
}

fn convert_files(cli: &Cli, ui: &Ui) -> Result<(), String> {
    if cli.output.is_some() && cli.files.len() != 1 {
        return Err(messages::output_needs_single_input());
    }
    let mut failures = Vec::new();
    for file in &cli.files {
        let target = match (&cli.output, cli.overwrite) {
            (Some(out), _) => out.clone(),
            (None, true) => docx_next_to(file),
            (None, false) => free_docx_name(file),
        };
        if let Err(text) = convert_one(file, &target, ui) {
            ui.error(&text);
            failures.push(text);
        }
    }
    // Each failure was reported as it happened; an empty message only sets the exit status.
    if failures.is_empty() { Ok(()) } else { Err(String::new()) }
}

fn convert_one(file: &Path, target: &Path, ui: &Ui) -> Result<(), String> {
    let report = crate::convert(file, target).map_err(|e| messages::describe(&e, file))?;
    ui.info(&messages::saved(target));
    for note in &report.notes {
        ui.info(note);
    }
    if !report.skipped.is_empty() {
        let kinds: Vec<String> = report.skipped.iter().map(|(k, n)| format!("{k}: {n}")).collect();
        ui.info(&messages::skipped(report.skipped.values().sum(), &kinds.join(", ")));
    }
    Ok(())
}

/// Converts next to the `.pages` file and opens the result: what a double-click does.
fn open(file: &Path, ui: &Ui) -> Result<(), String> {
    let config_path = config::path();
    let config = config::load_or_create(&config_path).map_err(|why| messages::config_invalid(&config_path, &why))?;
    let launcher = resolve_launcher(&config, &config_path)?;
    let existing = docx_next_to(file);
    let target = match config.existing_docx {
        ExistingDocx::Overwrite => existing,
        ExistingDocx::OpenIfNewer if is_newer(&existing, file) => {
            ui.info(&messages::reused(&existing));
            return start(&launcher, &existing);
        }
        _ => free_docx_name(file),
    };
    convert_one(file, &target, ui)?;
    start(&launcher, &target)
}

fn start(launcher: &Launcher, document: &Path) -> Result<(), String> {
    system::launch(launcher, document).map_err(|e| {
        let program = match launcher {
            Launcher::Program(p) => p.clone(),
            Launcher::Default => PathBuf::from("Windows"),
        };
        messages::launch_failed(&program, &e.to_string())
    })
}

fn resolve_launcher(config: &Config, config_path: &Path) -> Result<Launcher, String> {
    let choice = config.open_with.trim();
    if choice.eq_ignore_ascii_case("default") {
        return Ok(Launcher::Default);
    }
    let program = config.programs.get(choice).map_or(choice, String::as_str);
    if program.eq_ignore_ascii_case("default") {
        return Ok(Launcher::Default);
    }
    system::find_program(program).map(Launcher::Program).ok_or_else(|| messages::program_not_found(choice, config_path))
}

fn install(ui: &Ui) -> Result<(), String> {
    let config_path = config::path();
    let config = config::load_or_create(&config_path).map_err(|why| messages::config_invalid(&config_path, &why))?;
    // The file type shows the icon of the program it opens in.
    let icon = match resolve_launcher(&config, &config_path) {
        Ok(Launcher::Program(exe)) => Some(exe),
        _ => None,
    };
    let dir = system::install(icon.as_deref()).map_err(|e| messages::install_failed(&e.to_string()))?;
    ui.info(&messages::installed(&dir));
    Ok(())
}

fn show_config(ui: &Ui) -> Result<(), String> {
    let path = config::path();
    config::load_or_create(&path).map_err(|why| messages::config_invalid(&path, &why))?;
    ui.info(&messages::config_location(&path));
    if let Ok(text) = std::fs::read_to_string(&path) {
        ui.info(&text);
    }
    Ok(())
}

fn write_back(
    docx: &Path,
    pages: Option<&Path>,
    output: Option<&Path>,
    options: crate::writeback::Options,
    ui: &Ui,
) -> Result<(), String> {
    let pages = pages.map_or_else(|| pages_for(docx), Path::to_path_buf);
    if !pages.is_file() {
        return Err(messages::pages_not_found(&pages));
    }
    let output = output.map_or_else(|| free_name(&pages, "bearbeitet"), Path::to_path_buf);
    let report = crate::writeback::write_back(&pages, docx, &output, options).map_err(|e| messages::describe(&e, &pages))?;
    if report.is_unchanged() && !output.exists() {
        ui.info(messages::NO_CHANGES);
    } else {
        ui.info(&messages::written_back(&output, report.paragraphs_changed, report.cells_changed));
    }
    if !report.not_written.is_empty() {
        let kinds: Vec<String> = report.not_written.iter().map(|(k, n)| format!("{k} ({n})")).collect();
        ui.info(&messages::not_written(report.not_written.values().sum(), &kinds.join(", ")));
    }
    Ok(())
}

/// The `.pages` a converted `.docx` belongs to: same name, without a ` (2)` the converter adds
/// when the plain name was taken.
fn pages_for(docx: &Path) -> PathBuf {
    let direct = docx.with_extension("pages");
    if direct.exists() {
        return direct;
    }
    let stem = docx.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let base = stem
        .rsplit_once(" (")
        .filter(|(_, n)| n.strip_suffix(')').is_some_and(|n| n.chars().all(|c| c.is_ascii_digit())))
        .map_or(stem.as_str(), |(base, _)| base);
    docx.with_file_name(format!("{base}.pages"))
}

/// `X (label).pages` next to `file`, or `X (label 2).pages` … if that is taken.
fn free_name(file: &Path, label: &str) -> PathBuf {
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = file.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    std::iter::once(format!("{stem} ({label}).{ext}"))
        .chain((2..).map(|n| format!("{stem} ({label} {n}).{ext}")))
        .map(|name| file.with_file_name(name))
        .find(|candidate| !candidate.exists())
        .expect("an unused name exists")
}

fn docx_next_to(file: &Path) -> PathBuf {
    file.with_extension("docx")
}

/// `X.docx`, or `X (2).docx`, `X (3).docx` … if that name is taken.
fn free_docx_name(file: &Path) -> PathBuf {
    let first = docx_next_to(file);
    if !first.exists() {
        return first;
    }
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    (2..)
        .map(|n| file.with_file_name(format!("{stem} ({n}).docx")))
        .find(|candidate| !candidate.exists())
        .unwrap_or(first)
}

fn is_newer(candidate: &Path, than: &Path) -> bool {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    matches!((modified(candidate), modified(than)), (Some(a), Some(b)) if a >= b)
}
