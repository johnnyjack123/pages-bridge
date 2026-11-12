//! Operating system integration: installed fonts, launching programs, message boxes, and the
//! per-user installation with its file type registration for `.pages`. Only Windows is
//! supported; elsewhere these degrade to no-ops or errors.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Folder name under `%APPDATA%`, `%LOCALAPPDATA%` and `%LOCALAPPDATA%\Programs`.
pub(crate) const APP_DIR: &str = "pages-bridge";
/// The program's former name, whose installation is replaced.
pub(crate) const OLD_APP_DIR: &str = "pages2docx";
pub(crate) const PROG_ID: &str = "PagesBridge.Document";
const OLD_PROG_ID: &str = "Pages2Docx.Document";
pub(crate) const GUI_EXE: &str = "pages-bridgew.exe";
pub(crate) const CLI_EXE: &str = "pages-bridge.exe";
#[cfg(windows)]
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\pages-bridge";
pub(crate) const REPOSITORY: &str = "https://github.com/johnnyjack123/pages-bridge";

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

fn local_app_data() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// `%LOCALAPPDATA%\pages-bridge`: backups, the log and session files.
pub(crate) fn data_dir() -> PathBuf {
    local_app_data().join(APP_DIR)
}

/// `%LOCALAPPDATA%\Programs\pages-bridge`, where the programs are installed for this user.
pub(crate) fn install_dir() -> PathBuf {
    local_app_data().join("Programs").join(APP_DIR)
}

/// Whether this program runs from its installation folder.
pub(crate) fn runs_installed() -> bool {
    std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| same_dir(dir, &install_dir()))).unwrap_or(false)
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

/// The program files to install: (file name, contents). The running program provides the one it
/// is; the other comes from `embedded_cli` (the setup carries the console program inside) or
/// from the same folder (a build folder holds both).
fn program_files(embedded_cli: &[u8]) -> io::Result<Vec<(&'static str, Vec<u8>)>> {
    let current = std::env::current_exe()?;
    let dir = current.parent().ok_or_else(|| io::Error::other("program folder unknown"))?;
    let is_cli = current.file_name().is_some_and(|n| n.eq_ignore_ascii_case(CLI_EXE));
    let (gui, cli) = if is_cli {
        (std::fs::read(dir.join(GUI_EXE))?, std::fs::read(&current)?)
    } else {
        let cli = if embedded_cli.is_empty() { std::fs::read(dir.join(CLI_EXE))? } else { embedded_cli.to_vec() };
        (std::fs::read(&current)?, cli)
    };
    Ok(vec![(GUI_EXE, gui), (CLI_EXE, cli)])
}

/// Writes `data` to `path`. A program that is running (an open editing session) cannot be
/// overwritten but can be renamed, so it is moved aside first; the next install removes it.
#[cfg(windows)]
fn put_file(path: &Path, data: &[u8]) -> io::Result<()> {
    match std::fs::write(path, data) {
        Ok(()) => Ok(()),
        Err(_) if path.exists() => {
            let aside = path.with_extension(format!("old-{}", std::process::id()));
            std::fs::rename(path, &aside)?;
            std::fs::write(path, data)
        }
        Err(e) => Err(e),
    }
}

/// Copies the programs to [`install_dir`] and registers them with Windows for the current user:
/// an entry in Settings › Apps (to change or remove it) and a Start menu entry for the settings.
/// Replaces an installation under the former name. Returns the installation folder.
#[cfg(windows)]
pub(crate) fn install(embedded_cli: &[u8]) -> io::Result<PathBuf> {
    let target = install_dir();
    std::fs::create_dir_all(&target)?;
    if let Ok(entries) = std::fs::read_dir(&target) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().contains(".old-") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let running_installed = runs_installed();
    for (name, data) in program_files(embedded_cli)? {
        let path = target.join(name);
        // The running program is already in place when it is the installed one.
        if !(running_installed && std::env::current_exe().is_ok_and(|exe| same_file(&exe, &path))) {
            put_file(&path, &data)?;
        }
    }
    register_uninstall_entry(&target)?;
    create_shortcut(&target);
    if let Err(e) = edit_user_path(&target, true) {
        log(&format!("PATH not extended: {e}"));
    }
    remove_old_installation();
    Ok(target)
}

