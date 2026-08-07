#!/usr/bin/env python3
"""Register this installation's workload key with the vault that guards it.

The broker will not redeem a capability unless the vault holds a live token
entry for the agent the capability was issued to, carrying that workload's
public key. `provision-skarbiec-trust` generates the key pair and records the
public half in `registry.json`; nothing then told the vault about it, so every
redemption was denied with `capability redemption denied` — a message that names
neither the agent nor the missing entry.

This closes that gap from what the installation already holds: the public key
comes out of the installation's own registry, and the capabilities come out of
`capability-routes.json`, so the token grants exactly the vault coordinates the
broker will be asked to read and nothing more.

The private half never leaves the installation, and nothing here prints a
secret.
"""

import base64
import json
import os
import pathlib
import stat
import subprocess
import tempfile

OWNER_ONLY = stat.S_IRUSR | stat.S_IWUSR

home = pathlib.Path.home()
env_file = home / ".config" / "brama" / "service.env"

settings = {}
for line in env_file.read_text().splitlines():
    name, separator, value = line.partition("=")
    if separator and not name.lstrip().startswith("#"):
        settings[name.strip()] = value.strip().strip("'\"")

# The launcher passes these directly, and when it does they are authoritative:
# it has just provisioned the very directory whose key must be registered.
# Falling back to the service env keeps the script usable on its own, and the
# running installation is the last resort because `current` can lag a boot
# behind what the launcher resolved.
router = os.environ.get("ENTITLEMENTS_ROUTER_BIN") or settings.get("ENTITLEMENTS_ROUTER_BIN")
vault = os.environ.get("SKARBIEC_VAULT_FILE") or settings.get("SKARBIEC_VAULT_FILE")
config_dir = os.environ.get("BRAMA_SKARBIEC_CONFIG_DIR") or settings.get(
    "BRAMA_SKARBIEC_CONFIG_DIR"
)
if not router or not vault:
    raise SystemExit(
        "ENTITLEMENTS_ROUTER_BIN and SKARBIEC_VAULT_FILE must be named by the "
        "environment or the service env"
    )

registry_path = pathlib.Path(config_dir) / "registry.json" if config_dir else None
if registry_path is None or not registry_path.is_file():
    running = (home / ".stado" / "services" / "brama" / "current").resolve()
    architecture = running / "darwin-arm"
    root = architecture if architecture.is_dir() else running
    registry_path = root / "etc" / "brama-skarbiec" / "registry.json"
if not registry_path.is_file():
    raise SystemExit(f"no workload registry at {registry_path}")

workload = next(
    iter(json.loads(registry_path.read_text()).get("workloads", {}).values()), {}
)
public_key = workload.get("proof_key")
agents = workload.get("agent_ids", [])
if not public_key or not agents:
    raise SystemExit(f"{registry_path} names no proof key or agent to bind it to")


def public_key_pem(raw_base64):
    """Wrap a raw Ed25519 public key as the PEM the vault insists on.

    The registry records the key as raw bytes; `token-mint` validates a
    SubjectPublicKeyInfo PEM with openssl. The prefix below is that structure's
    fixed header for Ed25519, so the two representations are the same key.
    """
    ED25519_SPKI_PREFIX = bytes.fromhex("302a300506032b6570032100")
    body = base64.encodebytes(ED25519_SPKI_PREFIX + base64.b64decode(raw_base64)).decode()
    return f"-----BEGIN PUBLIC KEY-----\n{body}-----END PUBLIC KEY-----\n"

# The broker is told where the routes table lives, and on a running host that is
# not beside the vault it opens: the launcher copies the vault into the runtime
# directory and exports the durable table's path separately. Looking only beside
# the vault therefore finds nothing, grants nothing, and leaves every redemption
# denied — silently, because "there is nothing to grant" reads like a benign
# state rather than the cause.
exported = os.environ.get("SKARBIEC_CAPABILITY_ROUTES_FILE") or settings.get(
    "SKARBIEC_CAPABILITY_ROUTES_FILE"
)
candidates = [pathlib.Path(exported)] if exported else []
candidates.append(pathlib.Path(vault).with_name("capability-routes.json"))
candidates.append(home / ".stado" / "capability-routes.json")
routes_path = next((path for path in candidates if path.is_file()), None)
if routes_path is None:
    raise SystemExit(
        "no capability routes table found at "
        + ", ".join(str(path) for path in candidates)
    )
routes = json.loads(routes_path.read_text())
capabilities = sorted(
    f"acquire:{entry['item']}#{entry['field']}"
    for entry in routes.values()
    if isinstance(entry, dict) and entry.get("item") and entry.get("field")
)
print(f"routes:   {routes_path}")
if not capabilities:
    raise SystemExit(f"{routes_path} maps nothing, so there is nothing to grant")

# When the vault already names this key there is nothing to do, and saying so is
# not a formality: minting requires the vault owner's secret key, which a running
# service deliberately does not hold, so an unnecessary attempt fails with a gpg
# error on every boot and buries the one case that matters. A durable workload
# seed means this is the normal answer after the first registration.
already = subprocess.run(
    [router, "tokens"],
    capture_output=True,
    text=True,
    check=False,
    env={**os.environ, **settings},
)
if already.returncode == int():
    try:
        recorded = json.loads(already.stdout)
    except ValueError:
        recorded = {}
    entries = recorded if isinstance(recorded, dict) else {}
    for agent in agents:
        entry = entries.get(agent) if isinstance(entries.get(agent), dict) else {}
        if entry.get("workload_bound") or entry.get("workload_public_key"):
            print(f"{agent}: already registered")
            agents = [name for name in agents if name != agent]
if not agents:
    raise SystemExit(int())

environment = dict(os.environ)
environment.update(settings)

print(f"registry: {registry_path}")
print(f"agents:   {', '.join(agents)}")
print(f"granting: {', '.join(capabilities)}")

for agent in agents:
    with tempfile.NamedTemporaryFile("w", delete=False) as handle:
        handle.write(public_key_pem(public_key))
        key_file = pathlib.Path(handle.name)
    key_file.chmod(OWNER_ONLY)
    try:
        minted = subprocess.run(
            [
                router,
                "token-mint",
                agent,
                "--capabilities",
                ",".join(capabilities),
                "--workload-public-key-file",
                str(key_file),
                "--replace-capabilities",
            ],
            capture_output=True,
            text=True,
            check=False,
            env=environment,
        )
    finally:
        key_file.unlink()
    if minted.returncode:
        detail = (minted.stderr.strip() or minted.stdout.strip()).replace("\n", " ")
        raise SystemExit(f"token-mint refused {agent}: {detail}")
    answer = json.loads(minted.stdout) if minted.stdout.strip() else {}
    print(
        f"{agent}: workload_bound={answer.get('workload_bound')} "
        f"expires_at={answer.get('expires_at')}"
    )
