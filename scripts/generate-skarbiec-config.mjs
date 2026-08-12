import { spawnSync } from 'node:child_process';
import { createHash, createPrivateKey, createPublicKey, generateKeyPairSync, sign } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { isAbsolute, join } from 'node:path';

const [, , binaryPath, outputDir, subscriptionsPath, executablePath = binaryPath, workloadUidInput, workloadGidInput] = process.argv;
if (!binaryPath || !outputDir || !subscriptionsPath || !isAbsolute(executablePath)) {
  throw new Error('usage: generate-skarbiec-config.mjs <brama-binary> <output-dir> <subscriptions-json> [absolute-runtime-binary] [uid] [gid]');
}

const workloadUid = 10001;
const workloadGid = 10001;
const configuredWorkloadUid = workloadUidInput === undefined ? workloadUid : Number(workloadUidInput);
const configuredWorkloadGid = workloadGidInput === undefined ? workloadGid : Number(workloadGidInput);
if (
  !Number.isSafeInteger(configuredWorkloadUid) ||
  !Number.isSafeInteger(configuredWorkloadGid) ||
  String(configuredWorkloadUid).startsWith('-') ||
  String(configuredWorkloadGid).startsWith('-')
) {
  throw new Error('workload uid and gid must be non-negative safe integers');
}
const maxTtlSeconds = 315_360_000;
const maxUses = 10_000_000;
const subscriptions = JSON.parse(readFileSync(subscriptionsPath, 'utf8'));
const subscriptionAgentIds = ['echo', 'content-platform', 'oko', 'wisent-app', 'lem', 'probierz'];
const requestSignAgentIds = ['wisent-app'];
if (!Array.isArray(subscriptions) || subscriptions.length === 0) {
  throw new Error('subscriptions manifest must be a non-empty array');
}
for (const subscription of subscriptions) {
  if (
    !subscription ||
    typeof subscription.id !== 'string' ||
    typeof subscription.provider !== 'string' ||
    !subscriptionAgentIds.some((agentId) => subscription.id.startsWith(`brama-sub-${agentId}-`)) ||
    !/^[a-z0-9-]+$/.test(subscription.provider) ||
    (subscription.agents !== undefined && (
      !Array.isArray(subscription.agents) ||
      subscription.agents.length === 0 ||
      subscription.agents.some((agentId) => !subscriptionAgentIds.includes(agentId))
    ))
  ) {
    throw new Error('subscriptions manifest contains an invalid entry');
  }
}
const now = Math.floor(Date.now() / 1000);
const expiresAt = now + maxTtlSeconds;
const policyDomain = Buffer.from('SKARBIEC-AGENT-POLICY\0v1\0', 'utf8');
const registryDomain = Buffer.from('SKARBIEC-WORKLOAD-REGISTRY\0v1\0', 'utf8');

mkdirSync(outputDir, { recursive: true, mode: 0o700 });
const wormPath = join(outputDir, 'worm-receipt');
writeFileSync(wormPath, '#!/bin/sh\ncat >/dev/null\nprintf receipt\n', { mode: 0o700 });
const wormDigest = createHash('sha256').update(readFileSync(wormPath)).digest('hex');

function ed25519() {
  const pair = generateKeyPairSync('ed25519');
  const publicJwk = pair.publicKey.export({ format: 'jwk' });
  const privateJwk = pair.privateKey.export({ format: 'jwk' });
  if (!publicJwk.x || !privateJwk.d) throw new Error('Ed25519 JWK export is incomplete');
  return {
    privateKey: pair.privateKey,
    publicRaw: Buffer.from(publicJwk.x, 'base64url'),
    privateSeed: Buffer.from(privateJwk.d, 'base64url'),
  };
}

// The broker verifies a redemption against the public key the VAULT holds for
// this workload, so the private half is an identity, not a build artifact.
// Minting a fresh one on every run meant an update -- which lands the bundle
// under a new digest directory and provisions it there -- silently replaced
// the identity the vault knows. The authority kept issuing capabilities and
// the broker kept refusing to redeem them, which surfaces only as a credential
// that is "unavailable".
//
// So a key that already exists is kept. BRAMA_PROOF_KEY_FILE names one to
// carry forward from the installation being replaced; otherwise a key already
// sitting in the output directory is reused. Only a first provision mints.
function ed25519FromSeed(seedHex) {
  const seed = Buffer.from(seedHex.trim(), 'hex');
  const prefix = Buffer.from('302e020100300506032b657004220420', 'hex');
  const privateKey = createPrivateKey({
    key: Buffer.concat([prefix, seed]),
    format: 'der',
    type: 'pkcs8',
  });
  const publicJwk = createPublicKey(privateKey).export({ format: 'jwk' });
  if (!publicJwk.x) throw new Error('Ed25519 JWK export is incomplete');
  return {
    privateKey,
    publicRaw: Buffer.from(publicJwk.x, 'base64url'),
    privateSeed: seed,
  };
}

