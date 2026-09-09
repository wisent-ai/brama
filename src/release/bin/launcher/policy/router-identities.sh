# Preload only the two bearers whose exact alias sets Brama validates at
# startup. Every other bearer is resolved through Skarbiec introspection on its
# first request. Each router invocation loads the large vault, so bounded
# concurrent reads keep required startup work inside Stado's candidate deadline.
: "${BRAMA_ALLOWED_MODELS:?set exact closed Brama model allowlist}"
identities_file="$runtime_dir/model-router-client-identities.json"
printf '%s\n' "reading required model-router identities" >/dev/stderr
"$PYTHON_BIN" - "$ENTITLEMENTS_ROUTER_BIN" "$backend_models" >"$identities_file" <<'PY'
from concurrent.futures import ThreadPoolExecutor
import json
import os
import subprocess
import sys

arguments = iter(sys.argv)
next(arguments)
router = next(arguments)
all_models = os.environ["BRAMA_ALLOWED_MODELS"].split(",")
# Extra operator aliases remain routable; they are not automatically added to
# the backend client's exact startup grant.
backend_models = json.loads(next(arguments))
# Keep both Brama-owned paths the worker may request. `weles` is the worker's
# own alias and selects whatever the route table declares for it; `best` remains
# its subscription-funded alias. The server validates this exact pair for the
# `weles` client at startup (`requires_exact_aliases("weles", …)`).
weles_models = ["best", "weles"]
# The credential-renewal caller. Weles's reauth trajectories present the bearer
# from `wisent-app-model-router` and sign as agent `wisent-app`; that bearer was
# in no boot table and behind no grant, so every renewal read answered
# `model_bearer_unrecognized` and the pool those trajectories exist to refill
# stayed empty while three providers waited to be signed in. The routes are
# derived from the same closed allowlist the backend's are, never a second copy
# of the model names: each provider's own route is what a trajectory probes, and
# `best` is the subscription alias those grants serve.
renewal_providers = ("claude-code", "codex", "kimi")
renewal_models = sorted(
    {model for model in all_models if model.split("/", 1)[0] in renewal_providers}
    | {"best"}
)
# The last field says whether the gateway refuses to start without this client.
# Renewal is not: it repairs the pool, it does not serve traffic, and a
# gateway that will not start serves none of it.
sources = [
    ("weles", "weles-model-router", "weles", weles_models, True),
    ("wisent-backend", "wisent-backend-model-router", "wisent-app", backend_models, True),
    ("wisent-app", "wisent-app-model-router", "wisent-app", renewal_models, False),
]

def field(item, name):
    # check=True hides the reason: the traceback names the command and drops
    # everything the router said about why it refused, which turns a one-line
    # cause into an afternoon of guessing from the outside.
    result = subprocess.run(
        [router, "get", item],
        check=False,
        capture_output=True,
        text=True,
        env=os.environ,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "").strip()
        raise SystemExit(f"reading {item} through the entitlements router failed: {detail}")
    payload = json.loads(result.stdout)
    if payload.get("schema") != "skarbiec.item.v2":
        raise RuntimeError(f"{item} did not return a Skarbiec v2 item")
    fields = payload.get("fields")
    if not isinstance(fields, dict):
        raise RuntimeError(f"{item} did not return a fields object")
    value = fields.get(name)
    if not isinstance(value, str) or not value or value.strip() != value:
        raise RuntimeError(f"{item}/{name} is not a single non-empty value")
    return value

def identity(source):
    client_id, item, agent_id, allowed_models, required = source
    try:
        token = field(item, "token")
    except SystemExit as refusal:
        if required:
            raise
        # Named rather than swallowed: the silent version of this state is what
        # let a client be missing from the table for as long as it was.
        print(f"optional client {client_id} skipped: {refusal}", file=sys.stderr)
        return None
    result = {"client_id": client_id, "token": token, "agent_id": agent_id}
    if allowed_models is not None:
        result["allowed_models"] = allowed_models
    return result

with ThreadPoolExecutor(max_workers=len(sources)) as executor:
    identities = [entry for entry in executor.map(identity, sources) if entry]
