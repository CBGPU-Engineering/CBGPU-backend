import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync, lstatSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

// Only the SQLx manifest is adapted. Driver Rust and both upstream licenses
// remain byte-identical to the checksum-verified official crates.io archive.
const root = fileURLToPath(new URL('../rust/vendor/sqlx-sqlite-0.9.0/', import.meta.url));
const provenance = JSON.parse(readFileSync(path.join(root, 'provenance.json'), 'utf8')) as {
  version: string;
  archive_url: string;
  archive_sha256: string;
  archive_files_not_included: string[];
  unmodified_file_sha256: Record<string, string>;
};
assert.equal(provenance.version, '0.9.0');
assert.equal(provenance.archive_url, 'https://static.crates.io/crates/sqlx-sqlite/sqlx-sqlite-0.9.0.crate');
assert.match(provenance.archive_sha256, /^[a-f0-9]{64}$/);
assert.deepEqual(provenance.archive_files_not_included, ['Cargo.lock']);
const digest = (file: string) => createHash('sha256').update(readFileSync(file)).digest('hex');
const files: string[] = [];
function visit(directory: string) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const file = path.join(directory, entry.name);
    assert.ok(!lstatSync(file).isSymbolicLink(), 'Vendored sources must not contain symlinks');
    if (entry.isDirectory()) visit(file);
    else files.push(path.relative(root, file));
  }
}
visit(root);
for (const [name, expected] of Object.entries(provenance.unmodified_file_sha256)) {
  assert.ok(!name.startsWith('..') && !path.isAbsolute(name));
  assert.equal(digest(path.join(root, name)), expected, `Upstream file changed: ${name}`);
}
assert.ok(files.filter(name => name.endsWith('.rs')).length > 40);
for (const name of files) {
  assert.ok(name === 'Cargo.toml' || name === 'provenance.json' || name in provenance.unmodified_file_sha256,
    `Unexpected vendored file: ${name}`);
}
for (const license of ['LICENSE-APACHE', 'LICENSE-MIT']) {
  assert.ok(license in provenance.unmodified_file_sha256, 'Keep both original licenses');
}
const upstream = readFileSync(path.join(root, 'Cargo.toml.upstream'), 'utf8');
const expectedManifest = upstream
  .replace('[package]\n', '[package]\npublish = false\n')
  .replace(/(\[dependencies.libsqlite3-sys\]\n)version = "[^"]+"/, '$1version = "=0.39.1"')
  .replace('    "unlock_notify",', '    "unlock_notify",\n    "column_metadata",');
assert.equal(readFileSync(path.join(root, 'Cargo.toml'), 'utf8'), expectedManifest,
  'Only the reviewed manifest adaptation is allowed');
console.log('PASS official SQLx SQLite driver source and licenses; manifest-only patched SQLite binding');
