const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {spawnSync} = require('node:child_process');
const YAML = require('yaml');
const {failures} = require('./check-updater-readiness.cjs');
const ready = () => JSON.parse(fs.readFileSync('updater-readiness.json'));
test('actual readiness CLI fails closed and writes the same blockers to the CI summary', t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'atlas-readiness-'));
  t.after(() => fs.rmSync(root, {recursive: true, force: true}));
  const manifest = path.join(root, 'readiness.json');
  const report = path.join(root, 'summary.md');
  function run(value) {
    fs.writeFileSync(manifest, JSON.stringify(value));
    fs.writeFileSync(report, '');
    const result = spawnSync(process.execPath, ['scripts/check-updater-readiness.cjs', manifest], {
      encoding: 'utf8', env: {...process.env, GITHUB_STEP_SUMMARY: report},
    });
    assert.ifError(result.error);
    return {...result, report: fs.readFileSync(report, 'utf8')};
  }
  assert.equal(run(ready()).status, 0);
  const pending = {...ready(), requiredChecks: [], blockers: ['MISSING_AUTOMATED_EVIDENCE']};
  const result = run(pending);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /requiredChecks/);
  assert.match(result.report, /MISSING_AUTOMATED_EVIDENCE/);
  assert.match(result.report, /PHYSICAL_REBOOT_RECOVERY: NOT_TESTED/);
  assert.equal(run({}).status, 1);
  assert.equal(run(null).status, 1);
  const checkedIn = JSON.parse(fs.readFileSync('updater-readiness.json', 'utf8'));
  assert.equal(run(checkedIn).status, failures(checkedIn).length ? 1 : 0);
});
test('PR and release use one readiness gate; publication cannot start before it passes', () => {
  const read = name => YAML.parse(fs.readFileSync(`.github/workflows/${name}.yml`, 'utf8'));
  const pr = read('pr-check'), release = read('release'), gate = read('updater-readiness');
  assert.equal(pr.jobs['release-readiness'].uses, './.github/workflows/updater-readiness.yml');
  assert.equal(release.jobs['release-readiness'].uses, pr.jobs['release-readiness'].uses);
  assert.deepEqual([release.jobs['publish-windows'].needs].flat(), ['release-readiness']);
  assert.equal(release.jobs['publish-windows'].if, undefined, 'must not bypass failed dependency with always()');
  assert(Object.hasOwn(gate.on, 'workflow_call'));
  const check = gate.jobs.readiness.steps.find(step => step.run === 'node scripts/check-updater-readiness.cjs');
  assert(check);
  assert.equal(check['continue-on-error'], undefined);
  assert.equal(gate.jobs.readiness['continue-on-error'], undefined);
  assert.equal(gate.permissions.contents, 'read');
});

test('both pipelines run publication preparation after installed acceptance on the same candidate',()=>{
  const read=name=>YAML.parse(fs.readFileSync(`.github/workflows/${name}.yml`,'utf8'));
  const connected=read('connected-upgrade'), release=read('release');
  const prRun=connected.jobs['connected-upgrade'].steps.find(step=>step.run?.includes('publish-verified-update.ps1')).run;
  const releaseRun=release.jobs['publish-windows'].steps.find(step=>step.run?.includes('publish-verified-update.ps1')).run;
  for(const code of [prRun,releaseRun]) assert(code.indexOf('test-connected-upgrade.ps1')<code.indexOf('publish-verified-update.ps1'));
  assert(prRun.includes('-VerifyOnly'));
  assert(!releaseRun.includes('-VerifyOnly'));
  assert(prRun.includes('github.event.pull_request.head.sha'));
  assert.equal(connected.jobs['connected-upgrade'].needs,'build-installer');
  assert.equal(connected.jobs['build-installer'].needs,'release-readiness');
  assert.equal(connected.jobs['connected-upgrade'].steps.find(step=>step.uses?.startsWith('actions/download-artifact')).with['run-id'],'${{ github.run_id }}');
  const acceptance=fs.readFileSync('scripts/test-connected-upgrade.ps1','utf8');
  assert(acceptance.includes('test-installed-recovery.ps1'));
});
