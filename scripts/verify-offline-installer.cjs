const fs = require('node:fs');
const crypto = require('node:crypto');
const {verifySignature} = require('./verify-installer.cjs');
const manifest = JSON.parse(fs.readFileSync('installer/manifest.json','utf8'));
const parts = Array.from({length:manifest.parts},(_,i)=>fs.readFileSync(`installer/atlas-setup.part${i+1}`));
for (const part of parts) if (part.length >= 100*1024*1024) throw Error('Part exceeds GitHub regular Git file limit');
const data = Buffer.concat(parts);
if (data.length !== manifest.size || crypto.createHash('sha256').update(data).digest('hex') !== manifest.sha256) throw Error('Payload hash mismatch');
const config = JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json','utf8'));
console.log(verifySignature(data,fs.readFileSync('installer/atlas-setup.exe.sig','utf8'),config.plugins.updater.pubkey));
if (fs.readFileSync('Install Atlas.exe').subarray(0,2).toString() !== 'MZ') throw Error('Launcher is not a Windows EXE');
// CI can exercise the exact signed offline package without waiting for a new
// Rust build. Materialize only after all hash/signature checks, never overwrite.
if (process.argv[2]) {
  fs.writeFileSync(process.argv[2], data, {flag:'wx'});
  fs.copyFileSync('installer/atlas-setup.exe.sig', `${process.argv[2]}.sig`, fs.constants.COPYFILE_EXCL);
}
