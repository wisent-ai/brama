import { spawnSync } from 'node:child_process';
import { createHash, createPrivateKey, createPublicKey, generateKeyPairSync, sign } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { isAbsolute, join } from 'node:path';
import { ed25519, proofIdentity, writeSigned } from './skarbiec-config/keys.mjs';
import { macosCodeSigningRequirement } from './skarbiec-config/requirement.mjs';

const [
  ,,
  binaryPath,
  outputDir,
  executablePath = binaryPath,
  workloadUidInput,
  workloadGidInput,
  controlConfigInput,
] = process.argv;
if (!binaryPath || !outputDir || !isAbsolute(executablePath)) {
  throw new Error(
    'usage: generate-skarbiec-config.mjs <brama-binary> <output-dir> [absolute-runtime-binary] [uid] [gid] [control-config]',
  );
}

// A designated requirement is at most 4 KiB; the emitted files are owner-only.
const MAX_REQUIREMENT_CHARS = 4096;
const OWNER_ONLY_DIRECTORY = 0o700;
const OWNER_ONLY_FILE = 0o600;

// Which subscriptions exist is read out of the vault, not out of a manifest.
//
// It used to take a hand-written JSON list and emit one policy rule per entry, so
// a subscription the operator paid for was invisible to the gateway until someone
// remembered to add a line -- and one of them, a second Claude account, sat unused
// in the pool for weeks because nobody did.
//
// The vault already says it, on the item: `brama:subscription` marks one,
// `brama:provider:<p>` and `brama:id:<id>` name it. That is the same material the
// gateway reads, so the policy and the gateway can no longer disagree about what
// exists. A `brama:agent:<a>` tag records who banked an account and gates nothing:
// every subscription serves every caller.
const vaultPath = process.env.SKARBIEC_VAULT_FILE
  || join(process.env.HOME || '/nonexistent', '.stado', 'skarbiec.vault.json');
const MARK = 'brama:subscription';
const tagValue = (tags, prefix) => {
  const found = tags.find((tag) => tag.startsWith(prefix));
  return found === undefined ? null : found.slice(prefix.length);
};

// The Brama workload runs as uid/gid 10001 unless the deployment says otherwise.
const DEFAULT_WORKLOAD_UID = 10001;
const DEFAULT_WORKLOAD_GID = 10001;
const workloadUid = DEFAULT_WORKLOAD_UID;
const workloadGid = DEFAULT_WORKLOAD_GID;
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
// A grant lives at most ten years (315 360 000 s) and may be used ten million times.
const SECONDS_PER_DAY = 24 * 60 * 60;
const GRANT_MAX_YEARS = 10;
const DAYS_PER_YEAR = 365;
const maxTtlSeconds = GRANT_MAX_YEARS * DAYS_PER_YEAR * SECONDS_PER_DAY;
const GRANT_MAX_USES = 10_000_000;
const maxUses = GRANT_MAX_USES;
const requestSignAgentIds = ['wisent-app'];

// Vault ownership and routing are deliberately separate. Any item with both
// `brama:id:` and `brama:provider:` is Brama credential material, so the
// service policy may reacquire and repair it. Only items carrying the
// subscription marker are routed. This distinction lets a release repair tags
// stripped by an older credential write without exposing that incomplete item
// to a caller.
const vault = existsSync(vaultPath)
  ? JSON.parse(readFileSync(vaultPath, 'utf8'))
  : (process.stderr.write(`no vault at ${vaultPath}; the policy will grant no subscriptions\n`), {});
const credentialSubscriptions = Object.values(vault?.items ?? {})
  .filter((item) => Array.isArray(item?.tags))
  .map((item) => ({
    id: tagValue(item.tags, 'brama:id:'),
    provider: tagValue(item.tags, 'brama:provider:'),
    marked: item.tags.includes(MARK),
  }))
  .filter(({ id, provider }) => typeof id === 'string' && typeof provider === 'string'
    && /^[a-z0-9-]+$/.test(provider))
  .sort((left, right) => left.id.localeCompare(right.id));
const subscriptions = credentialSubscriptions.filter(({ marked }) => marked);

if (credentialSubscriptions.length === 0) {
  process.stderr.write(`no item in ${vaultPath} carries valid brama:id: and brama:provider: tags; the policy will grant none\n`);
}
if (subscriptions.length === 0 && credentialSubscriptions.length !== 0) {
  process.stderr.write(`no Brama credential in ${vaultPath} carries ${MARK}; incomplete items can be repaired but are not routed\n`);
}
const controlConfigPath = controlConfigInput || process.env.BRAMA_CONTROL_CONFIG;
let directProviderIds = ['local-openai'];
if (controlConfigPath) {
  const controlConfig = JSON.parse(readFileSync(controlConfigPath, 'utf8'));
  const requiredProviders = controlConfig?.services?.brama?.required_provider_capabilities;
  if (
    !Array.isArray(requiredProviders) ||
    requiredProviders.length === 0 ||
    requiredProviders.some((provider) => typeof provider !== 'string' || !/^[a-z0-9-]+$/.test(provider)) ||
    new Set(requiredProviders).size !== requiredProviders.length
  ) {
    throw new Error(
      'services.brama.required_provider_capabilities must be a non-empty unique provider list',
    );
  }
  directProviderIds = [...new Set([...requiredProviders, 'local-openai'])];
}
const now = Math.floor(Date.now() / 1000);
const expiresAt = now + maxTtlSeconds;
const policyDomain = Buffer.from('SKARBIEC-AGENT-POLICY\0v1\0', 'utf8');
const registryDomain = Buffer.from('SKARBIEC-WORKLOAD-REGISTRY\0v1\0', 'utf8');

mkdirSync(outputDir, { recursive: true, mode: 0o700 });
const wormPath = join(outputDir, 'worm-receipt');
writeFileSync(wormPath, '#!/bin/sh\ncat >/dev/null\nprintf receipt\n', { mode: 0o700 });
const wormDigest = createHash('sha256').update(readFileSync(wormPath)).digest('hex');


const subscriptionRules = credentialSubscriptions.map(({ id, provider }) => ({
  purpose: 'brama.provider.authenticate',
  resource: `provider:${provider}:${id}`,
  target: 'brama',
  max_ttl_seconds: maxTtlSeconds,
  max_uses: maxUses,
  delegation_depth: 0,
}));
const directProviderRules = [...new Set([...credentialSubscriptions.map(({ provider }) => provider), ...directProviderIds])].map((provider) => ({
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
writeSigned(outputDir, 'policy', policy, policyDomain, policyKey);
writeSigned(outputDir, 'registry', registry, registryDomain, registryKey);
for (const name of ['trust.json', 'brama-proof.key', 'policy.json', 'policy.sig', 'registry.json', 'registry.sig', 'worm-receipt']) {
  chmodSync(join(outputDir, name), name === 'worm-receipt' ? OWNER_ONLY_DIRECTORY : OWNER_ONLY_FILE);
}
