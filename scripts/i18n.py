#!/usr/bin/env python3
"""Keeps the message catalogs in step with the English written in the Rust code.

Rust code writes user-facing text in English and looks it up by a name made from that text:
`t.msg("Text")`, `t.msgf("Hello {name}", ...)`, and every `AppError::Validation("…")` /
`BadRequest("…")` (translated on the way out by the `html_errors` middleware). The key is `msg.`
plus the words of the text in lower case, joined by underscores (`msg.first_like`), as
`bf_core::i18n::msg_key` computes it. Two texts that would get the same name (`Closed` and `closed`)
are written down in locales/keys.json with distinct names, and `sync` keeps that file (and its copies
for the browser bundles) in step; a name never changes once it is there.

Browser code does the same with `tr("Text", {name})`: the islands (web/islands/src, `dom.js`)
get `js.<name>` keys, which every page sends; the admin panel (web/admin/src, `#lib/i18n.js`)
gets `admin.<name>`, sent by the panel's shell. A browser text that the Rust code already has
takes its translation from there when `sync` runs.

  python scripts/i18n.py check            list texts without a Polish or Arabic translation, and lint the
                                          catalogs (exit 1 if either finds anything)
  python scripts/i18n.py sync             rewrite the English block from the code; write the missing ones
                                          to target/i18n-missing.json as {key: [english, "", ""]}
  python scripts/i18n.py apply FILE.json  add translations: {key: [english, polish, arabic]}

The catalogs are Fluent files (crates/bf-core/locales/{en,pl,ar}.ftl). A message id is the catalog key
with its dots written as hyphens (`nav.forums` is `nav-forums`); `{name}` in the code is `{ $name }` in
the files. Each catalog ends with a block that starts with MARKER; the English block is generated, the
Polish and Arabic blocks hold a `# English` comment above each translation for the translator.
The JSON files (`target/i18n-missing.json`, `apply`) keep the code's own `{name}` syntax.

Above the generated block a catalog holds hand-written messages, which may use the whole of Fluent as
the Rust runtime supports it (selects, terms `-brand = ...`, message references, NUMBER()). `read` and
`write` only touch the block. `check` lints every message with a small Fluent parser (below): a
translation must use the same `$variables` as the English, plural variants must be valid CLDR
categories of the locale, a select needs a `*` default, ids are unique and exist in English.
"""
import glob
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOCALES = os.path.join(ROOT, 'crates', 'bf-core', 'locales')
MARKER = '## Messages from the code (scripts/i18n.py): named after their English text'
RUST_STR = r'"((?:[^"\\]|\\.)*)"'
PATTERNS = [
    re.compile(r'(?:Validation|BadRequest)\(\s*' + RUST_STR),
    re.compile(r'\.msgf?\(\s*' + RUST_STR),
    # Text marked where it is written and translated where it is shown: i18n::en("...").
    re.compile(r'\ben\(\s*' + RUST_STR),
    # Text built deep in a request: http::i18n::tr / trf / trh("...").
    re.compile(r'\btr[fh]?\(\s*' + RUST_STR),
    # Errors with values in their text: AppError::invalid("... {name} ...", ...).
    re.compile(r'\binvalid\(\s*' + RUST_STR),
    # AppError's own titles (bf-core/src/error.rs): (status, "kind", "Title").
    re.compile(r'StatusCode::[A-Z_]+,\s*"[a-z-]+",\s*' + RUST_STR),
]


# Page helpers (bf-server http/ui.rs) that translate their text arguments: every literal argument
# with a capital letter or a space is English to translate (names like "raw" are not).
HELPERS = re.compile(r'\b(?:section|list_box|empty|notice|badge|table|table_box|empty_row|button_form|md_field|text_field|dl_heading|dl_fact|file_grid|dl_open|dl_action|md_editor|open_button|dialog_body|manage_open|manage_post|empty_state|fact|stat)\(')


