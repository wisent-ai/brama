//! Writing the journey step the operator is standing on to the terminal.

use serde_json::Value;
use wisent_onboarding_client::{JourneyClient, JourneyError, Transport};

pub(super) fn render_current_step<T: Transport, S: wisent_onboarding_client::Storage>(
    journey: &JourneyClient<T, S>,
) -> Result<(), JourneyError> {
    let bundle = journey.bundle().ok_or(JourneyError::NotStarted)?;
    let progress = journey.progress().ok_or(JourneyError::NotStarted)?;
    let screen = bundle
        .definition
        .screens
        .iter()
        .find(|screen| screen.screen_id == progress.current_screen_id)
        .ok_or_else(|| JourneyError::Invalid("Brama current screen".into()))?;
    let title = screen
        .presentation
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or(&screen.title_key);
    let body = screen
        .presentation
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or(&screen.body_key);
    println!("{title}");
    println!("{body}");
    for key in ["request_example", "response_example"] {
        if let Some(value) = screen.presentation.get(key).and_then(Value::as_str) {
            println!("{key}: {value}");
        }
    }
    Ok(())
}
