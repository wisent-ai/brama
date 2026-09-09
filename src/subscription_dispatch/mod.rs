pub mod dispatch;
pub mod model_catalog;
pub mod plan_usage;
pub mod pool;
pub mod probe;
pub mod quality;
pub mod refresh_sweep;
pub mod sign_in;
pub mod usage;

pub(crate) use dispatch::{authenticate_agent, registry_models_for_agent};
pub use dispatch::{
    dispatch_any_subscription, dispatch_any_subscription_stream,
    dispatch_any_vision_capable_subscription, dispatch_any_vision_capable_subscription_stream,
    dispatch_best_subscription, dispatch_best_subscription_for_agent,
    dispatch_best_subscription_stream, dispatch_best_subscription_stream_for_agent,
    dispatch_direct, dispatch_direct_openai_typed, dispatch_direct_stream, dispatch_subscription,
    dispatch_subscription_for_agent, dispatch_subscription_stream,
    dispatch_subscription_stream_for_agent, dispatch_task_subscription,
    dispatch_task_subscription_stream, is_subscription_model, provider_requires_caller_identity,
    RoutedStream,
};
pub use quality::{collect_task_quality, TaskQualityOptions};

/// Whether a browser sign-in has already been driven against this
/// subscription's exact stored credential.
///
/// One reader for a question three surfaces ask: the sweep, to decide; the
/// pool document, to say so per account; and readiness, to report the set. The
/// gate itself lives in `refresh_sweep::verdict`, beside the incident that
/// shaped it.
pub fn sign_in_already_driven(subscription_id: &str) -> bool {
    !refresh_sweep::verdict_outranks_last_sign_in(
        usage::credential_recorded_at_ms(subscription_id),
        crate::journal::latest_subscription_sign_in_at_ms(subscription_id),
    )
}
