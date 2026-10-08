const {test} = require('node:test');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {spawnSync} = require('node:child_process');
const {prepare} = require('./prepare-release.cjs');
function fixture() {
  const {privateKey, publicKey} = crypto.generateKeyPairSync('ed25519');
  const id = crypto.randomBytes(8);
  const key = Buffer.from('untrusted comment: test\n' + Buffer.concat([Buffer.from('Ed'), id, publicKey.export({format:'der',type:'spki'}).subarray(-32)]).toString('base64') + '\n').toString('base64');
  function sign(bytes) {
    const sig = crypto.sign(null,crypto.createHash('blake2b512').update(bytes).digest(),privateKey), comment = Buffer.from('fixture only');
    return Buffer.from(['untrusted comment: test',Buffer.concat([Buffer.from('ED'),id,sig]).toString('base64'),'trusted comment: '+comment,crypto.sign(null,Buffer.concat([sig,comment]),privateKey).toString('base64')].join('\n')).toString('base64');
  }
  const artifact=Buffer.from('signed installer fixture'), version='2.4.2', build='a'.repeat(40), asset=`Atlas_${version}_x64-setup.exe`, repository='snaps-creator/atlas';
  const manifest={schema:2,installer_kind:'transactional-v1',version,build,asset,platform:'windows-x86_64',url:`https://github.com/${repository}/releases/download/v${version}/${asset}`,size:artifact.length,sha256:crypto.createHash('sha256').update(artifact).digest('hex'),signature:sign(artifact)};
  const bytes=Buffer.from(JSON.stringify(manifest));
  return {sign,manifest,input:{artifact,artifactSignature:manifest.signature,manifestBytes:bytes,manifestSignature:sign(bytes),publicKey:key,version,build,asset,repository}};
}
test('release preparation binds channel metadata to exact signed schema-2 artifact, repository and commit',()=>{
  const {input}=fixture(),result=prepare(input);
  assert.equal(result.tag,'v2.4.2');
  assert.equal(result.latest.platforms['windows-x86_64'].signature,input.artifactSignature);
  assert.equal(result.verified.build,input.build);
  for(const patch of [{build:'b'.repeat(40)},{repository:'other/atlas'},{version:'2.4.3'},{asset:'other.exe'},{artifact:Buffer.from('modified')},{manifestSignature:'bad'},{artifactSignature:'bad'}]) assert.throws(()=>prepare({...input,...patch}));
});
test('authentic but legacy/unknown installer schema is rejected before publication',()=>{
  for(const patch of [{schema:1},{schema:3},{installer_kind:'untrusted'}]) {
    const {input,manifest,sign}=fixture();const bytes=Buffer.from(JSON.stringify({...manifest,...patch}));
    assert.throws(()=>prepare({...input,manifestBytes:bytes,manifestSignature:sign(bytes)}));
  }
});
test('actual PowerShell VerifyOnly prepares all assets without any GitHub API call', {skip:process.platform!=='win32'}, t=>{
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'atlas-release-dry-run-'));
  t.after(()=>fs.rmSync(root,{recursive:true,force:true}));
  fs.mkdirSync(path.join(root,'scripts'));fs.mkdirSync(path.join(root,'src-tauri'));fs.mkdirSync(path.join(root,'artifacts'));
  for(const name of ['publish-verified-update.ps1','check-updater-readiness.cjs','prepare-release.cjs','verify-update-contract.cjs','verify-installer.cjs']) fs.copyFileSync(path.join('scripts',name),path.join(root,'scripts',name));
  fs.writeFileSync(path.join(root,'scripts/test-packaged-installer.ps1'),"param([string]$Installer)\n# UI execution is tested separately with the real signed installer on CI.\n");
  fs.copyFileSync('updater-readiness.json',path.join(root,'updater-readiness.json'));
  const {input}=fixture();
  fs.writeFileSync(path.join(root,'src-tauri/tauri.conf.json'),JSON.stringify({version:input.version,plugins:{updater:{pubkey:input.publicKey}}}));
  const installer=path.join(root,'artifacts',input.asset);
  fs.writeFileSync(installer,input.artifact);fs.writeFileSync(installer+'.sig',input.artifactSignature);
  fs.writeFileSync(path.join(root,'artifacts/update-manifest.json'),input.manifestBytes);fs.writeFileSync(path.join(root,'artifacts/update-manifest.json.sig'),input.manifestSignature);
  fs.writeFileSync(path.join(root,'run.ps1'),`function Invoke-RestMethod { throw 'FORBIDDEN_API_CALL' }\n& ./scripts/publish-verified-update.ps1 -Installer './artifacts/${input.asset}' -VerifyOnly\n`);
  const result=spawnSync('pwsh',['-NoProfile','-File','run.ps1'],{cwd:root,encoding:'utf8',env:{...process.env,GITHUB_REPOSITORY:input.repository,ATLAS_BUILD_ID:input.build,GITHUB_TOKEN:'',GITHUB_STEP_SUMMARY:''}});
  assert.ifError(result.error);assert.equal(result.status,0,result.stdout+'\n'+result.stderr);
  assert.match(result.stdout,/no GitHub release or upload performed/);
  const latest=JSON.parse(fs.readFileSync(path.join(root,'artifacts/latest.json')));
  assert.equal(latest.platforms['windows-x86_64'].signature,input.artifactSignature);
  assert.equal(JSON.parse(fs.readFileSync(path.join(root,'artifacts/release-preparation.json'))).verified.build,input.build);

  // Execute the real publication branch with an in-process API double. It must
  // keep a draft private until every asset upload has succeeded, and must never
  // publish when an upload fails. No request can leave this PowerShell process.
  fs.writeFileSync(path.join(root,'run.ps1'),`$global:atlasTestApiCalls = @()
function Invoke-RestMethod {
  param([Parameter(Position=0)][string]$Uri,[string]$Method,$Headers,[string]$ContentType,$Body,[string]$InFile)
  $textBody = if ($Body -is [byte[]]) { [Text.Encoding]::UTF8.GetString($Body) } else { $Body }
  $global:atlasTestApiCalls += [pscustomobject]@{uri=$Uri;method=$Method;body=$textBody;file=$InFile}
  if ($InFile) {
    if (-not (Test-Path -LiteralPath $InFile)) { throw 'Missing upload input' }
    if ($env:ATLAS_TEST_FAIL_UPLOAD -eq '1') { throw 'Injected upload failure' }
    return @{id=2}
  }
  if ($Method -eq 'Post') { return @{id=42;upload_url='https://uploads.invalid/42/assets{?name,label}'} }
  return @{html_url='https://example.invalid/release';tag_name='v2.4.2'}
}
try { & ./scripts/publish-verified-update.ps1 -Installer './artifacts/${input.asset}' }
finally { ConvertTo-Json -InputObject $global:atlasTestApiCalls -Depth 5 | Set-Content calls.json -Encoding utf8NoBOM }
`);
  for (const fail of ['0','1']) {
    const published=spawnSync('pwsh',['-NoProfile','-File','run.ps1'],{cwd:root,encoding:'utf8',env:{...process.env,
      GITHUB_REPOSITORY:input.repository,ATLAS_BUILD_ID:input.build,GITHUB_SHA:input.build,GITHUB_ACTIONS:'true',GITHUB_REF:'refs/heads/main',GITHUB_TOKEN:'test-only-token',GITHUB_STEP_SUMMARY:'',ATLAS_TEST_FAIL_UPLOAD:fail}});
    assert.ifError(published.error);
    assert(fs.existsSync(path.join(root,'calls.json')),published.stdout+'\n'+published.stderr);
    const calls=JSON.parse(fs.readFileSync(path.join(root,'calls.json')));
    assert(calls.length>0,published.stdout+'\n'+published.stderr);
    const create=JSON.parse(calls[0].body);
    assert.equal(create.draft,true);assert.equal(create.target_commitish,input.build);assert.equal(create.tag_name,'v2.4.2');
    if(fail==='1') {
      assert.notEqual(published.status,0);
      assert(!calls.some(call=>call.method==='Patch'));
    } else {
      assert.equal(published.status,0,published.stdout+'\n'+published.stderr);
      assert.equal(calls.length,7);
      assert.deepEqual(calls.slice(1,-1).map(call=>path.basename(call.file)),[input.asset,input.asset+'.sig','latest.json','update-manifest.json','update-manifest.json.sig']);
      assert.equal(calls.at(-1).method,'Patch');
      assert.equal(JSON.parse(calls.at(-1).body).draft,false);
    }
  }
});
