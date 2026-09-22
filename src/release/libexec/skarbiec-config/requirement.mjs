// What the operating system says the signed binary is, which the policy names
// so a capability is redeemable only by that exact executable.

import { spawnSync } from 'node:child_process';

// A designated requirement is at most 4 KiB.
const MAX_REQUIREMENT_CHARS = 4096;

function macosCodeSigningRequirement(path) {
  if (process.platform !== 'darwin') return undefined;
  const verified = spawnSync(
    '/usr/bin/codesign',
    ['--verify', '--strict', '--all-architectures', path],
    { encoding: 'utf8' },
  );
  if (verified.error || verified.status !== 0) {
    throw new Error(`Brama binary does not have a valid macOS code signature: ${verified.stderr || verified.error}`);
  }
  const displayed = spawnSync(
    '/usr/bin/codesign',
    ['--display', '--requirements', '-', path],
    { encoding: 'utf8' },
  );
  if (displayed.error || displayed.status !== 0) {
    throw new Error(`cannot read Brama designated requirement: ${displayed.stderr || displayed.error}`);
  }
  const output = `${displayed.stdout}\n${displayed.stderr}`;
  const requirement = output
    .split(/\r?\n/u)
    .map((line) => line.startsWith('# ') ? line.slice(2) : line)
    .find((line) => line.startsWith('designated => '))
    ?.slice('designated => '.length)
    .trim();
  if (!requirement || requirement.length > MAX_REQUIREMENT_CHARS || requirement.includes('\0')) {
    throw new Error('Brama designated requirement is missing or invalid');
  }
  return requirement;
}

export { macosCodeSigningRequirement };
