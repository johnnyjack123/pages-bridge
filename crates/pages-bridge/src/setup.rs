//! The window for installing pages-bridge and for changing its settings later: which word
//! processor opens Pages documents, and whether a double-click on a `.pages` file opens it with
//! pages-bridge. It is a Windows task dialog, so it needs no UI library.

use crate::config::{self, Config};
use crate::messages;
use crate::system;

/// Started as the downloaded setup, or from the installed program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Install,
    Settings,
}

/// The word processors on offer: keys of the settings file's `programs`, or `default`.
const CHOICES: [(&str, &str); 3] =
    [("wordcraft", messages::CHOICE_WORDCRAFT), ("word", messages::CHOICE_WORD), ("default", messages::CHOICE_DEFAULT)];

const BUTTON_SAVE: i32 = 100;
const BUTTON_UNINSTALL: i32 = 101;
/// Radio buttons are numbered from here; the dialog reports the chosen one's number.
const FIRST_RADIO: i32 = 1000;

/// Shows the window and does what the user chose. `embedded_cli` is the console program the
/// setup carries for installing.
pub(crate) fn run(mode: Mode, embedded_cli: &[u8]) -> Result<(), String> {
    let config_path = config::path();
    let mut config = config::load_or_create(&config_path).map_err(|why| messages::config_invalid(&config_path, &why))?;
    let mut choices: Vec<(String, String)> = CHOICES
        .iter()
        .map(|(key, label)| {
            let label = if is_available(&config, key) { (*label).to_owned() } else { messages::not_found(label) };
            ((*key).to_owned(), label)
        })
        .collect();
    let current = config.open_with.trim().to_owned();
    if !choices.iter().any(|(key, _)| key.eq_ignore_ascii_case(&current)) {
        choices.push((current.clone(), messages::current_program(&current)));
    }
    let preselected = choices
        .iter()
        .position(|(key, _)| key.eq_ignore_ascii_case(&current))
        .or_else(|| choices.iter().position(|(key, _)| is_available(&config, key)))
        .unwrap_or(0);

    let install = mode == Mode::Install;
    let mut buttons = vec![(BUTTON_SAVE, if install { messages::BUTTON_INSTALL } else { messages::BUTTON_SAVE })];
    if !install {
        buttons.push((BUTTON_UNINSTALL, messages::BUTTON_UNINSTALL));
    }
    let answer = show(&Dialog {
        instruction: if install { messages::SETUP_INSTRUCTION } else { messages::SETTINGS_INSTRUCTION },
        content: messages::SETUP_CONTENT,
        radios: choices.iter().map(|(_, label)| label.clone()).collect(),
        preselected,
        verification: Some(messages::SETUP_ASSOCIATE),
        verified: install || system::is_associated(),
        buttons,
        common: Common::Cancel,
        footer: install.then_some(messages::SETUP_FOOTER),
    })?;

    match answer.button {
        BUTTON_SAVE => {
            let (key, label) = &choices[answer.radio.min(choices.len() - 1)];
            config.open_with.clone_from(key);
            config::save(&config_path, &config).map_err(|why| messages::config_invalid(&config_path, &why))?;
            if install {
                system::install(embedded_cli).map_err(|e| messages::install_failed(&e.to_string()))?;
            }
            let registered = if answer.verified {
                system::associate(crate::cli::editor_exe(&config, &config_path).as_deref())
            } else {
                system::remove_association()
            };
            registered.map_err(|e| messages::install_failed(&e.to_string()))?;
            if install {
                notice(&messages::setup_done(label, answer.verified));
            }
            Ok(())
        }
        BUTTON_UNINSTALL if confirm(messages::UNINSTALL_CONFIRM)? => uninstall(),
        _ => Ok(()),
    }
}

/// Removes the program and says so; Settings › Apps runs this directly.
pub(crate) fn uninstall() -> Result<(), String> {
    system::uninstall().map_err(|e| messages::uninstall_failed(&e.to_string()))?;
    notice(&messages::uninstalled(&system::data_dir()));
    Ok(())
}

fn is_available(config: &Config, key: &str) -> bool {
    key == "default" || config.programs.get(key).is_some_and(|program| system::find_program(program).is_some())
}

fn notice(text: &str) {
    let _ = show(&Dialog { content: text, common: Common::Ok, ..Dialog::default() });
}

