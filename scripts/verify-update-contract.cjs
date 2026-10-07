const fs = require('node:fs');
const crypto = require('node:crypto');
const { verifySignature } = require('./verify-installer.cjs');
const sha256 = bytes => crypto.createHash('sha256').update(bytes).digest('hex');

function verifyUpdate({ manifestBytes, manifestSignature, artifact, publicKey, expected }) {
  // Authenticate the exact manifest bytes before interpreting its destinations.
  verifySignature(manifestBytes, manifestSignature, publicKey);
  const manifest = JSON.parse(manifestBytes.toString('utf8'));
  if (manifest.schema !== 1 || manifest.version !== expected.version ||
      manifest.build !== expected.build || manifest.platform !== 'windows-x86_64' ||
      manifest.asset !== expected.asset || manifest.url !== expected.url ||
      manifest.version !== expected.installerVersion) throw Error('Manifest identity mismatch');
  const url = new URL(manifest.url);
  if (url.protocol !== 'https:' || url.username || url.password || url.hash || url.search ||
      url.hostname !== 'github.com' || !url.pathname.endsWith('/' + manifest.asset)) throw Error('Invalid immutable artifact URL');
  if (!Number.isSafeInteger(manifest.size) || manifest.size <= 0 || manifest.size !== artifact.length ||
      !/^[a-f0-9]{64}$/.test(manifest.sha256) || sha256(artifact) !== manifest.sha256) throw Error('Artifact digest mismatch');
  verifySignature(artifact, manifest.signature, publicKey);
  return { verified: true, version: manifest.version, build: manifest.build,
    artifactSha256: sha256(artifact), manifestSha256: sha256(manifestBytes),
    manifestSignatureSha256: sha256(Buffer.from(manifestSignature)),
    artifactSignatureSha256: sha256(Buffer.from(manifest.signature)) };
}
module.exports = { verifyUpdate };
if (require.main === module) {
  try {
    const [manifest, signature, artifact, expectation] = process.argv.slice(2);
    const config = JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json'));
    console.log(JSON.stringify(verifyUpdate({ manifestBytes: fs.readFileSync(manifest),
      manifestSignature: fs.readFileSync(signature, 'utf8'), artifact: fs.readFileSync(artifact),
      publicKey: config.plugins.updater.pubkey, expected: JSON.parse(fs.readFileSync(expectation)) }), null, 2));
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
