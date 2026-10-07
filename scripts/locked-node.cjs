// NODE_OPTIONS preloader inherited by npm, Tauri beforeBuildCommand and Node children.
const fs = require('node:fs');
const path = require('node:path');
if (process.versions.node !== '22.23.3') throw Error(`Atlas build requires Node 22.23.3; actual ${process.versions.node}`);
const cli = process.env.ATLAS_BUILD_NPM_CLI;
if (!cli || JSON.parse(fs.readFileSync(path.resolve(path.dirname(cli), '../package.json'))).version !== '11.16.0') {
  throw Error('Atlas build requires npm 11.16.0');
}
