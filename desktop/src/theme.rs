// The theme choice is one value for the whole app, kept in the config dir. The
// page sends it through lp-local, because remote pages get no Tauri IPC, and
// the shell uses it to color the native title bar to match the page.

use std::str::FromStr;
use std::sync::Mutex;

#[cfg(target_os = "linux")]
use gtk::gio::{self, prelude::DBusProxyExt};
#[cfg(target_os = "linux")]
use gtk::glib::{self, prelude::ObjectExt, prelude::ToVariant};
#[cfg(target_os = "linux")]
use gtk::prelude::GtkSettingsExt;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::webview::Color;
use tauri::Manager;

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
    // The desktop's GTK preference, read once at startup. It is the fallback
    // when the portal gives no color scheme. Once the shell overrides the GTK
    // setting, GTK no longer reports the operating system's preference.
    #[cfg(target_os = "linux")]
    gtk_dark: bool,
    // The operating system's preference, kept current by watch_system.
    #[cfg(target_os = "linux")]
    system_dark: AtomicBool,
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
        #[cfg(target_os = "linux")]
        let (gtk_dark, system_dark) = {
            let gtk_dark = gtk::Settings::default()
                .map(|settings| settings.is_gtk_application_prefer_dark_theme())
                .unwrap_or(false);
            let system_dark = match portal_scheme().and_then(scheme_dark) {
                Some(dark) => dark,
                None => {
                    shell_log::write("the desktop portal gave no color scheme; following GTK");
                    gtk_dark
                }
            };
            (gtk_dark, system_dark)
        };
        Self {
            choice: Mutex::new(choice),
            #[cfg(target_os = "linux")]
            gtk_dark,
            #[cfg(target_os = "linux")]
            system_dark: AtomicBool::new(system_dark),
        }
    }

    pub fn choice(&self) -> Choice {
        *self.choice.lock().unwrap_or_else(|err| err.into_inner())
    }

    // The theme to hand to a window. On Windows and macOS no theme means the
    // window follows the operating system. On Linux tao implements no theme as
    // "prefer dark is false", which would force light on a dark desktop after a
    // switch from dark back to system. So Linux hands over the live portal
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
        Some(if self.system_dark.load(Ordering::Relaxed) {
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
    // theme of its own yet. It is read from the operating system directly, so
    // it is right before any window exists.
    pub fn system_dark(&self, app: &tauri::AppHandle) -> bool {
        #[cfg(target_os = "linux")]
        {
            let _ = app;
            self.system_dark.load(Ordering::Relaxed)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = app;
            os_dark()
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

// Follows the operating system's preference for as long as the app runs. Only
// Linux needs a watch: Windows and macOS are read directly when asked.
#[cfg(target_os = "linux")]
pub fn watch_system(app: &tauri::AppHandle) {
    let Some(proxy) = portal_proxy() else {
        return;
    };
    let handle = app.clone();
    proxy.connect_local("g-signal", false, move |args| {
        let signal = args.get(2)?.get::<String>().ok()?;
        if signal != "SettingChanged" {
            return None;
        }
        let params = args.get(3)?.get::<glib::Variant>().ok()?;
        let namespace = params.child_value(0).get::<String>();
        let key = params.child_value(1).get::<String>();
        if namespace.as_deref() != Some("org.freedesktop.appearance")
            || key.as_deref() != Some("color-scheme")
        {
            return None;
        }
        let theme = handle.state::<Theme>();
        let dark = unbox_u32(&params.child_value(2))
            .and_then(scheme_dark)
            .unwrap_or(theme.gtk_dark);
        theme.system_dark.store(dark, Ordering::Relaxed);
        if theme.choice() == Choice::System {
            for window in handle.webview_windows().values() {
                let _ = window.set_theme(theme.tauri_theme(Choice::System));
            }
        }
        None
    });
    // The proxy must live as long as the app, or its signal handler goes with it.
    std::mem::forget(proxy);
}

#[cfg(not(target_os = "linux"))]
pub fn watch_system(_app: &tauri::AppHandle) {}

// Maps the portal's color-scheme value to dark. Zero means no preference.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn scheme_dark(value: u32) -> Option<bool> {
    match value {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn portal_proxy() -> Option<gio::DBusProxy> {
    gio::DBusProxy::for_bus_sync(
        gio::BusType::Session,
        gio::DBusProxyFlags::NONE,
        None,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Settings",
        gio::Cancellable::NONE,
    )
    .ok()
}

// Reads the raw color-scheme value. Older portals only have Read, which wraps
// the value in one more variant.
#[cfg(target_os = "linux")]
fn portal_scheme() -> Option<u32> {
    let proxy = portal_proxy()?;
    let args = ("org.freedesktop.appearance", "color-scheme").to_variant();
    let reply = proxy
        .call_sync(
            "ReadOne",
            Some(&args),
            gio::DBusCallFlags::NONE,
            1000,
            gio::Cancellable::NONE,
        )
        .or_else(|_| {
            proxy.call_sync(
                "Read",
                Some(&args),
                gio::DBusCallFlags::NONE,
                1000,
                gio::Cancellable::NONE,
            )
        })
        .ok()?;
    unbox_u32(&reply.child_value(0))
}

// Strips every variant layer and returns the u32 inside.
#[cfg(target_os = "linux")]
fn unbox_u32(value: &glib::Variant) -> Option<u32> {
    let mut value = value.clone();
    while let Some(inner) = value.as_variant() {
        value = inner;
    }
    value.get::<u32>()
}

#[cfg(target_os = "windows")]
fn os_dark() -> bool {
    // Zero in AppsUseLightTheme means the apps use the dark theme.
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|key| key.get_value::<u32, _>("AppsUseLightTheme"))
        .map(|light| light == 0)
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn os_dark() -> bool {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    let key = objc2_foundation::NSString::from_str("AppleInterfaceStyle");
    let dark = objc2_foundation::NSString::from_str("Dark");
    // The key is absent in light mode, so a nil value means light.
    unsafe {
        let defaults: *mut AnyObject =
            msg_send![objc2::class!(NSUserDefaults), standardUserDefaults];
        let value: *mut AnyObject = msg_send![defaults, stringForKey: &*key];
        if value.is_null() {
            return false;
        }
        let is_dark: bool = msg_send![value, isEqualToString: &*dark];
        is_dark
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn os_dark() -> bool {
    false
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

    #[test]
    fn portal_scheme_values_map_to_dark() {
        assert_eq!(scheme_dark(1), Some(true));
        assert_eq!(scheme_dark(2), Some(false));
        assert_eq!(scheme_dark(0), None);
        assert_eq!(scheme_dark(7), None);
    }
}
