// Verify Tauri/minisign updater signatures against the app's configured public key.
const fs = require('node:fs');
const crypto = require('node:crypto');
function verifySignature(data, signature, publicKey) {
  const keyLines = Buffer.from(publicKey.trim(), 'base64').toString('utf8').trim().split(/\r?\n/);
  const lines = Buffer.from(signature.trim(), 'base64').toString('utf8').trim().split(/\r?\n/);
  const key = Buffer.from(keyLines[1] || '', 'base64');
  const packet = Buffer.from(lines[1] || '', 'base64');
  if (key.length !== 42 || packet.length !== 74 || key.subarray(0,2).toString() !== 'Ed' ||
      !key.subarray(2,10).equals(packet.subarray(2,10)) || !lines[2]?.startsWith('trusted comment: ')) {
    throw Error('Invalid updater signature structure or signing key ID');
  }
  const algorithm = packet.subarray(0,2).toString();
  if (!['Ed','ED'].includes(algorithm)) throw Error('Unsupported signature algorithm');
  const publicKeyObject = crypto.createPublicKey({key:Buffer.concat([
    Buffer.from('302a300506032b6570032100','hex'),key.subarray(10)]),format:'der',type:'spki'});
  const message = algorithm === 'ED' ? crypto.createHash('blake2b512').update(data).digest() : data;
  if (!crypto.verify(null,message,publicKeyObject,packet.subarray(10))) throw Error('Installer signature does not match bytes');
  const trusted = Buffer.from(lines[2].slice('trusted comment: '.length));
  if (!crypto.verify(null,Buffer.concat([packet.subarray(10),trusted]),publicKeyObject,Buffer.from(lines[3] || '', 'base64'))) {
    throw Error('Trusted signature comment verification failed');
  }
  return {sha256:crypto.createHash('sha256').update(data).digest('hex'),bytes:data.length};
}
module.exports = {verifySignature};
if (require.main === module) {
  try {
    const config = JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json','utf8'));
    const version = config.version.replace(/^1\.0\.0-beta\./,'');
    const file = process.argv[2] || `Atlas Beta ${version} Setup.exe`;
    const result = verifySignature(fs.readFileSync(file),fs.readFileSync(file+'.sig','utf8'),config.plugins.updater.pubkey);
    console.log(JSON.stringify({verified:true,version:config.version,file,...result}));
  } catch (error) { console.error(error.message); process.exitCode=1; }
}
