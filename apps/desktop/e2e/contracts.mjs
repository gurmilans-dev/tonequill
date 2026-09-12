import { execFileSync } from 'node:child_process';
import { readFile, mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import assert from 'node:assert/strict';
const temporary = resolve('../../target/phase4/contracts-check.ts');
await mkdir(resolve('../../target/phase4'), { recursive: true });
execFileSync(
  'cargo',
  [
    'run',
    '--manifest-path',
    '../../Cargo.toml',
    '-p',
    'tonequill-live',
    '--example',
    'export_bindings',
    '--',
    temporary,
  ],
  { stdio: 'inherit' },
);
const normalize = (value) => value.replaceAll('\r\n', '\n');
assert.equal(
  normalize(await readFile('src/domain/generated.ts', 'utf8')),
  normalize(await readFile(temporary, 'utf8')),
  'Rust/TypeScript contracts drifted. Run npm run contracts.',
);
console.log('Canonical Rust/TypeScript contract matches.');