def helper_texts(src):
    for m in HELPERS.finditer(src):
        i, depth = m.end(), 1
        while i < len(src) and depth:
            c = src[i]
            if c == '"':
                j = i + 1
                while src[j] != '"':
                    j += 2 if src[j] == '\\' else 1
                lit = unescape(src[i + 1:j])
                if re.search(r'[A-Z]| ', lit) and '{' not in lit and '<' not in lit:
                    yield lit
                i = j + 1
                continue
            depth += {'(': 1, ')': -1}.get(c, 0)
            i += 1


KEYS_FILE = os.path.join(LOCALES, 'keys.json')
# Copies for the browser bundles, written by `sync`.
KEYS_COPIES = ('web/islands/src/keys.json', 'web/admin/src/lib/keys.json')
KEY_MAX = 56

TRANSLIT = {}
TABLE = {}          # {prefix: {text: key name}} for texts whose plain name is taken by another text
COLLECTING = False
FILE_TABLE = {}


def load_keys():
    global TRANSLIT, TABLE, FILE_TABLE
    try:
        data = json.load(open(KEYS_FILE, encoding='utf-8'))
    except FileNotFoundError:
        data = {}
    TRANSLIT = data.get('translit', {})
    TABLE = FILE_TABLE = data.get('texts', {})


def fnv8(text):
    h = 0x811C9DC5
    for b in text.encode('utf-8'):
        h ^= b
        h = (h * 0x01000193) & 0xFFFFFFFF
    return f'{h:08x}'


def slug(text):
    """The readable name of a text: its words in lower case, joined by underscores (`First like` is
    `first_like`). Accented Latin letters are written plain (TRANSLIT); a text with no Latin words
    at all is `text_` and its hash. bf_core::i18n and the browser bundles compute the same."""
    words, cur = [], ''
    for ch in text:
        for c in TRANSLIT.get(ch, ch):
            if c.isascii() and c.isalnum():
                cur += c.lower()
            elif cur:
                words.append(cur)
                cur = ''
    if cur:
        words.append(cur)
    name = '_'.join(words)
    if not name:
        return 'text_' + fnv8(text)
    if len(name) > KEY_MAX:
        cut = name[:KEY_MAX]
        name = cut[:cut.rfind('_')] if '_' in cut else cut
    return name


def msg_key(text, prefix='msg'):
    if COLLECTING:
        return f'{prefix}{chr(0)}{text}'
    return f'{prefix}.{TABLE.get(prefix, {}).get(text) or slug(text)}'


def assign_names(raw, table):
    """The table of explicit names. Two texts that read the same once punctuation and case are
    gone (`Closed` and `closed`) cannot both be `closed`: every one of them is written down, so a
    name never changes when another text is added later."""
    groups = {}
    for key in raw:
        prefix, text = key.split(chr(0), 1)
        groups.setdefault((prefix, slug(text)), []).append(text)
    out = {}
    for (prefix, name), texts in sorted(groups.items()):
        if len(texts) == 1 and texts[0] not in table.get(prefix, {}):
            continue
        known = table.get(prefix, {})
        taken = {known[t] for t in texts if t in known}
        for text in sorted(texts):
            if text in known:
                out.setdefault(prefix, {})[text] = known[text]
                continue
            pick, n = name, 1
            while pick in taken or any(pick == v for v in out.get(prefix, {}).values()):
                n += 1
                pick = f'{name}_{n}'
            taken.add(pick)
            out.setdefault(prefix, {})[text] = pick
    return out


def write_keys(table):
    data = {'translit': TRANSLIT, 'texts': {p: dict(sorted(t.items())) for p, t in sorted(table.items())}}
    body = json.dumps(data, ensure_ascii=False, indent=1) + chr(10)
    for path in (KEYS_FILE, *[os.path.join(ROOT, c) for c in KEYS_COPIES]):
        os.makedirs(os.path.dirname(path), exist_ok=True)
        open(path, 'w', encoding='utf-8', newline=chr(10)).write(body)


load_keys()


