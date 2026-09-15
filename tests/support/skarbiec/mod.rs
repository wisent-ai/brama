//! One real Skarbiec vault over data this test owns.
//!
//! Brama's subscription inventory is whatever `ENTITLEMENTS_ROUTER_BIN`
//! answers, and in production that is `skarbiec-entitlements-router` or
//! `skarbiec` itself. Until this fixture existed a three-line `/bin/sh`
//! script stood there, answering `list` by printing a JSON file: a stand-in
//! for another Wisent product, which could not go out of date and so could
//! never tell us when Brama and Skarbiec disagreed. What is isolated here is
//! the data, never the component.
//!
//! | part | what it owns |
//! |---|---|
//! | `vault` | creating, seeding and deleting inside one isolated vault |
//! | `commands` | running the real binaries against it, and reading it back |
//!
//! Both parts open with `use super::*;`, so the list below is this
//! fixture's single import list.

pub(crate) use std::fs;
pub(crate) use std::io::Write as _;
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::process::{Command, Output, Stdio};

pub(crate) use serde_json::Value;

pub(crate) use super::{fixture_name, sibling_binary, temp_base};

mod commands;
mod vault;

pub use vault::SkarbiecVault;
