//! Optional Wayland text-input-v3. The compositor and input method consume
//! composition keys; forwarded keyboard events keep the ordinary key path.
use super::*;
use crate::app::ime::InputContext;
use smithay_client_toolkit::reexports::protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3::ZwpTextInputManagerV3,
    zwp_text_input_v3::{self, ZwpTextInputV3, ContentHint, ContentPurpose, ChangeCause},
};

#[derive(Default)]
pub(super) struct InputMethod {
    pub manager: Option<ZwpTextInputManagerV3>,
    input: Option<ZwpTextInputV3>,
    entered: bool,
    enabled: bool,
    serial: u32,
    sent: Option<InputContext>,
    preedit: Option<(String,i32,i32)>,
    commit: Option<String>,
    delete: (u32,u32),
    from_input: bool,
    waiting: bool,
}
impl State {
    pub(super) fn input_seat(&mut self, seat: &WlSeat) {
        if self.ime.input.is_none() {
            self.ime.input = self.ime.manager.as_ref().map(|m| m.get_text_input(seat,&self.qh,Plain));
        }
    }
    pub(super) fn input_sync(&mut self) {
        let context = if self.ime.entered { self.app.input_context() } else { None };
        let Some(input) = &self.ime.input else { return };
        if self.ime.waiting { return; }
        if context == self.ime.sent { return; }
        if let Some(context) = context {
            let external = !self.ime.from_input && self.ime.sent.as_ref().is_some_and(|old| old.key != context.key);
            if external && self.ime.enabled {
                input.disable();
                self.ime.enabled = false;
                // Context already carries the new generation when explicitly cancelled.
                self.app.preedit_clear();
            }
            if !self.ime.enabled {
                input.enable();
                input.set_content_type(ContentHint::None,ContentPurpose::Normal);
                self.ime.enabled = true;
            }
            input.set_text_change_cause(if self.ime.from_input { ChangeCause::InputMethod } else { ChangeCause::Other });
            input.set_surrounding_text(context.text.clone(),context.cursor,context.anchor);
            let (x,y,w,h) = context.rect;
            input.set_cursor_rectangle(x,y,w,h.max(1));
            input.commit();
            self.ime.serial = self.ime.serial.wrapping_add(1);
            self.ime.sent = Some(context);
        } else {
            if self.ime.enabled {
                input.disable();
                input.commit();
                self.ime.serial = self.ime.serial.wrapping_add(1);
            }
            self.ime.enabled = false;
            self.ime.sent = None;
            self.app.preedit_clear();
        }
        self.ime.from_input = false;
    }
}
impl Dispatch2<ZwpTextInputManagerV3, State> for Plain {
    fn event(&self,_: &mut State,_: &ZwpTextInputManagerV3,_: <ZwpTextInputManagerV3 as Proxy>::Event,
        _: &Connection,_: &QueueHandle<State>) {}
}
impl Dispatch2<ZwpTextInputV3, State> for Plain {
    fn event(&self,state: &mut State,_: &ZwpTextInputV3,event: zwp_text_input_v3::Event,
        _: &Connection,_: &QueueHandle<State>) {
        match event {
            zwp_text_input_v3::Event::Enter { surface } if &surface == state.window.wl_surface() => {
                state.ime.entered = true;
                state.input_sync();
            }
            zwp_text_input_v3::Event::Leave { .. } => {
                state.ime.entered = false;
                state.ime.waiting = false;
                state.ime.preedit = None;
                state.ime.commit = None;
                state.ime.delete = (0,0);
                state.input_sync();
                state.app.input_clear();
            }
            zwp_text_input_v3::Event::PreeditString { text,cursor_begin,cursor_end } => {
                state.ime.preedit = Some((text.unwrap_or_default(),cursor_begin,cursor_end));
            }
            zwp_text_input_v3::Event::CommitString { text } => state.ime.commit = text,
            zwp_text_input_v3::Event::DeleteSurroundingText { before_length,after_length } => {
                state.ime.delete = (before_length,after_length);
            }
            zwp_text_input_v3::Event::Done { serial } => {
                let preedit = state.ime.preedit.take();
                let commit = state.ime.commit.take();
                let delete = std::mem::take(&mut state.ime.delete);
                if let Some(context) = state.ime.sent.as_ref().filter(|_| state.ime.enabled) {
                    state.app.input_apply(context,delete,commit,preedit);
                }
                state.ime.waiting = serial != state.ime.serial;
                state.ime.from_input = true;
                state.input_sync();
            }
            _ => {}
        }
    }
}

pub(super) fn bind(globals: &smithay_client_toolkit::reexports::client::globals::GlobalList,qh: &QueueHandle<State>) -> InputMethod {
    InputMethod { manager: globals.bind::<ZwpTextInputManagerV3,State,Plain>(qh,1..=1,Plain).ok(), ..Default::default() }
}
