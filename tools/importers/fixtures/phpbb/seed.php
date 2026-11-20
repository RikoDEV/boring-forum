<?php
// Fills the phpBB test install with content, using phpBB's own message parser so posts are stored
// exactly as phpBB stores them. Run inside the fx-php container: php /fx/seed-phpbb.php
define('IN_PHPBB', true);
$phpbb_root_path = '/src/phpBB/';
$phpEx = 'php';
$_SERVER += ['SERVER_NAME' => 'localhost', 'SERVER_PORT' => '8000', 'HTTP_HOST' => 'localhost:8000', 'REQUEST_URI' => '/', 'SCRIPT_NAME' => '/seed.php', 'REMOTE_ADDR' => '127.0.0.1', 'REQUEST_METHOD' => 'GET', 'HTTP_USER_AGENT' => 'seed'];
chdir($phpbb_root_path);
include $phpbb_root_path . 'common.php';
include $phpbb_root_path . 'includes/functions_user.php';
include $phpbb_root_path . 'includes/functions_posting.php';
include $phpbb_root_path . 'includes/message_parser.php';
$user->session_begin();
$auth->acl($user->data);
$user->lang = []; $user->lang_name = "en";

$t0 = time() - 86400 * 400;
$q = fn($sql) => $db->sql_query($sql);
$esc = fn($s) => $db->sql_escape($s);
$q("SET SESSION sql_mode=''");

// ---- forums (nested set by hand: the tree is small) ---------------------------------------------
$q("UPDATE phpbb_forums SET forum_name='General', forum_desc='Everything about the board' WHERE forum_id=1");
$q("UPDATE phpbb_forums SET forum_name='Announcements', forum_desc='News', left_id=2, right_id=3 WHERE forum_id=2");
$forum = function ($id, $parent, $left, $right, $name, $type, $desc = '') use ($q, $esc) {
    $q("INSERT INTO phpbb_forums (forum_id, parent_id, left_id, right_id, forum_name, forum_desc, forum_type, forum_posts_approved, forum_topics_approved) VALUES ($id, $parent, $left, $right, '" . $esc($name) . "', '" . $esc($desc) . "', $type, 0, 0)");
};
$forum(3, 1, 4, 7, 'Discussion', 1, 'Talk about anything');
$forum(4, 3, 5, 6, 'Off topic', 1);
$forum(5, 1, 8, 9, 'Staff area', 1, 'Moderators only');
$forum(6, 0, 11, 14, 'Support', 0);
$forum(7, 6, 12, 13, 'Help', 1, 'Ask for help');
$q("UPDATE phpbb_forums SET left_id=1, right_id=10 WHERE forum_id=1");
foreach ([3, 4, 7] as $f) {
    $q("INSERT INTO phpbb_acl_groups (group_id, forum_id, auth_option_id, auth_role_id, auth_setting) SELECT group_id, $f, auth_option_id, auth_role_id, auth_setting FROM phpbb_acl_groups WHERE forum_id=2");
}
// Staff area: only moderators and administrators.
$q("INSERT INTO phpbb_acl_groups (group_id, forum_id, auth_option_id, auth_role_id, auth_setting) SELECT group_id, 5, auth_option_id, auth_role_id, auth_setting FROM phpbb_acl_groups WHERE forum_id=2 AND group_id IN (4,5)");