PREFIXES = ('msg.', 'js.', 'admin.')
# tr("…"), tr('…') or tr(`…`) without ${}: the first argument is the English.
JS_TR = re.compile(r"""(?<![\w$.])tr\(\s*(["'`])((?:\\.|(?!\1).)*?)\1""", re.S)
JS_SOURCES = (('web/islands/src', 'js'), ('web/admin/src', 'admin'))


def js_unescape(s):
    return re.sub(r'\\(.)', lambda m: {'n': '\n', 't': '\t'}.get(m.group(1), m.group(1)), s)


def js_texts(seen):
    for base, prefix in JS_SOURCES:
        for dirpath, dirs, files in os.walk(os.path.join(ROOT, base)):
            dirs[:] = [d for d in dirs if d not in ('node_modules', '.svelte-kit', 'build')]
            for f in sorted(files):
                if not f.endswith(('.js', '.svelte')):
                    continue
                src = open(os.path.join(dirpath, f), encoding='utf-8').read()
                for m in JS_TR.finditer(src):
                    text = js_unescape(m.group(2))
                    if m.group(1) == '`' and '${' in text:
                        sys.exit(f'{f}: tr(`…${{}}…`) cannot be looked up; use tr("… {{name}} …", {{ name }})')
                    if text.strip():
                        seen.setdefault(msg_key(text, prefix), text)


def unescape(s):
    return re.sub(r'\\(.)', lambda m: {'n': '\n', 't': '\t'}.get(m.group(1), m.group(1)), s)


def _collect():
    """Every message in the code, keyed by prefix and text, in order of first appearance."""
    seen = {}
    for base in ('crates',):
        for dirpath, _, files in os.walk(os.path.join(ROOT, base)):
            if os.sep + 'target' in dirpath:
                continue
            for f in sorted(files):
                # Askama templates write their text the same way: layout.t.msg("English").
                if not (f.endswith('.rs') or (f.endswith('.html') and os.sep + 'templates' in dirpath)):
                    continue
                path = os.path.join(dirpath, f)
                src = open(path, encoding='utf-8').read().split('#[cfg(test)]')[0]
                if f.endswith('.html'):
                    for m in re.finditer(r'\.msgf?\(\s*' + RUST_STR, src):
                        text = unescape(m.group(1))
                        if text.strip():
                            seen.setdefault(msg_key(text), text)
                    continue
                for pat in PATTERNS:
                    for m in pat.finditer(src):
                        text = unescape(m.group(1))
                        if text.strip():
                            seen.setdefault(msg_key(text), text)
                if f != 'ui.rs':
                    for text in helper_texts(src):
                        seen.setdefault(msg_key(text), text)
    js_texts(seen)
    setting_labels(seen)
    return seen


def code_texts():
    """Every message in the code as {catalog key: English}, in order of first appearance."""
    global COLLECTING, TABLE
    COLLECTING = True
    try:
        raw = _collect()
    finally:
        COLLECTING = False
    TABLE = assign_names(raw, FILE_TABLE)
    out = {}
    for item in raw:
        prefix, text = item.split(chr(0), 1)
        out.setdefault(msg_key(text, prefix), text)
    return out


def setting_labels(seen):
    """The admin panel names a setting without a label of its own after its key
    (`contact_email` is "Contact email") and looks that name up by its text."""
    defs_dir = os.path.join(ROOT, 'crates', 'bf-core', 'src', 'settings', 'defs')
    src = ''.join(open(os.path.join(defs_dir, f), encoding='utf-8').read() for f in sorted(os.listdir(defs_dir)) if f.endswith('.rs'))
    page = open(os.path.join(ROOT, 'web', 'admin', 'src', 'lib', 'settings-meta.js'), encoding='utf-8').read()
    labelled = set(re.findall(r'\b([a-z0-9_]+): tr\(', page))
    for key in re.findall(r'\b(?:def|int)\(\s*"([a-z0-9_]+)"', src):
        if key not in labelled:
            text = key[0].upper() + key[1:].replace('_', ' ')
            seen.setdefault(msg_key(text, 'admin'), text)


