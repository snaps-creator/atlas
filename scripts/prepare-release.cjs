const fs = require('node:fs');
const path = require('node:path');
const {verifyUpdate} = require('./verify-update-contract.cjs');
function prepare({manifestBytes, manifestSignature, artifact, artifactSignature, publicKey, version, build, repository, asset, notes}) {
  if (!/^[a-f0-9]{40}$/.test(build || '') || !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository || '')) throw Error('Release build/repository context is invalid');
  if (!/^\d+\.\d+\.\d+$/.test(version) || asset !== `Atlas-Setup-${version}.exe`) throw Error('Unexpected release version/asset');
  const tag = `v${version}`;
  const url = `https://github.com/${repository}/releases/download/${tag}/${asset}`;
  const verified = verifyUpdate({manifestBytes, manifestSignature, artifact, publicKey,
    expected: {schema: 2, version, installerVersion: version, build, asset, url}});
  const authenticated = JSON.parse(manifestBytes.toString('utf8'));
  if (authenticated.signature.trim() !== artifactSignature.trim()) throw Error('Detached installer signature differs from authenticated manifest');
  const latest = {version, notes: notes || 'Восстановление сети Atlas и транзакционное обновление. Подробности — в docs/UPDATES.md.',
    pub_date: new Date().toISOString(), platforms: {'windows-x86_64': {signature: artifactSignature.trim(), url}}};
  return {tag, latest, verified};
}
module.exports = {prepare};
if (require.main === module) {
  try {
    const installer = process.argv[2];
    const directory = path.dirname(installer), manifest = path.join(directory, 'update-manifest.json');
    const config = JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json'));
    const result = prepare({manifestBytes: fs.readFileSync(manifest), manifestSignature: fs.readFileSync(manifest + '.sig', 'utf8'),
      artifact: fs.readFileSync(installer), artifactSignature: fs.readFileSync(installer + '.sig', 'utf8'),
      publicKey: config.plugins.updater.pubkey, version: config.version, build: process.env.ATLAS_BUILD_ID,
      repository: process.env.GITHUB_REPOSITORY, asset: path.basename(installer),
      notes: fs.existsSync(`docs/releases/${config.version}/RELEASE-NOTES.md`) ? fs.readFileSync(`docs/releases/${config.version}/RELEASE-NOTES.md`,'utf8') : undefined});
    fs.writeFileSync(path.join(directory, 'latest.json'), JSON.stringify(result.latest, null, 2) + '\n');
    fs.writeFileSync(path.join(directory, 'release-preparation.json'), JSON.stringify(result, null, 2) + '\n');
    console.log(JSON.stringify({verified: true, ...result.verified, tag: result.tag}));
  } catch (error) {console.error(error.message); process.exitCode = 1;}
}
