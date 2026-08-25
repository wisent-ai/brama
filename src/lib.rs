// Response and failure envelopes are intentionally returned by value across the
// existing public API. Boxing them would be a breaking type change; Rust 1.98
// began warning about their size after this contract was published.
#![allow(clippy::result_large_err)]

pub mod build_info;
pub mod capability;
pub mod core;
pub mod crypto;
pub mod detection;
pub mod gateway;
pub mod journal;
pub mod mcp;
pub mod onboarding;
pub mod providers;
pub mod subscription_dispatch;
pub mod types;

pub use crate::build_info::{current as build_info, BuildInfo};
pub use crate::core::server::start_server;
pub use crate::detection::{detect_compute_resources, select_model_for_resources};
pub use crate::providers::adapter as provider_registry;
pub use crate::types::{
    ComputeResources, Message, ModelRequest, ModelResponse, RouterError, Tool, ToolCall,
    ToolCallFunction, ToolFunction,
};