PLACEHOLDER = re.compile(r'\{(\w+)\}')
ENTRY = re.compile(r'^((?:msg|js|admin)-[a-z0-9_]+) =')
LOCALE_CODES = ('en', 'pl', 'ar')


def ftl_text(text):
    """The code's text (`{name}` placeholders, literal braces) as Fluent pattern text."""
    # Fluent drops a block's leading and trailing line breaks, so they are written as the escape \u000A.
    body = text.rstrip('\n')
    lead = len(body) - len(body.lstrip('\n'))
    head = '{ "\\u000A" }' * lead
    tail = '{ "\\u000A" }' * (len(text) - len(body))
    # Fluent trims the spaces around a value, as the old catalogs did.
    text = body.lstrip('\n').strip(' ')
    out, last = [], 0
    for m in PLACEHOLDER.finditer(text):
        out.append(brace_literals(text[last:m.start()]))
        out.append('{ $' + m.group(1) + ' }')
        last = m.end()
    out.append(brace_literals(text[last:]))
    return head + ''.join(out) + tail


def brace_literals(text):
    return re.sub(r'[{}]', lambda m: '{ "' + m.group() + '" }', text)


def ftl_entry(key, value):
    """Lines of one message. A value with line breaks is a block under its id; a line there may
    not start with `[`, `*` or `.`, which Fluent reads as syntax, so that character is quoted. A
    leading space is quoted too: Fluent would take it for common indentation."""
    ident = key.replace('.', '-')
    if '\n' not in value:
        return [f'{ident} = {value}']
    lines = [f'{ident} =']
    for line in value.split('\n'):
        if line[:1] in ('[', '*', '.', ' '):
            line = '{ "' + line[0] + '" }' + line[1:]
        lines.append('    ' + line if line else '')
    return lines


# --- A small Fluent parser: enough to lint the catalogs without external packages ----------------

IDENT = re.compile(r'[A-Za-z][A-Za-z0-9_-]*')
ENTRY_HEAD = re.compile(r'(-?[A-Za-z][A-Za-z0-9_-]*)[ ]*=')
ATTR = re.compile(r'\.([A-Za-z][A-Za-z0-9_-]*)[ ]*=')
NUMBER = re.compile(r'-?[0-9]+(?:\.[0-9]+)?')
ARG_NAME = re.compile(r'([A-Za-z][A-Za-z0-9_-]*)[ ]*:')
UNICODE_ESCAPE = re.compile(r'u([0-9a-fA-F]{4})|U([0-9a-fA-F]{6})')


class FtlError(Exception):
    pass


class Entry:
    """A message or term: `value` and each attribute are patterns, lists of ('text', s) and
    ('place', expr); an expr is ('str', s), ('num', s), ('var', name), ('ref', id), ('term', id,
    args), ('call', name, args), ('nested', expr) or ('select', selector, [(key, pattern, is_default)])."""

    def __init__(self, ident, line, value, attrs, raw):
        self.id, self.line, self.value, self.attrs, self.raw = ident, line, value, attrs, raw

    @property
    def patterns(self):
        return ([self.value] if self.value else []) + list(self.attrs.values())


