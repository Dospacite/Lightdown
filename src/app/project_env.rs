//! The project's direnv, mise or Nix environment as the window shows it:
//! choosing to load it, loading it again, and its item in the status bar.

use super::App;
use super::chrome::Hit;
use crate::env_loaders::{self, State};
use crate::font::{self, Fonts};
use crate::gfx::Canvas;
use crate::icons::Icon;
use crate::theme;
use crate::ui::px;

impl App {
    /// Turns loading the project's environment on or off.
    pub(super) fn toggle_env_loader(&mut self) {
        let on = !env_loaders::enabled(&self.root);
        if on && env_loaders::detect(&self.root, env_loaders::on_path).is_none() {
            self.message = Some(
                "No .envrc with direnv, mise config with mise, or flake.nix or shell.nix with nix in this project".into(),
            );
            return;
        }
        if let Err(e) = env_loaders::set_enabled(&self.root, on) {
            self.message = Some(format!("Could not remember the choice: {e}"));
            return;
        }
        env_loaders::set_waker(self.waker.clone());
        self.message = Some(if on {
            env_loaders::reload(&self.root);
            "Loading the project environment; commands and terminals started from now on use it".into()
        } else {
            "Project environment off; restart terminals and kernels to leave it".into()
        });
        self.redraw = true;
    }

    pub(super) fn reload_env_loader(&mut self) {
        if !env_loaders::enabled(&self.root) {
            self.message = Some("Run Environment: Use direnv/mise/Nix for this project first".into());
            return;
        }
        env_loaders::set_waker(self.waker.clone());
        env_loaders::reload(&self.root);
        self.message = Some("Reloading the project environment".into());
        self.redraw = true;
    }

    /// The status bar's item for the environment, at `x` on its left
    /// side; gives where the next item goes.
    pub(super) fn project_env_item(&mut self, c: &mut Canvas, fonts: &mut Fonts, x: f32, right: f32) -> f32 {
        let Some((state, news)) = env_loaders::status(&self.root) else { return x };
        env_loaders::set_waker(self.waker.clone());
        let (label, ink, tip) = match &state {
            State::Loading(tool) => (format!("{} loading", tool.name()), theme::warning(), "Loading the project environment".to_string()),
            State::Ready(tool, vars) => {
                (tool.name().to_string(), theme::dim(), format!("{} variables from {}; click to reload", vars.len(), tool.name()))
            }
            State::Failed(tool, e) => (format!("{} failed", tool.name()), theme::danger(), format!("{e}; click to retry")),
        };
        if news {
            match &state {
                State::Failed(tool, e) => self.message = Some(format!("{} environment failed: {e}", tool.name())),
                State::Ready(tool, vars) => {
                    self.message = Some(format!("Loaded {} variables from {}", vars.len(), tool.name()))
                }
                State::Loading(_) => {}
            }
        }
        let s = self.scale;
        let w = fonts.width(font::UI, px(theme::SMALL, s), &label) + px(30.0, s);
        if x + w >= right {
            return x;
        }
        let hit = Some((Hit::ProjectEnv, tip.as_str(), self.keys_of("env_reload")));
        let end = self.status_item(c, fonts, x + w, &label, Some((Icon::Dot, ink)), theme::dim(), hit);
        end + w + px(2.0, s)
    }
}
