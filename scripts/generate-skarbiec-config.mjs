import { chmodSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createHash, generateKeyPairSync, sign } from 'node:crypto';
import { join } from 'node:path';

const [binaryPath, outputDir] = process.argv.slice(2);
if (!binaryPath || !outputDir) {
  throw new Error('usage: generate-skarbiec-config.mjs <brama-binary> <output-dir>');
}

const workloadUid = 10001;
const workloadGid = 10001;
const maxTtlSeconds = 315_360_000;
const maxUses = 10_000_000;
const subscriptionIds = [
  'brama-sub-wisent-app-claude-1',
  'brama-sub-wisent-app-claude-2',
  'brama-sub-wisent-app-claude-3',
];
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

function writeSigned(name, document, domain, key) {
  const bytes = Buffer.from(JSON.stringify(document), 'utf8');
  writeFileSync(join(outputDir, `${name}.json`), bytes, { mode: 0o600 });
  const signature = sign(null, Buffer.concat([domain, bytes]), key.privateKey);
  writeFileSync(join(outputDir, `${name}.sig`), `${signature.toString('base64')}\n`, { mode: 0o600 });
}

const subscriptionRules = subscriptionIds.map((id) => ({
  purpose: 'brama.provider.authenticate',
  resource: `provider:claude-code:${id}`,
  target: 'brama',
  max_ttl_seconds: maxTtlSeconds,
  max_uses: maxUses,
  delegation_depth: 0,
}));
const requestSignRule = {
  purpose: 'brama.request.sign',
  resource: 'agent:wisent-app',
  target: 'brama',
  max_ttl_seconds: maxTtlSeconds,
  max_uses: maxUses,
  delegation_depth: 0,
};
const rules = [requestSignRule, ...subscriptionRules];
const policyKey = ed25519();
const registryKey = ed25519();
const proofKey = ed25519();
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
    'brama-cloudrun': {
      target: 'brama',
      uid: workloadUid,
      gid: workloadGid,
      executable_path: '/usr/local/bin/brama',
      executable_sha256: createHash('sha256').update(readFileSync(binaryPath)).digest('hex'),
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
writeSigned('policy', policy, policyDomain, policyKey);
writeSigned('registry', registry, registryDomain, registryKey);
for (const name of ['trust.json', 'brama-proof.key', 'policy.json', 'policy.sig', 'registry.json', 'registry.sig', 'worm-receipt']) {
  chmodSync(join(outputDir, name), name === 'worm-receipt' ? 0o700 : 0o600);
}
