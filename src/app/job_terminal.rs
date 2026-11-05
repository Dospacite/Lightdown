//! PTY jobs keep a bounded diagnostic stream separate from terminal scrollback.
use super::*;
use crate::term::JobState;
pub(super) struct Job {
    pub state: Arc<std::sync::Mutex<JobState>>,
    pub pid: i32,
    /// The terminal showing this job, by its stable id.
    pub term: u64,
}
impl Job {
    pub fn start(app: &mut App, name: &str, command: std::process::Command) -> std::io::Result<Self> {
        let state = Arc::new(std::sync::Mutex::new(JobState::default()));
        let mut term = crate::term::Term::new_in(&app.root, &app.root, app.fonts.clone(), app.waker.clone());
        term.start_job(name, command, state.clone())?;
        let pid = term.job_pid().unwrap();
        let id = term.agent_id;
        app.terms.push(term);
        app.show_in_pane(app.terms.len() - 1);
        app.show_term(true);
        Ok(Self { state, pid, term: id })
    }
    /// The output and exit status, once the job has ended.
    pub fn poll(&self) -> Option<(String, Option<i32>)> {
        let state = self.state.lock().unwrap();
        state.code.map(|code| (state.text.clone(), code))
    }
    /// Whether `pattern` appears in output written since `seen` bytes.
    pub fn seen(&self, pattern: &regex::Regex, seen: &mut u64) -> bool {
        let state = self.state.lock().unwrap();
        let new = (state.total - *seen) as usize;
        *seen = state.total;
        // Look back a little so a line split across reads still matches.
        let mut from = state.text.len().saturating_sub(new + 1024);
        while !state.text.is_char_boundary(from) {
            from += 1;
        }
        plain_text(&state.text[from..]).lines().any(|l| pattern.is_match(l))
    }
    pub fn stop(self) {
        // SAFETY: this is the session process group created for this job.
        unsafe { libc::kill(-self.pid, libc::SIGKILL); }
    }
}
fn plain_text(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}
