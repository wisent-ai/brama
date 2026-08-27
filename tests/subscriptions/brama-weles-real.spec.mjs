import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';

const repo = resolve(import.meta.dirname, '../..');
const launcher = resolve(repo, 'scripts/start-with-skarbiec.sh');
const cargo = process.env.CARGO || resolve(process.env.HOME, '.cargo/bin/cargo');
const result = spawnSync(
  launcher,
  [
    '--exec',
    cargo,
    'test',
    '--test',
    'subscription_real',
    'sign_in_reauthorizes_the_real_',
    '--',
    '--test-threads=1',
  ],
  {
    cwd: repo,
    env: {
      ...process.env,
      BRAMA_BIN_OVERRIDE: resolve(repo, 'target/debug/brama'),
    },
    encoding: 'utf8',
    maxBuffer: 32 * 1024 * 1024,
  },
);

process.stdout.write(result.stdout || '');
process.stderr.write(result.stderr || '');
if (result.error) throw result.error;
if (result.status !== 0) {
  throw new Error(`all three real Weles sign-ins must pass (exit ${result.status ?? 'unknown'})`);
}