class _Parser:
    def __init__(self, s, i):
        self.s, self.i, self.n = s, i, len(s)

    def err(self, msg):
        raise FtlError(msg)

    def ws(self):
        while self.i < self.n and self.s[self.i] in ' \n\t':
            self.i += 1

    def expect(self, text):
        if not self.s.startswith(text, self.i):
            self.err(f'expected {text!r}')
        self.i += len(text)

    def ident(self):
        m = IDENT.match(self.s, self.i)
        if not m:
            self.err('expected an identifier')
        self.i = m.end()
        return m.group()

    def body(self):
        """After `id =`: the value and the attributes. Ends at the start of a line."""
        value = self.pattern(False)
        attrs = {}
        while True:
            j = self.i
            while j < self.n and self.s[j] == ' ':
                j += 1
            m = ATTR.match(self.s, j)
            if not m:
                return value, attrs
            self.i = m.end()
            attrs[m.group(1)] = self.pattern(False)

    def pattern(self, variant):
        s, n = self.s, self.n
        els, buf = [], []
        while self.i < n:
            c = s[self.i]
            if c == '{':
                if buf:
                    els.append(('text', ''.join(buf)))
                    buf = []
                els.append(('place', self.placeable()))
            elif c == '}':
                if variant:
                    break
                self.err('a literal "}" must be written { "}" }')
            elif c == '\n':
                k, blanks = self.i + 1, 0
                while k < n:
                    e = s.find('\n', k)
                    e = n if e < 0 else e
                    if s[k:e].strip():
                        break
                    blanks += 1
                    k = e + 1
                if k >= n:
                    self.i = n
                    break
                indent = len(s[k:]) - len(s[k:].lstrip(' '))
                rest = s[k + indent:]
                if variant:
                    if rest[0] in '[}' or rest.startswith('*[') or not indent:
                        self.i = k
                        break
                else:
                    if not indent or ATTR.match(rest):
                        self.i = k
                        break
                    if rest[0] in '[*.':
                        self.err(f'a line starts with {rest[0]!r}, which Fluent reads as syntax; quote it as {{ "{rest[0]}" }}')
                if buf:
                    els.append(('text', ''.join(buf)))
                    buf = []
                els.append(('nl', 1 + blanks, indent))
                self.i = k + indent
            else:
                buf.append(c)
                self.i += 1
        if buf:
            els.append(('text', ''.join(buf)))
        return self.finish(els)

    @staticmethod
    def finish(els):
        """Fluent's whitespace: drop the indentation the block lines share, and the blank edges."""
        indents = [e[2] for e in els if e[0] == 'nl']
        common = min(indents) if indents else 0
        out = []
        for e in els:
            if e[0] == 'nl':
                e = ('text', '\n' * e[1] + ' ' * (e[2] - common))
            if e[0] == 'text' and out and out[-1][0] == 'text':
                out[-1] = ('text', out[-1][1] + e[1])
            else:
                out.append(e)
        if out and out[0][0] == 'text':
            out[0] = ('text', out[0][1].lstrip(' \n'))
        if out and out[-1][0] == 'text':
            out[-1] = ('text', out[-1][1].rstrip(' \n'))
        return [e for e in out if e != ('text', '')]

    def placeable(self):
        self.expect('{')
        self.ws()
        expr = self.inline()
        self.ws()
        if self.s.startswith('->', self.i):
            self.i += 2
            expr = ('select', expr, self.variants())
            self.ws()
        self.expect('}')
        return expr

    def variants(self):
        out = []
        while True:
            self.ws()
            if self.i >= self.n:
                self.err('a select is never closed')
            if self.s[self.i] == '}':
                break
            star = self.s[self.i] == '*'
            if star:
                self.i += 1
            self.expect('[')
            end = self.s.find(']', self.i)
            if end < 0:
                self.err('a variant key is never closed')
            key = self.s[self.i:end].strip()
            self.i = end + 1
            out.append((key, self.pattern(True), star))
        if not out:
            self.err('a select without variants')
        return out

    def inline(self):
        s = self.s
        if self.i >= self.n:
            self.err('a placeable is never closed')
        c = s[self.i]
        if c == '"':
            return ('str', self.string())
        if c == '{':
            return ('nested', self.placeable())
        if c == '$':
            self.i += 1
            return ('var', self.ident())
        m = NUMBER.match(s, self.i)
        if m:
            self.i = m.end()
            return ('num', m.group())
        if c == '-':
            self.i += 1
            ident = '-' + self.ident()
            if s.startswith('.', self.i):
                self.i += 1
                ident += '.' + self.ident()
            return ('term', ident, self.args() if s.startswith('(', self.i) else [])
        ident = self.ident()
        if s.startswith('.', self.i):
            self.i += 1
            ident += '.' + self.ident()
        if s.startswith('(', self.i):
            return ('call', ident, self.args())
        return ('ref', ident)

    def args(self):
        self.expect('(')
        out = []
        while True:
            self.ws()
            if self.s.startswith(')', self.i):
                self.i += 1
                return out
            if out:
                self.expect(',')
                self.ws()
            m = ARG_NAME.match(self.s, self.i)
            if m:
                self.i = m.end()
                self.ws()
            out.append(self.inline())

    def string(self):
        self.expect('"')
        out = []
        while True:
            if self.i >= self.n or self.s[self.i] == '\n':
                self.err('a string is never closed')
            c = self.s[self.i]
            if c == '"':
                self.i += 1
                return ''.join(out)
            if c == '\\':
                nxt = self.s[self.i + 1:self.i + 2]
                if nxt in ('"', '\\'):
                    out.append(nxt)
                    self.i += 2
                    continue
                m = UNICODE_ESCAPE.match(self.s, self.i + 1)
                if not m:
                    self.err('an unknown escape in a string')
                out.append(chr(int(m.group(1) or m.group(2), 16)))
                self.i = m.end()
                continue
            out.append(c)
            self.i += 1


