// Minimal OIDC provider for end-to-end tests. Not for anything else.
//   node tests/support/mock-oidc.mjs [port]
// POST /next-user {sub, email, email_verified, preferred_username} sets who logs in next.
// /authorize redirects straight back with a code; /token checks the client secret,
// the redirect URI and PKCE (S256); /userinfo returns the user for the access token.
import http from "node:http";
import { createHash, createHmac, createPrivateKey, createPublicKey, randomBytes, verify } from "node:crypto";

const port = Number(process.argv[2] ?? 9911);
const CLIENT_ID = "test-client";
const CLIENT_SECRET = "test-secret";
let nextUser = { sub: "ext-0", email: "nobody@example.com", email_verified: false, preferred_username: "nobody" };
const codes = new Map(); // code -> { challenge, redirect_uri, user }
const hooks = [];
const tokens = new Map(); // access token -> user

const b64url = (buf) => buf.toString("base64").replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const body = (req) => new Promise((ok) => { let d = ""; req.on("data", (c) => (d += c)); req.on("end", () => ok(d)); });
const json = (res, status, obj) => { res.writeHead(status, { "Content-Type": "application/json" }); res.end(JSON.stringify(obj)); };

http.createServer(async (req, res) => {
  const url = new URL(req.url, `http://127.0.0.1:${port}`);
  if (req.method === "POST" && url.pathname === "/next-user") {
    nextUser = JSON.parse(await body(req));
    return json(res, 200, { ok: true });
  }
  if (url.pathname === "/authorize") {
    const q = url.searchParams;
    if (q.get("client_id") !== CLIENT_ID || q.get("code_challenge_method") !== "S256") return json(res, 400, { error: "bad request" });
    const code = b64url(randomBytes(16));
    codes.set(code, { challenge: q.get("code_challenge"), redirect_uri: q.get("redirect_uri"), user: nextUser });
    const back = new URL(q.get("redirect_uri"));
    back.searchParams.set("code", code);
    back.searchParams.set("state", q.get("state"));
    res.writeHead(302, { Location: back.toString() });
    return res.end();
  }
  if (req.method === "POST" && url.pathname === "/token") {
    const f = new URLSearchParams(await body(req));
    const c = codes.get(f.get("code"));
    codes.delete(f.get("code"));
    if (!c || f.get("client_id") !== CLIENT_ID || f.get("client_secret") !== CLIENT_SECRET) return json(res, 400, { error: "invalid_client" });
    if (f.get("redirect_uri") !== c.redirect_uri) return json(res, 400, { error: "invalid_grant", detail: "redirect_uri" });
    if (b64url(createHash("sha256").update(f.get("code_verifier") ?? "").digest()) !== c.challenge) return json(res, 400, { error: "invalid_grant", detail: "pkce" });
    const token = b64url(randomBytes(16));
    tokens.set(token, c.user);
    return json(res, 200, { access_token: token, token_type: "Bearer" });
  }
  if (url.pathname === "/userinfo") {
    const user = tokens.get((req.headers.authorization ?? "").replace(/^Bearer /, ""));
    return user ? json(res, 200, user) : json(res, 401, { error: "invalid_token" });
  }
  // Webhook receiver: /hook records deliveries, /hook-fail refuses them, /hook-log reports them.
  if (req.method === "POST" && (url.pathname === "/hook" || url.pathname === "/hook-fail")) {
    hooks.push({ path: url.pathname, signature: req.headers["x-bf-signature"], event: req.headers["x-bf-event"], body: await body(req) });
    return json(res, url.pathname === "/hook" ? 200 : 500, {});
  }
  if (url.pathname === "/hook-log") return json(res, 200, hooks);
  // DiscourseConnect-style login site: signs a fixed user back to return_sso_url.
  if (url.pathname === "/sso") {
    const key = process.env.BF_SSO_SECRET;
    const hmac = (t) => createHmac("sha256", key).update(t).digest("hex");
    const sso = url.searchParams.get("sso");
    if (hmac(sso) !== url.searchParams.get("sig")) return json(res, 400, { error: "bad signature" });
    const q = new URLSearchParams(Buffer.from(sso, "base64").toString());
    const out = Buffer.from(new URLSearchParams({ nonce: q.get("nonce"), external_id: "sso-1", email: "sso.user@example.com", username: "ssouser" }).toString()).toString("base64");
    const back = new URL(q.get("return_sso_url"));
    back.searchParams.set("sso", out);
    back.searchParams.set("sig", hmac(out));
    res.writeHead(302, { Location: back.toString() });
    return res.end();
  }
  // Registration challenge: the token "good" passes.
  if (req.method === "POST" && url.pathname === "/siteverify") {
    return json(res, 200, { success: new URLSearchParams(await body(req)).get("response") === "good" });
  }
  // Sign in with Apple: /auth/authorize answers with what the browser's form post would carry;
  // /auth/token verifies the ES256 client secret against BF_OAUTH_APPLE_KEY and returns an id_token.
  if (url.pathname === "/auth/authorize") {
    const q = url.searchParams;
    if (q.get("response_mode") !== "form_post") return json(res, 400, { error: "form_post required" });
    const code = b64url(randomBytes(16));
    codes.set(code, { user: nextUser });
    return json(res, 200, { code, state: q.get("state"), redirect_uri: q.get("redirect_uri") });
  }
  if (req.method === "POST" && url.pathname === "/auth/token") {
    const f = new URLSearchParams(await body(req));
    const c = codes.get(f.get("code"));
    codes.delete(f.get("code"));
    const [h, p, s] = (f.get("client_secret") ?? "..").split(".");
    const pub = createPublicKey(createPrivateKey(process.env.BF_OAUTH_APPLE_KEY));
    const good = verify("sha256", Buffer.from(`${h}.${p}`), { key: pub, dsaEncoding: "ieee-p1363" }, Buffer.from(s, "base64url"));
    const claims = JSON.parse(Buffer.from(p, "base64url"));
    if (!c || !good || claims.sub !== f.get("client_id") || claims.iss !== process.env.BF_OAUTH_APPLE_TEAM_ID) return json(res, 400, { error: "invalid_client" });
    const enc = (o) => b64url(Buffer.from(JSON.stringify(o)));
    const id_token = `${enc({ alg: "none" })}.${enc({ iss: `http://127.0.0.1:${port}`, aud: f.get("client_id"), exp: Math.floor(Date.now() / 1000) + 60, sub: c.user.sub, email: c.user.email, email_verified: String(c.user.email_verified) })}.x`;
    return json(res, 200, { access_token: "unused", id_token });
  }
  json(res, 404, { error: "not found" });
}).listen(port, "127.0.0.1", () => console.log(`mock oidc on 127.0.0.1:${port}`));
