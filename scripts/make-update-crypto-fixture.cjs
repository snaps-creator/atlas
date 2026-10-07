// Generates public test evidence. The ephemeral private key never leaves memory.
const fs=require('node:fs'),crypto=require('node:crypto');
const {privateKey,publicKey}=crypto.generateKeyPairSync('ed25519');const id=crypto.randomBytes(8);
const key=Buffer.from('untrusted comment: test only\n'+Buffer.concat([Buffer.from('Ed'),id,publicKey.export({format:'der',type:'spki'}).subarray(-32)]).toString('base64')).toString('base64');
function sign(bytes){const signature=crypto.sign(null,crypto.createHash('blake2b512').update(bytes).digest(),privateKey);const comment=Buffer.from('Atlas fixture');return Buffer.from(['untrusted comment: test only',Buffer.concat([Buffer.from('ED'),id,signature]).toString('base64'),'trusted comment: '+comment,crypto.sign(null,Buffer.concat([signature,comment]),privateKey).toString('base64')].join('\n')).toString('base64');}
const artifact=Buffer.from('native updater cryptographic test payload');const sha256=crypto.createHash('sha256').update(artifact).digest('hex');
const manifest={schema:1,version:'2.4.1',build:'testbuild',platform:'windows-x86_64',asset:'Atlas-2.4.1-Setup.exe',url:'https://github.com/snaps-creator/atlas/releases/download/v2.4.1/Atlas-2.4.1-Setup.exe',size:artifact.length,sha256,signature:sign(artifact),helper_protocol:1,
files:['Atlas.exe','Atlas.Service.exe','AtlasMaintenance.exe','libcef.dll','resources/Atlas.Core.exe','resources/Atlas.Xray.exe'].map(path=>({path,size:artifact.length,sha256}))};
const manifestBytes=Buffer.from(JSON.stringify(manifest));
const bad=Buffer.from(JSON.stringify({...manifest,helper_protocol:99}));
fs.mkdirSync('src-tauri/testdata',{recursive:true});fs.writeFileSync('src-tauri/testdata/update-crypto.json',JSON.stringify({publicKey:key,artifact:artifact.toString('base64'),manifest:manifestBytes.toString('base64'),signature:sign(manifestBytes),incompatibleManifest:bad.toString('base64'),incompatibleSignature:sign(bad)},null,2)+'\n');
