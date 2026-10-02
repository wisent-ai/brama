import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  accessSync, constants, existsSync, mkdirSync, mkdtempSync,
  readFileSync, realpathSync, rmSync, statSync, writeFileSync,
} from 'node:fs';
import { delimiter, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repository = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const args = process.argv.slice(2);
assert.equal(args.length, 2, 'usage: node tests/oauth_refresh/real.mjs --expected-revision <native-source-revision>');
assert.equal(args[0], '--expected-revision');
assert.match(args[1], /^[0-9a-f]{40}$/);
const expectedRevision = args[1];
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');

function executable(name) {
  const candidates = name.includes('/') ? [resolve(name)]
    : (process.env.PATH || '').split(delimiter).map((directory) => join(directory, name));
  // fallback-justified: standard PATH lookup skips absent executable locations,
  // not providers; a selected executable's failure never runs another program.
  for (const candidate of candidates) {
    try {
      accessSync(candidate, constants.X_OK);
      if (statSync(candidate).isFile()) return realpathSync(candidate);
    } catch { /* An absent PATH candidate is not the selected executable. */ }
  }
  throw new Error(`EXECUTABLE_UNAVAILABLE: ${name}`);
}

// A short path also keeps GnuPG's Unix socket names within the kernel's capacity.
mkdirSync(join(repository, '.build'), { recursive: true });
const root = mkdtempSync(join(repository, '.build/rf-'));
const reportPath = join(root, 'report.json');
const report = {
  expected_native_revision: expectedRevision,
  runner_sha256: digest(readFileSync(fileURLToPath(import.meta.url))),
  started_at: new Date().toISOString(), status: 'running', commands: [], cases: [],
  fixture: root,
};
const save = () => writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
const environment = {
  PATH: process.env.PATH || '', HOME: root, TMPDIR: join(root, 'tmp'),
  GNUPGHOME: join(root, 'g'), SKARBIEC_VAULT_FILE: join(root, 'vault.json'),
  SKARBIEC_AUDIT_FILE: join(root, 'audit.jsonl'),
  BRAMA_STATE_DIR: join(root, 'state'), BRAMA_SUBSCRIPTION_USAGE_FILE: join(root, 'usage.json'),
};
for (const directory of ['g', 'tmp', 'state']) mkdirSync(join(root, directory), { mode: 0o700 });
save();

function run(program, argv, { input, env = environment } = {}) {
  const output = spawnSync(program, argv, { cwd: repository, env, input, encoding: 'utf8' });
  report.commands.push({
    program, args: argv, exit_status: output.status, signal: output.signal,
    stdout: output.stdout, stderr: output.stderr,
    ...(input === undefined ? {} : { input_sha256: digest(input) }),
    ...(output.error ? { error: output.error.message } : {}),
  });
  save();
  if (output.error) throw output.error;
  assert.equal(output.signal, null, `${program} was terminated by ${output.signal}`);
  return output;
}

function document(program, argv, options) {
  const output = run(program, argv, options);
  assert.equal(output.status, 0, `${program} ${argv.join(' ')}: ${output.stderr || output.stdout}`);
  return JSON.parse(output.stdout);
}

function exercise(name, action) {
  const item = { name, status: 'running' };
  report.cases.push(item);
  save();
  try {
    action(item);
    item.status = 'passed';
  } catch (error) {
    item.status = 'failed';
    item.error = error.stack || String(error);
  }
  save();
}

let vaultOpened = false;
try {
  const brama = executable(process.env.BRAMA_REAL_BIN || 'brama');
  const vault = executable(process.env.SKARBIEC_BIN || 'skarbiec');
  environment.ENTITLEMENTS_ROUTER_BIN = vault;
  report.checkout_revision = run('git', ['rev-parse', 'HEAD']).stdout.trim();
  report.brama = document(brama, ['version']);
  report.brama_binary_sha256 = digest(readFileSync(brama));
  report.skarbiec = document(vault, ['version']);
  report.skarbiec_binary_sha256 = digest(readFileSync(vault));
  save();
  if (report.brama.source_revision !== expectedRevision) {
    report.status = 'blocked';
    throw new Error(`BRAMA_REVISION_NOT_INSTALLED: expected ${expectedRevision}, observed ${report.brama.source_revision}`);
  }

  vaultOpened = true;
  document(vault, ['init', 'brama-oauth-refresh-test']);
  const id = 'brama-oauth-refresh-test';
  // Invalid input sent to the real provider, never a real account or a simulated endpoint.
  const credential = JSON.stringify({ tokens: {
    access_token: 'brama-real-test-invalid-access-token',
    refresh_token: 'brama-real-test-invalid-refresh-token',
  } });
  const item = JSON.stringify({
    schema: 'skarbiec.item.v2', kind: 'bundle', context: { source_kind: 'donation' },
    fields: { value: credential },
  });
  document(vault, ['set-json', id, '--type', 'bundle', '--tags',
    `brama:subscription,brama:provider:codex,brama:id:${id},brama:agent:brama-test`], { input: item });
  const original = document(vault, ['get', id]);
  assert.equal(original.fields.value, credential);

  const inventory = document(vault, ['list']);
  assert.deepEqual(inventory.filter((entry) => entry.tags?.includes('brama:subscription'))
    .map((entry) => entry.id), [id], 'only the isolated test subscription may be refreshed');

  function credentialState() {
    const path = environment.BRAMA_SUBSCRIPTION_USAGE_FILE;
    if (!existsSync(path)) return null;
    const ledger = JSON.parse(readFileSync(path, 'utf8'));
    return ledger.subscriptions[id]?.credential ?? null;
  }

  const before = credentialState();
  report.initial_credential_state = before;
  save();
  exercise('connection refusal preserves its cause and does not disown the grant', (test) => {
    // TCP port zero has no listening endpoint. Only HTTPS is redirected; the vault stays local.
    const env = { ...environment, HTTPS_PROXY: 'http://127.0.0.1:0', https_proxy: 'http://127.0.0.1:0' };
    const output = run(brama, ['subscription', 'refresh', 'codex', '--reason',
      'real-test: refused OAuth transport must retain its cause', '--json'], { env });
    test.verdict = JSON.parse(output.stdout);
    test.after = credentialState();
    const stored = document(vault, ['get', id]);
    assert.equal(output.status, 1);
    assert.equal(test.verdict.attempted, 1);
    assert.equal(test.verdict.result, 'failed');
    assert.deepEqual(stored.fields, original.fields, 'transport failure must not overwrite the stored grant');
    assert.deepEqual(test.after, before, 'transport failure must not change the saved grant state');
    assert.match(test.verdict.detail, /https:\/\/auth\.openai\.com\/oauth\/token/);
    assert.match(test.verdict.detail, /connection refused/i);
  });

  exercise('a complete authentication refusal makes the invalid grant unusable', (test) => {
    const output = run(brama, ['subscription', 'refresh', 'codex', '--reason',
      'real-test: real provider rejection of a deliberately invalid grant', '--json']);
    test.verdict = JSON.parse(output.stdout);
    test.after = credentialState();
    const stored = document(vault, ['get', id]);
    assert.equal(output.status, 1);
    assert.equal(test.verdict.attempted, 1);
    assert.equal(test.verdict.result, 'failed');
    assert.deepEqual(stored.fields, original.fields, 'a rejected grant must not be overwritten');
    assert.equal(test.after?.state, 'needs_reauthorization', 'a definitively rejected grant must not remain usable');
  });
  document(vault, ['delete', id]);
  report.remaining_items = document(vault, ['list']);
  assert.deepEqual(report.remaining_items, [], 'the test must leave no live fixture credential');
  report.audit = document(vault, ['audit-query']);
  report.persisted = ['vault.json', 'audit.jsonl', 'usage.json', 'state/journal.jsonl']
    .filter((name) => existsSync(join(root, name)))
    .map((name) => ({ path: join(root, name), sha256: digest(readFileSync(join(root, name))) }));
  report.status = report.cases.every((test) => test.status === 'passed') ? 'passed' : 'failed';
} catch (error) {
  if (report.status !== 'blocked') report.status = 'failed';
  report.error = error.stack || String(error);
} finally {
  if (vaultOpened) {
    try {
      // End only the daemon owned by this new test keyring, never a shared service.
      const cleanup = run('gpgconf', ['--homedir', environment.GNUPGHOME, '--kill', 'all']);
      assert.equal(cleanup.status, 0, cleanup.stderr);
      rmSync(environment.GNUPGHOME, { recursive: true });
      rmSync(environment.TMPDIR, { recursive: true });
      report.cleanup = { status: 'passed', keyring_removed: true };
    } catch (error) {
      report.cleanup = { status: 'failed', error: error.stack || String(error) };
      report.status = 'failed';
    }
  }
  report.finished_at = new Date().toISOString();
  save();
  console.log(JSON.stringify({ status: report.status, report: reportPath, cases: report.cases.map(({ name, status }) => ({ name, status })) }, null, 2));
  if (report.status !== 'passed') process.exitCode = 1;
}
