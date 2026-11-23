//! The one avatar-and-name row that lists of members share (the `.bf-people` list), for pages
//! built in handlers. Templates use `layout.user_link` and the same markup.

use crate::http::ui::user_link;
use bf_core::html::esc;

/// One member: avatar, linked name, and `extra` (HTML, may be empty) after the name.
pub(crate) fn row(name: &str, extra: &str) -> String {
    let avatar = if name.is_empty() || name == "deleted" {
        String::new()
    } else {
        format!(
            "<img class=\"avatar\" src=\"/avatar/{}.svg\" width=\"32\" height=\"32\" alt=\"\" loading=\"lazy\">",
            esc(name)
        )
    };
    format!("<li>{avatar}<div>{}</div>{extra}</li>", user_link(Some(name)))
}

/// A list of [`row`]s.
pub(crate) fn list(rows: &[String]) -> String {
    format!("<ul class=\"bf-people\">{}</ul>", rows.concat())
}

/// The shared empty state: an icon from the sprite and one line of (already translated) text.
pub(crate) fn empty(icon: &str, text: &str) -> String {
    format!(
        "<div class=\"bf-empty-state\"><span class=\"bf-empty-icon\" aria-hidden=\"true\"><svg class=\"bf-icon\" aria-hidden=\"true\"><use href=\"{}#{icon}\"></use></svg></span><p>{}</p></div>",
        bf_web::assets::ICONS.url(),
        esc(text)
    )
}
