// One manifest-driven command sequence for local and hosted CI evidence.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { isAbsolute, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const fmctl = process.argv[2];
assert(fmctl, 'Usage: node formal/check.mjs /absolute/path/to/fmctl');

function run(program, args, timeout = 650_000) {
  const result = spawnSync(program, args, {
    cwd: root, encoding: 'utf8', timeout, maxBuffer: 8 * 1024 * 1024,
  });
  if (result.status !== 0) {
    process.stderr.write(result.stdout ?? '');
    process.stderr.write(result.stderr ?? '');
    throw new Error(`${program} ${args.join(' ')} failed (${result.status ?? result.error?.code})`);
  }
  // Verifier details are retained in fmctl's bounded artifacts. Keep the
  // human-facing log short without treating warnings as absent evidence.
  process.stderr.write(result.stderr ?? '');
  process.stdout.write(`Passed: ${args[0] === 'replay' ? 'Rust trace replay' : args.join(' ')}\n`);
}

run(process.execPath, ['formal/replay.mjs', '--prepare'], 320_000);
for (const operation of ['validate', 'check', 'simulate', 'verify', 'trace']) {
  run(fmctl, [operation]);
}
const trace = JSON.parse(readFileSync(new URL('../.formal-artifacts/fmctl/trace.result.json', import.meta.url)));
assert.equal(trace.success, true);
const countArgument = trace.args.find(value => value.startsWith('--n-traces='));
assert(countArgument, 'Trace report must record the corpus size');
const count = Number(countArgument.slice('--n-traces='.length));
assert(Number.isInteger(count) && count >= 1 && count <= 64);
const pattern = trace.artifacts.trace_pattern;
assert.equal(pattern.split('{seq}').length, 2);
const paths = Array.from({ length: count }, (_, index) => {
  const path = relative(root, pattern.replace('{seq}', String(index)));
  assert(path && !isAbsolute(path) && path !== '..' && !path.startsWith(`..${sep}`));
  return path;
});
run(fmctl, ['replay', '--adapter', 'rust', ...paths.flatMap(path => ['--trace', path])]);
run(process.execPath, ['formal/negative-controls.mjs', fmctl], 650_000);
