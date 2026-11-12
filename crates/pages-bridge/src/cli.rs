//! Command line and double-click entry points.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::config::{self, Config, DocxLocation, ExistingDocx};
use crate::messages;
use crate::system::{self, Launcher};

#[derive(Parser)]
#[command(
    name = "pages-bridge",
    version,
    about = messages::ABOUT,
    after_help = messages::HELP_AFTER,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
    /// Bare file names: the same as `to-docx`.
    #[command(flatten)]
    to_docx: ToDocx,
}

#[derive(Args)]
struct ToDocx {
    #[arg(help = messages::HELP_FILES)]
    files: Vec<PathBuf>,
    #[arg(short, long, help = messages::HELP_OUTPUT)]
    output: Option<PathBuf>,
    #[arg(long, help = messages::HELP_OVERWRITE)]
    overwrite: bool,
}

#[derive(Subcommand)]
enum Cmd {
    #[command(about = messages::HELP_TO_DOCX)]
    ToDocx(ToDocx),
    #[command(about = messages::HELP_TO_PAGES, alias = "writeback")]
    ToPages {
        #[arg(help = messages::HELP_TO_PAGES_DOCX)]
        docx: PathBuf,
        #[arg(long, help = messages::HELP_TO_PAGES_ORIGINAL)]
        pages: Option<PathBuf>,
        #[arg(short, long, help = messages::HELP_OUTPUT_PAGES, conflicts_with = "in_place")]
        output: Option<PathBuf>,
        #[arg(short, long, help = messages::HELP_IN_PLACE)]
        in_place: bool,
        /// Leave Pages' cached layout and selection as they are (for testing).
        #[arg(long, hide = true)]
        keep_view_state: bool,
        /// Re-encode the text even without edits (for testing the writer on its own).
        #[arg(long, hide = true)]
        rewrite_unchanged: bool,
    },
    #[command(about = messages::HELP_OPEN)]
    Open {
        #[arg(help = messages::HELP_OPEN_FILE)]
        file: PathBuf,
    },
    #[command(about = messages::HELP_SETTINGS)]
    Settings,
    #[command(about = messages::HELP_CONFIG)]
    Config(ConfigArgs),
    #[command(about = messages::HELP_INSTALL)]
    Install {
        #[arg(long, value_name = "PROGRAMM", help = messages::HELP_OPEN_WITH)]
        open_with: Option<String>,
        #[arg(long, help = messages::HELP_NO_ASSOCIATE)]
        no_associate: bool,
    },
    #[command(about = messages::HELP_UNINSTALL)]
    Uninstall,
}

#[derive(Args)]
struct ConfigArgs {
    #[arg(long, value_name = "PROGRAMM", help = messages::HELP_OPEN_WITH)]
    open_with: Option<String>,
    #[arg(long, value_name = "AN_AUS", help = messages::HELP_WRITE_BACK)]
    write_back: Option<Switch>,
    #[arg(long, value_enum, help = messages::HELP_DOCX_LOCATION)]
    docx_location: Option<LocationArg>,
}

#[derive(Clone, Copy, ValueEnum)]
enum Switch {
    #[value(alias = "an", alias = "true")]
    On,
    #[value(alias = "aus", alias = "false")]
    Off,
}

#[derive(Clone, Copy, ValueEnum)]
enum LocationArg {
    Temp,
    NextToPages,
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
        self.notify(text, true);
    }

    /// Something the user must see even without a console.
    fn notify(&self, text: &str, is_error: bool) {
        if self.gui {
            system::message_box(text, is_error);
        } else if is_error {
            eprintln!("{text}");
        } else {
            println!("{text}");
        }
    }
}

