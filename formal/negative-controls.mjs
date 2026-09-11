// These artifacts are intentionally invalid. A verifier outage or parser
// error is not a successful negative control: require a real counterexample
// and a real implementation mismatch respectively.
import assert from 'node:assert/strict';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const fmctl = process.argv[2];
assert(fmctl, 'Pass the verified fmctl executable path');
const directory = join(root, '.formal-artifacts', 'negative-controls');
mkdirSync(directory, { recursive: true });

function replaceOnce(source, before, after) {
  assert.equal(source.split(before).length, 2, 'Negative-control anchor drifted');
  return source.replace(before, after);
}

const model = replaceOnce(readFileSync(join(root, 'formal/browser_mutation.qnt'), 'utf8'),
  'delegate_called: true, returned_cookie: true',
  'delegate_called: true, returned_cookie: false');
writeFileSync(join(directory, 'rotation_loss.qnt'), model);
let manifest = replaceOnce(readFileSync(join(root, 'formal/fm.toml'), 'utf8'),
  'spec = "formal/browser_mutation.qnt"',
  'spec = ".formal-artifacts/negative-controls/rotation_loss.qnt"');
manifest = replaceOnce(manifest, 'artifacts_dir = ".formal-artifacts"',
  'artifacts_dir = ".formal-artifacts/negative-controls/verification"');
const manifestPath = join(directory, 'rotation_loss.fm.toml');
writeFileSync(manifestPath, manifest);
const verification = spawnSync(fmctl, ['--manifest', manifestPath, 'verify'], {
  cwd: root, encoding: 'utf8', timeout: 650_000, maxBuffer: 8 * 1024 * 1024,
});
const verificationLog = `${verification.stdout ?? ''}\n${verification.stderr ?? ''}`;
writeFileSync(join(directory, 'rotation-loss.log'), verificationLog);
assert.notEqual(verification.status, null, 'Verifier timed out or could not start');
assert.notEqual(verification.status, 0, 'Broken rotation model incorrectly passed');
assert.match(verificationLog, /invariant[^\n]*violated|violation found/i,
  'Require a counterexample, not an infrastructure/configuration failure');

const traceReport = JSON.parse(readFileSync(join(root, '.formal-artifacts/fmctl/trace.result.json')));
const trace = JSON.parse(readFileSync(traceReport.artifacts.trace_pattern.replace('{seq}', '0')));
const last = trace.states.at(-1).s;
assert.equal(last.phase.tag, 'Done');
assert.equal(typeof last.returned_cookie, 'boolean');
last.returned_cookie = !last.returned_cookie;
const tracePath = join(directory, 'wrong-cookie.itf.json');
writeFileSync(tracePath, JSON.stringify(trace));
const replay = spawnSync(process.execPath, ['formal/replay.mjs'], {
  cwd: root, encoding: 'utf8', timeout: 65_000, maxBuffer: 1024 * 1024,
  input: JSON.stringify({ protocol: 'fmctl.adapter.v1', project: 'zed-web-server',
    model: 'browser-mutation-v1', adapter: 'rust',
    specification: join(root, 'formal/browser_mutation.qnt'), traces: [tracePath] }),
});
writeFileSync(join(directory, 'wrong-cookie-replay.stderr.log'), replay.stderr ?? '');
assert.notEqual(replay.status, null);
assert.notEqual(replay.status, 0, 'Rust adapter accepted a contradictory model observation');
const response = JSON.parse(replay.stdout);
assert.equal(response.success, false);
assert.equal(response.traces_passed, 0);
assert(response.mismatches.length > 0);
writeFileSync(join(directory, 'wrong-cookie-replay.json'), JSON.stringify(response) + '\n');
process.stdout.write('Negative controls passed: TLC counterexample and Rust replay mismatch detected.\n');