// ---- members -------------------------------------------------------------------------------------
$people = [
    ['alice', 'Alice-pass-1', 400], ['bob', 'Bob-pass-2', 300], ['carol', 'Carol-pass-3', 200],
    ['dave', 'Dave-pass-4', 100], ['erin_ł', 'Erin-pass-5', 50], ['frank', 'Frank-pass-6', 10],
];
$ids = [];
foreach ($people as [$name, $pass, $daysAgo]) {
    $ids[$name] = user_add([
        'username' => $name, 'user_password' => phpbb_hash($pass), 'user_email' => "$name@example.org",
        'group_id' => 2, 'user_type' => USER_NORMAL, 'user_regdate' => time() - 86400 * $daysAgo,
        'user_lang' => 'en', 'user_timezone' => 'UTC', 'user_ip' => '127.0.0.1', 'user_new' => 0,
    ]);
}
// An old phpBB 3.0 hash, an inactive account and a moderator.
$q("UPDATE phpbb_users SET user_password='\$H\$9" . "1234567" . "eZYgtfUoDkYnyDAsU9nbLpZvRM6EQ0' WHERE user_id=" . $ids['dave']);
$q("UPDATE phpbb_users SET user_type=1 WHERE user_id=" . $ids['frank']);
group_user_add(4, [$ids['carol']]);
$gid = 0;
group_create($gid, GROUP_OPEN, 'Beta testers', 'People who test things', ['group_colour' => '00AA00', 'group_rank' => 0]);
group_user_add($gid, [$ids['alice'], $ids['bob']]);
// Avatars (uploaded) for alice and bob, and a signature for alice.
foreach (['alice' => [200, 120, 40], 'bob' => [40, 120, 200]] as $n => $rgb) {
    $im = imagecreatetruecolor(100, 100);
    imagefill($im, 0, 0, imagecolorallocate($im, ...$rgb));
    $file = $config['avatar_salt'] . '_' . $ids[$n] . '.png';
    imagepng($im, $phpbb_root_path . 'images/avatars/upload/' . $file);
    $q("UPDATE phpbb_users SET user_avatar='" . $ids[$n] . "_" . time() . ".png', user_avatar_type='avatar.driver.upload', user_avatar_width=100, user_avatar_height=100 WHERE user_id=" . $ids[$n]);
}
$sig = new parse_message('Regards, [b]Alice[/b]');
$sig->parse(true, true, true, true, true, true, true);
$q("UPDATE phpbb_users SET user_sig='" . $esc($sig->message) . "', user_sig_bbcode_uid='" . $esc($sig->bbcode_uid) . "', user_sig_bbcode_bitfield='" . $esc($sig->bbcode_bitfield) . "' WHERE user_id=" . $ids['alice']);

