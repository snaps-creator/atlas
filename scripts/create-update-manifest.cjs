// Public inputs only. Signing is a separate CI step using the existing Tauri
// signer and its environment; this module never reads a private key.
const fs=require('node:fs'),path=require('node:path');
const {hash}=require('./verify-package-content.cjs');
const {verifySignature}=require('./verify-installer.cjs');
function create({installer,evidence,version,build,publicKey}){
  const transactional=evidence.payloadVerified===true&&evidence.corruptedPayloadRejected===true;
  if(!(transactional||evidence.verified)||evidence.version!==version||!build||!/^[a-zA-Z0-9_.-]{1,100}$/.test(build))throw Error('Unverified package/build identity');
  const asset=path.basename(installer);if(asset!==`Atlas-Setup-${version}.exe`)throw Error('Unexpected installer asset name');
  const signature=fs.readFileSync(installer+'.sig','utf8').trim();
  const artifact=verifySignature(fs.readFileSync(installer),signature,publicKey);
  if(transactional&&evidence.sha256!==artifact.sha256)throw Error('Transactional installer differs from verification evidence');
  const files=evidence.files.filter(f=>!['bootstrap.exe','bootstrapc.exe'].includes(f.path)).map(f=>{
    if(!transactional&&f.stagedSha256!==f.extractedSha256)throw Error('Unverified staged bytes');
    const sha256=transactional?f.sha256:f.extractedSha256;
    if(!/^[a-f0-9]{64}$/i.test(sha256)||!Number.isSafeInteger(f.size)||f.size<0)throw Error('Invalid payload evidence');
    return {path:f.path,size:f.size,sha256};
  });
  for(const required of ['Atlas.exe','AtlasMaintenance.exe','AtlasUpdater.exe','libcef.dll','resources/Atlas.Core.exe','resources/Atlas.Xray.exe'])if(!files.some(f=>f.path===required))throw Error('Missing signed runtime member: '+required);
  const ui=files.find(f=>f.path==='Atlas.exe');
  const service=files.find(f=>f.path==='Atlas.Service.exe');
  if(service&&(service.sha256!==ui.sha256||service.size!==ui.size))throw Error('UI/service identity mismatch');
  if(!service)files.push({...ui,path:'Atlas.Service.exe'});files.sort((a,b)=>a.path<b.path?-1:a.path>b.path?1:0);
  return {schema:transactional?2:1,...(transactional?{installer_kind:'transactional-v1'}:{}),version,build,platform:'windows-x86_64',asset,url:`https://github.com/snaps-creator/atlas/releases/download/v${version}/${asset}`,
    size:artifact.bytes,sha256:artifact.sha256,signature,helper_protocol:1,files};
}
module.exports={create};
if(require.main===module){try{
  const [installer,evidenceFile,output]=process.argv.slice(2),config=JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json'));
  const value=create({installer,evidence:JSON.parse(fs.readFileSync(evidenceFile)),version:config.version,build:process.env.ATLAS_BUILD_ID,publicKey:config.plugins.updater.pubkey});
  fs.writeFileSync(output,JSON.stringify(value,null,2)+'\n',{flag:'wx'});console.log(JSON.stringify({manifest:output,manifestSha256:hash(output),artifactSha256:value.sha256}));
}catch(e){console.error(e.message);process.exitCode=1;}}
