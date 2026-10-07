const fs=require('node:fs');
function check(value) {
  const gates=['buildProvenance','packageVerification','signatureVerification','transactionSafety','rollback','crashRecovery','networkRecovery','integrationTests'];
  const failures=gates.filter(g=>value[g]!=='PASS');
  for(const gate of ['P0-A','P0-B','P0-C','P0-D'])if(value.phase0?.[gate]!=='PASS')failures.push(gate);
  if(value.releaseReady!==true)failures.push('releaseReady');
  if(failures.length)throw Error('Atlas release blocked: '+failures.join(', '));
}
module.exports={check};
if(require.main===module){try{check(JSON.parse(fs.readFileSync(process.argv[2]||'updater-readiness.json')));console.log('PASS updater release gates');}catch(e){console.error(e.message);process.exitCode=1;}}
