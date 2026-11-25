# Administering a forum

The admin panel is at `/admin`. It is shown to administrators only: anyone else, signed in or
not, gets the site's ordinary "not found" page, so the panel's existence is not confirmed to
visitors. Changes that matter (anything destructive, backups, API keys) ask for your password again.

This page explains how the pieces fit together and where each setting lives. For environment
variables and server settings see [Configuration](configuration.md); for backups and monitoring
see [Operations](operations.md).

## How a forum is organised

- **Categories** group forums on the index. **Forums** hold topics. A forum can have sub-forums,
  and can also be a link to another address or an archive.
- A forum's **mode** is *Discussion*, *Questions and answers* (answers are voted best first) or *Ideas* (members vote for topics).
- **Groups** hold members. Permissions are granted to groups, per forum.
- A member's **trust level** rises as they take part, and unlocks things. Staff set the rules.

## Permissions

Open **Forums**, pick a forum and open its **Permissions** tab. Each group has seven switches:

| Permission | Lets the group |
| --- | --- |
| view | see the forum in lists |
| read | read its topics |
| create | start topics |
| reply | reply to topics |
| attach | upload files to its posts |
| download | download those files |
| moderate | moderate it (hide, move, lock, merge and so on) |

A forum can **use its parent's permissions**, so changing the category changes everything under
it; untick the switch to give a forum its own. A category the group cannot view hides
everything inside it from them.

Administrators can do everything everywhere. A **scoped moderator** moderates one forum only:
add them on the forum's People tab. Silenced members and accounts waiting for approval can read
but not post.

**Admin, Maintenance, Permission check** answers "why can or can't this member…?": it lists each
group's grant and what changed the result.

Per-forum posting rules sit on the same page: hold all posts, or only posts with links, for
approval; auto-close topics after some days; require a minimum trust level; require a tag; and
whether posts count toward members' totals.

## The panel, section by section

**Overview**
- **Dashboard**: activity, health and anything that needs attention.

**People**
- **Members**: search, open a member, change their group, trust level or identity, warn, ban,
  sign them out, send a password reset, export their data.
- **Groups**: a group's name, colour and icon, which show beside its members' names.
- **Mass message**: write to many members at once, as an email, a private message or both.
  Members can switch the email off, and every email has an unsubscribe link.
- **Trust levels**: the levels and what a member must do to reach each. Earned levels rise by
  themselves when every requirement is met (checked every ten minutes); other levels are given by
  moderators. *Preview* shows who would move before you save.
- **Profile fields**: extra questions on members' profiles; each can be asked at sign-up,
  required, and shown or kept for staff.

**Content**
- **Forums**: the tree, each forum's settings, permissions and moderators.
- **Tags, emoji, invites**: tag management, custom emoji and sign-up invitations.
- **Community**: report reasons, reactions, tag groups, the rules that add members to groups
  automatically, and announcements.
- **Chat**: channels, flagged messages and forwarding forum activity to other chat services.
- **Badges**: members earn them automatically; you can restyle any of them.
- **Watched words**: whole words (`spam*` also matches "spammer") and what happens to a post that
  uses one: block it, censor the word, replace it, hold it for approval, publish and report it
  as spam, or hold it and silence its author. Forum moderators are not affected.
- **Saved moderator actions**: a named series of steps (lock, pin, move, post a canned reply) a
  moderator runs on a topic with one click.
- **Pages**: your own pages (rules, about, FAQ), with their own layout.
- **Site text**: replace any interface text, one language at a time. This is also how you
  rewrite the emails the forum sends.
- **Permalinks**: permanent redirects for addresses the forum does not know.
- **Legal**: the terms and privacy policy. Each save publishes a new version, and members accept
  the latest one when they register and again after a change.

**Appearance**
- **Themes**, **Colour schemes**, **Widgets** (the blocks on each page), **Advertisements** and
  **Navigation** (the header links). Most pages also have an *Edit layout* button on
  the site itself, for administrators. See [Features and settings](features.md).

**System**
- **Settings**: everything else, grouped (branding, general, sign-up and sign-in, spam protection,
  posts and topics, and more).
- **Backups**: [see Operations](operations.md#backup-and-restore).
- **Plugins**, **Webhooks and apps**, **Federation**, **AI**, **Automation**, **Data Explorer**.
- **Mail log**: outgoing email, newest first; failed messages are retried up to five times.
- **Staff log**: who did what. Entries cannot be edited, even by the application.
- **Logs** and **Maintenance**: recount statistics, re-cook posts, rebuild search indexes, run
  housekeeping, clear caches, offline mode, and the settings export and import.

## Share pictures

When someone shares a link to a page, chat apps and social networks show the page's *share
picture*. Pages have none of their own, so the site can draw one for each: **Settings → Share
pictures**. Switch on *Draw share pictures* and every public page gets a 1200×630 picture with its
title, its description, the kind of page (Topic, Forum…), your logo and your colours. Nothing has
to be set up per page, and a page a plugin gives its own picture keeps it.

- **Layout**: classic (text on the left), centered, or poster (a picture of your choice behind the
  text, darkened by the *Darken* percentage).
- **Pages**: which kinds of page get a picture (home page, topics, forums, tags, profiles, custom
  pages, other public pages). The others keep the *Share image* set under Branding, if there is one.
- **Colours, text size, bottom line**, and whether to show the logo, the site name, the kind of
  page and the description. The preview under the settings shows the saved look.
- Pages that search engines are told to skip, and everything when sign-in is required, get no
  picture. Titles are drawn in Noto Sans (Latin, Cyrillic, Greek) and Noto Sans Arabic, right to
  left where the text is; characters those fonts lack (emoji, Chinese, Japanese, Korean) are left
  out.
- A picture's address carries a signature over its text, so only text a page of the site really
  had can be drawn. The signing key comes from `BF_SECRET_KEY`, which an https site must have to
  start. Without it (local development, or `BF_ALLOW_NO_SECRET_KEY=1`) the key changes at each
  restart, and chat apps that kept an old address show nothing until they fetch the page again.
- Pictures are drawn on first request, two at a time, and kept in memory; a changed look gets new
  addresses.

## Offline mode

**Maintenance, Offline mode** (the `offline` setting) shows everyone but administrators a message
(`offline_message`) instead of the site. A restore uses it while it runs.

## Moving settings between sites

**Maintenance, Export changed settings** writes the settings that differ from the defaults as
JSON, without keys or secrets. **Import settings** applies such a file. Content, members and
uploads move with a [backup](operations.md#backup-and-restore), not with this.
