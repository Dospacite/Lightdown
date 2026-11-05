//! Spell checking with hunspell, which runs on another thread in its pipe
//! mode. Words are asked about as they come into view, and what hunspell
//! says of each is kept.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

use crate::kernel::Waker;

/// What hunspell said of a word: nothing if it is spelt right, or else
/// what it may have been meant to be.
pub type Verdict = Option<Vec<String>>;

enum Ask {
    Check(Vec<String>),
    /// Takes a word into the personal dictionary.
    Add(String),
}

#[derive(Default)]
struct Shared {
    said: Vec<(String, Verdict)>,
    /// hunspell would not start, or had no such dictionary.
    failed: bool,
}

pub struct Speller {
    /// The dictionary in use, as hunspell names it.
    pub dict: String,
    tx: Sender<Ask>,
    shared: Arc<Mutex<Shared>>,
    verdicts: HashMap<String, Verdict>,
    asked: HashSet<String>,
}

/// The dictionary to use: the one set, or else the one of the language
/// the session is in, or else American English.
pub fn dictionary(set: &str) -> String {
    if !set.trim().is_empty() {
        return set.trim().to_string();
    }
    let lang = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()));
    let lang = lang.unwrap_or_default();
    let lang = lang.split(['.', '@']).next().unwrap_or("");
    if lang.is_empty() || lang == "C" || lang == "POSIX" { "en_US".into() } else { lang.into() }
}

/// What a result line of hunspell's pipe mode says of a word.
fn verdict(line: &str) -> Verdict {
    match line.chars().next() {
        Some('&') => {
            let list = line.split_once(": ").map_or("", |(_, s)| s);
            Some(list.split(", ").map(str::to_string).filter(|s| !s.is_empty()).collect())
        }
        Some('#') => Some(Vec::new()),
        _ => None,
    }
}

impl Speller {
    pub fn new(dict: &str, waker: Waker) -> Speller {
        let (tx, rx) = channel::<Ask>();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (theirs, dict_name) = (shared.clone(), dict.to_string());
        std::thread::spawn(move || {
            let fail = || {
                theirs.lock().unwrap().failed = true;
                waker();
            };
            let mut command = Command::new("hunspell");
            // Tests must not read or add words to the user's personal dictionary.
            if cfg!(test) {
                let personal = std::path::Path::new(&dict_name)
                    .parent()
                    .filter(|p| p.is_dir() && p.is_absolute())
                    .map(|p| p.join("personal.pws"))
                    .unwrap_or_else(|| crate::config::data_dir().join("personal.pws"));
                if let Some(parent) = personal.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                command.arg("--no-default-personal").arg("-p").arg(personal);
            }
            let child = command
                .args(["-a", "-i", "utf-8", "-d", &dict_name])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            let Ok(mut child) = child else { return fail() };
            let (Some(mut input), Some(output)) = (child.stdin.take(), child.stdout.take()) else { return fail() };
            let mut output = BufReader::new(output);
            let mut line = String::new();
            // The banner first; without a dictionary hunspell says nothing.
            if output.read_line(&mut line).unwrap_or(0) == 0 {
                let _ = child.wait();
                return fail();
            }
            // Terse mode: right words give no line of their own.
            let _ = input.write_all(b"!\n");
            for ask in rx {
                match ask {
                    Ask::Add(word) => {
                        let _ = input.write_all(format!("*{word}\n#\n").as_bytes());
                        let _ = input.flush();
                    }
                    Ask::Check(words) => {
                        let mut said = Vec::new();
                        for word in words {
                            // `^` keeps a word from being read as a command.
                            if input.write_all(format!("^{word}\n").as_bytes()).and_then(|_| input.flush()).is_err() {
                                return fail();
                            }
                            let mut answer = None;
                            loop {
                                line.clear();
                                if output.read_line(&mut line).unwrap_or(0) == 0 {
                                    return fail();
                                }
                                let l = line.trim_end();
                                if l.is_empty() {
                                    break;
                                }
                                answer = answer.or(verdict(l));
                            }
                            said.push((word, answer));
                        }
                        theirs.lock().unwrap().said.extend(said);
                        waker();
                    }
                }
            }
            let _ = child.kill();
        });
        Speller { dict: dict.to_string(), tx, shared, verdicts: HashMap::new(), asked: HashSet::new() }
    }

    /// Whether hunspell could not be used.
    pub fn failed(&self) -> bool {
        self.shared.lock().unwrap().failed
    }

    /// Takes in what hunspell has said. Returns whether anything came.
    pub fn poll(&mut self) -> bool {
        let said = std::mem::take(&mut self.shared.lock().unwrap().said);
        let any = !said.is_empty();
        self.verdicts.extend(said);
        any
    }

    /// What is known of a word, asking about it if nothing is yet.
    pub fn check(&mut self, word: &str) -> Option<&Verdict> {
        if !self.verdicts.contains_key(word) && self.asked.insert(word.to_string()) {
            let _ = self.tx.send(Ask::Check(vec![word.to_string()]));
        }
        self.verdicts.get(word)
    }

