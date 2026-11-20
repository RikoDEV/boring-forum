"""Tests for scripts/i18n.py: python scripts/test_i18n.py  (or, from scripts/: python -m unittest test_i18n)."""
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import i18n  # noqa: E402


def decode(lines):
    """What Fluent makes of the lines ftl_entry wrote, back in the code's `{name}` syntax."""
    entries, errors = i18n.parse_catalog('\n'.join(lines) + '\n')
    assert not errors, errors
    assert len(entries) == 1
    return i18n.to_code(entries[0].value)


def round_trip(text):
    return decode(i18n.ftl_entry('msg.0000abcd', i18n.ftl_text(text)))


class RoundTrip(unittest.TestCase):
    def check(self, text, expected=None):
        self.assertEqual(round_trip(text), text if expected is None else expected)

    def test_plain_and_placeholders(self):
        self.check('Hello')
        self.check('Hello {name}, you have {n} posts')

    def test_placeholder_is_a_fluent_variable(self):
        self.assertEqual(i18n.ftl_text('Hi {name}!'), 'Hi { $name }!')

    def test_braces(self):
        self.check('Use {{ and }} for "quotes"', 'Use {{ and }} for "quotes"')
        self.assertEqual(i18n.ftl_text('a { b'), 'a { "{" } b')

    def test_multiline(self):
        self.check('first\nsecond {x}\n\nfourth')

    def test_trailing_newlines(self):
        self.check('Line\n')
        self.check('Line\n\n')
        self.assertTrue(i18n.ftl_text('Line\n').endswith('{ "\\u000A" }'))

    def test_leading_newline(self):
        self.check('\nLine')

    def test_lines_that_look_like_syntax(self):
        self.check('intro\n[brackets] here\n*star\n.dot\nend')
        lines = i18n.ftl_entry('msg.0000abcd', i18n.ftl_text('a\n[b]\n*c\n.d'))
        for line in lines[1:]:
            self.assertNotIn(line.lstrip()[:1], ('[', '*', '.'))

    def test_leading_space_on_a_line(self):
        self.check('a\n  indented\nb')
        self.check('   a\n   b'.strip(' '), 'a\n   b')

    def test_single_line_stays_on_the_id_line(self):
        self.assertEqual(i18n.ftl_entry('nav.x', 'Text'), ['nav-x = Text'])


SAMPLE = '''\
## Hand-written
-brand = Boring Forum
welcome = Welcome to { -brand }, { $name }
count-topics = { $n ->
    [one] { $n } topic
   *[other] { $n } topics
}
who = { $gender ->
    [male] He
    [female] She
   *[other] They
}

## Messages from the code (scripts/i18n.py): named after their English text
# Old
msg-00000001 = One
# Pick
msg-00000002 = { $n ->
    [one] one thing
   *[other] { $n } things
}
msg-00000003 =
    line one
    line two
'''