// ---- helpers for topics and posts ----------------------------------------------------------------
function uname($ids, $id) { global $db; $r = $db->sql_fetchrow($db->sql_query('SELECT username FROM phpbb_users WHERE user_id=' . (int) $id)); return $r['username']; }
function parsed($text) {
    $mp = new parse_message($text);
    $mp->parse(true, true, true, true, true, true, true);
    return $mp;
}
$posts_made = 0;
$add_post = function ($forum_id, $topic_id, $author, $subject, $text, $time, $visibility = 1) use ($q, $esc, $db, &$posts_made, $ids) {
    $mp = parsed($text);
    $q("INSERT INTO phpbb_posts (topic_id, forum_id, poster_id, post_time, post_visibility, post_subject, post_text, post_checksum, enable_bbcode, enable_smilies, enable_magic_url, enable_sig, bbcode_bitfield, bbcode_uid, poster_ip, post_username)
        VALUES ($topic_id, $forum_id, $author, $time, $visibility, '" . $esc($subject) . "', '" . $esc($mp->message) . "', '" . md5($mp->message) . "', 1, 1, 1, 1, '" . $esc($mp->bbcode_bitfield) . "', '" . $esc($mp->bbcode_uid) . "', '127.0.0.1', '')");
    $posts_made++;
    return $db->sql_nextid();
};
$topic = function ($forum_id, $author, $title, $text, $time, $opts = []) use ($q, $esc, $db, $add_post) {
    $opts += ['type' => 0, 'status' => 0, 'views' => 0, 'poll' => null];
    $q("INSERT INTO phpbb_topics (forum_id, topic_poster, topic_time, topic_title, topic_views, topic_status, topic_type, topic_visibility, topic_first_poster_name, topic_last_poster_name, topic_last_post_time, topic_last_post_subject)
        VALUES ($forum_id, $author, $time, '" . $esc($title) . "', " . (int) $opts['views'] . ", " . (int) $opts['status'] . ", " . (int) $opts['type'] . ", 1, '" . $esc(uname(null, $author)) . "', '" . $esc(uname(null, $author)) . "', $time, '" . $esc($title) . "')");
    $tid = $db->sql_nextid();
    $pid = $add_post($forum_id, $tid, $author, $title, $text, $time);
    $q("UPDATE phpbb_topics SET topic_first_post_id=$pid, topic_last_post_id=$pid, topic_posts_approved=1 WHERE topic_id=$tid");
    if ($opts['poll']) {
        [$ptitle, $options, $max, $length, $change, $votes] = $opts['poll'];
        $q("UPDATE phpbb_topics SET poll_title='" . $esc($ptitle) . "', poll_start=$time, poll_length=$length, poll_max_options=$max, poll_vote_change=$change WHERE topic_id=$tid");
        foreach ($options as $i => $text) {
            $q("INSERT INTO phpbb_poll_options (poll_option_id, topic_id, poll_option_text, poll_option_total) VALUES (" . ($i + 1) . ", $tid, '" . $esc($text) . "', " . count(array_filter($votes, fn($v) => $v[1] == $i + 1)) . ")");
        }
        foreach ($votes as [$uid, $opt]) {
            $q("INSERT INTO phpbb_poll_votes (topic_id, poll_option_id, vote_user_id, vote_user_ip) VALUES ($tid, $opt, $uid, '127.0.0.1')");
        }
    }
    return [$tid, $pid];
};
$reply = function ($forum_id, $tid, $author, $text, $time, $visibility = 1) use ($q, $add_post, $db) {
    $row = $db->sql_fetchrow($db->sql_query("SELECT topic_title FROM phpbb_topics WHERE topic_id=$tid"));
    $pid = $add_post($forum_id, $tid, $author, 'Re: ' . $row['topic_title'], $text, $time, $visibility);
    if ($visibility == 1) {
        $q("UPDATE phpbb_topics SET topic_last_post_id=$pid, topic_last_post_time=$time, topic_last_poster_id=$author, topic_last_poster_name='" . $db->sql_escape(uname(null, $author)) . "', topic_posts_approved=topic_posts_approved+1 WHERE topic_id=$tid");
    }
    return $pid;
};
$attach = function ($pid, $tid, $poster, $real, $bytes, $mime, $ext) use ($q, $esc, $phpbb_root_path) {
    $phys = md5($real . $pid) . '.' . $ext;
    file_put_contents($phpbb_root_path . 'files/' . $phys, $bytes);
    $q("INSERT INTO phpbb_attachments (post_msg_id, topic_id, in_message, is_orphan, poster_id, physical_filename, real_filename, attach_comment, extension, mimetype, filesize, filetime) VALUES ($pid, $tid, 0, 0, $poster, '$phys', '" . $esc($real) . "', 'a test file', '$ext', '$mime', " . strlen($bytes) . ", " . time() . ")");
    $q("UPDATE phpbb_posts SET post_attachment=1 WHERE post_id=$pid");
    $q("UPDATE phpbb_topics SET topic_attachment=1 WHERE topic_id=$tid");
};
$png = function ($w, $h, $rgb) { $im = imagecreatetruecolor($w, $h); imagefill($im, 0, 0, imagecolorallocate($im, ...$rgb)); ob_start(); imagepng($im); return ob_get_clean(); };

// ---- content --------------------------------------------------------------------------------------
$a = $ids['alice']; $b = $ids['bob']; $c = $ids['carol']; $d = $ids['dave']; $e = $ids['erin_ł'];
$T = fn($days) => time() - 86400 * $days;

[$t1, $p1] = $topic(2, 2, 'Welcome to the board', "Welcome everyone! Please read the [b]rules[/b].\n\n[list]\n[*]Be kind\n[*]No spam\n[/list]", $T(399), ['type' => 3, 'views' => 5120]);
[$t2, $p2] = $topic(3, $a, 'Formatting showcase', "[b]bold[/b] [i]italic[/i] [u]underline[/u] [s]strike[/s]\n[size=150]big text[/size] and [color=red]red text[/color]\n\n[quote=\"bob\"]A quoted line from Bob[/quote]\nA reply with a link: [url=https://example.org/page]Example site[/url] and magic https://example.org/auto\n\n[code]\$x = 1;\nprint(\$x);\n[/code]\n\n[list=1]\n[*]first\n[*]second\n[/list]\n\nSmilies :) :D ;) and an image:\n[img]https://example.org/pic.png[/img]\n\nMail: [email]alice@example.org[/email]", $T(300), ['views' => 842]);
$r = $reply(3, $t2, $b, "[quote=\"alice\" post_id={$p2} time=1700000000 user_id={$a}]\n[quote=\"bob\"]A quoted line from Bob[/quote]\nA reply with a link[/quote]\nThanks Alice, great showcase!", $T(299));
$reply(3, $t2, $c, "Nice. Here is [b]carol[/b]'s answer with an attachment:\n[attachment=0]screen.png[/attachment]", $T(298));
$cp = $db->sql_fetchrow($db->sql_query("SELECT max(post_id) m FROM phpbb_posts"))['m'];
$attach($cp, $t2, $c, 'screen.png', $png(320, 200, [30, 90, 160]), 'image/png', 'png');
$attach($cp, $t2, $c, 'notes.pdf', "%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF", 'application/pdf', 'pdf');
$reply(3, $t2, $d, 'An unapproved reply that should not be exported.', $T(297), 0);
$reply(3, $t2, $d, 'A soft-deleted reply.', $T(296), 2);

[$t3, $p3] = $topic(3, $b, 'Which editor do you use?', 'Vote in the poll below.', $T(200), [
    'views' => 300,
    'poll' => ['Favourite editor', ['Vim', 'Emacs', 'VS Code'], 1, 86400 * 30, 1, [[$a, 3], [$c, 3], [$d, 1], [$e, 2]]],
]);
[$t4, $p4] = $topic(3, $c, 'Pick your toppings (multiple choice)', 'Choose up to two.', $T(150), [
    'poll' => ['Toppings', ['Cheese', 'Ham', 'Pineapple', 'Mushrooms'], 2, 0, 0, [[$a, 1], [$a, 2], [$b, 4]]],
]);
[$t5, $p5] = $topic(4, $d, 'Locked thread', 'This one is closed.', $T(120), ['status' => 1, 'views' => 99]);
[$t6, $p6] = $topic(7, $e, 'Help: my build fails', "Here is the log:\n[code]error: linking failed\n[/code]\nUnicode: zażółć gęślą jaźń", $T(60), ['type' => 1, 'views' => 1500]);
$reply(7, $t6, $a, 'Did you try cleaning the build directory?', $T(59));
[$t7, $p7] = $topic(5, $c, 'Moderator-only planning', 'Nobody outside staff should see this.', $T(30));
$reply(5, $t7, 2, 'Agreed.', $T(29));
[$t8, $p8] = $topic(2, 2, 'Announcement: maintenance', 'The board will be down on Friday.', $T(20), ['type' => 2]);
// A moved topic leaves a shadow topic.
$q("INSERT INTO phpbb_topics (forum_id, topic_poster, topic_time, topic_title, topic_moved_id, topic_visibility) VALUES (4, $d, " . $T(10) . ", 'Moved: old topic', $t6, 1)");

// ---- private messages -----------------------------------------------------------------------------
$pm = function ($from, $to, $subject, $text, $time) use ($q, $esc, $db) {
    $mp = parsed($text);
    $q("INSERT INTO phpbb_privmsgs (root_level, author_id, message_time, enable_bbcode, enable_smilies, enable_magic_url, enable_sig, message_subject, message_text, message_edit_reason, message_edit_user, message_edit_time, message_edit_count, message_attachment, bbcode_bitfield, bbcode_uid, to_address, bcc_address, author_ip)
        VALUES (0, $from, $time, 1, 1, 1, 1, '" . $esc($subject) . "', '" . $esc($mp->message) . "', '', 0, 0, 0, 0, '" . $esc($mp->bbcode_bitfield) . "', '" . $esc($mp->bbcode_uid) . "', 'u_$to', '', '127.0.0.1')");
    $mid = $db->sql_nextid();
    $q("INSERT INTO phpbb_privmsgs_to (msg_id, user_id, author_id, pm_deleted, pm_new, pm_unread, pm_replied, pm_marked, pm_forwarded, folder_id) VALUES ($mid, $to, $from, 0, 0, 0, 0, 0, 0, 0)");
    $q("INSERT INTO phpbb_privmsgs_to (msg_id, user_id, author_id, pm_deleted, pm_new, pm_unread, pm_replied, pm_marked, pm_forwarded, folder_id) VALUES ($mid, $from, $from, 0, 0, 0, 0, 0, 0, -1)");
};
$pm($a, $b, 'Hello Bob', "Hi [b]Bob[/b], are you coming? :)", $T(90));
$pm($b, $a, 'Re: Hello Bob', 'Yes, see you there.', $T(89));

// ---- counters, as phpBB keeps them -----------------------------------------------------------------
$q("UPDATE phpbb_users u SET user_posts = (SELECT count(*) FROM phpbb_posts p WHERE p.poster_id=u.user_id AND p.post_visibility=1)");
$q("UPDATE phpbb_forums f SET forum_posts_approved = (SELECT count(*) FROM phpbb_posts p WHERE p.forum_id=f.forum_id AND p.post_visibility=1), forum_topics_approved = (SELECT count(*) FROM phpbb_topics t WHERE t.forum_id=f.forum_id AND t.topic_visibility=1 AND t.topic_moved_id=0)");
echo "seeded: $posts_made posts\n";
