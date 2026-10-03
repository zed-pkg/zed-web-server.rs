// Container argv layout contract (DEN-390; see fiducia-cloud/fiducia-mcp-server.rs#43).
//
// The SOPS wrapper and the service binary must both live in ENTRYPOINT, with
// CMD left empty. If the binary sits in CMD instead, `docker run image --flag`
// or a Kubernetes `args:` list replaces CMD, which removes the binary: the
// wrapper then tries to exec `--flag` and the service never starts.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const root = fileURLToPath(new URL('..', import.meta.url));
const wrapper = fileURLToPath(new URL('./sops-entrypoint.sh', import.meta.url));
const WRAPPER_IN_IMAGE = '/usr/local/bin/sops-entrypoint.sh';

// Split a Dockerfile into stages and record each stage's last ENTRYPOINT/CMD.
// Line continuations are joined first, so `HEALTHCHECK ... CMD ...` is never
// mistaken for a CMD instruction; comment lines are skipped.
function stages(text) {
  const out = [];
  let current;
  for (const raw of text.replace(/\\\r?\n/g, ' ').split(/\r?\n/)) {
    const line = raw.trim();
    if (/^FROM\s/i.test(line)) {
      current = { from: line };
      out.push(current);
      continue;
    }
    const match = current && /^(ENTRYPOINT|CMD)\s+(.*)$/i.exec(line);
    if (!match) continue;
    let value;
    try { value = JSON.parse(match[2]); } catch { value = match[2]; }
    current[match[1].toLowerCase()] = value;
  }
  return out;
}

// Docker/Kubernetes semantics: runtime args (docker run trailing args, or a
// pod's `args:`) replace CMD and are appended to ENTRYPOINT.
const argv = (stage, runtimeArgs) =>
  [...stage.entrypoint, ...(runtimeArgs.length ? runtimeArgs : (stage.cmd ?? []))];

const dockerfiles = readdirSync(root).filter(name => name === 'Dockerfile' || /^Dockerfile\..*\.dkf$/.test(name));
const wrapped = dockerfiles.flatMap(file =>
  stages(readFileSync(join(root, file), 'utf8'))
    .filter(stage => Array.isArray(stage.entrypoint) && stage.entrypoint[0] === WRAPPER_IN_IMAGE)
    .map(stage => ({ file, stage })));

test('at least one Dockerfile stage runs through the SOPS wrapper', () => {
  assert.ok(dockerfiles.includes('Dockerfile'), 'repository Dockerfile not found');
  assert.ok(wrapped.length > 0, 'no stage uses the exec-form SOPS wrapper ENTRYPOINT');
});

test('no stage uses a shell-form ENTRYPOINT for the SOPS wrapper', () => {
  for (const file of dockerfiles) {
    for (const stage of stages(readFileSync(join(root, file), 'utf8'))) {
      if (typeof stage.entrypoint === 'string') {
        assert.doesNotMatch(stage.entrypoint, /sops-entrypoint/, `${file}: ${stage.from}`);
      }
    }
  }
});

for (const { file, stage } of wrapped) {
  test(`${file} (${stage.from}): ENTRYPOINT holds wrapper and binary, CMD is empty`, () => {
    assert.equal(stage.entrypoint.length, 2, 'ENTRYPOINT must be [wrapper, binary]');
    const binary = stage.entrypoint[1];
    assert.match(binary, /^\/\S+$/, 'service binary must be an absolute path');
    assert.notEqual(binary, WRAPPER_IN_IMAGE);
    // An absent CMD is empty too: setting ENTRYPOINT clears any inherited CMD.
    assert.ok(stage.cmd === undefined || (Array.isArray(stage.cmd) && stage.cmd.length === 0),
      `CMD must be [] (found ${JSON.stringify(stage.cmd)})`);
    assert.deepEqual(argv(stage, []), [WRAPPER_IN_IMAGE, binary]);
    assert.deepEqual(argv(stage, ['--extra-runtime-flag']), [WRAPPER_IN_IMAGE, binary, '--extra-runtime-flag']);
  });

  test(`${file} (${stage.from}): an extra runtime arg reaches the binary through the real wrapper`, (t) => {
    const dir = mkdtempSync(join(tmpdir(), 'container-argv-'));
    t.after(() => rmSync(dir, { recursive: true, force: true }));
    const fakeBinary = join(dir, 'service');
    writeFileSync(fakeBinary, `#!${process.execPath}\nprocess.stdout.write(JSON.stringify(process.argv.slice(2)));\n`, { mode: 0o755 });
    // Substitute only the in-image paths; argv shape comes from the Dockerfile.
    const [, ...rest] = argv(stage, ['--extra-runtime-flag']);
    rest[0] = fakeBinary;
    const result = spawnSync('/bin/sh', [wrapper, ...rest], {
      env: { PATH: '/usr/bin:/bin', SOPS_REQUIRE_KEY: '0', SOPS_SECRETS_FILE: join(dir, 'absent') },
      encoding: 'utf8', timeout: 5000,
    });
    assert.ifError(result.error);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(JSON.parse(result.stdout), ['--extra-runtime-flag']);
  });
}
