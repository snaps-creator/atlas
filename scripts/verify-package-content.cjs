// Read-only verification of bytes extracted from NSIS against its actual inputs.
const fs=require('node:fs'),path=require('node:path'),crypto=require('node:crypto');
function hash(file){const fd=fs.openSync(file,'r'),buffer=Buffer.alloc(1024*1024),h=crypto.createHash('sha256');try{let n;while((n=fs.readSync(fd,buffer,0,buffer.length,null))>0)h.update(buffer.subarray(0,n));return h.digest('hex');}finally{fs.closeSync(fd);}}
function nsisStage(source){
  // tauri-utils 3.0.0-alpha.1 platform.rs documents this bundler patch. Reproduce
  // only this exact input->output transformation; never normalize extracted bytes.
  const bytes=fs.readFileSync(source),marker=Buffer.from('__TAURI_BUNDLE_TYPE_VAR_UNK');
  const at=bytes.indexOf(marker);if(at<0||bytes.indexOf(marker,at+1)!==-1)throw Error('Tauri bundle marker missing or ambiguous');
  const offset=at+marker.length-3;bytes.write('NSS',offset,'ascii');
  return {sha256:crypto.createHash('sha256').update(bytes).digest('hex'),patch:{kind:'tauri-nsis-bundle-tag',offset,from:'UNK',to:'NSS'}};
}
function pe(file){
  const fd=fs.openSync(file,'r'),size=fs.fstatSync(fd).size;
  function read(offset,count){if(!Number.isSafeInteger(offset)||offset<0||offset+count>size)throw Error('PE range outside file');const b=Buffer.alloc(count);if(fs.readSync(fd,b,0,count,offset)!==count)throw Error('Truncated PE');return b;}
  try{
    const dos=read(0,64);if(dos.toString('ascii',0,2)!=='MZ')throw Error('Not a PE image');
    const offset=dos.readUInt32LE(60),head=read(offset,24);if(head.readUInt32LE(0)!==0x4550)throw Error('Invalid PE signature');
    const machine=head.readUInt16LE(4),count=head.readUInt16LE(6),optionalSize=head.readUInt16LE(20);
    if(machine!==0x8664||count===0||count>96||optionalSize<128)throw Error('Payload is not Windows x64 PE');
    const optional=read(offset+24,optionalSize);if(optional.readUInt16LE(0)!==0x20b)throw Error('Payload is not PE32+');
    const sections=read(offset+24+optionalSize,count*40);
    function fileOffset(rva){for(let i=0;i<count;i++){const s=sections.subarray(i*40,(i+1)*40),va=s.readUInt32LE(12),rawSize=s.readUInt32LE(16);if(rva>=va&&rva-va<rawSize)return s.readUInt32LE(20)+rva-va;}throw Error('PE RVA has no file data');}
    const imports=[],importRva=optional.readUInt32LE(120);
    if(importRva){const start=fileOffset(importRva);let ended=false;for(let i=0;i<2048;i++){const d=read(start+i*20,20);if(d.every(v=>v===0)){ended=true;break;}const nameOffset=fileOffset(d.readUInt32LE(12));let name='';for(let j=0;j<1024;j++){const c=read(nameOffset+j,1)[0];if(!c)break;if(c<32||c>126)throw Error('Invalid import name');name+=String.fromCharCode(c);if(j===1023)throw Error('Unterminated import name');}if(!name||/[\\/:]/.test(name))throw Error('Invalid DLL import');imports.push(name);}if(!ended)throw Error('Unterminated PE imports');}
    return {machine:'AMD64',format:'PE32+',imports:imports.sort()};
  }finally{fs.closeSync(fd);}
}
function inventory(directory,relative=''){return fs.readdirSync(path.join(directory,relative),{withFileTypes:true}).flatMap(e=>{if(e.isSymbolicLink())throw Error('Reparse entry in extracted payload');const name=path.join(relative,e.name);return e.isDirectory()?inventory(directory,name):[name.replaceAll('\\','/')];});}
function verify({nsis,extracted,version,requireUpdater=true}){
  const script=fs.readFileSync(nsis,'utf8');
  const main=/!define MAINBINARYSRCPATH "([^"]+)"/.exec(script)?.[1];
  const actualVersion=/!define VERSION "([^"]+)"/.exec(script)?.[1];
  if(!main||actualVersion!==version)throw Error('Generated NSIS identity differs from expected build');
  const map=new Map([['Atlas.exe',main]]);
  for(const m of script.matchAll(/^\s*File \/a "\/oname=([^"]+)" "([^"]+)"/gm))map.set(m[1].replaceAll('\\','/'),m[2]);
  if(requireUpdater)map.set('AtlasUpdater.exe',path.join(path.dirname(main),'AtlasUpdater.exe'));
  for(const name of ['Atlas.exe','AtlasMaintenance.exe','libcef.dll','resources/Atlas.Core.exe','resources/Atlas.Xray.exe'])if(!map.has(name))throw Error('Required payload missing: '+name);
  const actual=inventory(extracted).filter(p=>!p.startsWith('$PLUGINSDIR/')).sort();
  if(JSON.stringify(actual)!==JSON.stringify([...map.keys()].sort()))throw Error('Extracted payload inventory differs from generated NSIS');
  const files=[];
  for(const [relative,source] of map){const file=path.join(extracted,relative),sourceHash=hash(source),extractedHash=hash(file);const stage=relative==='Atlas.exe'?nsisStage(source):{sha256:sourceHash,patch:null};if(stage.sha256!==extractedHash)throw Error('Packaged bytes differ: '+relative);files.push({path:relative,source,size:fs.statSync(file).size,sourceSha256:sourceHash,stagedSha256:stage.sha256,bundlePatch:stage.patch,extractedSha256:extractedHash,...(/\.(exe|dll)$/i.test(relative)?{pe:pe(file)}:{})});}
  const plugins=inventory(path.join(extracted,'$PLUGINSDIR')).filter(p=>p.endsWith('AtlasMaintenance.exe'));
  if(plugins.length!==1||hash(path.join(extracted,'$PLUGINSDIR',plugins[0]))!==hash(map.get('AtlasMaintenance.exe')))throw Error('Preinstall helper differs from installed helper');
  return {schema:1,version,verified:true,nsisSha256:hash(nsis),files,serviceCopy:{source:'Atlas.exe',destination:'Atlas.Service.exe',sha256:hash(path.join(extracted,'Atlas.exe')),installedCopy:'NOT_EXECUTED'}};
}
module.exports={hash,pe,verify};
if(require.main===module){try{const [nsis,extracted,version,output]=process.argv.slice(2);const evidence=verify({nsis,extracted,version});fs.writeFileSync(output,JSON.stringify(evidence,null,2)+'\n');console.log(`Verified ${evidence.files.length} package files`);}catch(e){console.error(e.message);process.exitCode=1;}}
