const {test}=require('node:test');const assert=require('node:assert/strict');const fs=require('node:fs');const cp=require('node:child_process');
const {check}=require('./check-updater-readiness.cjs');
test('readiness refuses every incomplete gate, including signed artifact',()=>{
  const value={phase0:Object.fromEntries(['P0-A','P0-B','P0-C','P0-D'].map(k=>[k,'PASS'])),releaseReady:true};
  for(const k of ['buildProvenance','packageVerification','signatureVerification','transactionSafety','rollback','crashRecovery','networkRecovery','integrationTests'])value[k]='PASS';
  check(value);for(const k of Object.keys(value).filter(k=>!['phase0','releaseReady'].includes(k))){assert.throws(()=>check({...value,[k]:'NOT VERIFIED'}));}
  assert.throws(()=>check({...value,phase0:{...value.phase0,'P0-C':'BLOCKED'}}));
  assert.throws(()=>check({...value,releaseReady:false}));
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