    /// Whether a word is known to be misspelt.
    #[cfg(test)]
    pub fn wrong(&self, word: &str) -> bool {
        matches!(self.verdicts.get(word), Some(Some(_)))
    }

    pub fn suggestions(&self, word: &str) -> Vec<String> {
        self.verdicts.get(word).cloned().flatten().unwrap_or_default()
    }

    /// Takes a word as spelt right from now on, here and in the personal
    /// dictionary.
    pub fn add(&mut self, word: &str) {
        self.verdicts.insert(word.to_string(), None);
        let _ = self.tx.send(Ask::Add(word.to_string()));
    }

    /// Waits for hunspell to answer what was asked. For tests and
    /// headless rendering.
    #[cfg(any(test, feature = "dump"))]
    pub fn settle(&mut self) {
        let start = std::time::Instant::now();
        while self.asked.iter().any(|w| !self.verdicts.contains_key(w))
            && !self.failed()
            && start.elapsed().as_secs() < 5
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.poll();
        }
    }
}

/// The words of a stretch of prose worth checking, by where they are:
/// runs of letters, with apostrophes inside them. Tokens that look like
/// addresses, paths, code or numbers are left alone, as are words in
/// capitals and words with capitals inside them.
pub fn words(text: &str, base: usize, out: &mut Vec<(usize, usize)>) {
    let mut at = 0;
    for token in text.split_inclusive(char::is_whitespace) {
        let start = at;
        at += token.len();
        let token = token.trim_end();
        if token.contains("://") || token.contains(['@', '/', '\\', '_', '=', '<', '>', '{', '}']) {
            continue;
        }
        if token.chars().any(|c| c.is_ascii_digit()) {
            continue;
        }
        let mut i = 0;
        let chars: Vec<(usize, char)> = token.char_indices().collect();
        while i < chars.len() {
            if !chars[i].1.is_alphabetic() {
                i += 1;
                continue;
            }
            let mut j = i;
            while j < chars.len()
                && (chars[j].1.is_alphabetic()
                    || (matches!(chars[j].1, '\'' | '\u{2019}')
                        && chars.get(j + 1).is_some_and(|c| c.1.is_alphabetic())))
            {
                j += 1;
            }
            let (a, b) = (chars[i].0, chars.get(j).map_or(token.len(), |c| c.0));
            let word = &token[a..b];
            let letters = word.chars().filter(|c| c.is_alphabetic()).count();
            let inner_capital = word.chars().skip(1).any(char::is_uppercase);
            if letters >= 2 && !inner_capital {
                out.push((base + start + a, base + start + b));
            }
            i = j;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(text: &str) -> Vec<&str> {
        let mut out = Vec::new();
        words(text, 0, &mut out);
        out.iter().map(|&(a, b)| &text[a..b]).collect()
    }

    #[test]
    fn words_are_picked_from_prose() {
        assert_eq!(found("It's a dog's life, isn't it?"), ["It's", "dog's", "life", "isn't", "it"]);
        assert_eq!(found("see https://x.org and a@b.c or src/main.rs"), ["see", "and", "or"]);
        assert_eq!(found("NASA and iPhone and x86 and naïve"), ["and", "and", "and", "naïve"]);
        assert_eq!(found("“quoted” (brackets)"), ["quoted", "brackets"]);
    }

    #[test]
    fn hunspell_lines_are_read() {
        assert_eq!(verdict("& helo 3 0: hello, help, hole"), Some(vec!["hello".into(), "help".into(), "hole".into()]));
        assert_eq!(verdict("# zxqv 0"), Some(vec![]));
        assert_eq!(verdict("*"), None);
        assert_eq!(dictionary(" en_GB "), "en_GB");
    }

    /// With a small dictionary of its own, hunspell answers.
    #[test]
    fn hunspell_is_asked() {
        let dir = std::env::temp_dir().join(format!("ld-spell-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tiny.aff"), "SET UTF-8\nTRY esianrtolcdugmphbyfvkwz\n").unwrap();
        std::fs::write(dir.join("tiny.dic"), "3\nhello\nworld\nhelp\n").unwrap();
        let mut s = Speller::new(&dir.join("tiny").to_string_lossy(), Arc::new(|| {}));
        s.check("hello");
        s.check("helo");
        s.settle();
        if s.failed() {
            // No hunspell here.
            return;
        }
        assert!(!s.wrong("hello") && s.wrong("helo"));
        assert!(s.suggestions("helo").contains(&"hello".to_string()));
        s.add("helo");
        assert!(!s.wrong("helo"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_dictionary_fails_quietly() {
        let mut s = Speller::new("xx_NOWHERE", Arc::new(|| {}));
        s.check("word");
        s.settle();
        assert!(s.failed() || !s.wrong("word"));
    }
}