def parse_catalog(text):
    """(entries, errors): the messages and terms of a Fluent file, and (line, problem) pairs."""
    s = text.replace('\r\n', '\n')
    entries, errors = [], []
    i, n = 0, len(s)
    while i < n:
        eol = s.find('\n', i)
        eol = n if eol < 0 else eol
        line = s[i:eol]
        if not line.strip() or line.startswith('#'):
            i = eol + 1
            continue
        m = ENTRY_HEAD.match(line)
        lineno = s.count('\n', 0, i) + 1
        if m:
            p = _Parser(s, i + m.end())
            try:
                value, attrs = p.body()
                if not value and not attrs:
                    raise FtlError('an empty message')
                raw = s[i:p.i].rstrip('\n ').split('\n')
                entries.append(Entry(m.group(1), lineno, value, attrs, raw))
                i = p.i
                continue
            except FtlError as e:
                errors.append((lineno, f'{m.group(1)}: {e}'))
        else:
            errors.append((lineno, f'not a message: {line[:50]!r}'))
        # Skip to the next line that can start an entry.
        i = eol + 1
        while i < n and (s[i] in ' \n}' or not s[i].strip()):
            e = s.find('\n', i)
            i = n if e < 0 else e + 1
    return entries, errors


def walk(expr, visit):
    """Calls visit(expr) on an expression and every expression inside it."""
    visit(expr)
    kind = expr[0]
    if kind == 'nested':
        walk(expr[1], visit)
    elif kind in ('term', 'call'):
        for a in expr[2]:
            walk(a, visit)
    elif kind == 'select':
        walk(expr[1], visit)
        for _, pat, _ in expr[2]:
            for e in pat:
                if e[0] == 'place':
                    walk(e[1], visit)


def exprs(entry):
    found = []
    for pat in entry.patterns:
        for e in pat:
            if e[0] == 'place':
                walk(e[1], found.append)
    return found


def variables(entry):
    return {e[1] for e in exprs(entry) if e[0] == 'var'}


def references(entry):
    return {e[1].split('.')[0] for e in exprs(entry) if e[0] in ('ref', 'term')}


def to_code(pattern):
    """A plain pattern back as the code's text: `{name}` placeholders, string literals as text."""
    out = []
    for kind, *rest in pattern:
        if kind == 'text':
            out.append(rest[0])
        elif rest[0][0] == 'str':
            out.append(rest[0][1])
        elif rest[0][0] == 'var':
            out.append('{' + rest[0][1] + '}')
        else:
            raise ValueError(f'not a plain pattern: {rest[0][0]}')
    return ''.join(out)


CLDR = ('zero', 'one', 'two', 'few', 'many', 'other')
CATEGORIES = {
    'en': {'one', 'other'},
    'pl': {'one', 'few', 'many', 'other'},
    'ar': {'zero', 'one', 'two', 'few', 'many', 'other'},
}
NUMERIC_KEY = re.compile(r'^-?[0-9]+(\.[0-9]+)?$')


