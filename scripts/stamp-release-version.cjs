const fs = require('node:fs');
function releaseVersion(base, run, attempt) {
  if (!/^\d+\.\d+\.\d+-alpha\.\d+(?:\.\d+)*$/.test(base)) throw Error('Expected an Atlas alpha version');
  if (![run, attempt].every(value => /^[1-9]\d*$/.test(String(value)))) throw Error('Invalid release build number');
  return `${base.split('-')[0]}-alpha.${run}.${attempt}`;
}
module.exports = { releaseVersion };
if (require.main === module) {
  const config = JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json', 'utf8'));
  const version = releaseVersion(config.version, process.env.GITHUB_RUN_NUMBER, process.env.GITHUB_RUN_ATTEMPT);
  for (const path of ['src-tauri/tauri.conf.json', 'package.json', 'package-lock.json']) {
    const value = JSON.parse(fs.readFileSync(path, 'utf8'));
    value.version = version;
    if (value.packages?.['']) value.packages[''].version = version;
    fs.writeFileSync(path, JSON.stringify(value, null, 2) + '\n');
  }
  const cargo = fs.readFileSync('src-tauri/Cargo.toml', 'utf8');
  fs.writeFileSync('src-tauri/Cargo.toml', cargo.replace(/^(version\s*=\s*)"[^"]+"/m, `$1"${version}"`));
  console.log(`Release build: ${version}; display label remains Alpha ${version.split('-')[0]}`);
}
