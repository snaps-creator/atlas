const { test } = require('node:test');
const assert = require('node:assert/strict');
const { verifyImports } = require('./verify-root-helper.cjs');
test('bootstrap accepts Windows libraries and rejects redistributable or sibling dependencies', () => {
  verifyImports(['KERNEL32.dll', 'USERENV.dll', 'api-ms-win-crt-runtime-l1-1-0.dll']);
  for (const external of ['VCRUNTIME140.dll', 'VCRUNTIME140_1.dll', 'MSVCP140.dll', 'libcef.dll', 'libssl.dll']) {
    assert.throws(() => verifyImports(['kernel32.dll', external]), /external DLL/);
  }
  assert.throws(() => verifyImports([]), /no inspectable/);
});
