//! POSIX permission bits as the `u32` that `PermissionsExt::mode` and
//! `OpenOptionsExt::mode` take and answer.
//!
//! The C library's `S_I*` constants are `u32` on Linux and `u16` on macOS, so
//! converting them to `u32` is required on one platform and refused by clippy
//! as a useless conversion on the other. The bits are the same on both, so
//! every mode Brama writes or checks is stated once here.

/// Owner read and write (`S_IRUSR | S_IWUSR`): every private file.
pub const OWNER_READ_WRITE: u32 = 0o600;
/// Group read (`S_IRGRP`).
pub const GROUP_READ: u32 = 0o040;
/// Every group and other bit (`S_IRWXG | S_IRWXO`): an owner-only file holds
/// none of them.
pub const GROUP_AND_OTHER: u32 = 0o077;
/// Every execute bit (`S_IXUSR | S_IXGRP | S_IXOTH`).
pub const ANY_EXECUTE: u32 = 0o111;
