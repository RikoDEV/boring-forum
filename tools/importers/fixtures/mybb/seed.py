"""Fills the MyBB test install: writes seed.sql (feed it to the mybb database) and the attachment and avatar files.
usage: python seed-mybb.py <mybb folder> <out.sql>"""
import hashlib, os, struct, sys, time, zlib

root, out_path = sys.argv[1], sys.argv[2]
NOW = int(time.time())
DAY = 86400
sql = ["SET SESSION sql_mode='';"]

def q(v):
    if v is None:
        return "NULL"
    if isinstance(v, int):
        return str(v)
    return "'" + str(v).replace("\\", "\\\\").replace("'", "''") + "'"

def insert(table, **cols):
    sql.append(f"INSERT INTO mybb_{table} ({', '.join(cols)}) VALUES ({', '.join(q(v) for v in cols.values())});")

def md5(s):
    return hashlib.md5(s.encode()).hexdigest()

def png(w, h, rgb):
    raw = b"".join(b"\x00" + bytes(rgb) * w for _ in range(h))
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")

# ---- members: MyBB's own scheme is md5(md5(salt) . md5(password)) --------------------------------------------
people = [("alice", "Alice-pass-1", 400, 2), ("bob", "Bob-pass-2", 300, 2), ("carol", "Carol-pass-3", 200, 6),
          ("dave", "Dave-pass-4", 100, 2), ("erin", "Erin-pass-5", 50, 2), ("frank", "Frank-pass-6", 10, 5)]
uid = {}
for i, (name, pw, days, group) in enumerate(people, start=2):
    salt = md5(name)[:8]
    uid[name] = i
    insert("users", uid=i, username=name, password=md5(md5(salt) + md5(pw)), salt=salt, loginkey=md5(name + "k")[:50],
           email=f"{name}@example.org", usergroup=group, additionalgroups="8" if name in ("alice", "bob") else "",
           regdate=NOW - DAY * days, lastactive=NOW - DAY, lastvisit=NOW - DAY, postnum=0, signature="Regards, [b]%s[/b]" % name if name == "alice" else "")
# alice and bob have uploaded avatars
os.makedirs(os.path.join(root, "uploads", "avatars"), exist_ok=True)
for name, rgb in (("alice", (200, 120, 40)), ("bob", (40, 120, 200))):
    fn = f"avatar_{uid[name]}.png"
    open(os.path.join(root, "uploads", "avatars", fn), "wb").write(png(100, 100, rgb))
    sql.append(f"UPDATE mybb_users SET avatar='./uploads/avatars/{fn}?dateline={NOW}', avatardimensions='100|100', avatartype='upload' WHERE uid={uid[name]};")
insert("usergroups", gid=8, type=2, title="Beta testers", description="People who test things", namestyle="{username}", canview=1, canviewthreads=1)

# ---- forums and who can see them ------------------------------------------------------------------------------------
def forum(fid, pid, name, kind, order, desc="", parents=""):
    insert("forums", fid=fid, name=name, description=desc, type=kind, pid=pid, parentlist=parents or str(fid), disporder=order, active=1, open=1, linkto="")
forum(1, 0, "General", "c", 1, "", "1")
forum(2, 1, "Announcements", "f", 1, "News", "1,2")
forum(3, 1, "Discussion", "f", 2, "Talk about anything", "1,3")
forum(4, 3, "Off topic", "f", 1, "", "1,3,4")
forum(5, 1, "Staff area", "f", 3, "Moderators only", "1,5")
forum(6, 0, "Support", "c", 2, "", "6")
forum(7, 6, "Help", "f", 1, "Ask for help", "6,7")
# Staff area: guests and members may not see it.
for g in (1, 2):
    insert("forumpermissions", fid=5, gid=g, canview=0, canviewthreads=0, canonlyviewownthreads=0, candlattachments=0, canpostthreads=0, canpostreplys=0, canpostattachments=0)

# ---- threads and posts ---------------------------------------------------------------------------------------------
pid_counter = [0]
def post(tid, fid, author, subject, message, days, visible=1, replyto=0):
    pid_counter[0] += 1
    insert("posts", pid=pid_counter[0], tid=tid, replyto=replyto, fid=fid, subject=subject, uid=uid[author], username=author,
           dateline=NOW - DAY * days, message=message, ipaddress="127.0.0.1", includesig=1, visible=visible)
    return pid_counter[0]
tid_counter = [0]
def thread(fid, author, subject, message, days, views=0, sticky=0, closed="", visible=1, poll=0):
    tid_counter[0] += 1
    tid = tid_counter[0]
    pid = post(tid, fid, author, subject, message, days, visible)
    insert("threads", tid=tid, fid=fid, subject=subject, uid=uid[author], username=author, dateline=NOW - DAY * days, firstpost=pid,
           lastpost=NOW - DAY * days, lastposter=author, lastposteruid=uid[author], views=views, sticky=sticky, closed=closed, visible=visible, poll=poll)
    return tid, pid
def reply(tid, fid, author, message, days, visible=1):
    pid = post(tid, fid, author, "RE: thread", message, days, visible)
    if visible == 1:
        sql.append(f"UPDATE mybb_threads SET lastpost={NOW - DAY * days}, lastposter='{author}', lastposteruid={uid[author]}, replies=replies+1 WHERE tid={tid};")
    return pid

