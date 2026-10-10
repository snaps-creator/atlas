const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const cp=require('node:child_process');
const YAML=require('yaml');
test('installers and split offline payloads cannot be distributed through the Git tree',()=>{
  const r=cp.spawnSync('git',['ls-files','--cached'],{encoding:'utf8'});assert.equal(r.status,0);
  const forbidden=r.stdout.split(/\r?\n/).filter(p=>/^(installer\/|Install Atlas\.exe$)/i.test(p)||(/\.exe(?:\.sig)?$/i.test(p)&&!p.startsWith('src-tauri/resources/')));
  assert.deepEqual(forbidden,[]);
  assert(!fs.existsSync('.gitattributes')||!/filter=lfs/.test(fs.readFileSync('.gitattributes','utf8')));
});
test('publication requires separate manual authorization on main and preserves signed verification',()=>{
  const workflow=YAML.parse(fs.readFileSync('.github/workflows/release.yml','utf8'));
  assert(workflow.on.workflow_dispatch);assert(!workflow.on.push);
  assert.match(workflow.jobs['publish-windows'].if,/refs\/heads\/main/);
  assert.match(workflow.jobs['publish-windows'].if,/inputs.confirmation/);
  const script=fs.readFileSync('scripts/publish-verified-update.ps1','utf8');
  for(const requirement of ['workflow_dispatch','ATLAS_RELEASE_AUTHORIZATION','ATLAS_BUILD_ID','GITHUB_SHA','prepare-release.cjs','test-packaged-installer.ps1'])assert(script.includes(requirement));
});
test('one public full installer is paired with metadata and temporary CI evidence',()=>{
  const version=JSON.parse(fs.readFileSync('package.json')).version;
  assert.equal(version,'2.5.1');
  const build=fs.readFileSync('scripts/build-release-installer.ps1','utf8');
  assert(build.includes('Atlas-Setup-${version}.exe'));
  assert(build.includes('verify-transactional-installer.ps1'));
  const pr=YAML.parse(fs.readFileSync('.github/workflows/pr-installer.yml','utf8'));
  assert(pr.jobs.installer.steps.some(s=>s.with?.path?.includes('Atlas-Setup-*.exe')));
});
