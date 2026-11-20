# Moderating a forum

Moderators and administrators work from two places on the site itself (not the admin panel):

- **Review** (`/review`): the queue of things waiting for a decision.
- **Moderator panel** (`/modcp`): look-ups and sanctions that are not tied to one item.

Both are shown only to people who moderate something. Everyone else gets the ordinary "not found"
page. A **scoped moderator** (set on a forum's People tab in **Admin, Forums**) sees and acts on
that forum only. A group given the *moderate* permission on a forum moderates it, and
administrators see everything.

## The review queue

`/review` lists, newest first, what needs a look. A moderator can **claim** an item so two people
do not work on the same one, then resolve it.

- **Reports.** Members report a post or a member and choose a reason (the reasons are yours to edit
  under **Admin, Community**). Once enough different members report a post it is hidden from other
  members until a moderator looks: the number is the `flag_hide_score` setting, 3 by default.
- **Held posts.** A forum can hold all posts, or only posts with links, for approval
  (**Admin, Forums, Posting**). Watched words and the spam rules can hold a post too. Approve it,
  or reject it.
- **Accounts waiting for approval.** With `require_account_approval` on, new accounts wait here and
  can read but not post.
- **Reported messages** (`/review/messages`). Private messages a member reported. A moderator reads one
  only through its report.
- **Deleted content** (`/review/deleted`): the log of what was removed, by whom, and a way back.
- **Content from plugins** (`/review/apps`), for plugins that hold submissions for review.
- **Notices and appeals** (`/review/dsa`): see below.

## What a moderator can do

On a topic or post: hide or show, edit, move, merge, split, lock, pin (with an expiry), delete and
restore. Moderators can see a poster's IP address; it is kept for
`ip_retention_days` (180 by default) and then cleared. **Saved moderator actions**
(**Admin, Saved moderator actions**) bundle steps such as lock, pin, move and post a canned reply
into one click.

On a member:

- **Warn** them. Warnings carry points. Active points silence a member at `warning_silence_points`
  (5) and suspend them at `warning_suspend_points` (10), for `warning_action_days` (7); points stop
  counting after `warning_expire_days` (90). Set a threshold to 0 to turn it off.
- **Silence** them: they can read but not post.
- **Ban** them, with a reason and an optional end date. The moderator panel also bans an IP address
  or an email address, and keeps an allow list.
- **Add a note** only staff can see. Notes can also be put on a single post.

Every staff action is written to the **Staff log** (**Admin, Staff log**). The log cannot be
edited, not even by the application: the database refuses.

## Keeping spam out

Most of this is on **Admin, Settings, Spam protection**:

- A challenge at sign-up (see `BF_CAPTCHA_SECRET` in [Configuration](configuration.md)) and a
  minimum time a sign-up form must take.
- Passwords are checked against known breaches.
- A limit on accounts created from one address within a window.
- A refusal of a post identical to the member's own from the last few minutes.
- Lists of domains members may not link to, and of domains whose links hold the post for review.
- Browser identifiers (user agents) to refuse.
- New members whose post is hidden by reports are silenced for a while, up to a trust level you
  choose; links to one site from many new members in a week are reported as spam.
- Optional AI checks, if you turn the AI features on (**Admin, AI**).

## Notices and appeals (EU Digital Services Act)

When a moderator hides or removes someone's content, or restricts their account, the member is owed
a **statement of reasons**. The forum writes it for them: it is sent as a notification and an
email and kept with the case. The member can **appeal**. Open notices and appeals at
`/review/dsa`, decide each, and the decision is recorded. See the same section in
[Features and settings](features.md#eu-digital-services-act) for what is sent.

## Good habits

- Claim before you act, and leave a note when the reason is not obvious.
- Prefer hiding to deleting while you decide: a hidden post can be shown again.
- Use warnings before bans so the record explains the ban.
- Look at the Staff log now and then. It is how you notice a moderator account that is doing
  more, or less, than it should.
