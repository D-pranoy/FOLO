// Validates an AsyncAPI document with the official parser, following file
// references (RB-02 step 6). Exits non-zero on errors.
//
//   node scripts/check-asyncapi.mjs ../specs/asyncapi.yaml

import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { Parser, fromFile } from '@asyncapi/parser';

const path = resolve(process.argv[2] ?? '../specs/asyncapi.yaml');
const parser = new Parser();
const { document, diagnostics } = await fromFile(parser, path).parse({
  source: pathToFileURL(path).href,
});
// Severity 0 is an error, 1 a warning
const errors = diagnostics.filter((d) => d.severity === 0);
for (const d of diagnostics) {
  const where = d.path?.length ? ` at ${d.path.join('/')}` : '';
  console.log(`${d.severity === 0 ? 'error' : 'warning'}: ${d.message}${where}`);
}
if (errors.length || !document) {
  console.error(`${path}: ${errors.length} error(s)`);
  process.exit(1);
}
const channels = document.channels().length;
const messages = document.messages().length;
console.log(`${path}: valid AsyncAPI ${document.version()} (${channels} channels, ${messages} messages)`);
