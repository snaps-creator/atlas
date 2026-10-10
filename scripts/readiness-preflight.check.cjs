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
  assert.deepEqual(release.jobs['publish-windows'].needs, ['release-readiness','release-acceptance','release-backend']);
  assert.equal(release.jobs['release-acceptance'].uses,'./.github/workflows/connected-upgrade.yml');
  assert.equal(release.jobs['release-backend'].uses,'./.github/workflows/pr-check.yml');
  assert(!release.jobs['publish-windows'].if?.includes('always()'), 'must not bypass failed dependency');
  assert.match(release.jobs['publish-windows'].if, /inputs.confirmation/);
  assert(release.on.workflow_dispatch && !release.on.push);
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
  assert(prRun.indexOf('test-connected-upgrade.ps1')<prRun.indexOf('publish-verified-update.ps1'));
  assert(!releaseRun.includes('build-release-installer.ps1'), 'publish must use the accepted artifact, not rebuild it');
  const publishedArtifact=release.jobs['publish-windows'].steps.find(step=>step.uses?.startsWith('actions/download-artifact')).with.name;
  const built=read('pr-installer').jobs.installer.steps.find(step=>step.with?.name?.startsWith('Atlas-Windows-x64')).with.name;
  assert.equal(publishedArtifact,built);
  assert(prRun.includes('-VerifyOnly'));
  assert(!releaseRun.includes('-VerifyOnly'));
  assert(prRun.includes('github.event.pull_request.head.sha || github.sha'));
  assert.deepEqual(connected.jobs['connected-upgrade'].needs,['build-installer','baseline-243']);
  assert.deepEqual(connected.jobs['connected-upgrade'].strategy.matrix.baseline,['legacy','2.4.2','2.4.3']);
  assert.equal(connected.jobs['build-installer'].needs,'release-readiness');
  assert.equal(connected.jobs['connected-upgrade'].steps.find(step=>step.uses?.startsWith('actions/download-artifact')).with['run-id'],'${{ github.run_id }}');
  const acceptance=fs.readFileSync('scripts/test-connected-upgrade.ps1','utf8');
  assert(acceptance.includes('test-installed-recovery.ps1'));
});

test('real PowerShell fixture replacement preserves the journal backup and supports repeated setup', {skip:process.platform!=='win32'}, t=>{
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'atlas-recovery-fixture-'));
  t.after(()=>fs.rmSync(root,{recursive:true,force:true}));
  const source=path.resolve('scripts/test-installed-recovery.ps1').replaceAll("'","''");
  const script=`$ErrorActionPreference = 'Stop'
$tokens=$null; $errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile('${source}',[ref]$tokens,[ref]$errors)
if ($errors.Count) { throw 'Fixture script does not parse' }
$function=$ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Write-InterruptedRecoveryFixture'},$true)
if (-not $function) { throw 'Missing fixture writer' }
. ([scriptblock]::Create($function.Extent.Text))
foreach ($sequence in @(1,10)) {
    $journal=[pscustomobject]@{stage='Committed';health_state='verified';sequence=$sequence;payload=@{hash='unchanged'}}
    $journal | ConvertTo-Json -Depth 5 | Set-Content current.json -Encoding utf8NoBOM
    Write-InterruptedRecoveryFixture -Root $PWD.Path -Journal $journal
    $active=Get-Content current.json -Raw | ConvertFrom-Json
    $backup=Get-Content recovery-before.json -Raw | ConvertFrom-Json
    if ($active.stage -ne 'HealthPending' -or $active.sequence -ne ($sequence+1) -or $active.payload.hash -ne 'unchanged') { throw 'Incorrect injected journal' }
    if ($backup.stage -ne 'Committed' -or $backup.sequence -ne $sequence) { throw 'Original journal backup lost' }
    if (Test-Path recovery-fixture.json) { throw 'Replacement left the staging file' }
}
`;
  fs.writeFileSync(path.join(root,'check.ps1'),script);
  const result=spawnSync('pwsh',['-NoProfile','-File','check.ps1'],{cwd:root,encoding:'utf8'});
  assert.ifError(result.error);assert.equal(result.status,0,result.stdout+'\n'+result.stderr);
});
