// Capture only public build inputs and artifacts; never enumerate environment variables.
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const cp = require('node:child_process');
const root = path.resolve(__dirname, '..');
const target = path.resolve(process.argv[2] || process.env.CARGO_TARGET_DIR || path.join(root, 'src-tauri/target'));
const out = path.resolve(process.argv[3] || path.join(root, 'temp/update-build/evidence'));
fs.mkdirSync(out, { recursive: true });
const config = JSON.parse(fs.readFileSync(path.join(root, 'src-tauri/tauri.conf.json')));
const hash = file => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const record = file => ({ path: file, size: fs.statSync(file).size, sha256: hash(file) });
const git = (...args) => cp.execFileSync('git', args, {cwd:root, encoding:'utf8'}).trim();
function tree(dir) {
  return fs.readdirSync(dir, {withFileTypes:true}).flatMap(e => e.isDirectory() ? tree(path.join(dir,e.name)) : [record(path.join(dir,e.name))]);
}
const lock = JSON.parse(fs.readFileSync(path.join(root, 'package-lock.json')));
const js = {};
for (const name of ['@tauri-apps/cli', '@tauri-apps/plugin-updater', '@tauri-apps/api']) {
  const actual = JSON.parse(fs.readFileSync(path.join(root, 'node_modules', name, 'package.json'))).version;
  if (actual !== lock.packages[`node_modules/${name}`].version) throw Error(`${name} differs from lockfile`);
  js[name] = actual;
}
const nsis = path.join(target, 'release/nsis/x64/installer.nsi');
const script = fs.readFileSync(nsis, 'utf8');
if (!script.includes(config.version)) throw Error('Generated NSIS does not contain current version');
fs.copyFileSync(nsis, path.join(out, 'installer.nsi'));
const installer = path.join(target, `release/bundle/nsis/Atlas_${config.version}_x64-setup.exe`);
const files = [installer, nsis, path.join(target, 'release/Atlas.exe'), path.join(target, 'release/AtlasMaintenance.exe'), path.join(target, 'release/AtlasUpdater.exe')];
if (fs.existsSync(installer + '.sig')) files.push(installer + '.sig');
const metadata = cp.execFileSync('cargo', ['metadata', '--locked', '--offline', '--filter-platform', 'x86_64-pc-windows-msvc', '--format-version', '1', '--manifest-path', path.join(root, 'src-tauri/Cargo.toml')], { maxBuffer: 32 * 1024 * 1024 });
fs.writeFileSync(path.join(out, 'cargo-metadata.json'), metadata);
const evidence = {
  gitCommit: git('rev-parse','HEAD'), gitTreeState: git('status','--porcelain') ? 'dirty' : 'clean',
  branch: git('branch','--show-current'), status: git('status','--porcelain'),
  gitTree: git('rev-parse','HEAD^{tree}'),
  workflowRunId: process.env.GITHUB_RUN_ID || null, workflowAttempt: process.env.GITHUB_RUN_ATTEMPT || null,
  timestampUtc: new Date().toISOString(), cargoTarget: 'x86_64-pc-windows-msvc',
  npm: JSON.parse(fs.readFileSync(path.join(path.dirname(process.env.ATLAS_BUILD_NPM_CLI || cp.execFileSync('where.exe',['npm.cmd'],{encoding:'utf8'}).trim().split(/\r?\n/)[0]), process.env.ATLAS_BUILD_NPM_CLI ? '../package.json' : 'node_modules/npm/package.json'))).version,
  version: config.version, build: process.env.ATLAS_BUILD_ID || 'local-dev',
  node: process.version, js,
  nativeTools: { msvc: process.env.VCToolsVersion || null, windowsSdk: process.env.WindowsSDKVersion || null },
  rust: cp.execFileSync('rustc', ['-vV'], { encoding: 'utf8' }).trim(),
  signedArtifactPresent: fs.existsSync(installer + '.sig'),
  inputs: ['package-lock.json', 'src-tauri/Cargo.lock', 'src-tauri/tauri.conf.json', 'src-tauri/installer-hooks.nsh', 'rust-toolchain.toml'].map(f => record(path.join(root, f))),
  artifacts: files.map(record),
  frontendDist: tree(path.join(root,'dist')),
  signature: fs.existsSync(installer + '.sig') ? record(installer + '.sig') : null,
  updateManifest: fs.existsSync(path.join(path.dirname(installer),'update-manifest.json')) ? record(path.join(path.dirname(installer),'update-manifest.json')) : null,
};
fs.writeFileSync(path.join(out, 'build.json'), JSON.stringify(evidence, null, 2) + '\n');
console.log(`Build evidence: ${out}`);
