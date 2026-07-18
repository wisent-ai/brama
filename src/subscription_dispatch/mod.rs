pub mod dispatch;
pub mod model_catalog;
pub mod provider_registry;
pub mod quality;

pub(crate) use dispatch::registry_models_for_agent;
pub use dispatch::{
    dispatch_any_subscription, dispatch_any_vision_capable_subscription, dispatch_subscription,
    dispatch_subscription_for_agent, dispatch_task_subscription, is_subscription_model,
};
pub use quality::{collect_task_quality, TaskQualityOptions};
