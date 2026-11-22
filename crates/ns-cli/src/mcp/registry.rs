//! The server's open sessions, by number: what the reader of the standard
//! input routes a tool call by, and what a refusal names. Each session's
//! worker publishes its state here at every change, so that routing and
//! the refusals never ask a worker that may be waiting for its program.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use super::worker::Input;

/// What a session's program does, as its worker last published.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionState {
    /// Its `debug_start` has not answered yet.
    Starting,
    Running,
    Stopped,
    /// Its program ended; the session stays open until `debug_stop`.
    Ended,
}

impl SessionState {
    /// The state as a refusal names it.
    pub(crate) fn word(self) -> &'static str {
        match self {
            SessionState::Starting => "starting",
            SessionState::Running => "running",
            SessionState::Stopped => "stopped",
            SessionState::Ended => "ended",
        }
    }
}

/// An open session as the registry holds it: its number, its label -- the
/// program's name -- its published state, and the inbox of its worker.
pub(crate) struct SessionHandle {
    pub(crate) number: u32,
    pub(crate) label: String,
    pub(crate) state: Mutex<SessionState>,
    pub(crate) inbox: Sender<Input>,
}

impl SessionHandle {
    /// Publishes `state`.
    pub(crate) fn publish(&self, state: SessionState) {
        *self.state.lock().expect("a session's state") = state;
    }

    fn description(&self) -> String {
        let state = *self.state.lock().expect("a session's state");
        format!("session {} ({}, {})", self.number, self.label, state.word())
    }
}

/// The open sessions.
pub(crate) struct Registry {
    sessions: Mutex<Sessions>,
}

struct Sessions {
    /// The number the next session takes: numbers count from 1 and are
    /// never given twice in the server's life, so that a number a client
    /// still holds never names another program.
    next_number: u32,
    open: BTreeMap<u32, Arc<SessionHandle>>,
}

impl Registry {
    pub(crate) fn new() -> Registry {
        Registry {
            sessions: Mutex::new(Sessions {
                next_number: 1,
                open: BTreeMap::new(),
            }),
        }
    }

    /// Registers a session of `label` as starting: its number, the inbox
    /// its worker reads and its handle.
    pub(crate) fn open(&self, label: String) -> (u32, Receiver<Input>, Arc<SessionHandle>) {
        let (inbox, receiver) = mpsc::channel();
        let mut sessions = self.sessions.lock().expect("the sessions");
        let number = sessions.next_number;
        sessions.next_number += 1;
        let handle = Arc::new(SessionHandle {
            number,
            label,
            state: Mutex::new(SessionState::Starting),
            inbox,
        });
        sessions.open.insert(number, Arc::clone(&handle));
        (number, receiver, handle)
    }

    /// The session `asked` names, or the one open when none is asked; or
    /// why there is none, naming the open sessions.
    pub(crate) fn find(&self, asked: Option<u32>) -> Result<Arc<SessionHandle>, String> {
        let sessions = self.sessions.lock().expect("the sessions");
        let open = || {
            sessions
                .open
                .values()
                .map(|handle| handle.description())
                .collect::<Vec<_>>()
                .join(", ")
        };
        match asked {
            Some(number) => match sessions.open.get(&number) {
                Some(handle) => Ok(Arc::clone(handle)),
                None if sessions.open.is_empty() => Err(format!(
                    "no session {number} is open; debug_start starts one"
                )),
                None => Err(format!("no session {number} is open: {}", open())),
            },
            None => match sessions.open.len() {
                0 => Err("no program is being debugged; debug_start starts one".to_string()),
                1 => Ok(Arc::clone(
                    sessions.open.values().next().expect("one open session"),
                )),
                count => Err(format!(
                    "{count} sessions are open: {} — pass session",
                    open()
                )),
            },
        }
    }

    /// Removes session `number`; a later call naming it is refused.
    pub(crate) fn close(&self, number: u32) {
        self.sessions
            .lock()
            .expect("the sessions")
            .open
            .remove(&number);
    }

    /// Tells every open session's worker that the client cancelled its
    /// request `identifier`: whichever serves or holds it lets it go.
    pub(crate) fn cancel(&self, identifier: &serde_json::Value) {
        let sessions = self.sessions.lock().expect("the sessions");
        for handle in sessions.open.values() {
            // A worker that already ended holds no request.
            let _ = handle.inbox.send(Input::Cancelled(identifier.clone()));
        }
    }

    /// Asks every open session's worker to end.
    pub(crate) fn end_all(&self) {
        let sessions = self.sessions.lock().expect("the sessions");
        for handle in sessions.open.values() {
            // A worker that already ended has nothing to end.
            let _ = handle.inbox.send(Input::End);
        }
    }
}

/// The label of the program `program` names, as `debug_start` was given
/// it: a `.ns` file's name, or the name of a project's directory, given as
/// the directory or as its `newspeak.json`.
pub(crate) fn label_of(program: &str) -> String {
    let path = std::path::absolute(program).unwrap_or_else(|_| Path::new(program).to_path_buf());
    let name = |path: &Path| {
        path.components()
            .next_back()
            .map(|component| component.as_os_str().to_string_lossy().to_string())
            .unwrap_or_else(|| program.to_string())
    };
    if path.file_name().is_some_and(|file| file == "newspeak.json")
        && let Some(directory) = path.parent()
    {
        return name(directory);
    }
    name(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_names_the_file_or_the_project_s_directory() {
        let directory = std::env::temp_dir().join("Bench");
        let project = directory.join("newspeak.json");
        let file = directory.join("Bounce.ns");
        assert_eq!(label_of(&file.to_string_lossy()), "Bounce.ns");
        assert_eq!(label_of(&project.to_string_lossy()), "Bench");
        assert_eq!(label_of(&directory.to_string_lossy()), "Bench");
    }

    #[test]
    fn numbers_are_never_given_twice() {
        let registry = Registry::new();
        let (first, _first_inbox, _) = registry.open("A.ns".to_string());
        registry.close(first);
        let (second, _second_inbox, _) = registry.open("B.ns".to_string());
        assert_eq!((first, second), (1, 2));
        assert_eq!(
            registry.find(Some(1)).err().as_deref(),
            Some("no session 1 is open: session 2 (B.ns, starting)")
        );
    }
}
