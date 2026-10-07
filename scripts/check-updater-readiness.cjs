const fs = require('node:fs');
const requiredChecks = ['frontend', 'verificationContracts', 'backend', 'restrictedProcess', 'cleanSignedBuild', 'packageAndSignatures', 'connectedUpgrade', 'installedRecovery', 'publicationPreparation'];
function failures(value, expectedVersion) {
  const result = [];
  if (value?.schema !== 2) result.push('schema (legacy cached results are not publication policy)');
  if (!/^\d+\.\d+\.\d+$/.test(value?.version || '') || (expectedVersion && value.version !== expectedVersion)) result.push('version');
  if (!Array.isArray(value?.requiredChecks) || value.requiredChecks.length !== requiredChecks.length ||
      requiredChecks.some(name => !value.requiredChecks.includes(name))) result.push('requiredChecks');
  if ('releaseReady' in (value || {})) result.push('releaseReady (static success flags are not accepted)');
  const risks = value?.acceptedRisks;
  if (!Array.isArray(risks) || !risks.some(risk => risk.id === 'PHYSICAL_REBOOT_RECOVERY')) result.push('physical reboot disclosure');
  else for (const risk of risks) {
    if (risk.status !== 'NOT_TESTED' || risk.accepted !== true || !risk.description || !risk.authorization) result.push(`unaccepted/unexplained risk: ${risk.id}`);
  }
  if (Array.isArray(value?.blockers) && value.blockers.length) result.push(...value.blockers);
  return result;
}
function check(value, expectedVersion) {
  const blocked = failures(value, expectedVersion);
  if (blocked.length) throw Error('Atlas publication policy blocked: ' + blocked.join(', '));
}
function summary(value, expectedVersion) {
  const blocked = failures(value, expectedVersion);
  const lines = [blocked.length ? '## Atlas publication policy is blocked' : '## Atlas publication policy accepted; candidate checks still required', ''];
  lines.push(...blocked.map(item => `- ${item}`));
  if (Array.isArray(value?.acceptedRisks)) lines.push(...value.acceptedRisks.map(risk => `- ${risk.id}: ${risk.status}. ${risk.description}`));
  lines.push('', 'Preflight is not a test result. CI must complete all required automated checks for the actual candidate before publication.');
  return lines.join('\n') + '\n';
}
module.exports = {check, failures, summary, requiredChecks};
if (require.main === module) {
  try {
    const value = JSON.parse(fs.readFileSync(process.argv[2] || 'updater-readiness.json'));
    const version = JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json')).version;
    const report = summary(value, version);
    if (process.env.GITHUB_STEP_SUMMARY) fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY, report);
    console.log(report);
    check(value, version);
    console.log('PASS publication policy; automated acceptance remains mandatory');
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
