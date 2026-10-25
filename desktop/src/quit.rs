// Quitting asks every window first. The app exits by ending every window, so a
// local project open in a window the person did not quit from would lose the
// folder write it has pending. A round sends each page `librepaper:quit-requested`
// with the round's id. The page settles its folder and answers through
// `/shell/quit-ready`. The app exits when every window has answered ok or the
// deadline has passed. A window that answers not ok ends the round at once,
// the app stays open, and every window is sent `librepaper:quit-cancelled` so
// it can say so.

use std::collections::BTreeSet;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::shell_log;

// How long the windows have to settle before the app exits without them.
const DEADLINE: Duration = Duration::from_secs(10);

// Sent to a window. A page that settles for itself cancels the event, so a
// page that does not (the landing page, Settings) is answered ok here. The
// base URL comes from the page's own init script.
const ASK_SCRIPT: &str = r#"(() => {
  const shell = window.__LIBREPAPER_DESKTOP__;
  if (!shell) return;
  const id = {id};
  const event = new CustomEvent("librepaper:quit-requested", { detail: { id }, cancelable: true });
  if (window.dispatchEvent(event)) {
    fetch(shell.base + "shell/quit-ready", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ id, ok: true }),
    }).catch(() => {});
  }
})()"#;

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Wait,
    Exit,
    Stay,
}

// One quit request: the windows asked and the ones still to answer.
struct Round {
    id: u64,
    asked: BTreeSet<String>,
    waiting: BTreeSet<String>,
    refused: bool,
}

impl Round {
    fn new(id: u64, windows: impl IntoIterator<Item = String>) -> Self {
        let asked: BTreeSet<String> = windows.into_iter().collect();
        Self {
            id,
            waiting: asked.clone(),
            asked,
            refused: false,
        }
    }

    // Takes a window's answer. Only the first answer of a window counts, and
    // a window that was not asked has none.
    fn answer(&mut self, window: &str, ok: bool) -> Decision {
        if self.waiting.remove(window) && !ok {
            self.refused = true;
        }
        self.decision()
    }

    // Any window that could not save keeps the app open, even while others
    // are still working. Otherwise the app waits for all of them.
    fn decision(&self) -> Decision {
        if self.refused {
            Decision::Stay
        } else if self.waiting.is_empty() {
            Decision::Exit
        } else {
            Decision::Wait
        }
    }

    // The deadline passed. A window that never answered does not hold the
    // app open.
    fn expire(&self) -> Decision {
        if self.refused {
            Decision::Stay
        } else {
            Decision::Exit
        }
    }
}

// A round that has ended: how, and the windows that were asked.
struct Closed {
    id: u64,
    decision: Decision,
    asked: BTreeSet<String>,
}

#[derive(Default)]
struct State {
    last: u64,
    round: Option<Round>,
}

impl State {
    // Opens a round over `windows` and returns its id, or None while another
    // round is open.
    fn open(&mut self, windows: Vec<String>) -> Option<u64> {
        if self.round.is_some() {
            return None;
        }
        self.last += 1;
        self.round = Some(Round::new(self.last, windows));
        Some(self.last)
    }

    // An answer to a round that has ended, or to an older one, changes nothing.
    fn answer(&mut self, id: u64, window: &str, ok: bool) -> Option<Closed> {
        let round = self.round.as_mut().filter(|round| round.id == id)?;
        let decision = round.answer(window, ok);
        self.close(decision)
    }

    fn expire(&mut self, id: u64) -> Option<Closed> {
        let round = self.round.as_ref().filter(|round| round.id == id)?;
        let decision = round.expire();
        self.close(decision)
    }

    // Ends the round when the decision is final.
    fn close(&mut self, decision: Decision) -> Option<Closed> {
        if decision == Decision::Wait {
            return None;
        }
        let round = self.round.take()?;
        Some(Closed {
            id: round.id,
            decision,
            asked: round.asked,
        })
    }
}

#[derive(Default)]
pub struct Quit(Mutex<State>);

impl Quit {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|err| err.into_inner())
    }
}

// Asks every window to settle its work, and exits when they all have.
pub fn request(app: &AppHandle) {
    let windows = asked(app);
    if windows.is_empty() {
        exit(app);
        return;
    }
    let quit = app.state::<Quit>();
    let Some(id) = quit.lock().open(windows.clone()) else {
        // A round is open already, and its answers decide.
        return;
    };
    let script = ASK_SCRIPT.replace("{id}", &id.to_string());
    for label in &windows {
        let sent = app
            .get_webview_window(label)
            .is_some_and(|window| window.eval(&script).is_ok());
        if !sent {
            // A window that cannot be asked has nothing to save.
            answer(app, label, id, true);
        }
    }
    let waiting = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(DEADLINE).await;
        let quit = waiting.state::<Quit>();
        let closed = quit.lock().expire(id);
        if let Some(closed) = closed {
            finish(&waiting, closed);
        }
    });
}

// A window's answer to the request with this id.
pub fn answer(app: &AppHandle, window: &str, id: u64, ok: bool) {
    let quit = app.state::<Quit>();
    let closed = quit.lock().answer(id, window, ok);
    if let Some(closed) = closed {
        finish(app, closed);
    }
}