/// Runs the program. `gui` is set for `pages-bridgew.exe`, which has no console; it carries
/// the console program as `embedded_cli` when it is the downloadable setup.
pub fn run(gui: bool, embedded_cli: &[u8]) -> ExitCode {
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
        Some(Cmd::ToDocx(args)) => convert_files(args, &ui),
        Some(Cmd::ToPages { docx, pages, output, in_place, keep_view_state, rewrite_unchanged }) => {
            let options = crate::writeback::Options { keep_view_state: *keep_view_state, rewrite_unchanged: *rewrite_unchanged };
            to_pages(docx, pages.as_deref(), output.as_deref(), *in_place, options, &ui)
        }
        Some(Cmd::Open { file }) => open(file, &ui),
        Some(Cmd::Settings) => settings(system::runs_installed(), embedded_cli),
        Some(Cmd::Config(args)) => configure(args, &ui),
        Some(Cmd::Install { open_with, no_associate }) => install(open_with.as_deref(), !no_associate, embedded_cli, &ui),
        Some(Cmd::Uninstall) => uninstall(&ui),
        // Started without anything to do: the downloaded setup installs, the installed program
        // shows its settings, and the console program its help.
        None if cli.to_docx.files.is_empty() && gui => settings(system::runs_installed(), embedded_cli),
        None if cli.to_docx.files.is_empty() => {
            let _ = <Cli as clap::CommandFactory>::command().print_help();
            Ok(())
        }
        // Files dropped onto the windowed program: open them, as a double-click would.
        None if gui => cli.to_docx.files.iter().map(|f| open(f, &ui)).find(Result::is_err).unwrap_or(Ok(())),
        None => convert_files(&cli.to_docx, &ui),
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

fn convert_files(args: &ToDocx, ui: &Ui) -> Result<(), String> {
    if args.output.is_some() && args.files.len() != 1 {
        return Err(messages::output_needs_single_input());
    }
    let mut failures = Vec::new();
    for file in &args.files {
        let target = match (&args.output, args.overwrite) {
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

/// What a double-click does: convert, open in the chosen program, and with write-back on, keep
/// the `.pages` in step with every save until the document is closed.
fn open(file: &Path, ui: &Ui) -> Result<(), String> {
    let config_path = config::path();
    let config = config::load_or_create(&config_path).map_err(|why| messages::config_invalid(&config_path, &why))?;
    let launcher = resolve_launcher(&config, &config_path)?;
    system::log(&format!("open {}", file.display()));
    let (docx, convert) = match config.docx_location {
        DocxLocation::Temp => (temp_docx(file), true),
        DocxLocation::NextToPages => choose_docx(file, config.existing_docx),
    };
    if convert {
        if let Some(dir) = docx.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        convert_one(file, &docx, ui)?;
    } else {
        ui.info(&messages::reused(&docx));
    }
    let editor = start(&launcher, &docx)?;
    if !config.write_back {
        return Ok(());
    }
    let outcome = crate::sync::run(file, &docx, editor);
    finish(file, &docx, config.docx_location, &outcome, ui);
    Ok(())
}

/// The `.docx` next to `file` to open, and whether it must be converted first.
fn choose_docx(file: &Path, policy: ExistingDocx) -> (PathBuf, bool) {
    let existing = docx_next_to(file);
    match policy {
        ExistingDocx::Overwrite => (existing, true),
        ExistingDocx::NewCopy if existing.exists() => (free_docx_name(file), true),
        _ if !existing.exists() => (existing, true),
        // Newer than the .pages: it may hold edits, open it as it is.
        ExistingDocx::OpenIfNewer if is_newer(&existing, file) => (existing, false),
        // An older conversion of ours is stale; a file from elsewhere is kept.
        ExistingDocx::OpenIfNewer if is_converted(&existing) => (existing, true),
        _ => (free_docx_name(file), true),
    }
}

/// Whether `docx` was written by this converter (editors keep the document properties).
fn is_converted(docx: &Path) -> bool {
    let Ok(file) = std::fs::File::open(docx) else { return false };
    let Ok(mut archive) = zip::ZipArchive::new(file) else { return false };
    let mut core = String::new();
    archive
        .by_name("docProps/core.xml")
        .ok()
        .and_then(|mut entry| std::io::Read::read_to_string(&mut entry, &mut core).ok())
        .is_some_and(|_| core.contains(crate::CONVERTED_MARKER) || core.contains(crate::OLD_CONVERTED_MARKER))
}

/// A private place for the `.docx`, named like the `.pages` so the editor shows that name.
fn temp_docx(file: &Path) -> PathBuf {
    let open = system::data_dir().join("open");
    remove_stale(&open);
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    open.join(format!("{}", std::process::id())).join(format!("{stem}.docx"))
}

/// Removes private `.docx` folders a crashed or killed session left behind. Only old ones: a
/// document may stay open for days.
fn remove_stale(open: &Path) {
    const STALE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);
    let Ok(entries) = std::fs::read_dir(open) else { return };
    for entry in entries.flatten() {
        let age = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
        if age.is_some_and(|age| age > STALE) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Tells the user what needs their attention after editing, and tidies up a private docx.
fn finish(file: &Path, docx: &Path, location: DocxLocation, outcome: &crate::sync::Outcome, ui: &Ui) {
    let mut lines = Vec::new();
    if let Some(error) = &outcome.error {
        lines.push(error.clone());
    }
    if let Some(copy) = &outcome.conflict_copy {
        lines.push(messages::conflict(copy));
    }
    if !outcome.not_written.is_empty() {
        let kinds: Vec<String> = outcome.not_written.keys().cloned().collect();
        lines.push(messages::session_not_written(&kinds.join(", ")));
    }
    if location == DocxLocation::Temp {
        let complete = outcome.error.is_none() && outcome.not_written.is_empty();
        if complete {
            if let Some(dir) = docx.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
        } else {
            // The docx holds what the .pages could not take: move it where the user finds it.
            let kept = crate::sync::free_name(&file.with_extension("docx"), messages::KEPT_DOCX_LABEL);
            if std::fs::copy(docx, &kept).is_ok() {
                lines.push(messages::kept_docx(&kept));
                if let Some(dir) = docx.parent() {
                    let _ = std::fs::remove_dir_all(dir);
                }
            }
        }
    }
    if !lines.is_empty() {
        ui.notify(&lines.join("\n\n"), outcome.error.is_some());
    }
}

fn start(launcher: &Launcher, document: &Path) -> Result<Option<std::process::Child>, String> {
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

/// The word processor's program file, whose icon `.pages` files show.
pub(crate) fn editor_exe(config: &Config, config_path: &Path) -> Option<PathBuf> {
    match resolve_launcher(config, config_path) {
        Ok(Launcher::Program(exe)) => Some(exe),
        _ => None,
    }
}

fn load_config() -> Result<(PathBuf, Config), String> {
    let path = config::path();
    let config = config::load_or_create(&path).map_err(|why| messages::config_invalid(&path, &why))?;
    Ok((path, config))
}

#[cfg(windows)]
fn settings(installed: bool, embedded_cli: &[u8]) -> Result<(), String> {
    let mode = if installed { crate::setup::Mode::Settings } else { crate::setup::Mode::Install };
    crate::setup::run(mode, embedded_cli)
}

#[cfg(not(windows))]
fn settings(_installed: bool, _embedded_cli: &[u8]) -> Result<(), String> {
    Err(messages::NOT_ON_THIS_SYSTEM.to_owned())
}

/// Shows the settings, or changes the ones given.
fn configure(args: &ConfigArgs, ui: &Ui) -> Result<(), String> {
    let (path, mut config) = load_config()?;
    let changed = args.open_with.is_some() || args.write_back.is_some() || args.docx_location.is_some();
    if let Some(program) = &args.open_with {
        config.open_with.clone_from(program);
        resolve_launcher(&config, &path)?;
    }
    if let Some(switch) = args.write_back {
        config.write_back = matches!(switch, Switch::On);
    }
    if let Some(location) = args.docx_location {
        config.docx_location = match location {
            LocationArg::Temp => DocxLocation::Temp,
            LocationArg::NextToPages => DocxLocation::NextToPages,
        };
    }
    if changed {
        config::save(&path, &config).map_err(|why| messages::config_invalid(&path, &why))?;
        // The file type shows the word processor's icon.
        #[cfg(windows)]
        if args.open_with.is_some() && system::is_associated() {
            system::associate(editor_exe(&config, &path).as_deref()).map_err(|e| messages::install_failed(&e.to_string()))?;
        }
    }
    ui.info(&messages::config_location(&path));
    if let Ok(text) = std::fs::read_to_string(&path) {
        ui.info(text.trim_end());
    }
    Ok(())
}

fn install(open_with: Option<&str>, associate: bool, embedded_cli: &[u8], ui: &Ui) -> Result<(), String> {
    let (path, mut config) = load_config()?;
    if let Some(program) = open_with {
        config.open_with = program.to_owned();
        resolve_launcher(&config, &path)?;
        config::save(&path, &config).map_err(|why| messages::config_invalid(&path, &why))?;
    }
    let dir = system::install(embedded_cli).map_err(|e| messages::install_failed(&e.to_string()))?;
    if associate {
        system::associate(editor_exe(&config, &path).as_deref()).map_err(|e| messages::install_failed(&e.to_string()))?;
    }
    ui.info(&messages::installed(&dir, associate));
    Ok(())
}

fn uninstall(ui: &Ui) -> Result<(), String> {
    #[cfg(windows)]
    if ui.gui {
        return crate::setup::uninstall();
    }
    system::uninstall().map_err(|e| messages::uninstall_failed(&e.to_string()))?;
    ui.info(&messages::uninstalled(&system::data_dir()));
    Ok(())
}

/// Writes the edits of `docx` into the `.pages` it was converted from: into a new file next
/// to it, the file given, or with `in_place` into the original after a backup.
fn to_pages(
    docx: &Path,
    pages: Option<&Path>,
    output: Option<&Path>,
    in_place: bool,
    options: crate::writeback::Options,
    ui: &Ui,
) -> Result<(), String> {
    let pages = pages.map_or_else(|| pages_for(docx), Path::to_path_buf);
    if !pages.is_file() {
        return Err(messages::pages_not_found(&pages));
    }
    let output = match output {
        Some(output) => output.to_path_buf(),
        // Next to the original, so the final rename stays on one volume.
        None if in_place => pages.with_extension("pages.tmp"),
        None => crate::sync::free_name(&pages, messages::CONFLICT_COPY_LABEL),
    };
    let report = crate::writeback::write_back(&pages, docx, &output, options).map_err(|e| messages::describe(&e, &pages))?;
    if !report.written {
        ui.info(messages::NO_CHANGES);
    } else if in_place {
        let backup = crate::sync::backup(&pages).map_err(|e| e.to_string())?;
        std::fs::rename(&output, &pages).map_err(|e| e.to_string())?;
        ui.info(&messages::written_back(&pages, report.paragraphs_changed, report.cells_changed));
        ui.info(&messages::backed_up(&backup));
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
