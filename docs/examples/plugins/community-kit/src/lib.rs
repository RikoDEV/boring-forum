//! Community kit: the plugin SDK end to end. Build with `scripts/plugin.sh pack examples/plugins/community-kit`.
//!
//! - `post.before_create`: masks or refuses words from the plugin's settings.
//! - `panel.render` (profile): a page on every profile, in the viewer's language.
//! - `widget.render`: a block for the page builder, with settings an administrator fills in.
//! - `slot.render`: a line under every post, a profile header and a member card with badges and
//!   profile fields, and a note above a forum: all per viewer.
//! - `action.run`: the composer button inserts a shrug; the post button opens a dialog.
//! - `cook.rule`: `!!text!!` in a post becomes bold capitals.
//! - `admin.form.save`: refuses a banned-words list that is too long.

use bf_plugin_api::hooks::{ActionRun, AdminFormSave, CookRule, PanelRender, PostBeforeCreate, SlotRender, WidgetRender};
use bf_plugin_api::{Reply, Request, esc, export, host, t};

fn on_hook(req: Request) -> Reply {
    match req.hook.as_str() {
        "post.before_create" => filter(&req),
        "panel.render" => panel(&req),
        "widget.render" => widget(&req),
        "slot.render" => slot(&req),
        "action.run" => action(&req),
        "cook.rule" => cook_rule(&req),
        "admin.form.save" => form_save(&req),
        _ => Reply::none(),
    }
}

