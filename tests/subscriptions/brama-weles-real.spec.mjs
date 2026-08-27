import { spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';

const requireFromProbierz = createRequire(resolve(process.cwd(), 'package.json'));
const { expect, test } = requireFromProbierz('@playwright/test');

const repo = resolve(import.meta.dirname, '../..');
const launcher = resolve(repo, 'scripts/start-with-skarbiec.sh');
const cargo = process.env.CARGO || resolve(process.env.HOME, '.cargo/bin/cargo');

test('Weles reauthorizes every real Brama subscription account', () => {
  test.setTimeout(60 * 60 * 1000);
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
  expect(result.status, 'all three real Weles sign-ins must pass').toBe(0);
});
