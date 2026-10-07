const { test } = require('node:test');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const { verifyUpdate } = require('./verify-update-contract.cjs');
// Ephemeral test keys only. No production key is read, copied, or persisted.
function signer() {
  const { privateKey, publicKey } = crypto.generateKeyPairSync('ed25519');
  const id = crypto.randomBytes(8);
  const raw = publicKey.export({ format: 'der', type: 'spki' }).subarray(-32);
  const key = Buffer.from('untrusted comment: test\n' + Buffer.concat([Buffer.from('Ed'), id, raw]).toString('base64') + '\n').toString('base64');
  return { key, sign(bytes) {
    const sig = crypto.sign(null, crypto.createHash('blake2b512').update(bytes).digest(), privateKey);
    const comment = Buffer.from('Atlas test');
    return Buffer.from(['untrusted comment: test', Buffer.concat([Buffer.from('ED'), id, sig]).toString('base64'),
      'trusted comment: ' + comment, crypto.sign(null, Buffer.concat([sig, comment]), privateKey).toString('base64')].join('\n')).toString('base64');
  }};
}
function fixture() {
  const s = signer(), artifact = Buffer.from('exact executable fixture bytes');
  const expected = {version:'2.4.1', installerVersion:'2.4.1', build:'a'.repeat(40), asset:'Atlas-2.4.1-Setup.exe', url:'https://github.com/snaps-creator/atlas/releases/download/v2.4.1/Atlas-2.4.1-Setup.exe'};
  const manifest = { schema:1, ...expected, platform:'windows-x86_64', size:artifact.length,
    sha256:crypto.createHash('sha256').update(artifact).digest('hex'), signature:s.sign(artifact) };
  const manifestBytes = Buffer.from(JSON.stringify(manifest));
  return {s, manifest, input:{manifestBytes, manifestSignature:s.sign(manifestBytes), artifact, publicKey:s.key, expected}};
}
test('exact artifact, signed manifest, public key and build identity verify together', () => assert.equal(verifyUpdate(fixture().input).verified,true));
test('one altered installer byte is rejected', () => {const {input}=fixture();input.artifact[0]^=1;assert.throws(()=>verifyUpdate(input));});
test('damaged manifest signature is rejected', () => {const {input}=fixture();input.manifestSignature='AAAA';assert.throws(()=>verifyUpdate(input));});
test('wrong public key is rejected', () => {const {input}=fixture();input.publicKey=signer().key;assert.throws(()=>verifyUpdate(input));});
for(const [field,value] of [['version','2.4.2'],['build','b'.repeat(40)],['asset','other.exe'],['url','https://github.com/other/package.exe'],['signature','AAAA']]) {
  test(`authentically signed but wrong ${field} is rejected`,()=>{const {s,manifest,input}=fixture();manifest[field]=value;input.manifestBytes=Buffer.from(JSON.stringify(manifest));input.manifestSignature=s.sign(input.manifestBytes);assert.throws(()=>verifyUpdate(input));});
}
test('manifest tampering is rejected before parsing',()=>{const {input}=fixture();input.manifestBytes[0]^=1;assert.throws(()=>verifyUpdate(input));});