fn confirm(text: &str) -> Result<bool, String> {
    const IDYES: i32 = 6;
    Ok(show(&Dialog { content: text, common: Common::YesNo, ..Dialog::default() })?.button == IDYES)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Common {
    #[default]
    Ok,
    Cancel,
    YesNo,
}

#[derive(Debug, Default)]
struct Dialog<'a> {
    instruction: &'a str,
    content: &'a str,
    radios: Vec<String>,
    preselected: usize,
    verification: Option<&'a str>,
    verified: bool,
    /// Id and text of each button besides the common ones.
    buttons: Vec<(i32, &'a str)>,
    common: Common,
    footer: Option<&'a str>,
}

struct Answer {
    /// One of [`Dialog::buttons`], or a common button's `ID…` value.
    button: i32,
    /// Index into [`Dialog::radios`].
    radio: usize,
    verified: bool,
}

#[allow(unsafe_code)]
fn show(d: &Dialog) -> Result<Answer, String> {
    use windows_sys::Win32::UI::Controls::{
        TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TDCBF_CANCEL_BUTTON, TDCBF_NO_BUTTON, TDCBF_OK_BUTTON, TDCBF_YES_BUTTON,
        TDF_ALLOW_DIALOG_CANCELLATION, TDF_VERIFICATION_FLAG_CHECKED, TaskDialogIndirect,
    };
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    fn ptr(s: Option<&Vec<u16>>) -> *const u16 {
        s.map_or(std::ptr::null(), |s| s.as_ptr())
    }
    fn count(n: usize) -> u32 {
        u32::try_from(n).unwrap_or(0)
    }
    // Every buffer below must outlive the call that reads it through raw pointers.
    let title = wide(messages::SETUP_TITLE);
    let instruction = (!d.instruction.is_empty()).then(|| wide(d.instruction));
    let content = wide(d.content);
    let verification = d.verification.map(wide);
    let footer = d.footer.map(wide);
    let radio_texts: Vec<Vec<u16>> = d.radios.iter().map(|r| wide(r)).collect();
    let radios: Vec<TASKDIALOG_BUTTON> = (FIRST_RADIO..)
        .zip(&radio_texts)
        .map(|(id, text)| TASKDIALOG_BUTTON { nButtonID: id, pszButtonText: text.as_ptr() })
        .collect();
    let button_texts: Vec<Vec<u16>> = d.buttons.iter().map(|(_, text)| wide(text)).collect();
    let buttons: Vec<TASKDIALOG_BUTTON> = d
        .buttons
        .iter()
        .zip(&button_texts)
        .map(|((id, _), text)| TASKDIALOG_BUTTON { nButtonID: *id, pszButtonText: text.as_ptr() })
        .collect();

    let config = TASKDIALOGCONFIG {
        cbSize: count(std::mem::size_of::<TASKDIALOGCONFIG>()),
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | (if d.verified { TDF_VERIFICATION_FLAG_CHECKED } else { 0 }),
        dwCommonButtons: match d.common {
            Common::Ok => TDCBF_OK_BUTTON,
            Common::Cancel => TDCBF_CANCEL_BUTTON,
            Common::YesNo => TDCBF_YES_BUTTON | TDCBF_NO_BUTTON,
        },
        pszWindowTitle: title.as_ptr(),
        pszMainInstruction: ptr(instruction.as_ref()),
        pszContent: content.as_ptr(),
        cButtons: count(buttons.len()),
        pButtons: if buttons.is_empty() { std::ptr::null() } else { buttons.as_ptr() },
        nDefaultButton: buttons.first().map_or(0, |b| b.nButtonID),
        cRadioButtons: count(radios.len()),
        pRadioButtons: if radios.is_empty() { std::ptr::null() } else { radios.as_ptr() },
        nDefaultRadioButton: FIRST_RADIO + i32::try_from(d.preselected).unwrap_or(0),
        pszVerificationText: ptr(verification.as_ref()),
        pszFooter: ptr(footer.as_ref()),
        ..TASKDIALOGCONFIG::default()
    };

    let (mut button, mut radio, mut verified) = (0i32, 0i32, 0);
    // SAFETY: `config` points only at the buffers above, which live until the end of this
    // function; the three out-pointers are valid for writes.
    let result = unsafe { TaskDialogIndirect(&config, &mut button, &mut radio, &mut verified) };
    if result < 0 {
        return Err(format!("TaskDialogIndirect: {result:#010x}"));
    }
    Ok(Answer { button, radio: usize::try_from(radio - FIRST_RADIO).unwrap_or(0), verified: verified != 0 })
}
