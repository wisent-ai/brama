"""Which providers this host may authenticate as, and which aliases can use them.

Three files have to agree before one alias answers: policy.json grants a
provider resource, capability-routes.json says where that resource lives in the
vault, and inference-routes.json points an alias at that provider. Any one of
them alone looks correct, so each is printed against the other two.
"""

import json
import pathlib

from host_layout import BEST_ALIAS, PROVIDER_PURPOSE, home, moment, normalize, settings


def print_policy_grants(config_dir):
    """The policy's provider grants against the authority-owned routes table.

    Returns the providers both files agree on, in the spelling a route uses.
    """
    print("\n=== policy grants against authority routes")
    # A capability is issued at final use. The durable agreement to diagnose is
    # therefore the exact resource shared by policy.json and capability-routes.json,
    # not a single-use id an older launcher happened to leave on disk.
    granted = set()
    policy_path = config_dir / "policy.json"
    if policy_path.is_file():
        policy = json.loads(policy_path.read_text())
        for rule in policy.get("roles", {}).get("brama-runtime", []):
            if rule.get("purpose") != PROVIDER_PURPOSE:
                continue
            resource = rule.get("resource")
            if isinstance(resource, str):
                granted.add(resource)
        print(f"  policy.json written {moment(policy_path.stat().st_mtime)}")
        print(f"  granted provider resources ({len(granted)}): "
              f"{', '.join(sorted(granted)) or 'none'}")
    else:
        print(f"  {policy_path}: absent")

    configured_routes = settings.get("SKARBIEC_CAPABILITY_ROUTES_FILE")
    configured_vault = settings.get("SKARBIEC_VAULT_FILE")
    if configured_routes:
        capability_routes_path = pathlib.Path(configured_routes)
    elif configured_vault:
        capability_routes_path = pathlib.Path(configured_vault).parent / "capability-routes.json"
    else:
        capability_routes_path = home / ".config" / "skarbiec" / "capability-routes.json"

    routed = set()
    if capability_routes_path.is_file():
        document = json.loads(capability_routes_path.read_text())
        table = document.get("routes", document)
        if isinstance(table, dict):
            routed = {
                resource
                for resource, coordinate in table.items()
                if isinstance(resource, str)
                and isinstance(coordinate, dict)
                and isinstance(coordinate.get("item"), str)
                and isinstance(coordinate.get("field"), str)
            }
        print(f"  {capability_routes_path}")
        missing_routes = sorted(granted - routed)
        if missing_routes:
            print(f"  granted but unrouted: {', '.join(missing_routes)}")
        unexpected_routes = sorted(
            resource for resource in routed - granted if resource.startswith("provider:")
        )
        if unexpected_routes:
            print(f"  routed without a provider grant: {', '.join(unexpected_routes)}")
    else:
        print(f"  {capability_routes_path}: absent")

    return {
        normalize(parts[1])
        for resource in granted & routed
        if len(parts := resource.split(":")) == 2 and parts[0] == "provider"
    }


def print_alias_routes(routed_direct_providers):
    """Every alias route against the providers policy and routes agree on."""
    print("\n=== alias routes against routed provider grants")
    routes_path = pathlib.Path(
        settings.get("BRAMA_INFERENCE_ROUTES_FILE")
        or (home / ".config" / "brama" / "inference-routes.json")
    )
    if routes_path.is_file():
        document = json.loads(routes_path.read_text())
        print(f"  {routes_path}")
        entries = dict(document.get("routes", {}))
        for alias, route in sorted(entries.items()):
            provider = route.split("/")[0]
            if route == BEST_ALIAS:
                verdict = "exempt: a subscription pays for best"
            elif "/" not in route:
                verdict = "REFUSED: names no provider"
            elif provider in routed_direct_providers:
                verdict = "ok"
            else:
                verdict = "REFUSED: no routed provider grant"
            print(f"    {alias} -> {route} [{verdict}]")
        deployments = [entry.get("name") for entry in document.get("deployments", [])]
        print(f"  deployments: {', '.join(name for name in deployments if name) or 'none'}")
    else:
        print(f"  {routes_path}: absent")
