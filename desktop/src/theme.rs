// The theme choice is one value for the whole app, kept in the config dir. The
// page sends it through lp-local, because remote pages get no Tauri IPC, and
// the shell uses it to color the native title bar to match the page.

use std::str::FromStr;
use std::sync::Mutex;

use tauri::Manager;
use tauri::webview::Color;

use crate::shell_log;

const FILE: &str = "theme";

// Radix sage step 1 in each scheme, so a new window shows the page's ground
// color before the page paints instead of flashing white.
const DARK_GROUND: Color = Color(16, 18, 17, 255);
const LIGHT_GROUND: Color = Color(249, 250, 249, 255);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    System,
    Light,
    Dark,
}

impl Choice {
    pub fn as_str(self) -> &'static str {
        match self {
            Choice::System => "system",
            Choice::Light => "light",
            Choice::Dark => "dark",
        }
    }
}

impl FromStr for Choice {
    type Err = ();

    fn from_str(word: &str) -> Result<Self, ()> {
        match word {
            "system" => Ok(Choice::System),
            "light" => Ok(Choice::Light),
            "dark" => Ok(Choice::Dark),
            _ => Err(()),
        }
    }
}

pub struct Theme {
    choice: Mutex<Choice>,
    // The desktop's own preference, read before any choice is applied. It is
    // the only record of it once the shell has overridden the setting.
    #[cfg(target_os = "linux")]
    gtk_dark: bool,
}

impl Theme {
    // A missing or broken file means following the system.
    pub fn load(app: &tauri::AppHandle) -> Self {
        let choice = app
            .path()
            .app_config_dir()
            .ok()
            .and_then(|dir| std::fs::read_to_string(dir.join(FILE)).ok())
            .and_then(|text| text.trim().parse::<Choice>().ok())
            .unwrap_or(Choice::System);
        Self {
            choice: Mutex::new(choice),
            #[cfg(target_os = "linux")]
            gtk_dark: gtk::Settings::default()
                .map(|settings| settings.is_gtk_application_prefer_dark_theme())
                .unwrap_or(false),
        }
    }

    pub fn choice(&self) -> Choice {
        *self.choice.lock().unwrap_or_else(|err| err.into_inner())
    }

    // The theme to hand to a window. On Windows and macOS no theme means the
    // window follows the operating system. On Linux tao implements no theme as
    // "prefer dark is false", which would force light on a dark desktop after a
    // switch from dark back to system. So Linux hands over the captured
    // preference instead.
    pub fn tauri_theme(&self, choice: Choice) -> Option<tauri::Theme> {
        match choice {
            Choice::Light => Some(tauri::Theme::Light),
            Choice::Dark => Some(tauri::Theme::Dark),
            Choice::System => self.system_theme(),
        }
    }

    #[cfg(target_os = "linux")]
    fn system_theme(&self) -> Option<tauri::Theme> {
        Some(if self.gtk_dark {
            tauri::Theme::Dark
        } else {
            tauri::Theme::Light
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn system_theme(&self) -> Option<tauri::Theme> {
        None
    }

    // Whether the operating system prefers dark, for a window that has no
    // theme of its own yet. An existing window already knows it.
    pub fn system_dark(&self, app: &tauri::AppHandle) -> bool {
        #[cfg(target_os = "linux")]
        {
            let _ = app;
            self.gtk_dark
        }
        #[cfg(not(target_os = "linux"))]
        {
            app.webview_windows()
                .values()
                .next()
                .and_then(|window| window.theme().ok())
                .is_some_and(|theme| theme == tauri::Theme::Dark)
        }
    }

    // Sets the choice on every window and saves it for the next launch.
    pub fn set(&self, app: &tauri::AppHandle, choice: Choice) {
        *self.choice.lock().unwrap_or_else(|err| err.into_inner()) = choice;
        let theme = self.tauri_theme(choice);
        for window in app.webview_windows().values() {
            let _ = window.set_theme(theme);
        }
        if let Err(err) = save(app, choice) {
            shell_log::write(&format!("could not save the theme: {err}"));
        }
    }
}

// The window ground for a choice. The system choice takes the operating
// system's preference.
pub fn background(choice: Choice, system_dark: bool) -> Color {
    let dark = match choice {
        Choice::Dark => true,
        Choice::Light => false,
        Choice::System => system_dark,
    };
    if dark {
        DARK_GROUND
    } else {
        LIGHT_GROUND
    }
}

fn save(app: &tauri::AppHandle, choice: Choice) -> Result<(), Box<dyn std::error::Error>> {
    let dir = app.path().app_config_dir()?;
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(FILE), choice.as_str())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_round_trip() {
        for choice in [Choice::System, Choice::Light, Choice::Dark] {
            assert_eq!(choice.as_str().parse::<Choice>(), Ok(choice));
        }
    }

    #[test]
    fn unknown_words_do_not_parse() {
        assert!("".parse::<Choice>().is_err());
        assert!("Dark".parse::<Choice>().is_err());
        assert!("auto".parse::<Choice>().is_err());
    }

    #[test]
    fn system_takes_the_operating_system() {
        assert_eq!(background(Choice::System, true), DARK_GROUND);
        assert_eq!(background(Choice::System, false), LIGHT_GROUND);
        assert_eq!(background(Choice::Light, true), LIGHT_GROUND);
        assert_eq!(background(Choice::Dark, false), DARK_GROUND);
    }
}
