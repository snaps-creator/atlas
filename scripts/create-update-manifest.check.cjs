const {test}=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),os=require('node:os'),path=require('node:path');
const {create}=require('./create-update-manifest.cjs');
test('manifest binds verified artifact and staged runtime without reading a signing key',()=>{
  const fixture=JSON.parse(fs.readFileSync('src-tauri/testdata/update-crypto.json'));
  const signed=JSON.parse(Buffer.from(fixture.manifest,'base64'));
  const dir=fs.mkdtempSync(path.join(os.tmpdir(),'atlas-manifest-test-'));
  const installer=path.join(dir,`Atlas-Setup-${signed.version}.exe`);
  try{
    fs.writeFileSync(installer,Buffer.from(fixture.artifact,'base64'));fs.writeFileSync(installer+'.sig',signed.signature);
    const files=[...signed.files.filter(f=>f.path!=='Atlas.Service.exe'),{...signed.files[0],path:'AtlasUpdater.exe'}].map(f=>({...f,stagedSha256:f.sha256,extractedSha256:f.sha256}));
    const args={installer,evidence:{verified:true,version:signed.version,files},version:signed.version,build:signed.build,publicKey:fixture.publicKey};
    const manifest=create(args);assert.equal(manifest.sha256,signed.sha256);assert.equal(manifest.version,signed.version);
    assert.deepEqual(manifest.files.find(f=>f.path==='Atlas.Service.exe'),{...manifest.files.find(f=>f.path==='Atlas.exe'),path:'Atlas.Service.exe'});
    assert.throws(()=>create({...args,evidence:{...args.evidence,verified:false}}));
    assert.throws(()=>create({...args,evidence:{...args.evidence,files:files.filter(f=>f.path!=='AtlasUpdater.exe')}}));
    assert.throws(()=>create({...args,evidence:{...args.evidence,files:files.map(f=>({...f,stagedSha256:'bad'}))}}));
    const evidence={version:signed.version,payloadVerified:true,corruptedPayloadRejected:true,sha256:signed.sha256,files};
    const transactional=create({...args,evidence});
    assert.equal(transactional.schema,2);
    assert.equal(transactional.installer_kind,'transactional-v1');
    for(const patch of [{sha256:'0'.repeat(64)},{payloadVerified:false},{corruptedPayloadRejected:false}]) {
      assert.throws(()=>create({...args,evidence:{...evidence,...patch}}));
    }
    fs.appendFileSync(installer,'bad');assert.throws(()=>create(args));
  }finally{fs.rmSync(dir,{recursive:true});}
});
