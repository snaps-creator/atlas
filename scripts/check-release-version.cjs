const fs = require('node:fs');
const {spawnSync} = require('node:child_process');

function checkReleaseVersion(version, query = args => spawnSync('git', args, {encoding: 'utf8', timeout: 30000})) {
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw Error('Expected a stable release version');
  const tag = `refs/tags/v${version}`;
  const result = query(['ls-remote', '--exit-code', '--tags', 'origin', tag]);
  if (result.error || ![0, 2].includes(result.status)) {
    throw Error(`Could not verify availability of v${version}; refusing to build a release candidate`);
  }
  if (result.status === 0) throw Error(`Version ${version} already has a release tag. Prepare a new version; published releases must not be overwritten.`);
  return `Version ${version} is available`;
}
module.exports = {checkReleaseVersion};
if (require.main === module) {
  try { console.log(checkReleaseVersion(JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json', 'utf8')).version)); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
