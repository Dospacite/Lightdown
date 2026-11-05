mod agent;
mod bidi;
mod app;
mod cells;
mod multibuffer;
mod citations;
mod chooser;
mod cli;
mod config;
mod copilot;
mod prediction_config;
mod config_schema;
mod container;
mod coverage;
mod editor;
mod diff;
mod merge;
mod dap;
mod diagrams;
mod environment;
mod env_loaders;
mod shell_integration;
mod export;
mod font;
mod http_file;
mod fuzzy;
mod gfx;
mod icon_theme;
mod icons;
mod html_export;
mod images;
mod kernel;
mod kernelspec;
mod keys;
mod latex;
mod layout;
mod lsp;
mod lsp_registry;
mod lsp_workspace;
mod math;
mod markdown_sync;
mod mcp;
mod md;
mod nb;
mod outline;
mod pane;
mod paste;
mod pdf;
mod pics;
mod platform;
mod ports;
mod preview;
mod pty;
mod py;
mod reader;
mod rpc;
mod remote;
mod side;
mod snippets;
mod spell;
mod split_tree;
mod synctex;
mod syntax;
mod treesitter;
mod table;
mod term;
mod tex;
mod theme;
mod themes;
mod tools;
mod tools_checks;
mod web_tools;
mod claude_code;
mod codex_agent;
mod outside_agent;
mod tree;
mod ui;
mod vt;


fn main() {
    let start = std::time::Instant::now();
    let mut targets = Vec::new();
    let mut wait = false;
    let mut literal = false;
    let mut args = std::env::args_os().skip(1);
    #[cfg(feature = "dump")]
    let mut dump = None;
    while let Some(arg) = args.next() {
        if literal {
            targets.push(cli::Target::parse(arg));
            continue;
        }
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!(
                    "Usage: lightdown [--wait] [FILE[:LINE[:COLUMN]] | FOLDER | NAME.lightdown-workspace] ...\n\nOpen files in tabs and folders together in one window. Positions are one-based. -- ends options.\n--wait waits for this window; closing with discarded edits exits with failure."
                );
                return;
            }
            Some("-V" | "--version") => {
                println!("lightdown {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            #[cfg(feature = "dump")]
            Some("--dump") => dump = Some(dump::Spec::parse(&mut args)),
            Some("--") => literal = true,
            Some("--wait") => wait = true,
            Some(s) if s.starts_with('-') => {
                eprintln!("lightdown: unknown option {s}");
                std::process::exit(2);
            }
            _ => targets.push(cli::Target::parse(arg)),
        }
    }
    let dirs: Vec<std::path::PathBuf> = targets.iter().filter(|t| t.path.is_dir()).map(|t| t.path.clone()).collect();
    let mut app = if dirs.is_empty() || targets.len() == 1 {
        app::App::new(targets.first().map(|t| t.path.clone()))
    } else {
        app::App::with_folders(dirs)
    };
    app.wait_mode = wait;
    for target in &targets { app.open_cli(target); }
    #[cfg(feature = "dump")]
    if let Some(spec) = dump {
        return dump::run(app, spec);
    }
    if let Err(e) = platform::run(app, start) {
        eprintln!("lightdown: {e}");
        std::process::exit(1);
    }
}

/// Headless rendering for tests: `--dump OUT.ppm WxH[@SCALE] [KEYS]` draws
/// one frame to a file after replaying a comma-separated key script.
#[cfg(feature = "dump")]
mod dump {
    use crate::app::{App, Key, Mods};
    use crate::gfx::Canvas;
    use std::io::Write;

    pub struct Spec {
        out: String,
        w: u32,
        h: u32,
        scale: f32,
        keys: String,
    }

    impl Spec {
        pub fn parse(args: &mut impl Iterator<Item = std::ffi::OsString>) -> Spec {
            let mut next = || args.next().and_then(|a| a.into_string().ok()).unwrap_or_default();
            let (out, size, keys) = (next(), next(), next());
            let (size, scale) = size.split_once('@').unwrap_or((&size, "1"));
            let (w, h) = size.split_once('x').unwrap_or(("900", "1000"));
            Spec { out, w: w.parse().unwrap(), h: h.parse().unwrap(), scale: scale.parse().unwrap(), keys }
        }
    }

