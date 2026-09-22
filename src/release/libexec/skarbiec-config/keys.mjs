// The keys this provisioning mints and the identity it proves with: a fresh
// signing pair for the policy and for the registry, and the proof key, which is
// reused when the deployment already carries one - minting a new one on every
// provision would invalidate every capability already issued against it.

import { createPrivateKey, createPublicKey, generateKeyPairSync, sign } from 'node:crypto';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

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
function writeSigned(outputDir, name, document, domain, key) {
  const bytes = Buffer.from(JSON.stringify(document), 'utf8');
  writeFileSync(join(outputDir, `${name}.json`), bytes, { mode: 0o600 });
  const signature = sign(null, Buffer.concat([domain, bytes]), key.privateKey);
  writeFileSync(join(outputDir, `${name}.sig`), `${signature.toString('base64')}\n`, { mode: 0o600 });
}

export { ed25519, ed25519FromSeed, proofIdentity, writeSigned };
