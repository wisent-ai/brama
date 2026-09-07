#![allow(dead_code)]
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

pub struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    pub fn new(story: &str) -> Self {
        let home = std::env::var_os("HOME").expect("HOME is required");
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = PathBuf::from(home)
            .join(".stado/work/brama-tests")
            .join(format!("{story}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).expect("create Brama test directory");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// One vault item as the entitlements router's `list` prints it, carrying the
/// four tags discovery reads: the marker, the owning agent, the provider and
/// the subscription id. An item missing any of them is not a degraded account
/// -- it does not exist for any caller -- which is the asymmetry
/// `tests/routing/subscription_tag_contract.rs` exists for.
pub fn vault_item(agent: &str, provider: &str, subscription_id: &str) -> Value {
    json!({
        "id": format!("provider:{provider}:{subscription_id}"),
        "type": "bundle",
        "tags": [
            "brama:subscription",
            format!("brama:agent:{agent}"),
            format!("brama:provider:{provider}"),
            format!("brama:id:{subscription_id}"),
        ],
        "updated_at": "2026-09-01T00:00:00Z",
        "deleted": false,
        "versions": 1,
    })
}

/// An isolated vault, reached exactly the way the product reaches the real
/// one: a router answering the one subcommand discovery shells, `list`, out of
/// a listing this test owns.
///
/// Pointing `ENTITLEMENTS_ROUTER_BIN` at a path that does not exist is not
/// isolation, it is an unreadable vault -- and Brama reports an unreadable
/// inventory as a failure rather than as an empty pool, which is the whole
/// point of that distinction. A test that wants an empty pool has to give the
/// gateway a vault that answers and holds nothing.
pub fn write_isolated_vault(root: &Path, items: &[Value]) -> PathBuf {
    let vault = root.join("vault.json");
    fs::write(
        &vault,
        serde_json::to_vec(&Value::Array(items.to_vec())).expect("vault listing"),
    )
    .expect("write the isolated vault listing");
    let router = root.join("entitlements-router");
    fs::write(
        &router,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"list\" ]; then cat {}; exit 0; fi\n\
             echo \"the isolated vault answers only 'list', not '$*'\" >&2\nexit 64\n",
            vault.display()
        ),
    )
    .expect("write the isolated entitlements router");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&router, fs::Permissions::from_mode(0o700))
            .expect("make the isolated router executable");
    }
    router
}

/// The isolated router's path, creating an empty vault behind it when this
/// test has not seeded one.
pub fn isolated_router(root: &Path) -> PathBuf {
    let router = root.join("entitlements-router");
    if root.join("vault.json").exists() && router.exists() {
        return router;
    }
    write_isolated_vault(root, &[])
}
