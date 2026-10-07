const {test} = require('node:test');
const assert = require('node:assert/strict');
const {releaseVersion,stampCargoLock} = require('./stamp-release-version.cjs');
test('same product label gets a different ordered build version for each release and retry', () => {
  assert.equal(releaseVersion('2.0.1-alpha.1', 42, 1), '2.0.1-alpha.42.1');
  assert.equal(releaseVersion('2.0.1-alpha.1', 43, 1), '2.0.1-alpha.43.1');
  assert.equal(releaseVersion('2.0.1-alpha.42.1', 42, 2), '2.0.1-alpha.42.2');
  assert.throws(() => releaseVersion('2.0.1-alpha.1', undefined, 1));
  assert.throws(() => releaseVersion('2.0.1-alpha.1', 0, 1));
  assert.equal(releaseVersion('2.4.2',55,1),'2.4.2');
  assert.equal(releaseVersion('2.4.2',undefined,undefined),'2.4.2');
});
test('stamping preserves locked dependencies and changes exactly the root package',()=>{
  const input='[[package]]\nname = "atlas-vpn"\nversion = "2.4.1"\n\n[[package]]\nname = "other"\nversion = "2.4.1"\n';
  assert.equal(stampCargoLock(input,'2.4.1-alpha.55.1'),input.replace('version = "2.4.1"','version = "2.4.1-alpha.55.1"'));
  assert.throws(()=>stampCargoLock('', '2.4.1'));assert.throws(()=>stampCargoLock(input+input,'2.4.1'));
});
