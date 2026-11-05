//! Shell integration: hooks that have bash, zsh and fish mark their
//! prompts, commands and exit statuses (OSC 133) and say their folder
//! (OSC 7). The hooks are written to a private folder of Lightdown's own
//! and given to the shell as it starts; the user's startup files are read
//! as usual and never changed.

use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(true);

/// Turns the hooks on or off for shells started from now on.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Read by bash in place of ~/.bashrc, which it reads first.
const BASH: &str = r#"# Lightdown shell integration for bash. Generated; edits are lost.
[ -r ~/.bashrc ] && . ~/.bashrc
if [[ $- == *i* && -z $__lightdown_hooked ]]; then
    __lightdown_hooked=1
    __lightdown_precmd() {
        local s=$? p=${PWD//%/%25}
        p=${p// /%20}; p=${p//;/%3B}
        printf '\e]133;D;%s\a\e]7;file://%s%s\a\e]133;A\a' "$s" "${HOSTNAME:-}" "$p"
        return $s
    }
    __lightdown_prompt() {
        local s=$?
        [[ $PS1 == *'\e]133;B'* ]] || PS1=$PS1'\[\e]133;B\a\]'
        [[ $PS0 == *'\e]133;C'* ]] || PS0=$PS0'\e]133;C\a'
        return $s
    }
    if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a"* ]]; then
        PROMPT_COMMAND=(__lightdown_precmd "${PROMPT_COMMAND[@]}" __lightdown_prompt)
    else
        PROMPT_COMMAND=__lightdown_precmd$'\n'${PROMPT_COMMAND:-}$'\n'__lightdown_prompt
    fi
fi
"#;

/// zsh's first startup file, found through ZDOTDIR. It puts ZDOTDIR back
/// so that zsh goes on to read the user's own files.
const ZSHENV: &str = r#"# Lightdown shell integration for zsh. Generated; edits are lost.
if [[ -n ${LIGHTDOWN_ZDOTDIR+x} ]]; then
    ZDOTDIR=$LIGHTDOWN_ZDOTDIR
else
    unset ZDOTDIR
fi
unset LIGHTDOWN_ZDOTDIR
__lightdown_file=${ZDOTDIR:-$HOME}/.zshenv
[[ -r $__lightdown_file ]] && source $__lightdown_file
unset __lightdown_file
if [[ -o interactive && -z $__lightdown_hooked ]]; then
    __lightdown_hooked=1
    __lightdown_precmd() {
        local s=$? p=${PWD//\%/%25}
        p=${p// /%20}; p=${p//;/%3B}
        print -nr -- $'\e]133;D;'$s$'\a\e]7;file://'${HOST}${p}$'\a\e]133;A\a'
        return $s
    }
    __lightdown_prompt() {
        [[ $PS1 == *$'\e]133;B'* ]] || PS1=$PS1$'%{\e]133;B\a%}'
    }
    __lightdown_preexec() {
        local c=${1//\\/\\\\}
        c=${c//;/\\x3b}; c=${c//$'\n'/\\x0a}; c=${c//$'\e'/\\x1b}; c=${c//$'\a'/\\x07}
        print -nr -- $'\e]633;E;'$c$'\a\e]133;C\a'
        # The prompt mark goes last, after hooks that set the prompt.
        precmd_functions=(${precmd_functions:#__lightdown_prompt} __lightdown_prompt)
    }
    precmd_functions=(__lightdown_precmd $precmd_functions __lightdown_prompt)
    preexec_functions+=(__lightdown_preexec)
fi
"#;

/// Sourced by fish once its own configuration is read. fish 4 marks its
/// prompts itself.
const FISH: &str = r#"# Lightdown shell integration for fish. Generated; edits are lost.
if status is-interactive; and not set -q __lightdown_hooked; and string match -qr '^[0-3]\.' -- $version
    set -g __lightdown_hooked 1
    function __lightdown_postexec --on-event fish_postexec
        printf '\e]133;D;%s\a' $status
    end
    function __lightdown_preexec --on-event fish_preexec
        printf '\e]133;C\a'
    end
    function __lightdown_prompt --on-event fish_prompt
        printf '\e]7;file://%s%s\a\e]133;A\a' (prompt_hostname) (string replace -a ' ' '%20' -- $PWD)
    end
    functions -q fish_prompt; and functions -c fish_prompt __lightdown_fish_prompt
    function fish_prompt
        __lightdown_fish_prompt
        printf '\e]133;B\a'
    end
end
"#;

/// A private folder for the hooks: the session's runtime folder, or else
/// Lightdown's cache.
fn folder() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|d| d.is_absolute() && d.is_dir());
    let dir = match runtime {
        Some(d) if !cfg!(test) => d.join("lightdown").join("shell"),
        _ => crate::config::cache_dir().join("shell"),
    };
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).ok()?;
    Some(dir)
}

/// Writes a hook file unless it already holds `text`, through a file of
/// this process's own so that two windows never see half of one.
fn write(path: &Path, text: &str) -> Option<()> {
    if std::fs::read_to_string(path).is_ok_and(|t| t == text) {
        return Some(());
    }
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file =
        std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&temp).ok()?;
    std::io::Write::write_all(&mut file, text.as_bytes()).ok()?;
    std::fs::rename(&temp, path).ok()
}

/// Which shell a program is, by its name.
fn kind(program: &str) -> Option<&'static str> {
    match Path::new(program).file_name()?.to_str()? {
        "bash" => Some("bash"),
        "zsh" => Some("zsh"),
        "fish" => Some("fish"),
        _ => None,
    }
}

/// Has a shell started by `cmd` load the hooks, if it is one that has
/// them and integration is on. Other programs are left as they are.
pub fn configure(cmd: &mut Command, program: &str) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let Some(shell) = kind(program) else { return };
    let Some(dir) = folder() else { return };
    match shell {
        "bash" => {
            let file = dir.join("bashrc");
            if write(&file, BASH).is_some() {
                cmd.arg("--rcfile").arg(file);
            }
        }
        "zsh" => {
            let zdot = dir.join("zsh");
            let made = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&zdot).is_ok();
            if made && write(&zdot.join(".zshenv"), ZSHENV).is_some() {
                if let Some(user) = std::env::var_os("ZDOTDIR") {
                    cmd.env("LIGHTDOWN_ZDOTDIR", user);
                }
                cmd.env("ZDOTDIR", zdot);
            }
        }
        _ => {
            let file = dir.join("lightdown.fish");
            if write(&file, FISH).is_some() {
                let quoted = format!("'{}'", file.to_string_lossy().replace('\\', "\\\\").replace('\'', "\\'"));
                cmd.arg("--init-command").arg(format!("source {quoted}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vt::Screen;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[test]
    fn shells_are_known_by_name_and_others_are_left_alone() {
        assert_eq!(kind("/usr/bin/bash"), Some("bash"));
        assert_eq!(kind("zsh"), Some("zsh"));
        assert_eq!(kind("/bin/sh"), None);
        let mut cmd = Command::new("/bin/sh");
        configure(&mut cmd, "/bin/sh");
        assert_eq!(cmd.get_args().count(), 0);
        let mut cmd = Command::new("bash");
        configure(&mut cmd, "bash");
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args[0], "--rcfile");
        assert_eq!(std::fs::read_to_string(&args[1]).unwrap(), BASH);
        let mut cmd = Command::new("zsh");
        configure(&mut cmd, "zsh");
        let zdot = cmd.get_envs().find(|(k, _)| *k == "ZDOTDIR").and_then(|(_, v)| v).unwrap();
        assert!(Path::new(zdot).join(".zshenv").is_file());
    }

    /// Runs a shell with the hooks in a scratch home on a real terminal,
    /// types `lines`, and gives the screen once it has settled.
    fn run(program: &str, lines: &[&str]) -> Option<Arc<Mutex<(vte::Parser, Screen)>>> {
        let path = std::env::var_os("PATH")?;
        let found = std::env::split_paths(&path).map(|d| d.join(program)).find(|p| p.is_file())?;
        let home = std::env::temp_dir().join(format!("ld-si-{program}-{}", std::process::id()));
        std::fs::create_dir_all(home.join("work dir")).unwrap();
        std::fs::write(home.join(".bashrc"), "PS1='$ '\nexport FROM_BASHRC=1\n").unwrap();
        let screen = Arc::new(Mutex::new((vte::Parser::new(), Screen::new(100, 30))));
        let shared = screen.clone();
        let mut cmd = Command::new(&found);
        configure(&mut cmd, found.to_str()?);
        cmd.current_dir(home.join("work dir")).env("HOME", &home).env("XDG_CONFIG_HOME", &home).env("XDG_DATA_HOME", &home);
        let output = move |bytes: &[u8]| {
            let mut guard = shared.lock().unwrap();
            let (parser, screen) = &mut *guard;
            parser.advance(screen, bytes);
            std::mem::take(&mut screen.reply)
        };
        let pty = crate::pty::Pty::spawn_command(Some(cmd), "", &home.join("work dir"), &home, (100, 30), output, |_| {}).ok()?;
        let settle = |n: usize| {
            let end = Instant::now() + Duration::from_secs(10);
            while Instant::now() < end && screen.lock().unwrap().1.commands().len() < n {
                std::thread::sleep(Duration::from_millis(50));
            }
            std::thread::sleep(Duration::from_millis(300));
        };
        settle(1);
        for (i, line) in lines.iter().enumerate() {
            pty.write(format!("{line}\r").as_bytes());
            settle(i + 2);
        }
        drop(pty);
        let _ = std::fs::remove_dir_all(&home);
        Some(screen)
    }

    #[test]
    fn bash_marks_its_prompts_commands_and_statuses() {
        let Some(screen) = run("bash", &["echo out; false", "", "echo $FROM_BASHRC"]) else { return };
        let screen = &screen.lock().unwrap().1;
        let commands = screen.commands();
        let ran: Vec<_> = commands.iter().filter(|c| c.output.is_some()).collect();
        assert_eq!(ran.len(), 2, "{commands:?}\n{}", screen.text());
        assert_eq!(ran[0].code, Some(1));
        assert_eq!(ran[1].code, Some(0));
        let (row, col) = ran[0].input.unwrap();
        assert_eq!(screen.row(row).unwrap().text(col, 100).trim(), "echo out; false");
        let (row, _) = ran[1].output.unwrap();
        assert_eq!(screen.row(row).unwrap().text(0, 100).trim(), "1", "the user's .bashrc was read");
        assert!(screen.cwd.as_deref().is_some_and(|d| d.ends_with("work dir")), "{:?}", screen.cwd);
    }

    #[test]
    fn fish_marks_its_commands() {
        let Some(screen) = run("fish", &["false", "echo hi"]) else { return };
        let screen = &screen.lock().unwrap().1;
        let ran: Vec<_> = screen.commands().into_iter().filter(|c| c.output.is_some()).collect();
        assert!(ran.len() >= 2, "{}", screen.text());
        assert_eq!(ran[0].code, Some(1));
        assert_eq!(ran[0].text.as_deref(), Some("false"));
        assert_eq!(ran[1].code, Some(0));
    }
}