sys.stdout.write(json.dumps(identities, separators=(",", ":")))
PY
printf '%s\n' "read required model-router identities" >/dev/stderr
BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES=$(cat "$identities_file")
rm -f "$identities_file"
# Unknown bearers are intentionally absent here and resolved by the
# introspection grant configured above.
export BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES
unset BRAMA_ALLOWED_MODELS backend_models

# Product request-sign identities are projected from their exact Skarbiec items.
# `wisent-app` is Jeden's public runtime identity and uses the dedicated
# `agent:wisent-app` item rather than a product-specific `agent_auth_secret`.
BRAMA_REQUEST_SIGN_IDENTITIES="$(
printf '%s\n' "reading request-sign identities" >/dev/stderr
  "$PYTHON_BIN" - "$ENTITLEMENTS_ROUTER_BIN" <<'PY'
from concurrent.futures import ThreadPoolExecutor
import json
import os
import subprocess
import sys

arguments = iter(sys.argv)
next(arguments)
router = next(arguments)
sources = {
    "echo": "echo-agent-auth",
    "content-platform": "content-platform-agent-auth",
    "oko": "oko-model-agent-auth",
    "weles": "weles-model-agent-auth",
    "lem": "lem-agent-auth",
    "probierz": "probierz-agent-auth",
    "wisent-app": "agent:wisent-app",
}

def item_fields(item):
    # check=True hides the reason: the traceback names the command and drops
    # everything the router said about why it refused, which turns a one-line
    # cause into an afternoon of guessing from the outside.
    result = subprocess.run(
        [router, "get", item],
        check=False,
        capture_output=True,
        text=True,
        env=os.environ,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "").strip()
        raise SystemExit(f"reading {item} through the entitlements router failed: {detail}")
    payload = json.loads(result.stdout)
    if payload.get("schema") != "skarbiec.item.v2":
        raise RuntimeError(f"{item} did not return a Skarbiec v2 item")
    fields = payload.get("fields")
    if not isinstance(fields, dict):
        raise RuntimeError(f"{item} did not return a fields object")
    return fields

def field(fields, item, name):
    value = fields.get(name)
    if not isinstance(value, str) or not value:
        raise RuntimeError(f"{item}/{name} is empty")
    return value

def identity(source):
    expected_id, item = source
    fields = item_fields(item)
    if expected_id == "wisent-app":
        return expected_id, field(fields, item, "value")
    actual_id = field(fields, item, "id")
    if actual_id != expected_id:
        raise RuntimeError(f"{item}/id does not match its product identity")
    return actual_id, field(fields, item, "agent_auth_secret")

with ThreadPoolExecutor(max_workers=4) as executor:
    identities = dict(executor.map(identity, sources.items()))
print(json.dumps(identities, separators=(",", ":")))
PY
)"
printf '%s\n' "read request-sign identities" >/dev/stderr
[ -n "$BRAMA_REQUEST_SIGN_IDENTITIES" ] || {
  printf '%s\n' "central request-sign identities are empty" >/dev/stderr
  false
}
export BRAMA_REQUEST_SIGN_IDENTITIES


# Brama and Weles authenticate this one route with the same Skarbiec-owned
# bearer. Brama acquires its copy through its own identity; Weles acquires its
# copy through its own identity. Neither service reads the other's files.
printf '%s\n' "reading Brama-Weles reauthentication identity" >/dev/stderr
BRAMA_WELES_REAUTH_TOKEN="$(
  "$ENTITLEMENTS_ROUTER_BIN" get brama-weles-reauth \
    | "$PYTHON_BIN" -c '
import json
import sys
payload = json.load(sys.stdin)
if payload.get("schema") != "skarbiec.item.v2":
    raise SystemExit("brama-weles-reauth did not return a Skarbiec v2 item")
value = payload.get("fields", {}).get("token")
if not isinstance(value, str) or not value:
    raise SystemExit("brama-weles-reauth/token is empty")
sys.stdout.write(value)
'
)"
printf '%s\n' "read Brama-Weles reauthentication identity" >/dev/stderr
[ -n "$BRAMA_WELES_REAUTH_TOKEN" ] || {
  printf '%s\n' "Brama-Weles reauthentication token is empty" >/dev/stderr
  false
}
export BRAMA_WELES_REAUTH_TOKEN
