// fmctl.adapter.v1 bridge to the production Rust unit-test harness. Build in
// the caller's normal Cargo environment; replay needs no private Git access.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, readdirSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const preparation = join(root, '.formal-artifacts', 'rust-adapter.json');
const testName = 'browser_auth::mutation_conformance::replay_generated_model_traces';
const sha = bytes => createHash('sha256').update(bytes).digest('hex');

function sourceFingerprint() {
  function rustFiles(directory) {
    return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
      const path = join(directory, entry.name);
      return entry.isDirectory() ? rustFiles(path) : entry.isFile() && path.endsWith('.rs') ? [path] : [];
    });
  }
  const paths = [...rustFiles(join(root, 'src')), ...[
    'Cargo.toml', 'Cargo.lock', 'formal/fm.toml', 'formal/browser_mutation.qnt', 'formal/replay.mjs',
  ].map(path => join(root, path))].sort();
  const hash = createHash('sha256');
  for (const path of paths) {
    hash.update(relative(root, path)).update('\0').update(readFileSync(path)).update('\0');
  }
  return hash.digest('hex');
}

function prepare() {
  const build = spawnSync('cargo', ['test', '--locked', '--lib', '--no-run', '--message-format=json'], {
    cwd: root, encoding: 'utf8', timeout: 300_000, maxBuffer: 8 * 1024 * 1024,
  });
  process.stderr.write(build.stderr ?? '');
  assert.equal(build.status, 0, 'Rust adapter compilation failed');
  const artifacts = build.stdout.trim().split('\n').map(line => JSON.parse(line)).filter(item =>
    item.reason === 'compiler-artifact' && item.target.name === 'zed_web_server'
    && item.profile.test && item.executable);
  assert.equal(artifacts.length, 1, 'Expected exactly one production Rust test harness');
  const executable = realpathSync(artifacts[0].executable);
  mkdirSync(dirname(preparation), { recursive: true });
  writeFileSync(preparation, JSON.stringify({
    executable, binarySha256: sha(readFileSync(executable)), sourceSha256: sourceFingerprint(),
  }) + '\n');
  process.stderr.write('Prepared source-bound Rust replay harness.\n');
}

async function replay() {
  const chunks = [];
  let size = 0;
  for await (const chunk of process.stdin) {
    size += chunk.length;
    assert(size <= 1024 * 1024, 'Adapter request exceeds the size limit');
    chunks.push(chunk);
  }
  const request = JSON.parse(Buffer.concat(chunks).toString('utf8'));
  assert.deepEqual(Object.keys(request).sort(),
    ['adapter', 'model', 'project', 'protocol', 'specification', 'traces']);
  assert.equal(request.protocol, 'fmctl.adapter.v1');
  assert.equal(request.project, 'zed-web-server');
  assert.equal(request.model, 'browser-mutation-v1');
  assert.equal(request.adapter, 'rust');
  assert.equal(realpathSync(request.specification), realpathSync(join(root, 'formal/browser_mutation.qnt')));
  assert(Array.isArray(request.traces) && request.traces.length >= 1 && request.traces.length <= 64);
  const artifactRoot = realpathSync(join(root, '.formal-artifacts'));
  for (const path of request.traces) {
    assert.equal(typeof path, 'string');
    assert(isAbsolute(path), 'Trace path must be canonical');
    const local = relative(artifactRoot, realpathSync(path));
    assert(local && !isAbsolute(local) && local !== '..' && !local.startsWith(`..${sep}`), 'Trace escapes artifacts');
  }
  assert.equal(new Set(request.traces.map(path => realpathSync(path))).size, request.traces.length);
  const prepared = JSON.parse(readFileSync(preparation, 'utf8'));
  assert.equal(prepared.sourceSha256, sourceFingerprint(), 'Sources changed; run --prepare again');
  assert.equal(prepared.binarySha256, sha(readFileSync(prepared.executable)), 'Rust harness changed; run --prepare again');
  const run = spawnSync(prepared.executable, ['--ignored', '--exact', testName, '--nocapture'], {
    cwd: root, env: { ...process.env, ZED_FM_TRACES: JSON.stringify(request.traces) },
    encoding: 'utf8', timeout: 60_000, maxBuffer: 1024 * 1024,
  });
  process.stderr.write(run.stderr ?? '');
  process.stderr.write(run.stdout ?? '');
  const success = run.status === 0 && /test result: ok\. 1 passed; 0 failed;/.test(run.stdout ?? '');
  process.stdout.write(JSON.stringify({
    protocol: 'fmctl.adapter.v1', success,
    traces_total: request.traces.length, traces_passed: success ? request.traces.length : 0,
    mismatches: success ? [] : [{ trace: request.traces[0], step: null, action: null,
      message: 'Rust mutation trace replay failed; see bounded adapter stderr', expected: 'all terminal observations agree', actual: 'Rust test failure' }],
    implementation: { language: 'rust', name: 'zed-web-server browser mutation', version: '0.1.0' },
  }) + '\n');
  if (!success) process.exitCode = 1;
}

try {
  if (process.argv.length === 3 && process.argv[2] === '--prepare') prepare();
  else {
    assert.equal(process.argv.length, 2, 'Unknown adapter arguments');
    await replay();
  }
} catch (error) {
  process.stderr.write(`Rust replay adapter rejected the operation: ${error.message}\n`);
  process.exitCode = 1;
}
