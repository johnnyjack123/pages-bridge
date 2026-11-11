//! Operating system integration: installed fonts, launching programs, message boxes and the
//! per-user file type registration for `.pages`. Only Windows is supported; elsewhere these
//! degrade to no-ops or errors.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) const PROG_ID: &str = "Pages2Docx.Document";
const GUI_EXE: &str = "pages2docxw.exe";
const CLI_EXE: &str = "pages2docx.exe";

/// How to open a converted document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Launcher {
    /// Whatever Windows opens `.docx` files with.
    Default,
    Program(PathBuf),
}

/// Opens `document` with `launcher` without waiting. Returns the started process when there is
/// one to watch; the Windows shell does not hand one out.
pub(crate) fn launch(launcher: &Launcher, document: &Path) -> io::Result<Option<std::process::Child>> {
    match launcher {
        Launcher::Default => shell_open(document).map(|()| None),
        Launcher::Program(exe) => {
            let mut cmd = Command::new(exe);
            cmd.arg(document);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const DETACHED_PROCESS: u32 = 0x0000_0008;
                cmd.creation_flags(DETACHED_PROCESS);
            }
            cmd.spawn().map(Some)
        }
    }
}

/// Whether another program holds the file open for writing, as Word does with an open document.
#[cfg(windows)]
pub(crate) fn is_locked(path: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 0x1;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    // Read access shared only with readers fails while someone has write access.
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .is_err_and(|e| e.raw_os_error() == Some(ERROR_SHARING_VIOLATION))
}

#[cfg(not(windows))]
pub(crate) fn is_locked(_path: &Path) -> bool {
    false
}

/// `%LOCALAPPDATA%\pages2docx`: backups, the log and session files.
pub(crate) fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("pages2docx")
}

/// Appends a line to `log.txt` in the data folder. The windowed program has no console, so this
/// is where its sessions can be followed afterwards.
pub(crate) fn log(line: &str) {
    use std::io::Write as _;
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("log.txt");
    // Keep the log from growing without bound.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1_000_000) {
        let _ = std::fs::rename(&path, dir.join("log.old.txt"));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{} [{}] {line}", crate::utc_timestamp(), std::process::id());
    }
}

/// Finds a bare program name such as `winword.exe` the way the Windows shell does: in the
/// App Paths registry key first, then on `PATH`.
pub(crate) fn find_program(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.components().count() > 1 || path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    if let Some(found) = app_path(name) {
        return Some(found);
    }
    let with_exe = if name.to_lowercase().ends_with(".exe") { name.to_owned() } else { format!("{name}.exe") };
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).map(|dir| dir.join(&with_exe)).find(|candidate| candidate.is_file())
    })
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn app_path(name: &str) -> Option<PathBuf> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    let with_exe = if name.to_lowercase().ends_with(".exe") { name.to_owned() } else { format!("{name}.exe") };
    let key = format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{with_exe}");
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].into_iter().find_map(|hive| {
        let value: String = RegKey::predef(hive).open_subkey(&key).ok()?.get_value("").ok()?;
        let path = PathBuf::from(value.trim_matches('"'));
        path.is_file().then_some(path)
    })
}

#[cfg(not(windows))]
fn app_path(_name: &str) -> Option<PathBuf> {
    None
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn shell_open(document: &Path) -> io::Result<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let (verb, file) = (wide("open"), wide(&document.to_string_lossy()));
    // SAFETY: both strings are NUL-terminated UTF-16 buffers that outlive the call; the other
    // pointer arguments may be null per the ShellExecuteW contract.
    let result = unsafe {
        ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL)
    };
    // ShellExecuteW reports success with a value greater than 32.
    if result as usize > 32 { Ok(()) } else { Err(io::Error::other(format!("ShellExecute error {}", result as usize))) }
}

#[cfg(not(windows))]
fn shell_open(document: &Path) -> io::Result<()> {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    Command::new(opener).arg(document).spawn().map(|_| ())
}

/// Shows a message for the windowed launcher, which has no console.
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) fn message_box(text: &str, is_error: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW};
    let (text, title) = (wide(text), wide(crate::messages::WINDOW_TITLE));
    let icon = if is_error { MB_ICONERROR } else { MB_ICONINFORMATION };
    // SAFETY: NUL-terminated UTF-16 buffers that outlive the call; a null owner window is allowed.
    unsafe {
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | icon);
    }
}

#[cfg(not(windows))]
pub(crate) fn message_box(text: &str, _is_error: bool) {
    eprintln!("{text}");
}

