pub mod dispatch;
pub mod model_catalog;
pub mod quality;
pub mod usage;

pub(crate) use dispatch::{authenticate_agent, registry_models_for_agent};
pub use dispatch::{
    dispatch_any_subscription, dispatch_any_vision_capable_subscription, dispatch_direct,
    dispatch_direct_openai_typed, dispatch_direct_with_fallback, dispatch_subscription,
    dispatch_subscription_for_agent, dispatch_task_subscription, is_subscription_model,
    provider_requires_caller_identity,
};
pub use quality::{collect_task_quality, TaskQualityOptions};
