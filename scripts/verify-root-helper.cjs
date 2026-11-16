// The bootstrap executes before the full installer supplies its app-local DLLs.
const { pe } = require('./verify-package-content.cjs');
const systemLibraries = new Set([
  'advapi32.dll', 'bcrypt.dll', 'bcryptprimitives.dll', 'combase.dll',
  'dnsapi.dll', 'fwpuclnt.dll', 'iphlpapi.dll', 'kernel32.dll', 'ntdll.dll',
  'ole32.dll', 'oleaut32.dll', 'shell32.dll', 'shlwapi.dll', 'user32.dll',
  'userenv.dll', 'version.dll', 'wininet.dll', 'ws2_32.dll', 'ucrtbase.dll',
  'msvcrt.dll',
]);
function verifyImports(imports) {
  if (!imports.length) throw Error('Root helper has no inspectable Windows imports');
  for (const name of imports) {
    const lower = name.toLowerCase();
    if (!systemLibraries.has(lower) && !/^api-ms-win-[a-z0-9-]+\.dll$/.test(lower)) {
      throw Error(`Root helper requires an external DLL: ${name}`);
    }
  }
}
module.exports = { verifyImports };
if (require.main === module) {
  try {
    const result = pe(process.argv[2]);
    verifyImports(result.imports);
    console.log(JSON.stringify({ verified: true, windowsOnlyImports: true, ...result }));
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
