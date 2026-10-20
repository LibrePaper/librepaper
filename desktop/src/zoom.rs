// The zoom level is one value for the whole app, kept in the config dir. The
// page asks for a step through lp-local, because remote pages get no Tauri IPC.

use std::sync::Mutex;

use tauri::Manager;

use crate::shell_log;

const STEPS: [f64; 13] = [
    0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
];
const FILE: &str = "zoom";
const MIN: f64 = 0.5;
const MAX: f64 = 3.0;

pub enum Step {
    In,
    Out,
    Reset,
}

// The next level for a step. A level between two steps moves to the one above or below.
pub fn next(current: f64, step: Step) -> f64 {
    match step {
        Step::In => STEPS
            .iter()
            .copied()
            .find(|level| *level > current + 1e-6)
            .unwrap_or(STEPS[STEPS.len() - 1]),
        Step::Out => STEPS
            .iter()
            .rev()
            .copied()
            .find(|level| *level < current - 1e-6)
            .unwrap_or(STEPS[0]),
        Step::Reset => 1.0,
    }
}

pub struct Zoom(Mutex<f64>);

impl Zoom {
    // A missing or broken file means the default level.
    pub fn load(app: &tauri::AppHandle) -> Self {
        let level = app
            .path()
            .app_config_dir()
            .ok()
            .and_then(|dir| std::fs::read_to_string(dir.join(FILE)).ok())
            .and_then(|text| text.trim().parse::<f64>().ok())
            .filter(|level| level.is_finite())
            .map_or(1.0, |level| level.clamp(MIN, MAX));
        Self(Mutex::new(level))
    }

    pub fn get(&self) -> f64 {
        *self.0.lock().unwrap_or_else(|err| err.into_inner())
    }

    // Sets the level on every window and saves it for the next launch.
    pub fn apply(&self, app: &tauri::AppHandle, step: Step) {
        let level = {
            let mut current = self.0.lock().unwrap_or_else(|err| err.into_inner());
            let level = next(*current, step);
            *current = level;
            level
        };
        for window in app.webview_windows().values() {
            let _ = window.set_zoom(level);
            // Pages that draw at devicePixelRatio need to hear about it.
            let _ = window.eval(format!(
                "window.dispatchEvent(new CustomEvent(\"librepaper:zoom\", {{ detail: {level} }}));"
            ));
        }
        if let Err(err) = save(app, level) {
            shell_log::write(&format!("could not save the zoom level: {err}"));
        }
    }
}

fn save(app: &tauri::AppHandle, level: f64) -> Result<(), Box<dyn std::error::Error>> {
    let dir = app.path().app_config_dir()?;
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(FILE), level.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_in_and_out() {
        assert_eq!(next(1.0, Step::In), 1.1);
        assert_eq!(next(1.0, Step::Out), 0.9);
        assert_eq!(next(3.0, Step::In), 3.0);
        assert_eq!(next(0.5, Step::Out), 0.5);
        assert_eq!(next(1.2, Step::In), 1.25);
    }

    #[test]
    fn reset_goes_to_one() {
        assert_eq!(next(2.5, Step::Reset), 1.0);
    }
}
