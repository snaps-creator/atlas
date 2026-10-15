const {test} = require('node:test');
const assert = require('node:assert/strict');
const {checkReleaseVersion} = require('./check-release-version.cjs');

test('release version preflight rejects existing tags and fails closed on query errors', () => {
  const query = result => args => {
    assert.deepEqual(args, ['ls-remote', '--exit-code', '--tags', 'origin', 'refs/tags/v3.0.4']);
    return result;
  };
  assert.match(checkReleaseVersion('3.0.4', query({status: 2})), /available/);
  assert.throws(() => checkReleaseVersion('3.0.4', query({status: 0})), /already has a release tag/);
  for (const result of [{status: 128}, {status: null, error: Error('timeout')}, {status: 1}]) {
    assert.throws(() => checkReleaseVersion('3.0.4', query(result)), /Could not verify/);
  }
  assert.throws(() => checkReleaseVersion('3.0.4 --help'), /Expected a stable/);
});