/// Adds `dir` to the user's `PATH` (so `pages-bridge` works in any terminal), or removes it.
/// Terminals opened afterwards see the change.
#[cfg(windows)]
fn edit_user_path(dir: &Path, add: bool) -> io::Result<()> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_EXPAND_SZ};
    use winreg::{RegKey, RegValue};
    let (env, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)?;
    // Raw, to keep the value's type: entries may use %VARIABLES% that must stay unexpanded.
    let current = match env.get_raw_value("Path") {
        Ok(value) => value,
        Err(e) if e.kind() == io::ErrorKind::NotFound => RegValue { bytes: Vec::new().into(), vtype: REG_EXPAND_SZ },
        Err(e) => return Err(e),
    };
    let units: Vec<u16> = current.bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
    let text = String::from_utf16_lossy(&units).trim_end_matches('\0').to_owned();
    let dir_text = dir.display().to_string();
    let is_dir = |entry: &str| entry.trim().trim_end_matches('\\').eq_ignore_ascii_case(dir_text.trim_end_matches('\\'));
    let mut entries: Vec<&str> = text.split(';').filter(|e| !e.is_empty()).collect();
    let present = entries.iter().any(|e| is_dir(e));
    if add == present {
        return Ok(());
    }
    if add {
        entries.push(&dir_text);
    } else {
        entries.retain(|e| !is_dir(e));
    }
    let bytes: Vec<u8> = entries.join(";").encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect();
    env.set_raw_value("Path", &RegValue { bytes: bytes.into(), vtype: current.vtype })?;
    notify_environment_changed();
    Ok(())
}

/// Running programs, Explorer among them, reread the environment when told it changed.
#[cfg(windows)]
#[allow(unsafe_code)]
fn notify_environment_changed() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE};
    let area = wide("Environment");
    let mut result = 0;
    // SAFETY: `area` is a NUL-terminated buffer that outlives the call; `result` is writable.
    unsafe {
        SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, 0, area.as_ptr() as isize, SMTO_ABORTIFHUNG, 5000, &mut result);
    }
}

/// Makes double-clicked `.pages` files open with the installed program. `icon` is the program
/// whose icon the files show, normally the word processor they open in.
#[cfg(windows)]
pub(crate) fn associate(icon: Option<&Path>) -> io::Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let gui = install_dir().join(GUI_EXE);
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
    Ok(())
}

/// Whether double-clicked `.pages` files open with this program.
#[cfg(windows)]
pub(crate) fn is_associated() -> bool {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Classes\.pages")
        .and_then(|ext| ext.get_value::<String, _>(""))
        .is_ok_and(|v| v == PROG_ID)
}

/// Removes the `.pages` registration of `prog_id`, leaving other programs' alone.
#[cfg(windows)]
fn dissociate(prog_id: &str) -> io::Result<()> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_ALL_ACCESS};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    match hkcu.delete_subkey_all(format!(r"Software\Classes\{prog_id}")) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    if let Ok(ext) = hkcu.open_subkey_with_flags(r"Software\Classes\.pages", KEY_ALL_ACCESS) {
        if ext.get_value::<String, _>("").is_ok_and(|v| v == prog_id) {
            ext.set_value("", &"")?;
        }
        if let Ok(ids) = ext.open_subkey_with_flags("OpenWithProgids", KEY_ALL_ACCESS) {
            let _ = ids.delete_value(prog_id);
        }
    }
    notify_associations_changed();
    Ok(())
}

#[cfg(windows)]
pub(crate) fn remove_association() -> io::Result<()> {
    dissociate(PROG_ID)
}