t1, p1 = thread(2, "carol", "Welcome to the board", "Welcome everyone! Please read the [b]rules[/b].\n\n[list]\n[*]Be kind\n[*]No spam\n[/list]", 399, views=5120, sticky=1)
t2, p2 = thread(3, "alice", "Formatting showcase", (
    "[b]bold[/b] [i]italic[/i] [u]underline[/u] [s]strike[/s]\n[size=large]big text[/size] and [color=red]red text[/color] [color=#ff8800]hex[/color]\n"
    "[align=center]centered[/align]\n\n[quote='bob' pid='99' dateline='1700000000']A quoted line from Bob[/quote]\n"
    "A reply with a link: [url=https://example.org/page]Example site[/url] and [url]https://example.org/auto[/url]\n\n"
    "[code]\nx = 1\nprint(x)\n[/code]\n\n[php]echo 'hi';[/php]\n\n[list=1]\n[*]first\n[*]second\n[/list]\n\n"
    "Smilies :) :D ;) and images:\n[img]https://example.org/pic.png[/img] [img=50x50]https://example.org/small.png[/img]\n"
    "Mail: [email]alice@example.org[/email]\n[hr]\n[video=youtube]https://www.youtube.com/watch?v=dQw4w9WgXcQ[/video]"), 300, views=842)
r = reply(t2, 3, "bob", f"[quote='alice' pid='{p2}' dateline='1700000000']\n[quote='bob']A quoted line[/quote]\nA reply with a link[/quote]\nThanks Alice, great showcase!", 299)
r2 = reply(t2, 3, "carol", "Nice. Here is [b]carol[/b]'s answer with an attachment:\n[attachment=1]", 298)
reply(t2, 3, "dave", "An unapproved reply that should not be exported.", 297, visible=0)
reply(t2, 3, "dave", "A soft-deleted reply.", 296, visible=-1)
t3, _ = thread(3, "bob", "Which editor do you use?", "Vote in the poll below.", 200, views=300, poll=1)
t4, _ = thread(3, "carol", "Pick your toppings (multiple choice)", "Choose up to two.", 150, poll=2)
t5, _ = thread(4, "dave", "Locked thread", "This one is closed.", 120, views=99, closed="1")
t6, _ = thread(7, "erin", "Help: my build fails", "Here is the log:\n[code]error: linking failed\n[/code]\nUnicode: zażółć gęślą jaźń", 60, views=1500)
reply(t6, 7, "alice", "Did you try cleaning the build directory?", 59)
t7, _ = thread(5, "carol", "Moderator-only planning", "Nobody outside staff should see this.", 30)
# A moved thread leaves a redirect behind.
tid_counter[0] += 1
insert("threads", tid=tid_counter[0], fid=4, subject="Moved: old topic", uid=uid["dave"], username="dave", dateline=NOW - DAY * 10, closed=f"moved|{t6}", visible=1)

# polls: options and votes are joined with ||~|~||
SEP = "||~|~||"
insert("polls", pid=1, tid=t3, question="Favourite editor", dateline=NOW - DAY * 200, options=SEP.join(["Vim", "Emacs", "VS Code"]), votes=SEP.join(["1", "1", "2"]),
       numoptions=3, numvotes=4, timeout=30, closed=0, multiple=0, public=1, maxoptions=0)
for u, o in (("alice", 3), ("carol", 3), ("dave", 1), ("erin", 2)):
    insert("pollvotes", pid=1, uid=uid[u], voteoption=o, dateline=NOW - DAY * 199)
insert("polls", pid=2, tid=t4, question="Toppings", dateline=NOW - DAY * 150, options=SEP.join(["Cheese", "Ham", "Pineapple", "Mushrooms"]), votes=SEP.join(["1", "1", "0", "1"]),
       numoptions=4, numvotes=2, timeout=0, closed=0, multiple=1, public=0, maxoptions=2)
for u, o in (("alice", 1), ("alice", 2), ("bob", 4)):
    insert("pollvotes", pid=2, uid=uid[u], voteoption=o, dateline=NOW - DAY * 149)

# attachments: files are named like MyBB names them, under uploads/YYYYMM/
month = time.strftime("%Y%m", time.gmtime(NOW - DAY * 298))
os.makedirs(os.path.join(root, "uploads", month), exist_ok=True)
for aid, (fname, ftype, data) in enumerate([("screen.png", "image/png", png(320, 200, (30, 90, 160))), ("notes.pdf", "application/pdf", b"%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF")], start=1):
    stored = f"post_{uid['carol']}_{NOW}_{md5(fname)[:10]}.attach"
    open(os.path.join(root, "uploads", month, stored), "wb").write(data)
    insert("attachments", aid=aid, pid=r2, posthash="", uid=uid["carol"], filename=fname, filetype=ftype, filesize=len(data), attachname=f"{month}/{stored}", downloads=3, dateuploaded=NOW - DAY * 298, visible=1)

# private messages: one row in the receiver's inbox and one in the sender's sent items
def pm(n, sender, receiver, subject, message, days):
    for owner, folder in ((receiver, 1), (sender, 2)):
        insert("privatemessages", uid=uid[owner], toid=uid[receiver], fromid=uid[sender], recipients=f'a:1:{{s:2:"to";a:1:{{i:0;s:{len(str(uid[receiver]))}:"{uid[receiver]}";}}}}',
               folder=folder, subject=subject, status=1, dateline=NOW - DAY * days, message=message)
pm(1, "alice", "bob", "Hello Bob", "Hi [b]Bob[/b], are you coming? :)", 90)
pm(2, "bob", "alice", "RE: Hello Bob", "Yes, see you there.", 89)

# counters
sql.append("UPDATE mybb_users u SET postnum=(SELECT count(*) FROM mybb_posts p WHERE p.uid=u.uid AND p.visible=1);")
open(out_path, "w", encoding="utf-8").write("\n".join(sql) + "\n")
print("seed written:", len(sql), "statements")
