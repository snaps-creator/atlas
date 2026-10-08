const {test}=require('node:test');const assert=require('node:assert/strict');const fs=require('node:fs');const cp=require('node:child_process');
const {check}=require('./check-updater-readiness.cjs');
test('publication policy cannot omit automated checks or conceal untested reboot',()=>{
  const value=JSON.parse(fs.readFileSync('updater-readiness.json'));
  check(value,value.version);
  for(const required of value.requiredChecks) assert.throws(()=>check({...value,requiredChecks:value.requiredChecks.filter(k=>k!==required)}));
  assert.throws(()=>check({...value,schema:1,releaseReady:true}));
  assert.throws(()=>check({...value,releaseReady:true}));
  assert.throws(()=>check(value,'9.9.9'));
  assert.throws(()=>check({...value,acceptedRisks:[]}));
  assert.throws(()=>check({...value,acceptedRisks:value.acceptedRisks.map(r=>({...r,status:'PASS'}))}));
  assert.throws(()=>check({...value,acceptedRisks:value.acceptedRisks.map(r=>({...r,accepted:false}))}));
});
test('unverified legacy installation cannot be invoked by frontend',()=>{
  const safe=c=>!c.permissions.some(p=>['updater:default','updater:allow-install','updater:allow-download-and-install'].includes(p));
  assert.equal(safe({permissions:['updater:default']}),false);
  assert.equal(safe(JSON.parse(fs.readFileSync('src-tauri/capabilities/default.json'))),true);
  assert(!fs.readFileSync('src/main.tsx','utf8').includes('latest.install('));
});
test('NSIS upgrade aborts before maintenance, including alternate install directory',()=>{
  const hooks=fs.readFileSync('src-tauri/installer-hooks.nsh','utf8');
  const pre=hooks.slice(hooks.indexOf('!macro NSIS_HOOK_PREINSTALL'),hooks.indexOf('!macro NSIS_HOOK_POSTINSTALL'));
  assert(pre.indexOf('AtlasNetworkService')<pre.indexOf('InitPluginsDir'));
  assert(pre.indexOf('Abort')<pre.indexOf('InitPluginsDir'));
  assert(pre.includes('ReadRegStr $0 SHCTX "${UNINSTKEY}" "InstallLocation"'));
  const entry=hooks.slice(hooks.indexOf('Function AtlasUpgradeEntry'),hooks.indexOf('FunctionEnd'));
  assert(entry.includes('HKLM "${UNINSTKEY}" "UninstallString"'));
  assert(entry.includes('HKCU "${UNINSTKEY}" "UninstallString"'));
  assert(entry.includes('SetErrorLevel 5'));assert(!entry.includes('ExecWait'));
  assert(!entry.includes('Return'));
});
