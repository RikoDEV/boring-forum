// Zips a directory, uncompressed, with no dependencies: node tests/support/mkzip.mjs <dir> <out.zip>
import fs from "node:fs";
import path from "node:path";
const [dir, out] = process.argv.slice(2);
const crcT = [...Array(256)].map((_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c >>> 0; });
const crc = (b) => { let c = ~0; for (const x of b) c = crcT[(c ^ x) & 255] ^ (c >>> 8); return ~c >>> 0; };
const walk = (d) => fs.readdirSync(d, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? walk(path.join(d, e.name)) : [path.join(d, e.name)]));
const parts = [], cd = [];
let off = 0, count = 0;
for (const f of walk(dir)) {
  const d = fs.readFileSync(f), nb = Buffer.from(path.relative(dir, f).replaceAll("\\", "/")), h = Buffer.alloc(30);
  h.writeUInt32LE(0x04034b50, 0); h.writeUInt16LE(20, 4); h.writeUInt32LE(crc(d), 14); h.writeUInt32LE(d.length, 18); h.writeUInt32LE(d.length, 22); h.writeUInt16LE(nb.length, 26);
  parts.push(h, nb, d);
  const c = Buffer.alloc(46);
  c.writeUInt32LE(0x02014b50, 0); c.writeUInt16LE(20, 4); c.writeUInt16LE(20, 6); c.writeUInt32LE(crc(d), 16); c.writeUInt32LE(d.length, 20); c.writeUInt32LE(d.length, 24); c.writeUInt16LE(nb.length, 28); c.writeUInt32LE(off, 42);
  cd.push(c, nb);
  off += 30 + nb.length + d.length;
  count++;
}
const cdb = Buffer.concat(cd), e = Buffer.alloc(22);
e.writeUInt32LE(0x06054b50, 0); e.writeUInt16LE(count, 8); e.writeUInt16LE(count, 10); e.writeUInt32LE(cdb.length, 12); e.writeUInt32LE(off, 16);
fs.writeFileSync(out, Buffer.concat([...parts, cdb, e]));
