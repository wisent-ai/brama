pub mod dispatch;
pub mod model_catalog;
pub mod plan_usage;
pub mod pool;
pub mod probe;
pub mod sign_in;
pub mod quality;
pub mod refresh_sweep;
pub mod usage;

pub(crate) use dispatch::{authenticate_agent, registry_models_for_agent};
pub use dispatch::{
    dispatch_any_subscription, dispatch_any_subscription_stream,
    dispatch_any_vision_capable_subscription, dispatch_any_vision_capable_subscription_stream,
    dispatch_best_subscription, dispatch_best_subscription_stream, dispatch_direct,
    dispatch_direct_openai_typed, dispatch_direct_stream, dispatch_direct_with_fallback,
    dispatch_direct_with_fallback_stream, dispatch_subscription, dispatch_subscription_for_agent,
    dispatch_subscription_stream, dispatch_subscription_stream_for_agent,
    dispatch_task_subscription, dispatch_task_subscription_stream, is_subscription_model,
    provider_requires_caller_identity, RoutedStream,
};
pub use quality::{collect_task_quality, TaskQualityOptions};
