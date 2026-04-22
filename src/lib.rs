pub mod core;
pub mod crypto;
pub mod detection;
pub mod provider;
pub mod providers;
pub mod types;

pub use crate::core::router::{
    build_default_router, ModelRouter,
};
pub use crate::core::server::start_server;
pub use crate::detection::{
    detect_compute_resources, select_model_for_resources,
};
pub use crate::provider::ModelProvider;
pub use crate::types::{
    ComputeResources, Message, ModelRequest, ModelResponse,
    RouterError, Tool, ToolCall, ToolCallFunction,
    ToolFunction,
};
