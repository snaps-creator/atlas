const {test}=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),os=require('node:os'),path=require('node:path');
const {pe}=require('./verify-package-content.cjs');
test('real shipped core is parsed as AMD64 PE, not inferred from its name',()=>{
  const image=pe('src-tauri/resources/Atlas.Xray.exe');assert.equal(image.machine,'AMD64');assert.equal(image.format,'PE32+');assert(image.imports.some(v=>v.toLowerCase()==='kernel32.dll'));
});
test('PE parser rejects renamed text, truncated headers and architecture mismatch',()=>{
  const dir=fs.mkdtempSync(path.join(os.tmpdir(),'atlas-pe-test-')),file=path.join(dir,'Atlas_x64.exe');
  try{
    fs.writeFileSync(file,'not an executable');assert.throws(()=>pe(file));
    const original=fs.readFileSync('src-tauri/resources/Atlas.Xray.exe');
    const header=original.subarray(0,1024),offset=header.readUInt32LE(60);
    fs.writeFileSync(file,header);assert.throws(()=>pe(file));
    const bad=Buffer.from(header);bad.writeUInt16LE(0x14c,offset+4);fs.writeFileSync(file,bad);assert.throws(()=>pe(file),/not Windows x64/);
  }finally{fs.rmSync(dir,{recursive:true});}
});