def lint_selects(code, entry):
    """Problems with the variants of every select in one message. A select whose keys include a
    CLDR category name besides `other` is a plural: its other keys must be valid categories of the locale or numbers.
    Other selects (gender, case) are only checked for a default."""
    problems = []
    for e in exprs(entry):
        if e[0] != 'select':
            continue
        keys = [k for k, _, _ in e[2]]
        defaults = [k for k, _, star in e[2] if star]
        if len(defaults) != 1:
            problems.append(f'{entry.id}: a select needs exactly one `*` default variant, has {len(defaults)}')
        if len(set(keys)) != len(keys):
            problems.append(f'{entry.id}: a select repeats a variant key')
        if any(k in CLDR and k != 'other' for k in keys):
            for k in keys:
                if k in CLDR and k not in CATEGORIES[code]:
                    valid = '/'.join(c for c in CLDR if c in CATEGORIES[code])
                    problems.append(f'{entry.id}: [{k}] is not a plural category of {code} (use {valid})')
                elif k not in CLDR and not NUMERIC_KEY.match(k):
                    problems.append(f'{entry.id}: [{k}] is mixed with plural categories')
    return problems


def lint(catalogs):
    """catalogs: {code: file text}. Returns the problems, one line of text each."""
    parsed, problems = {}, []
    for code, text in catalogs.items():
        entries, errors = parse_catalog(text)
        parsed[code] = entries
        problems += [f'{code}.ftl:{ln}: {msg}' for ln, msg in errors]
        seen = {}
        for e in entries:
            if e.id in seen:
                problems.append(f'{code}.ftl:{e.line}: {e.id} is defined twice (first at line {seen[e.id]})')
            seen.setdefault(e.id, e.line)
        for e in entries:
            problems += [f'{code}.ftl:{e.line}: {p}' for p in lint_selects(code, e)]
            for ref in sorted(references(e) - set(seen)):
                problems.append(f'{code}.ftl:{e.line}: {e.id} refers to {ref}, which {code}.ftl does not define')
    english = {e.id: e for e in parsed.get('en', [])}
    for code, entries in parsed.items():
        if code == 'en':
            continue
        for e in entries:
            en = english.get(e.id)
            if en is None:
                problems.append(f'{code}.ftl:{e.line}: {e.id} is not in en.ftl')
            elif variables(en) != variables(e):
                problems.append(f'{code}.ftl:{e.line}: {e.id} uses {fmt_vars(variables(e))} '
                                f'but English uses {fmt_vars(variables(en))}')
    return problems


def fmt_vars(names):
    return ', '.join('$' + v for v in sorted(names)) or 'no variables'


# --- The generated block ---------------------------------------------------------------------------


def read(code):
    """(path, head, entries): head is every line outside the generated block; entries maps a
    catalog key to the lines of its message, the `id =` line first."""
    path = os.path.join(LOCALES, code + '.ftl')
    text = open(path, encoding='utf-8').read().replace('\r\n', '\n').rstrip('\n')
    lines = text.split('\n')
    if MARKER in lines:
        i = lines.index(MARKER)
        head, block = lines[:i], lines[i + 1:]
    else:
        head, block = lines, []
    # The block is generated entries and the `# English` comments above them (regenerated by
    # `write`). Anything else in it (a named message written below the block by mistake, with
    # whatever selects it has) joins the head, its lines untouched.
    parsed, _ = parse_catalog('\n'.join(block))
    entries, stray = {}, []
    for e in parsed:
        if ENTRY.match(e.raw[0]):
            entries[e.id.replace('-', '.', 1)] = e.raw
        else:
            stray += [''] + e.raw
    return path, head + stray, entries


def relabel(key, lines):
    return [re.sub(r'^\S+', key.replace('.', '-'), lines[0], count=1)] + lines[1:]