/// The banned words, lower case. An empty setting bans nothing.
fn banned(req: &Request) -> Vec<String> {
    req.setting_str("banned_words", "")
        .split(',')
        .map(|w| w.trim().to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Replaces banned words with asterisks; returns the new text and how many were hit.
fn mask(text: &str, words: &[String]) -> (String, usize) {
    let mut hits = 0;
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let mut flush = |word: &mut String, out: &mut String| {
        if words.contains(&word.to_lowercase()) {
            hits += 1;
            out.push_str(&"*".repeat(word.chars().count()));
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in text.chars() {
        if c.is_alphanumeric() {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    (out, hits)
}

fn count() -> u64 {
    host::kv_get("filtered").ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(0)
}

fn filter(req: &Request) -> Reply {
    let Ok(post) = req.payload::<PostBeforeCreate>() else { return Reply::none() };
    let (masked, hits) = mask(&post.raw, &banned(req));
    if hits == 0 {
        return Reply::none();
    }
    // The counter is a nicety: if the store is not approved or full, the filter still works.
    let _ = host::kv_set("filtered", &(count() + hits as u64).to_string());
    if req.setting_str("mode", "mask") == "reject" {
        // Filters answer in English; the forum translates its own messages, not a plugin's.
        return Reply::reject("Please remove the words that are not allowed here.");
    }
    Reply::data(bf_plugin_api::json!({ "raw": masked }))
}

fn panel(req: &Request) -> Reply {
    let Ok(p) = req.payload::<PanelRender>() else { return Reply::none() };
    let line = t(&p.strings, "panel.filtered").replace("{n}", &count().to_string());
    let who = p.profile_id.map_or(String::new(), |id| {
        format!("<dl><dt>{}</dt><dd>{id}</dd></dl>", esc(t(&p.strings, "panel.member")))
    });
    Reply::html(format!("<div class=\"kit-panel\"><p>{}</p>{who}</div>", esc(&line)))
}

fn widget(req: &Request) -> Reply {
    let Ok(w) = req.payload::<WidgetRender>() else { return Reply::none() };
    let heading = w.settings.get("heading").and_then(|h| h.as_str()).unwrap_or("Filtered words");
    let number = if w.settings.get("bold").and_then(|b| b.as_bool()).unwrap_or(true) {
        format!("<strong>{}</strong>", count())
    } else {
        count().to_string()
    };
    Reply::html(format!("<p class=\"small\">{}: {number}</p>", esc(heading)))
}

fn slot(req: &Request) -> Reply {
    let Ok(s) = req.payload::<SlotRender>() else { return Reply::none() };
    match s.slot.as_str() {
        "post_footer" if s.viewer_id.is_some() && req.setting_bool("show_footer", true) => {
            Reply::html(format!("<p class=\"kit-footer small muted\">{}</p>", esc(t(&s.strings, "slot.footer"))))
        }
        // The profile header: the member's own latest badges and public profile fields, which
        // include the ones plugins declare with [[profile_fields]].
        "profile_header" => match s.subject.filter(|u| u.kind == "user") {
            Some(user) if !user.fields.is_empty() || !user.badges.is_empty() => {
                let fields: String = user
                    .fields
                    .iter()
                    .map(|f| format!("<li><strong>{}</strong> {}</li>", esc(&f.label), esc(&f.value)))
                    .collect();
                let badges: String = user.badges.iter().map(|b| format!("<li>{}</li>", esc(&b.name))).collect();
                Reply::html(format!(
                    "<div class=\"kit-header\"><h2 class=\"small\">{}</h2><ul class=\"kit-chips\">{badges}{fields}</ul></div>",
                    esc(t(&s.strings, "header.about"))
                ))
            }
            _ => Reply::none(),
        },
        // The body of the hover card over a member's name.
        "user_card" => match s.subject {
            Some(user) if !user.badges.is_empty() => {
                let line = t(&s.strings, "card.badges").replace("{n}", &user.badges.len().to_string());
                Reply::html(format!("<p class=\"small muted\">{}</p>", esc(&line)))
            }
            _ => Reply::none(),
        },
        "category_header" => Reply::html(format!("<p class=\"kit-note small muted\">{}</p>", esc(t(&s.strings, "forum.note")))),
        _ => Reply::none(),
    }
}

fn action(req: &Request) -> Reply {
    let Ok(a) = req.payload::<ActionRun>() else { return Reply::none() };
    match a.key.as_str() {
        "shrug" => Reply::insert(r"¯\_(ツ)_/¯"),
        "length" => {
            let line = t(&a.strings, "length.text").replace("{n}", &a.post_id.unwrap_or_default().to_string());
            Reply::html(format!("<p>{}</p>", esc(&line)))
        }
        _ => Reply::none(),
    }
}

/// `!!text!!` in a post: the host sends the text, and what comes back is sanitized like a post.
fn cook_rule(req: &Request) -> Reply {
    let Ok(rule) = req.payload::<CookRule>() else { return Reply::none() };
    if rule.rule != "shout" {
        return Reply::none();
    }
    Reply::html(format!("<strong class=\"plug-kit-shout\">{}</strong>", esc(&rule.text.to_uppercase())))
}

/// The admin page and the settings form: no more than the limit of banned words.
fn form_save(req: &Request) -> Reply {
    let Ok(save) = req.payload::<AdminFormSave>() else { return Reply::none() };
    let words = save.values.get("banned_words").and_then(|w| w.as_str()).map_or(0, |w| w.split(',').count());
    let limit = save.values.get("max_words").and_then(|m| m.as_i64()).unwrap_or(req.setting_i64("max_words", 20));
    if words as i64 > limit {
        return Reply::reject("There are more banned words than the limit allows.");
    }
    Reply::none()
}

export!(on_hook);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_words_are_masked_in_any_case() {
        let words = vec!["spam".to_owned()];
        assert_eq!(mask("No Spam, spammer!", &words), ("No ****, spammer!".to_owned(), 1));
        assert_eq!(mask("clean", &words).1, 0);
    }

    #[test]
    fn the_cook_rule_shouts() {
        let req: Request = serde_json::from_str(r#"{"hook":"cook.rule","data":{"rule":"shout","text":"hi <b>"}}"#).unwrap();
        assert_eq!(cook_rule(&req).0["html"], "<strong class=\"plug-kit-shout\">HI &lt;B&gt;</strong>");
    }
}
