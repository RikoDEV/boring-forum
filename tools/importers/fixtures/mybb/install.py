import re, sys, urllib.request, urllib.parse, http.cookiejar

BASE = "http://127.0.0.1:8001/install/index.php"
jar = http.cookiejar.CookieJar()
op = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
VALUES = {
    "dbengine": "mysqli", "dbhost": "fx-db", "dbuser": "root", "dbpass": "root", "dbname": "mybb", "tableprefix": "mybb_",
    "bbname": "Fixture Board", "bburl": "http://localhost:8001", "websitename": "Fixture", "websiteurl": "http://localhost:8001/",
    "cookiedomain": "", "cookiepath": "/", "contactemail": "admin@example.org", "pmsallowed": "1",
    "adminuser": "admin", "adminpass": "AdminPass-12345", "adminpass2": "AdminPass-12345", "adminemail": "admin@example.org",
    "boardurl": "http://localhost:8001",
    "config[mysqli][dbhost]": "fx-db", "config[mysqli][dbuser]": "root", "config[mysqli][dbpass]": "root",
    "config[mysqli][dbname]": "mybb", "config[mysqli][tableprefix]": "mybb_", "config[mysqli][encoding]": "utf8mb4",
}

def fetch(url, data=None):
    body = urllib.parse.urlencode(data).encode() if data is not None else None
    with op.open(urllib.request.Request(url, body), timeout=300) as r:
        return r.geturl(), r.read().decode("utf-8", "replace")

def form_fields(html):
    m = re.search(r"<form[^>]*>(.*?)</form>", html, re.S)
    if not m:
        return None
    fields = {}
    for tag in re.findall(r"<(?:input|select|textarea)[^>]*>", m.group(1)):
        name = re.search(r'name="([^"]+)"', tag)
        if not name:
            continue
        typ = re.search(r'type="([^"]+)"', tag)
        val = re.search(r'value="([^"]*)"', tag)
        if typ and typ.group(1) in ("submit", "button"):
            continue
        if typ and typ.group(1) in ("checkbox", "radio") and "checked" not in tag:
            fields.setdefault(name.group(1), None)
            continue
        fields[name.group(1)] = val.group(1) if val else ""
    return {k: v for k, v in fields.items() if v is not None} | {k: "" for k, v in fields.items() if v is None and k in VALUES}

url, html = fetch(BASE)
for step in range(30):
    text = re.sub(r"\s+", " ", re.sub(r"<[^>]+>", " ", html))
    title = re.search(r"<h2[^>]*>(.*?)</h2>|<title>(.*?)</title>", html, re.S)
    tt = re.sub("<[^>]+>", "", ((title.group(1) or title.group(2) or "") if title else "")).strip()[:60]
    print(f"step {step}: {tt!r} fields={list((form_fields(html) or {}).keys())[:12]}")
    if re.search(r"installation of MyBB is complete|has been finalized|Finish|installation is complete", text, re.I) and "action" not in (form_fields(html) or {}):
        print("done:", text[:160]); break
    f = form_fields(html)
    if f is None:
        print("no form:", text[:300]); break
    for k in list(f):
        if k in VALUES:
            f[k] = VALUES[k]
    if "dbengine" in f:
        f["dbengine"] = "mysqli"
    url, html = fetch(BASE, f)