class ReadWrite(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.old = i18n.LOCALES
        i18n.LOCALES = self.dir.name
        self.addCleanup(setattr, i18n, 'LOCALES', self.old)

    def test_select_in_the_block_survives(self):
        with open(os.path.join(self.dir.name, 'xx.ftl'), 'w', encoding='utf-8') as f:
            f.write(SAMPLE)
        path, head, entries = i18n.read('xx')
        self.assertEqual(sorted(entries), ['msg.00000001', 'msg.00000002', 'msg.00000003'])
        self.assertEqual(entries['msg.00000002'][-1], '}')
        texts = {'msg.00000001': 'One', 'msg.00000002': 'x', 'msg.00000003': 'y'}
        i18n.write(path, head, entries, texts, True)
        again = i18n.read_file(path)
        self.assertIn('count-topics = { $n ->\n    [one] { $n } topic\n   *[other] { $n } topics\n}\nwho', again)
        path, head2, entries2 = i18n.read('xx')
        self.assertEqual((head, entries), (head2, entries2))
        i18n.write(path, head2, entries2, texts, True)
        self.assertEqual(again, i18n.read_file(path))
        self.assertEqual(again.count(i18n.MARKER), 1)

    def test_stray_named_message_joins_the_head(self):
        with open(os.path.join(self.dir.name, 'xx.ftl'), 'w', encoding='utf-8') as f:
            f.write(SAMPLE + 'late = { $n ->\n    [one] a\n   *[other] b\n}\n')
        _, head, entries = i18n.read('xx')
        self.assertEqual(head[-1], '}')
        self.assertNotIn('late', ''.join(k for k in entries))

    def test_relabel(self):
        self.assertEqual(i18n.relabel('js.0000000f', ['msg-00000001 =', '    a']), ['js-0000000f =', '    a'])


def lint(**files):
    return i18n.lint(files)


EN = 'a = Hello { $name }\nitems = { $n ->\n    [one] { $n } item\n   *[other] { $n } items\n}\n'


class Lint(unittest.TestCase):
    def test_clean(self):
        pl = ('a = Cześć { $name }\nitems = { $n ->\n    [one] { $n } rzecz\n    [few] { $n } rzeczy\n'
              '    [many] { $n } rzeczy\n   *[other] { $n } rzeczy\n}\n')
        self.assertEqual(lint(en=EN, pl=pl), [])

    def test_variable_parity(self):
        out = lint(en=EN, pl='a = Cześć { $nick }\nitems = x\n')
        self.assertEqual(len(out), 2)
        self.assertIn('a uses $nick but English uses $name', out[0])
        self.assertIn('items uses no variables but English uses $n', out[1])

    def test_variables_inside_selects_and_selectors(self):
        en = 'g = { $who ->\n    [a] { $x }\n   *[other] { NUMBER($y) }\n}\n'
        entries, _ = i18n.parse_catalog(en)
        self.assertEqual(i18n.variables(entries[0]), {'who', 'x', 'y'})
        self.assertEqual(lint(en=en, pl='g = { $who ->\n    [a] { $x }\n   *[other] z\n}\n')[0].split(': ')[1][:1], 'g')

    def test_nested_select(self):
        en = 'g = { $a ->\n    [x] { $b ->\n        [one] 1\n       *[other] 2\n    }\n   *[other] { $c }\n}\n'
        entries, errors = i18n.parse_catalog(en)
        self.assertEqual(errors, [])
        self.assertEqual(i18n.variables(entries[0]), {'a', 'b', 'c'})

    def test_invalid_plural_category(self):
        out = lint(en=EN.replace('[one]', '[few]'))
        self.assertTrue(any('[few] is not a plural category of en' in p for p in out), out)
        pl = 'items = { $n ->\n    [zero] a\n   *[other] b\n}\n'
        self.assertTrue(any('[zero] is not a plural category of pl' in p for p in lint(en=EN, pl=pl)))

    def test_arabic_has_all_six(self):
        ar = ('items = { $n ->\n    [zero] a\n    [one] a\n    [two] a\n    [few] a\n    [many] a\n   *[other] a\n}\n')
        ar = ar.replace('a\n', '{ $n }\n')
        self.assertEqual(lint(en=EN, ar=ar), [])

    def test_numeric_keys_and_gender_selects_are_allowed(self):
        en = 'items = { $n ->\n    [0] none\n    [one] one\n   *[other] { $n }\n}\nwho = { $g ->\n    [male] He\n   *[other] They\n}\n'
        self.assertEqual(lint(en=en), [])

    def test_gender_key_mixed_with_categories(self):
        en = 'x = { $n ->\n    [one] a\n    [male] b\n   *[other] c\n}\n'
        self.assertTrue(any('[male] is mixed' in p for p in lint(en=en)))

    def test_missing_default(self):
        en = 'x = { $n ->\n    [one] a\n    [other] b\n}\n'
        self.assertTrue(any('exactly one `*` default' in p for p in lint(en=en)))

    def test_duplicate_ids(self):
        out = lint(en='a = 1\nb = 2\na = 3\n')
        self.assertEqual(len(out), 1)
        self.assertIn('a is defined twice (first at line 1)', out[0])

    def test_translation_without_english(self):
        out = lint(en='a = 1\n', pl='a = 1\nb = 2\n')
        self.assertEqual(out, ['pl.ftl:2: b is not in en.ftl'])

    def test_terms_and_references(self):
        en = '-brand = Forum\nhi = Welcome to { -brand }, { $name }\nbye = { hi }\n'
        self.assertEqual(lint(en=en, pl='-brand = Forum\nhi = Witaj w { -brand }, { $name }\nbye = { hi }\n'), [])
        out = lint(en='x = { -nope } { missing }\n')
        self.assertEqual(len(out), 2)

    def test_term_attributes_and_arguments(self):
        en = '-b = Forum\n    .gender = neuter\nx = { -b(case: "gen") } { -b.gender } { NUMBER($n, minimumFractionDigits: 2) }\n'
        entries, errors = i18n.parse_catalog(en)
        self.assertEqual(errors, [])
        self.assertEqual(i18n.variables(entries[1]), {'n'})

    def test_parse_errors_are_reported(self):
        out = lint(en='a = { $n ->\n    [one] x\n')
        self.assertEqual(len(out), 1)
        self.assertIn('en.ftl:1', out[0])
        self.assertTrue(lint(en='a = unbalanced }\nb = ok\n'))

    def test_unquoted_syntax_line_is_an_error(self):
        out = lint(en='a =\n    one\n    [two]\n')
        self.assertTrue(out and 'quote it' in out[0], out)

    def test_comments_and_blank_lines_between_entries(self):
        en = '# Note\na = 1\n\n\n## Group\nb = { $n ->\n    [one] x\n   *[other] y\n}\n# tail\nc = 3\n'
        entries, errors = i18n.parse_catalog(en)
        self.assertEqual([e.id for e in entries], ['a', 'b', 'c'])
        self.assertEqual(errors, [])


class RealCatalogs(unittest.TestCase):
    def test_catalogs_parse(self):
        for code in i18n.LOCALE_CODES:
            text = i18n.read_file(os.path.join(i18n.LOCALES, code + '.ftl'))
            _, errors = i18n.parse_catalog(text)
            self.assertEqual(errors, [], code)


if __name__ == '__main__':
    unittest.main()
