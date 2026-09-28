const {test} = require('node:test');
const assert = require('node:assert/strict');
const {releaseVersion} = require('./stamp-release-version.cjs');
test('same product label gets a different ordered build version for each release and retry', () => {
  assert.equal(releaseVersion('2.0.1-alpha.1', 42, 1), '2.0.1-alpha.42.1');
  assert.equal(releaseVersion('2.0.1-alpha.1', 43, 1), '2.0.1-alpha.43.1');
  assert.equal(releaseVersion('2.0.1-alpha.42.1', 42, 2), '2.0.1-alpha.42.2');
  assert.throws(() => releaseVersion('2.0.1-alpha.1', undefined, 1));
  assert.throws(() => releaseVersion('2.0.1-alpha.1', 0, 1));
});
