use serde::Serialize;

pub const PRODUCT: &str = "brama";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const SOURCE_REVISION: &str = match option_env!("BRAMA_SOURCE_REVISION") {
    Some(value) => value,
    None => "development",
};
pub const PLATFORM: &str = match option_env!("BRAMA_BUILD_PLATFORM") {
    Some(value) => value,
    None => "development-host",
};
pub const BUILT_AT: &str = match option_env!("BRAMA_BUILD_TIMESTAMP") {
    Some(value) => value,
    None => "not-recorded",
};

#[derive(Debug, Clone, Copy, Serialize)]
pub struct BuildInfo {
    pub product: &'static str,
    pub version: &'static str,
    pub source_revision: &'static str,
    pub platform: &'static str,
    pub built_at: &'static str,
}

pub const fn current() -> BuildInfo {
    BuildInfo {
        product: PRODUCT,
        version: VERSION,
        source_revision: SOURCE_REVISION,
        platform: PLATFORM,
        built_at: BUILT_AT,
    }
}