def write(path, head, entries, texts, english_comments):
    out = list(head)
    while out and out[-1] == '':
        out.pop()
    out += ['', MARKER]
    # The comment above a translation is the English on one line, a line break written as \n.
    one_line = lambda t: t.replace('\n', '\\n')
    for key, text in texts.items():
        if key in entries:
            if english_comments:
                out.append('# ' + one_line(text))
            out += entries[key]
    open(path, 'w', encoding='utf-8', newline='\n').write('\n'.join(out) + '\n')


def read_file(path):
    with open(path, encoding='utf-8') as f:
        return f.read()


BARE_DESCRIPTION = re.compile(
    r'def\(\s*"([a-z0-9_]+)"\s*,\s*[A-Za-z:]+\s*,\s*(?:true|false)\s*,\s*"')


def bare_setting_descriptions():
    """A setting described with a plain string is never found by `sync`, so its description stays
    English in every language: it has to be written en("...")."""
    problems = []
    for path in sorted(glob.glob(os.path.join(ROOT, 'crates', 'bf-core', 'src', 'settings', 'defs', '*.rs'))):
        text = read_file(path)
        for m in BARE_DESCRIPTION.finditer(text):
            line = text.count(chr(10), 0, m.start()) + 1
            problems.append(f'{os.path.relpath(path, ROOT)}:{line}: the description of {m.group(1)} is not wrapped in en("...")')
    return problems


def lint_files():
    return lint({c: read_file(os.path.join(LOCALES, c + '.ftl')) for c in LOCALE_CODES}) + bare_setting_descriptions()


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else 'check'
    texts = code_texts()
    if cmd == 'sync':
        write_keys(TABLE)
        path, head, _ = read('en')
        write(path, head, {k: ftl_entry(k, ftl_text(t)) for k, t in texts.items()}, texts, False)
    missing = {}
    for code in ('pl', 'ar'):
        path, head, entries = read(code)
        if cmd == 'sync':
            # The same English already translated under another key (a browser text the server
            # also uses) takes that translation.
            by_text = {texts[k]: v for k, v in entries.items() if k in texts}
            entries.update({k: relabel(k, by_text[t]) for k, t in texts.items() if k not in entries and t in by_text})
            write(path, head, entries, texts, True)
        for key, text in texts.items():
            if key not in entries:
                missing.setdefault(key, [text, '', ''])
    if cmd == 'apply':
        new = json.load(open(sys.argv[2], encoding='utf-8'))
        refused = 0
        for i, code in ((1, 'pl'), (2, 'ar')):
            path, head, entries = read(code)
            for key, row in new.items():
                if key in texts and row[i].strip():
                    want, got = set(PLACEHOLDER.findall(texts[key])), set(PLACEHOLDER.findall(row[i]))
                    if want != got:
                        print(f'refused {code} {key}: uses {sorted(got)} but the English uses {sorted(want)}')
                        refused += 1
                        continue
                    entries[key] = ftl_entry(key, ftl_text(row[i].strip()))
            write(path, head, entries, texts, True)
        print(f'applied {len(new)} translations; {refused} refused')
        return 1 if refused else 0
    if cmd == 'sync':
        os.makedirs(os.path.join(ROOT, 'target'), exist_ok=True)
        out = os.path.join(ROOT, 'target', 'i18n-missing.json')
        json.dump(missing, open(out, 'w', encoding='utf-8'), ensure_ascii=False, indent=1)
        print(f'{len(texts)} messages in the code; {len(missing)} without a translation, listed in {out}')
        return 0
    for key, row in missing.items():
        print(f'{key}  {row[0]}')
    print(f'{len(missing)} of {len(texts)} messages need a translation')
    problems = lint_files()
    if TABLE != FILE_TABLE:
        problems.append('keys.json is out of date: run python scripts/i18n.py sync')
    for p in problems:
        print(p)
    print(f'{len(problems)} problems in the catalogs')
    return 1 if missing or problems else 0


if __name__ == '__main__':
    sys.exit(main())