function proofIdentity(outputDir) {
  const carried = process.env.BRAMA_PROOF_KEY_FILE;
  const existing = join(outputDir, 'brama-proof.key');
  for (const candidate of [carried, existing]) {
    if (candidate && existsSync(candidate)) {
      return ed25519FromSeed(readFileSync(candidate, 'utf8'));
    }
  }
  return ed25519();
}
function writeSigned(name, document, domain, key) {
  const bytes = Buffer.from(JSON.stringify(document), 'utf8');
  writeFileSync(join(outputDir, `${name}.json`), bytes, { mode: 0o600 });
  const signature = sign(null, Buffer.concat([domain, bytes]), key.privateKey);
  writeFileSync(join(outputDir, `${name}.sig`), `${signature.toString('base64')}\n`, { mode: 0o600 });
}

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
  if (!requirement || requirement.length > 4096 || requirement.includes('\0')) {
    throw new Error('Brama designated requirement is missing or invalid');
  }
  return requirement;
}

const subscriptionRules = subscriptions.map(({ id, provider }) => ({
  purpose: 'brama.provider.authenticate',
  resource: `provider:${provider}:${id}`,
  target: 'brama',
  max_ttl_seconds: maxTtlSeconds,
  max_uses: maxUses,
  delegation_depth: 0,
}));
const directProviderRules = [...new Set([...subscriptions.map(({ provider }) => provider), 'local-openai'])].map((provider) => ({
  purpose: 'brama.provider.authenticate',
  resource: `provider:${provider}`,
  target: 'brama',
  max_ttl_seconds: maxTtlSeconds,
  max_uses: maxUses,
  delegation_depth: Number(false),
}));
const requestSignRules = requestSignAgentIds.map((agentId) => ({
  purpose: 'brama.request.sign',
  resource: `agent:${agentId}`,
  target: 'brama',
  max_ttl_seconds: maxTtlSeconds,
  max_uses: maxUses,
  delegation_depth: Number(false),
}));
const rules = [...requestSignRules, ...directProviderRules, ...subscriptionRules];
const policyKey = ed25519();
const registryKey = ed25519();
const proofKey = proofIdentity(outputDir);
const macosRequirement = macosCodeSigningRequirement(binaryPath);
const policy = {
  version: 'v1',
  sequence: 1,
  environment: 'production',
  worm_command_sha256: wormDigest,
  roles: { 'brama-runtime': rules },
  agents: { 'brama-runtime': { roles: ['brama-runtime'] } },
  agent_grants: {
    'brama-runtime': [{
      grant_id: 'brama-runtime-v1',
      not_before: now - 60,
      expires_at: expiresAt,
      revoked: false,
      rules,
    }],
  },
  environment_allow: rules,
  deny: [],
  leases: { 'brama-runtime': { not_before: now - 60, expires_at: expiresAt } },
  rate: { issue_per_minute: 100, redeem_failures_per_minute: 100 },
};
const registry = {
  version: 'v1',
  sequence: 1,
  workloads: {
    'brama-service': {
      target: 'brama',
      uid: configuredWorkloadUid,
      gid: configuredWorkloadGid,
      executable_path: executablePath,
      executable_sha256: createHash('sha256').update(readFileSync(binaryPath)).digest('hex'),
      ...(macosRequirement === undefined
        ? {}
        : { macos_code_signing_requirement: macosRequirement }),
      proof_key: proofKey.publicRaw.toString('base64'),
      agent_ids: ['brama-runtime'],
    },
  },
};
const trust = {
  version: 'v1',
  policy_key: policyKey.publicRaw.toString('base64'),
  workload_key: registryKey.publicRaw.toString('base64'),
};

writeFileSync(join(outputDir, 'trust.json'), JSON.stringify(trust), { mode: 0o600 });
writeFileSync(join(outputDir, 'brama-proof.key'), `${proofKey.privateSeed.toString('hex')}\n`, { mode: 0o600 });
// `proofIdentity` reads a carried key but never creates one, so the first
// provision on a host still has to leave it somewhere later installations will
// find. Without that, every new generation mints a fresh key, and the vault
// grant that key needs can only be authorised by the owner — so the gateway is
// back to `capability redemption denied` on the next deploy.
const carriedKeyPath = process.env.BRAMA_PROOF_KEY_FILE;
if (carriedKeyPath && !existsSync(carriedKeyPath)) {
  mkdirSync(join(carriedKeyPath, '..'), { recursive: true });
  writeFileSync(carriedKeyPath, `${proofKey.privateSeed.toString('hex')}\n`);
  chmodSync(carriedKeyPath, statSync(join(outputDir, 'brama-proof.key')).mode);
}
writeSigned('policy', policy, policyDomain, policyKey);
writeSigned('registry', registry, registryDomain, registryKey);
for (const name of ['trust.json', 'brama-proof.key', 'policy.json', 'policy.sig', 'registry.json', 'registry.sig', 'worm-receipt']) {
  chmodSync(join(outputDir, name), name === 'worm-receipt' ? 0o700 : 0o600);
}
