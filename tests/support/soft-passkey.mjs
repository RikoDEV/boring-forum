// Software WebAuthn authenticator for end-to-end tests (ES256, "none" attestation).
//   node tests/support/soft-passkey.mjs register <state.json> <origin> '<options json>'
//   node tests/support/soft-passkey.mjs login    <state.json> <origin> '<options json>'
// Prints the JSON body the browser island would POST. Keeps key and counter in state.json.
import { createHash, generateKeyPairSync, createPrivateKey, sign, randomBytes } from "node:crypto";
import { readFileSync, writeFileSync, existsSync } from "node:fs";

const [cmd, statePath, origin, optionsJson] = process.argv.slice(2);
const opts = JSON.parse(optionsJson);
const b64 = (b) => Buffer.from(b).toString("base64url");
const sha = (b) => createHash("sha256").update(b).digest();

// Minimal CBOR: unsigned/negative ints, byte/text strings, maps.
function cbor(v) {
  const head = (major, n) => {
    if (n < 24) return Buffer.from([(major << 5) | n]);
    if (n < 256) return Buffer.from([(major << 5) | 24, n]);
    return Buffer.from([(major << 5) | 25, n >> 8, n & 255]);
  };
  if (typeof v === "number") return v >= 0 ? head(0, v) : head(1, -1 - v);
  if (typeof v === "string") { const b = Buffer.from(v); return Buffer.concat([head(3, b.length), b]); }
  if (Buffer.isBuffer(v)) return Buffer.concat([head(2, v.length), v]);
  if (v instanceof Map) return Buffer.concat([head(5, v.size), ...[...v].flatMap(([k, x]) => [cbor(k), cbor(x)])]);
  throw new Error("cbor: unsupported");
}

let state = existsSync(statePath) ? JSON.parse(readFileSync(statePath, "utf8")) : null;
const rpId = cmd === "register" ? opts.rp.id : opts.rpId;
const authData = (flags, extra = Buffer.alloc(0)) => {
  const count = Buffer.alloc(4);
  count.writeUInt32BE(state.count);
  return Buffer.concat([sha(rpId), Buffer.from([flags]), count, extra]);
};
const clientData = (type) => Buffer.from(JSON.stringify({ type, challenge: opts.challenge, origin, crossOrigin: false }));

if (cmd === "register") {
  const { privateKey } = generateKeyPairSync("ec", { namedCurve: "P-256" });
  const jwk = privateKey.export({ format: "jwk" });
  state = { jwk, id: b64(randomBytes(16)), count: 0 };
  const cose = new Map([[1, 2], [3, -7], [-1, 1], [-2, Buffer.from(jwk.x, "base64url")], [-3, Buffer.from(jwk.y, "base64url")]]);
  const id = Buffer.from(state.id, "base64url");
  const len = Buffer.alloc(2);
  len.writeUInt16BE(id.length);
  const acd = Buffer.concat([Buffer.alloc(16), len, id, cbor(cose)]);
  const att = cbor(new Map([["fmt", "none"], ["attStmt", new Map()], ["authData", authData(0x01 | 0x04 | 0x40, acd)]]));
  writeFileSync(statePath, JSON.stringify(state));
  console.log(JSON.stringify({ clientDataJSON: b64(clientData("webauthn.create")), attestationObject: b64(att), name: "Soft key" }));
} else if (cmd === "login") {
  state.count += 1;
  const ad = authData(0x01 | 0x04);
  const cd = clientData("webauthn.get");
  const key = createPrivateKey({ key: state.jwk, format: "jwk" });
  const sig = sign("sha256", Buffer.concat([ad, sha(cd)]), key); // DER-encoded ECDSA
  writeFileSync(statePath, JSON.stringify(state));
  console.log(JSON.stringify({ id: state.id, clientDataJSON: b64(cd), authenticatorData: b64(ad), signature: b64(sig) }));
} else {
  console.error("usage: register|login <state.json> <origin> '<options json>'");
  process.exit(2);
}
