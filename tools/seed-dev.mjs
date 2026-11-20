// Fills a dev database through the running server. Two steps, with the admin promoted
// in between (the first account is only an admin on an empty database):
//   node tools/seed-dev.mjs register
//   UPDATE users SET primary_group_id = 4 WHERE username = 'siteadmin';
//   node tools/seed-dev.mjs content
const BASE = process.env.BF_URL ?? "http://127.0.0.1:8080";
const PASSWORD = process.env.BF_DEV_PASSWORD ?? "correct horse battery";

class Client {
  jar = new Map();
  async req(path, init = {}) {
    const cookie = [...this.jar].map(([k, v]) => `${k}=${v}`).join("; ");
    const r = await fetch(BASE + path, { redirect: "manual", ...init, headers: { cookie, ...init.headers } });
    for (const c of r.headers.getSetCookie()) {
      const [kv] = c.split(";");
      const i = kv.indexOf("=");
      this.jar.set(kv.slice(0, i), kv.slice(i + 1));
    }
    return { status: r.status, text: await r.text(), location: r.headers.get("location") ?? "" };
  }
  form(path, data) {
    const body = new URLSearchParams({ csrf: this.jar.get("__Host-bf_csrf") ?? "", ...data });
    return this.req(path, { method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body });
  }
  async api(path, body) {
    const headers = { "content-type": "application/json", "x-csrf-token": this.jar.get("__Host-bf_csrf") ?? "" };
    const r = await this.req(path, { method: "POST", headers, body: JSON.stringify(body) });
    if (r.status >= 300) throw new Error(`${path}: ${r.status} ${r.text.slice(0, 200)}`);
    return JSON.parse(r.text);
  }
}

const TREE = [
  ["Community", [
    ["Announcements", "News about the forum and its releases.", []],
    ["General discussion", "Anything that does not fit somewhere else.", ["Introductions", "Off topic"]],
  ]],
  ["Support", [
    ["Getting started", "Installation, upgrades and first steps.", ["Installation", "Upgrades", "Configuration"]],
    ["Themes and design", "Share and discuss themes, templates and layouts.", ["Showcase", "Requests"]],
    ["Plugins", "Using and writing plugins.", ["Plugin support", "Development"]],
  ]],
];

const mode = process.argv[2];
const admin = new Client();
const member = new Client();
const people = [[admin, "siteadmin"], [member, "user"]];

if (mode === "register") {
  for (const [c, name] of people) {
    await c.req("/register");
    const r = await c.form("/register", { username: name, email: `${name}@example.com`, password: PASSWORD });
    console.log(`register ${name}: ${r.status}`);
  }
} else if (mode === "content") {
  for (const [c, name] of people) {
    await c.req("/login");
    const r = await c.form("/login", { login: name, password: PASSWORD });
    if (r.status !== 303) throw new Error(`login ${name}: ${r.status}`);
  }
  const forums = [];
  for (const [cat, list] of TREE) {
    const { id: catId } = await admin.api("/api/v1/admin/nodes", { kind: "category", title: cat });
    for (const [title, description, kids] of list) {
      const { id } = await admin.api("/api/v1/admin/nodes", { kind: "forum", title, description, parent_id: catId });
      forums.push([id, title]);
      for (const k of kids) {
        const child = await admin.api("/api/v1/admin/nodes", { kind: "forum", title: k, parent_id: id, description: `All about ${k.toLowerCase()}.` });
        forums.push([child.id, k]);
      }
    }
  }
  for (const [id, title] of forums) {
    for (const [c, label] of [[admin, "Welcome to"], [member, "A question about"]]) {
      const r = await c.form(`/c/${id}/new`, {
        title: `${label} ${title}`,
        body: `This is an example first post for ${title}, long enough to pass the length check.\n\nA second paragraph makes the page look more like a real discussion.`,
      });
      if (r.status !== 303) { console.log(`topic in ${title}: ${r.status} ${r.text.slice(0, 200)}`); continue; }
      const topic = r.location.match(/\/t\/[^/]+\/(\d+)/)[1];
      for (let i = 1; i <= 4; i++) {
        const who = i % 2 ? (c === admin ? member : admin) : c;
        const reply = await who.form(`/t/${topic}/reply`, { body: `Reply number ${i} in this example discussion, with enough text to read like a real answer.` });
        if (reply.status !== 303) console.log(`reply in ${title}: ${reply.status} ${reply.text.slice(0, 200)}`);
      }
    }
  }
  console.log(`${forums.length} forums seeded`);
} else {
  console.log("usage: node tools/seed-dev.mjs register|content");
}
