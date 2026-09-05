#!/usr/bin/env python3
"""Undo the last `brama-activate-generation` on this host.

Restores the `current` symlink to the generation recorded before the switch
and puts back the service env file saved beside it. Restarting the unit is a
separate act, through the service manager, exactly as activation was.
"""

import os
import pathlib
import shutil

home = pathlib.Path.home()
services = home / ".stado" / "services" / "brama"
env_file = home / ".config" / "brama" / "service.env"
record = home / ".stado" / "brama-previous-generation"

if not record.is_file():
    raise SystemExit("no activation to undo: nothing was recorded")

previous = record.read_text().strip()
if not previous:
    raise SystemExit("the recorded previous generation is empty")

backups = sorted(env_file.parent.glob(f"{env_file.name}.before-*"))
if not backups:
    raise SystemExit("no service env backup to restore")
newest = max(backups)
shutil.copyfile(newest, env_file)

current = services / "current"
temporary = services / "current.stado-restore"
if temporary.is_symlink() or temporary.exists():
    temporary.unlink()
temporary.symlink_to(previous)
os.replace(temporary, current)
print(f"current restored to {os.readlink(current)}")
