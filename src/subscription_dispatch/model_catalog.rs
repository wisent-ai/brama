//! Independent Wisent model catalog.
//!
//! models.dev supplies public provider/model metadata only. Skarbiec remains the
//! credential authority and Weles remains the account/subscription authority.
#![allow(unused_imports)]

mod catalog_protocol_part;
mod protocol_for_part;

pub use catalog_protocol_part::*;
pub use protocol_for_part::*;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use crate::providers::adapter::RegistryModel;
