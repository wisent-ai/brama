#!/usr/bin/env python3
"""Answer, on one host, why this gateway is or is not serving.

Every failure this host has had reads the same from outside - `/health` answers
and no route works, or nothing listens at all - while the cause was a different
file each time: a workload registry describing another installation, a router
without the verb the launcher calls, an operator route naming a deployment
instead of a provider, a policy granting a provider the authority did not route,
a listener bound where no caller looks. Each was found by opening one more file
than the message named.

So this opens all of them, in the order the launcher does, and prints what each
says beside what it has to agree with:

  1. the units that start Brama, and the release state that actually serves;
  2. every installed generation: completeness, router verbs, and whether the
     host trust registry describes that exact installation;
  3. the service env values beside the supervisor-owned runtime coordinates;
  4. the policy's provider grants against the authority-owned routes table;
  5. every alias route against the providers that policy and routes agree on;
  6. where the gateway is reachable, and by which scheme;
  7. the current boot attempt from the error log, and nothing older.

Read-only throughout.

The sections live in the `diagnose` directory beside this file and are imported
by a path derived from this file's own location, because the operator runs this
from wherever the shell happens to be and the installation it must describe is
the one this script was unpacked into, not the one the working directory hints
at.
"""

import pathlib
import sys

sys.path.insert(int(), str(pathlib.Path(__file__).resolve().parent / "diagnose"))

import capability_report
import installation_report
import reachability_report

resolved = installation_report.print_units()
installation_report.print_generations(resolved)
config_dir = installation_report.print_service_env(resolved)
routed_direct_providers = capability_report.print_policy_grants(config_dir)
capability_report.print_alias_routes(routed_direct_providers)
reachability_report.print_reachability()
reachability_report.print_boot_attempt()