/// The entry in Settings › Apps: "Ändern" opens the settings, "Deinstallieren" removes it.
#[cfg(windows)]
fn register_uninstall_entry(dir: &Path) -> io::Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let gui = dir.join(GUI_EXE);
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(UNINSTALL_KEY)?;
    key.set_value("DisplayName", &"pages-bridge")?;
    key.set_value("DisplayVersion", &env!("CARGO_PKG_VERSION"))?;
    key.set_value("Publisher", &"pages-bridge")?;
    key.set_value("URLInfoAbout", &REPOSITORY)?;
    key.set_value("InstallLocation", &dir.display().to_string())?;
    key.set_value("DisplayIcon", &gui.display().to_string())?;
    key.set_value("UninstallString", &format!("\"{}\" uninstall", gui.display()))?;
    key.set_value("ModifyPath", &format!("\"{}\" settings", gui.display()))?;
    key.set_value("NoRepair", &1u32)?;
    let size_kb: u64 = [GUI_EXE, CLI_EXE].iter().filter_map(|f| std::fs::metadata(dir.join(f)).ok()).map(|m| m.len() / 1024).sum();
    key.set_value("EstimatedSize", &u32::try_from(size_kb).unwrap_or(u32::MAX))?;
    Ok(())
}

/// `%APPDATA%\Microsoft\Windows\Start Menu\Programs\…`, the user's Start menu.
#[cfg(windows)]
fn shortcut_path() -> Option<PathBuf> {
    let roaming = std::env::var_os("APPDATA")?;
    Some(
        PathBuf::from(roaming)
            .join(r"Microsoft\Windows\Start Menu\Programs")
            .join(format!("{}.lnk", crate::messages::SHORTCUT_NAME)),
    )
}

/// A Start menu entry that opens the settings. Best effort: the program works without it.
#[cfg(windows)]
fn create_shortcut(dir: &Path) {
    let Some(path) = shortcut_path() else { return };
    let gui = dir.join(GUI_EXE);
    let Ok(mut link) = mslnk::ShellLink::new(&gui) else { return };
    link.set_arguments(Some("settings".to_owned()));
    link.set_working_dir(Some(dir.display().to_string()));
    link.set_name(Some(crate::messages::SHORTCUT_DESCRIPTION.to_owned()));
    if let Err(e) = link.create_lnk(&path) {
        log(&format!("Start menu entry not created: {e}"));
    }
}

/// Removes what an installation under the program's former name left: its file type
/// registration and its program folder. Its settings were already taken over.
#[cfg(windows)]
fn remove_old_installation() {
    let _ = dissociate(OLD_PROG_ID);
    let _ = std::fs::remove_dir_all(local_app_data().join("Programs").join(OLD_APP_DIR));
}

/// Removes the registration, the Settings › Apps entry, the Start menu entry and the program
/// files, including the running program once it has exited. Settings, backups and the log stay.
#[cfg(windows)]
pub(crate) fn uninstall() -> io::Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    remove_association()?;
    remove_old_installation();
    let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(UNINSTALL_KEY);
    if let Some(path) = shortcut_path() {
        let _ = std::fs::remove_file(path);
    }
    let dir = install_dir();
    if let Err(e) = edit_user_path(&dir, false) {
        log(&format!("PATH not cleaned up: {e}"));
    }
    if runs_installed() {
        // A running program cannot be deleted, but it can be moved out of the way; it removes
        // itself from there when it exits.
        let exe = std::env::current_exe()?;
        let aside = std::env::temp_dir().join(format!("pages-bridge-uninstall-{}.exe", std::process::id()));
        if std::fs::rename(&exe, &aside).is_ok() {
            self_replace::self_delete_at(&aside)?;
        } else {
            // The temporary folder is on another drive: delete the program at exit, in place,
            // and everything else now. The empty folder stays.
            self_replace::self_delete_outside_path(&dir)?;
            for name in [GUI_EXE, CLI_EXE] {
                if !std::env::current_exe().is_ok_and(|exe| same_file(&exe, &dir.join(name))) {
                    let _ = std::fs::remove_file(dir.join(name));
                }
            }
            return Ok(());
        }
    }
    match std::fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(not(windows))]
pub(crate) fn install(_embedded_cli: &[u8]) -> io::Result<PathBuf> {
    Err(io::Error::other(crate::messages::NOT_ON_THIS_SYSTEM))
}

#[cfg(not(windows))]
pub(crate) fn associate(_icon: Option<&Path>) -> io::Result<()> {
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

fn same_dir(a: &Path, b: &Path) -> bool {
    same_file(a, b)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
