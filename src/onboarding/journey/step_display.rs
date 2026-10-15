//! Writing the journey step the operator is standing on to the terminal.

use serde_json::Value;
use wisent_onboarding_client::{JourneyClient, JourneyError, Transport};

pub(crate) fn render_current_step<T: Transport, S: wisent_onboarding_client::Storage>(
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
    // Every screen of Brama's journey presents its own title and body; one
    // that does not is a malformed journey, and printing its translation key
    // in their place would show the operator an identifier, not a step.
    let presented = |field: &str| {
        screen
            .presentation
            .get(field)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                JourneyError::Invalid(format!(
                    "Brama journey screen {} presents no {field}",
                    screen.screen_id
                ))
            })
    };
    let title = presented("title")?;
    let body = presented("body")?;
    println!("{title}");
    println!("{body}");
    for key in ["request_example", "response_example"] {
        if let Some(value) = screen.presentation.get(key).and_then(Value::as_str) {
            println!("{key}: {value}");
        }
    }
    Ok(())
}
