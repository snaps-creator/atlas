const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const YAML = require('yaml');

function constant(file, name) {
  const source = fs.readFileSync(path.join(__dirname, '..', file), 'utf8');
  const match = source.match(new RegExp(`\\bconst ${name}(?:: u64)? = ([\\d_]+);`));
  assert(match, `Missing numeric constant ${name} in ${file}`);
  return Number(match[1].replaceAll('_', ''));
}

test('packaged latency fixture expires under both production freshness thresholds', () => {
  const frontend = constant('src/latency.ts', 'LATENCY_FRESH_MS');
  const backend = constant('src-tauri/src/probe_state.rs', 'FRESH_MS');
  const staleAge = constant('src-tauri/src/latency_ui_smoke.rs', 'STALE_AGE_MS');
  assert.equal(frontend, backend, 'Frontend and native freshness must agree');
  assert(staleAge > frontend, 'The expired UI fixture must actually be expired');
});

test('fixture freshness is checked before compiling the signed installer', () => {
  const workflow = YAML.parse(fs.readFileSync(path.join(__dirname, '../.github/workflows/pr-installer.yml'), 'utf8'));
  const steps = workflow.jobs.installer.steps;
  const check = steps.findIndex(step => step.run?.includes('node --test') && step.run.includes('latency-ui-fixture.check.cjs'));
  const build = steps.findIndex(step => step.run?.includes('build-release-installer.ps1'));
  assert(check >= 0 && build > check);
  assert.equal(steps[check]['continue-on-error'], undefined);
});
