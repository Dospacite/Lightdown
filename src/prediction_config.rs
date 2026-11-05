//! Migration for the original Suggestions while typing preference.
impl crate::config::Config {
    pub(crate) fn load_predictions(&mut self, v: &serde_json::Value) {
        self.auto_suggest = match v["edit_predictions"].as_str() {
            Some("on") => true,
            Some("off") => false,
            _ => v["auto_suggest"].as_bool().unwrap_or(false),
        };
        if let Some(provider @ ("model" | "copilot")) = v["edit_prediction_provider"].as_str() {
            self.edit_prediction_provider = provider.into();
        }
        if let Some(delay) = v["edit_prediction_delay_ms"].as_u64() {
            self.edit_prediction_delay_ms = delay.clamp(100, 5000);
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn legacy_and_explicit_preferences() {
        let mut c = crate::config::Config::default();
        c.load_predictions(&serde_json::json!({"auto_suggest":true}));
        assert!(c.auto_suggest);
        c.load_predictions(
            &serde_json::json!({"auto_suggest":true,"edit_predictions":"off","edit_prediction_delay_ms":1}),
        );
        assert!(!c.auto_suggest);
        assert_eq!(c.edit_prediction_delay_ms, 100);
    }
}