/// Lower-case family names of the installed fonts, from the registry's font list, which also
/// covers per-user installs. `None` where that list is not available.
#[cfg(windows)]
pub(crate) fn installed_font_families() -> Option<HashSet<String>> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    const STYLE_WORDS: &[&str] = &[
        "bold", "italic", "oblique", "light", "semilight", "semibold", "black", "regular", "medium", "thin", "heavy",
        "condensed", "narrow", "extra", "extralight", "demi", "ultra", "book",
    ];
    let mut families = HashSet::new();
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        let Ok(key) = RegKey::predef(hive).open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts") else {
            continue;
        };
        for (name, _) in key.enum_values().flatten() {
            // "Arial Bold (TrueType)", "Cambria & Cambria Math (TrueType)"
            let name = name.split(" (").next().unwrap_or(&name).to_lowercase();
            for part in name.split(" & ") {
                let mut words: Vec<&str> = part.split_whitespace().collect();
                families.insert(words.join(" "));
                while words.last().is_some_and(|w| STYLE_WORDS.contains(w)) {
                    words.pop();
                    families.insert(words.join(" "));
                }
            }
        }
    }
    (!families.is_empty()).then_some(families)
}

#[cfg(not(windows))]
pub(crate) fn installed_font_families() -> Option<HashSet<String>> {
    None
}

/// Copies the programs to `%LOCALAPPDATA%\Programs\pages2docx` and registers them for
/// double-clicks on `.pages` files, for the current user only. Returns the install folder.
#[cfg(windows)]
pub(crate) fn install(icon: Option<&Path>) -> io::Result<PathBuf> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let local = std::env::var_os("LOCALAPPDATA").ok_or_else(|| io::Error::other("LOCALAPPDATA is not set"))?;
    let target = PathBuf::from(local).join("Programs").join("pages2docx");
    std::fs::create_dir_all(&target)?;
    let current = std::env::current_exe()?;
    let source = current.parent().ok_or_else(|| io::Error::other("program folder unknown"))?;
    if !same_dir(source, &target) {
        for exe in [CLI_EXE, GUI_EXE] {
            std::fs::copy(source.join(exe), target.join(exe))?;
        }
    }
    let gui = target.join(GUI_EXE);
    let classes = RegKey::predef(HKEY_CURRENT_USER).create_subkey(r"Software\Classes")?.0;
    let (prog, _) = classes.create_subkey(PROG_ID)?;
    prog.set_value("", &crate::messages::FILE_TYPE_NAME)?;
    let icon = icon.filter(|p| p.is_file()).unwrap_or(&gui);
    prog.create_subkey("DefaultIcon")?.0.set_value("", &format!("\"{}\",0", icon.display()))?;
    prog.create_subkey(r"shell\open\command")?.0.set_value("", &format!("\"{}\" open \"%1\"", gui.display()))?;
    let (ext, _) = classes.create_subkey(".pages")?;
    ext.set_value("", &PROG_ID)?;
    ext.create_subkey("OpenWithProgids")?.0.set_value(PROG_ID, &"")?;
    notify_associations_changed();
    Ok(target)
}

#[cfg(windows)]
pub(crate) fn uninstall() -> io::Result<()> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_ALL_ACCESS};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    match hkcu.delete_subkey_all(format!(r"Software\Classes\{PROG_ID}")) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    if let Ok(ext) = hkcu.open_subkey_with_flags(r"Software\Classes\.pages", KEY_ALL_ACCESS) {
        if ext.get_value::<String, _>("").is_ok_and(|v| v == PROG_ID) {
            ext.set_value("", &"")?;
        }
        if let Ok(ids) = ext.open_subkey_with_flags("OpenWithProgids", KEY_ALL_ACCESS) {
            let _ = ids.delete_value(PROG_ID);
        }
    }
    notify_associations_changed();
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn install(_icon: Option<&Path>) -> io::Result<PathBuf> {
    Err(io::Error::other(crate::messages::NOT_ON_THIS_SYSTEM))
}

#[cfg(not(windows))]
pub(crate) fn uninstall() -> io::Result<()> {
    Err(io::Error::other(crate::messages::NOT_ON_THIS_SYSTEM))
}

/// Explorer caches file associations until told that they changed.
#[cfg(windows)]
#[allow(unsafe_code)]
fn notify_associations_changed() {
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    // windows-sys declares the constant as u32 but the parameter as i32; 0x0800_0000 fits.
    let event = i32::try_from(SHCNE_ASSOCCHANGED).unwrap_or(i32::MAX);
    // SAFETY: SHCNE_ASSOCCHANGED takes no items, so both item pointers are null.
    unsafe { SHChangeNotify(event, SHCNF_IDLIST, std::ptr::null(), std::ptr::null()) };
}

#[cfg(windows)]
fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