    pub fn run(mut app: App, spec: Spec) {
        let (w, h) = ((spec.w as f32 * spec.scale) as u32, (spec.h as f32 * spec.scale) as u32);
        app.resize(w, h, spec.scale);
        let mut px = vec![0u32; (w * h) as usize];
        let mut canvas = Canvas::new(&mut px, w as i32, h as i32);
        let t = std::time::Instant::now();
        app.draw(&mut canvas);
        let first = t.elapsed();
        let t = std::time::Instant::now();
        app.draw(&mut canvas);
        eprintln!("first draw {first:.1?}, redraw {:.1?}", t.elapsed());
        for step in spec.keys.split(',').filter(|s| !s.is_empty()) {
            let mut mods = Mods::default();
            let mut name = step;
            while let Some((m, rest)) = name.split_once('+').filter(|(m, _)| matches!(*m, "ctrl" | "shift" | "alt")) {
                match m {
                    "ctrl" => mods.ctrl = true,
                    "shift" => mods.shift = true,
                    _ => mods.alt = true,
                }
                name = rest;
            }
            let (name, count) = name.split_once('*').map_or((name, 1), |(n, c)| (n, c.parse().unwrap()));
            for _ in 0..count {
                if let Some(text) = name.strip_prefix("type:") {
                    for ch in text.chars() {
                        app.key(Key::Char(ch), mods, Some(&ch.to_string()));
                    }
                } else if let Some(at) = name.strip_prefix("click:") {
                    let (x, y) = at.split_once(':').unwrap();
                    let (x, y) = (x.parse::<f32>().unwrap() * spec.scale, y.parse::<f32>().unwrap() * spec.scale);
                    app.press(x, y, mods, 1);
                    app.release();
                } else if let Some(at) = name.strip_prefix("press:") {
                    let (x, y) = at.split_once(':').unwrap();
                    let (x, y) = (x.parse::<f32>().unwrap() * spec.scale, y.parse::<f32>().unwrap() * spec.scale);
                    app.press(x, y, mods, 1);
                } else if let Some(at) = name.strip_prefix("rclick:") {
                    let (x, y) = at.split_once(':').unwrap();
                    let (x, y) = (x.parse::<f32>().unwrap() * spec.scale, y.parse::<f32>().unwrap() * spec.scale);
                    app.motion(x, y);
                    app.press_button(x, y, 2);
                    app.release_button(2);
                } else if let Some(at) = name.strip_prefix("move:") {
                    let (x, y) = at.split_once(':').unwrap();
                    app.motion(x.parse::<f32>().unwrap() * spec.scale, y.parse::<f32>().unwrap() * spec.scale);
                } else if let Some(n) = name.strip_prefix("scroll:") {
                    app.scroll_lines(n.parse().unwrap());
                } else if name == "csd" {
                    app.csd = true;
                } else if name == "wait" {
                    app.settle();
                } else {
                    let key = match name {
                        "left" => Key::Left,
                        "right" => Key::Right,
                        "up" => Key::Up,
                        "down" => Key::Down,
                        "home" => Key::Home,
                        "end" => Key::End,
                        "pgup" => Key::PageUp,
                        "pgdn" => Key::PageDown,
                        "bksp" => Key::Backspace,
                        "del" => Key::Delete,
                        "enter" => Key::Enter,
                        "tab" => Key::Tab,
                        "esc" => Key::Escape,
                        "space" => Key::Char(' '),
                        f if f.len() > 1 && f.starts_with('f') && f[1..].parse::<u8>().is_ok() => {
                            Key::F(f[1..].parse().unwrap())
                        }
                        "comma" => Key::Char(','),
                        "dot" => Key::Char('.'),
                        s => Key::Char(s.chars().next().unwrap()),
                    };
                    app.key(key, mods, None);
                }
                app.draw(&mut canvas);
            }
        }
        app.settle();
        app.draw(&mut canvas);
        let mut f = std::io::BufWriter::new(std::fs::File::create(&spec.out).unwrap());
        write!(f, "P6 {w} {h} 255 ").unwrap();
        for p in canvas.px.iter() {
            f.write_all(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]).unwrap();
        }
        println!("{}", app.doc.buf.text);
    }
}