// The windows whose page can answer: the ones showing a trusted server. A
// window on the offline page has no unsaved project.
fn asked(app: &AppHandle) -> Vec<String> {
    app.webview_windows()
        .keys()
        .filter(|label| {
            crate::page_origin(app, label.as_str())
                .is_some_and(|origin| crate::trusted(app, &origin))
        })
        .cloned()
        .collect()
}

fn finish(app: &AppHandle, closed: Closed) {
    match closed.decision {
        Decision::Exit => exit(app),
        Decision::Stay => {
            let id = closed.id;
            let script = format!(
                "window.dispatchEvent(new CustomEvent(\"librepaper:quit-cancelled\", {{ detail: {{ id: {id} }} }}));"
            );
            for label in &closed.asked {
                if let Some(window) = app.get_webview_window(label) {
                    if let Err(err) = window.eval(&script) {
                        shell_log::write(&format!("could not tell a window the app stays: {err}"));
                    }
                }
            }
        }
        Decision::Wait => {}
    }
}

// RunEvent::Exit stops the service.
fn exit(app: &AppHandle) {
    let main = app.clone();
    if let Err(err) = app.run_on_main_thread(move || main.exit(0)) {
        shell_log::write(&format!("could not quit: {err}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(windows: &[&str]) -> Round {
        Round::new(1, windows.iter().map(|label| label.to_string()))
    }

    #[test]
    fn exits_when_every_window_answers_ok() {
        let mut round = round(&["main", "w2"]);
        assert_eq!(round.decision(), Decision::Wait);
        assert_eq!(round.answer("main", true), Decision::Wait);
        assert_eq!(round.answer("w2", true), Decision::Exit);
    }

    #[test]
    fn one_window_that_cannot_save_keeps_the_app_open() {
        let mut round = round(&["main", "w2", "w3"]);
        assert_eq!(round.answer("w2", false), Decision::Stay);
        // The others have not answered, and the answer no longer changes.
        assert_eq!(round.answer("main", true), Decision::Stay);
        assert_eq!(round.answer("w3", true), Decision::Stay);
    }

    #[test]
    fn the_deadline_exits_without_the_silent_windows() {
        let mut round = round(&["main", "w2"]);
        assert_eq!(round.answer("main", true), Decision::Wait);
        assert_eq!(round.expire(), Decision::Exit);
    }

    #[test]
    fn the_deadline_does_not_override_a_refusal() {
        let mut round = round(&["main", "w2"]);
        round.answer("w2", false);
        assert_eq!(round.expire(), Decision::Stay);
    }

    #[test]
    fn a_window_answers_once_and_only_when_asked() {
        let mut round = round(&["main", "w2"]);
        assert_eq!(round.answer("main", true), Decision::Wait);
        // A second answer from the same window changes nothing.
        assert_eq!(round.answer("main", false), Decision::Wait);
        // A window that was not asked cannot refuse.
        assert_eq!(round.answer("stranger", false), Decision::Wait);
        assert_eq!(round.answer("w2", true), Decision::Exit);
    }

    #[test]
    fn a_round_with_no_windows_exits_at_once() {
        assert_eq!(round(&[]).decision(), Decision::Exit);
    }

    #[test]
    fn a_state_closes_the_round_with_the_decision() {
        let mut state = State::default();
        let id = state
            .open(vec!["main".into(), "w2".into()])
            .expect("a round");
        assert!(state.answer(id, "main", true).is_none());
        let closed = state.answer(id, "w2", true).expect("the round ends");
        assert_eq!(closed.id, id);
        assert_eq!(closed.decision, Decision::Exit);
        assert_eq!(closed.asked.len(), 2);
        // The round is gone, so a late answer is ignored.
        assert!(state.answer(id, "w2", false).is_none());
    }

    #[test]
    fn a_refusal_ends_the_round_and_names_the_windows_to_tell() {
        let mut state = State::default();
        let id = state
            .open(vec!["main".into(), "w2".into()])
            .expect("a round");
        let closed = state.answer(id, "w2", false).expect("the round ends");
        assert_eq!(closed.decision, Decision::Stay);
        assert!(closed.asked.contains("main") && closed.asked.contains("w2"));
        // The window that was still working answers into a closed round.
        assert!(state.answer(id, "main", true).is_none());
        // The deadline of the ended round finds nothing to do.
        assert!(state.expire(id).is_none());
    }

    #[test]
    fn the_deadline_ends_an_open_round_with_an_exit() {
        let mut state = State::default();
        let id = state
            .open(vec!["main".into(), "w2".into()])
            .expect("a round");
        assert!(state.answer(id, "main", true).is_none());
        let closed = state.expire(id).expect("the round ends");
        assert_eq!(closed.decision, Decision::Exit);
        assert!(state.round.is_none());
    }

    #[test]
    fn only_one_round_is_open_and_an_old_deadline_leaves_a_new_round_alone() {
        let mut state = State::default();
        let first = state.open(vec!["main".into()]).expect("a round");
        assert!(state.open(vec!["main".into()]).is_none());
        assert_eq!(
            state
                .answer(first, "main", false)
                .map(|closed| closed.decision),
            Some(Decision::Stay)
        );
        let second = state.open(vec!["main".into()]).expect("a new round");
        assert_ne!(first, second);
        // The first round's deadline and answers carry its id, not the new one.
        assert!(state.expire(first).is_none());
        assert!(state.answer(first, "main", true).is_none());
        assert!(state.round.is_some());
    }
}
