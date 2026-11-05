use super::*;
impl App {
    pub(super) fn toggle_plan(&mut self) {
        if self.ai.busy() { self.message = Some("Finish or stop the agent before changing plan mode".into()); return; }
        self.ai.plan_mode = !self.ai.plan_mode;
        self.ai.proposed_plan = None;
        self.message = Some(format!("Plan mode {}", if self.ai.plan_mode { "on" } else { "off" }));
        self.show_thread();
    }
    pub(super) fn edit_plan(&mut self) {
        if self.ai.busy() || !self.ai.side.input.buf.text.trim().is_empty() { self.message = Some("Finish the turn and clear the draft first".into()); return; }
        if let Some(plan) = &self.ai.proposed_plan {
            self.ai.side.input.set_text(plan.clone(), Doc::Text);
            self.show_side(Some(Tab::Agent));
            self.focus = Focus::Chat;
        } else { self.message = Some("Ask for a plan first".into()); }
    }
    pub(super) fn approve_plan(&mut self) {
        if self.ai.busy() { self.message = Some("Wait for the plan to finish".into()); return; }
        let Some(plan) = self.ai.proposed_plan.clone() else { self.message = Some("Ask for a plan first".into()); return; };
        if !self.ai.side.input.buf.text.trim().is_empty() { self.message = Some("Send your edited plan for review before approving".into()); return; }
        self.ai.plan_mode = false;
        self.ai.proposed_plan = None;
        self.ai.side.input.set_text(format!("Implement this approved plan:\n\n{plan}"), Doc::Text);
        self.send();
    }
}
