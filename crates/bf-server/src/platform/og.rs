//! Share pictures: `GET /og.png`, the 1200×630 picture a link to a page shows on social networks
//! and in chats (the look is `bf_core::og::Style`, set in the admin panel under Share pictures).
//!
//! The address carries the text to draw and a signature over it (`bf_core::og::picture_path`), so
//! only text a page of this site really had is ever drawn. A picture is built as SVG, with the
//! text wrapped by measuring it in the same fonts, and rasterised by resvg. The fonts are inside
//! the program (Noto Sans and Noto Sans Arabic, SIL Open Font License), so the picture does not
//! depend on what the server has installed, and Arabic is shaped and written right to left.
//! Finished pictures are kept in memory; the drawing runs on the blocking pool, two at a time.

use crate::state::AppState;
use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use base64::Engine as _;
use bf_core::AppError;
use bf_core::og::Style;
use resvg::{tiny_skia, usvg};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

pub fn router() -> Router<AppState> {
    Router::new().route("/og.png", get(picture_png)).route("/og.jpg", get(picture_jpg)).route("/api/v1/admin/og-preview", get(preview))
}

const W: f32 = 1200.0;
const H: f32 = 630.0;
const MARGIN: f32 = 72.0;
const FAMILY: &str = "Noto Sans, Noto Sans Arabic";

static FONTS: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
    let mut db = usvg::fontdb::Database::new();
    for data in [
        &include_bytes!("../../assets/fonts/NotoSans-Regular.ttf")[..],
        &include_bytes!("../../assets/fonts/NotoSans-Bold.ttf")[..],
        &include_bytes!("../../assets/fonts/NotoSansArabic-Regular.ttf")[..],
        &include_bytes!("../../assets/fonts/NotoSansArabic-Bold.ttf")[..],
    ] {
        db.load_font_data(data.to_vec());
    }
    db.set_sans_serif_family("Noto Sans");
    Arc::new(db)
});

fn options() -> usvg::Options<'static> {
    usvg::Options { fontdb: FONTS.clone(), font_family: "Noto Sans".into(), ..usvg::Options::default() }
}

/// Pictures drawn at one time: each takes tens of milliseconds and a few megabytes.
static DRAWING: LazyLock<Arc<tokio::sync::Semaphore>> = LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));

/// Finished pictures by address and look, about 30 MB at most.
static DONE: LazyLock<moka::sync::Cache<String, axum::body::Bytes>> = LazyLock::new(|| {
    moka::sync::Cache::builder()
        .weigher(|_, v: &axum::body::Bytes| u32::try_from(v.len()).unwrap_or(u32::MAX))
        .max_capacity(30 * 1024 * 1024)
        .time_to_live(Duration::from_secs(6 * 3600))
        .build()
});

/// A picture from the site's uploads, made ready to place: re-encoded at the size it is drawn.
struct Asset {
    data_url: String,
    width: f32,
    height: f32,
}

static ASSETS: LazyLock<moka::sync::Cache<String, Option<Arc<Asset>>>> =
    LazyLock::new(|| moka::sync::Cache::builder().max_capacity(16).time_to_live(Duration::from_secs(600)).build());

/// The largest uploaded file read for a logo or background.
const ASSET_BYTES: u64 = 12 * 1024 * 1024;

/// Reads, checks and shrinks an uploaded picture. `cover` fills the whole picture; otherwise it
/// fits inside `bounds` (a logo).
fn prepare(bytes: &[u8], cover: bool, bounds: (u32, u32)) -> Option<Asset> {
    use image::{ImageReader, Limits, imageops::FilterType};
    if crate::forum::uploads::too_many_pixels(bytes) {
        return None;
    }
    let mut reader = ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(crate::forum::uploads::MAX_DECODE_SIDE);
    limits.max_image_height = Some(crate::forum::uploads::MAX_DECODE_SIDE);
    limits.max_alloc = Some(192 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let mut out = std::io::Cursor::new(Vec::new());
    let (mime, w, h) = if cover {
        let img = img.resize_to_fill(bounds.0, bounds.1, FilterType::Triangle).to_rgb8();
        img.write_to(&mut out, image::ImageFormat::Jpeg).ok()?;
        ("image/jpeg", img.width(), img.height())
    } else {
        let img = img.resize(bounds.0, bounds.1, FilterType::Lanczos3).to_rgba8();
        img.write_to(&mut out, image::ImageFormat::Png).ok()?;
        ("image/png", img.width(), img.height())
    };
    let data_url = format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(out.into_inner()));
    Some(Asset { data_url, width: w as f32, height: h as f32 })
}

/// An uploaded picture by its path in the settings (`/uploads/<key>`), made ready; `None` when
/// the setting is empty or the file cannot be used. A failure is remembered for ten minutes too.
async fn asset(path: &str, cover: bool) -> Option<Arc<Asset>> {
    let key = path.strip_prefix("/uploads/")?;
    if !crate::forum::uploads::valid_key(key) {
        return None;
    }
    let cache_key = format!("{cover}:{key}");
    if let Some(hit) = ASSETS.get(&cache_key) {
        return hit;
    }
    let store = crate::forum::uploads::byte_store();
    // Files are kept under the first two characters of their key.
    let stored = format!("{}/{key}", &key[..2]);
    let made = match store.size(&stored).await {
        Some(n) if n <= ASSET_BYTES => match store.get(&stored).await {
            Some(bytes) => {
                let bounds = if cover { (W as u32, H as u32) } else { (480, 112) };
                tokio::task::spawn_blocking(move || prepare(&bytes, cover, bounds)).await.ok().flatten().map(Arc::new)
            }
            None => None,
        },
        _ => None,
    };
    ASSETS.insert(cache_key, made.clone());
    made
}

fn esc(s: &str) -> String {
    bf_core::html::esc(s)
}

/// Text as it is drawn: whitespace folded; control characters, joiners and emoji dropped (the
/// fonts have no emoji, and a box in the middle of a title looks worse than a gap).
fn clean(s: &str) -> String {
    let kept: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .filter(|c| {
            let n = u32::from(*c);
            !matches!(n, 0x200B..=0x200F | 0x202A..=0x202E | 0x2066..=0x2069 | 0xFE00..=0xFE0F | 0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF | 0xE000..=0xF8FF)
        })
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether text reads right to left: its first letter with a direction is Arabic or Hebrew.
fn is_rtl(s: &str) -> bool {
    s.chars().find(|c| c.is_alphabetic()).is_some_and(|c| matches!(u32::from(c), 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF))
}

/// How wide the text is when drawn in the fonts, by laying it out as a one-line picture.
fn measure(text: &str, size: f32, bold: bool) -> f32 {
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"6000\" height=\"400\"><text x=\"0\" y=\"200\" font-family=\"{FAMILY}\" font-size=\"{size}\" font-weight=\"{}\">{}</text></svg>",
        if bold { 700 } else { 400 },
        esc(text)
    );
    let estimate = text.chars().count() as f32 * size * 0.6;
    usvg::Tree::from_str(&svg, &options()).ok().and_then(|t| t.root().abs_bounding_box()).map_or(estimate, |r| r.width())
}

/// Breaks text into lines no wider than `width`, at most `max` of them; the last is cut with an
/// ellipsis when the text does not fit. The second value says whether it was cut.
fn wrap(text: &str, size: f32, bold: bool, width: f32, max: usize) -> (Vec<String>, bool) {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut words: std::collections::VecDeque<String> = text.split(' ').map(str::to_owned).collect();
    while let Some(word) = words.pop_front() {
        let candidate = if line.is_empty() { word.clone() } else { format!("{line} {word}") };
        if measure(&candidate, size, bold) <= width {
            line = candidate;
            continue;
        }
        if line.is_empty() {
            // One word wider than the picture: split it where it fits.
            let mut head = String::new();
            let mut rest: Vec<char> = word.chars().collect();
            while !rest.is_empty() {
                let mut probe = head.clone();
                probe.push(rest[0]);
                if !head.is_empty() && measure(&probe, size, bold) > width {
                    break;
                }
                head = probe;
                rest.remove(0);
            }
            if !rest.is_empty() {
                words.push_front(rest.into_iter().collect());
            }
            lines.push(head);
        } else {
            lines.push(std::mem::take(&mut line));
            words.push_front(word);
        }
        if lines.len() == max {
            break;
        }
    }
    let cut = lines.len() == max && (!line.is_empty() || !words.is_empty());
    if lines.len() < max && !line.is_empty() {
        lines.push(line);
    }
    if cut && let Some(last) = lines.last_mut() {
        let mut s: String = last.clone();
        while !s.is_empty() && measure(&format!("{s}…"), size, bold) > width {
            s.pop();
        }
        *last = format!("{}…", s.trim_end());
    }
    (lines, cut)
}

/// Black or white, whichever reads on this `#rrggbb`.
fn on(colour: &str) -> &'static str {
    if luminance(colour) > 0.55 { "#111111" } else { "#ffffff" }
}

fn luminance(colour: &str) -> f32 {
    let part = |i: usize| f32::from(u8::from_str_radix(colour.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0)) / 255.0;
    0.2126 * part(1) + 0.7152 * part(3) + 0.0722 * part(5)
}

/// What is drawn: the text of the page, the site's identity and its pictures.
struct Card {
    style: Style,
    kind_label: String,
    title: String,
    description: String,
    site_name: String,
    footer: String,
    logo: Option<Arc<Asset>>,
    background: Option<Arc<Asset>>,
}

/// The picture as SVG.
fn svg(c: &Card) -> String {
    let s = &c.style;
    let title = clean(&c.title);
    let description = if s.description { clean(&c.description) } else { String::new() };
    let rtl = is_rtl(&title) || (title.is_empty() && is_rtl(&c.site_name));
    let poster = s.template == "poster" && c.background.is_some();
    let centered = s.template == "centered";
    let (text, accent) = (if poster { "#ffffff" } else { s.text.as_str() }, s.accent.as_str());
    let scale = f32::from(s.font_scale) / 100.0;
    let dark_bg = poster || luminance(&s.background) < 0.5;

    // Where text starts and which way it runs.
    let (x, anchor) = if centered { (W / 2.0, "middle") } else if rtl { (W - MARGIN, "end") } else { (MARGIN, "start") };
    let dir = if rtl { " direction=\"rtl\" unicode-bidi=\"embed\"" } else { "" };
    let width = W - 2.0 * MARGIN - if centered { 0.0 } else { 16.0 };

    let mut o = String::with_capacity(8 * 1024);
    let _ = write!(o, "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{W}\" height=\"{H}\" viewBox=\"0 0 {W} {H}\">");
    // Background.
    if s.background_end.is_empty() {
        let _ = write!(o, "<rect width=\"{W}\" height=\"{H}\" fill=\"{}\"/>", s.background);
    } else {
        let _ = write!(
            o,
            "<defs><linearGradient id=\"g\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"><stop offset=\"0\" stop-color=\"{}\"/><stop offset=\"1\" stop-color=\"{}\"/></linearGradient></defs><rect width=\"{W}\" height=\"{H}\" fill=\"url(#g)\"/>",
            s.background, s.background_end
        );
    }
    if let Some(bg) = c.background.as_ref().filter(|_| poster) {
        let _ = write!(o, "<image x=\"0\" y=\"0\" width=\"{W}\" height=\"{H}\" preserveAspectRatio=\"xMidYMid slice\" xlink:href=\"{}\"/>", bg.data_url);
        let _ = write!(o, "<rect width=\"{W}\" height=\"{H}\" fill=\"#000000\" fill-opacity=\"{:.2}\"/>", f32::from(s.overlay) / 100.0);
        // Text sits low on a poster: a darker foot keeps it readable on any picture.
        let _ = write!(o, "<defs><linearGradient id=\"f\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\"><stop offset=\"0.2\" stop-color=\"#000\" stop-opacity=\"0\"/><stop offset=\"1\" stop-color=\"#000\" stop-opacity=\"0.8\"/></linearGradient></defs><rect width=\"{W}\" height=\"{H}\" fill=\"url(#f)\"/>");
    }
    // The stripe: on the reading side.
    if !centered {
        let bar_x = if rtl { W - 16.0 } else { 0.0 };
        let _ = write!(o, "<rect x=\"{bar_x}\" y=\"0\" width=\"16\" height=\"{H}\" fill=\"{accent}\"/>");
    } else {
        let _ = write!(o, "<rect x=\"{}\" y=\"0\" width=\"{}\" height=\"10\" fill=\"{accent}\"/>", 0, W);
    }

    // The top row: logo and name.
    let mut row_h = 0.0f32;
    let logo = c.logo.as_ref().filter(|_| s.logo);
    let name = clean(&c.site_name);
    let show_name = s.site_name && !name.is_empty();
    let name_size = 30.0 * scale.min(1.15);
    let (mut logo_w, logo_h) = (0.0f32, 56.0f32);
    if let Some(l) = logo {
        logo_w = (l.width / l.height * logo_h).min(300.0);
        row_h = logo_h;
    }
    let name_w = if show_name { measure(&name, name_size, true) } else { 0.0 };
    if show_name {
        row_h = row_h.max(name_size);
    }
    let gap = if logo.is_some() && show_name { 18.0 } else { 0.0 };
    let total = logo_w + gap + name_w;
    let row_y = 60.0;
    let mut cursor = if centered { (W - total) / 2.0 } else if rtl { W - MARGIN - total } else { MARGIN };
    if let Some(l) = logo {
        let _ = write!(o, "<image x=\"{cursor:.1}\" y=\"{row_y}\" width=\"{logo_w:.1}\" height=\"{logo_h}\" preserveAspectRatio=\"xMidYMid meet\" xlink:href=\"{}\"/>", l.data_url);
        cursor += logo_w + gap;
    }
    if show_name {
        let base = row_y + (row_h.max(name_size) + name_size * 0.7) / 2.0 - if logo.is_some() { 4.0 } else { 0.0 };
        let _ = write!(o, "<text x=\"{cursor:.1}\" y=\"{base:.1}\" font-family=\"{FAMILY}\" font-size=\"{name_size:.1}\" font-weight=\"700\" fill=\"{text}\" fill-opacity=\"0.92\"{dir}>{}</text>", esc(&name));
    }

    // The title block: the largest size at which it fits.
    let (title, description) = if title.is_empty() && description.is_empty() { (name.clone(), String::new()) } else { (title, description) };
    let max_title_lines = if description.is_empty() { 4 } else { 3 };
    let label = clean(&c.kind_label);
    let has_chip = s.kind_label && !label.is_empty();
    let (chip_h, chip_gap) = (44.0f32, 28.0f32);
    // The space between the top row and the foot, less the label.
    let top_limit = 140.0f32;
    let bottom_limit = H - 84.0;
    let room = bottom_limit - top_limit - if has_chip { chip_h + chip_gap } else { 0.0 };
    // The largest size at which the title (and what is under it) fits in the room.
    let mut chosen = None;
    for step in [1.0f32, 0.88, 0.78, 0.68, 0.6, 0.52] {
        let size = 72.0 * scale * step;
        let lines = wrap(&title, size, true, width, max_title_lines);
        let desc_size = (32.0 * scale).min(size * 0.55).max(20.0);
        let desc_lines = if description.is_empty() { (Vec::new(), false) } else { wrap(&description, desc_size, false, width, 2) };
        let gap_td = if desc_lines.0.is_empty() { 0.0 } else { 24.0 };
        let block_h = lines.0.len() as f32 * size * 1.18 + gap_td + desc_lines.0.len() as f32 * desc_size * 1.35;
        let fits = !lines.1 && block_h <= room;
        chosen = Some((size, lines, desc_size, desc_lines, gap_td, block_h));
        if fits {
            break;
        }
    }
    let (size, lines, desc_size, desc_lines, gap_td, block_h) = chosen.unwrap_or_default();
    let line_h = size * 1.18;
    let full = block_h + if has_chip { chip_h + chip_gap } else { 0.0 };
    let top = if s.template == "poster" { (bottom_limit - full).max(top_limit) } else { (top_limit + (bottom_limit - top_limit - full) / 2.0).max(top_limit) };
    let mut y = top;
    if has_chip {
        let chip_size = 24.0;
        let chip_w = measure(&label, chip_size, true) + 40.0;
        let chip_x = if centered { (W - chip_w) / 2.0 } else if rtl { W - MARGIN - chip_w } else { MARGIN };
        let _ = write!(
            o,
            "<rect x=\"{chip_x:.1}\" y=\"{y:.1}\" width=\"{chip_w:.1}\" height=\"{chip_h}\" rx=\"{}\" fill=\"{accent}\"/><text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-family=\"{FAMILY}\" font-size=\"{chip_size}\" font-weight=\"700\" fill=\"{}\"{dir}>{}</text>",
            chip_h / 2.0,
            chip_x + chip_w / 2.0,
            y + chip_h / 2.0 + chip_size * 0.35,
            on(accent),
            esc(&label)
        );
        y += chip_h + chip_gap;
    }
    for line in &lines.0 {
        y += line_h;
        let _ = write!(o, "<text x=\"{x}\" y=\"{:.1}\" text-anchor=\"{anchor}\" font-family=\"{FAMILY}\" font-size=\"{size:.1}\" font-weight=\"700\" fill=\"{text}\"{dir}>{}</text>", y - line_h * 0.22, esc(line));
    }
    if !desc_lines.0.is_empty() {
        y += gap_td - line_h * 0.22 + 4.0;
        for line in &desc_lines.0 {
            y += desc_size * 1.35;
            let _ = write!(o, "<text x=\"{x}\" y=\"{y:.1}\" text-anchor=\"{anchor}\" font-family=\"{FAMILY}\" font-size=\"{desc_size:.1}\" fill=\"{text}\" fill-opacity=\"0.78\"{dir}>{}</text>", esc(line));
        }
    }
    // The foot: the address, or the text the site chose.
    let footer = clean(&c.footer);
    if !footer.is_empty() {
        let fsize = 26.0 * scale.min(1.1);
        let _ = write!(
            o,
            "<text x=\"{x}\" y=\"{:.1}\" text-anchor=\"{anchor}\" font-family=\"{FAMILY}\" font-size=\"{fsize:.1}\" fill=\"{text}\" fill-opacity=\"{}\"{dir}>{}</text>",
            H - 52.0,
            if dark_bg { "0.65" } else { "0.6" },
            esc(&footer)
        );
    }
    o.push_str("</svg>");
    o
}

/// Draws the picture, as the file type the style asks for. CPU work: call it from the blocking pool.
fn draw(c: &Card) -> Option<Vec<u8>> {
    let tree = usvg::Tree::from_str(&svg(c), &options()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(W as u32, H as u32)?;
    resvg::render(&tree, tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    if c.style.extension() == "png" {
        return pixmap.encode_png().ok();
    }
    // The background is opaque, so the pixels are final as they are: only the alpha byte goes.
    let rgb: Vec<u8> = pixmap.data().chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 86)
        .encode(&rgb, pixmap.width(), pixmap.height(), image::ExtendedColorType::Rgb8)
        .ok()?;
    Some(out.into_inner())
}

/// The words for each kind of page, in the site's language.
fn kind_label(state: &AppState, kind: &str) -> String {
    let t = bf_core::i18n::T::new(bf_core::i18n::site_language(state.settings().str("default_locale")));
    match kind {
        "topic" => t.msg("Topic"),
        "forum" => t.msg("Forum"),
        "tag" => t.msg("Tag"),
        "profile" => t.msg("Profile"),
        "page" => t.msg("Page"),
        _ => String::new(),
    }
}

/// The logo for this background: the dark-scheme logo on a dark background when there is one.
fn logo_path(state: &AppState, style: &Style) -> String {
    let s = state.settings();
    let dark = style.template == "poster" || luminance(&style.background) < 0.5;
    let (main, alt) = (s.str("logo_url"), s.str("logo_dark_url"));
    if dark && !alt.is_empty() { alt.to_owned() } else if !main.is_empty() { main.to_owned() } else { alt.to_owned() }
}

async fn build(state: &AppState, style: &Style, kind: &str, title: String, description: String) -> Result<axum::body::Bytes, AppError> {
    let s = state.settings();
    let wants_background = style.template == "poster" && !style.background_image.is_empty();
    let card = Card {
        style: style.clone(),
        kind_label: kind_label(state, kind),
        title,
        description,
        site_name: s.str("site_name").to_owned(),
        footer: if style.footer.is_empty() {
            state.config.base_url.split_once("://").map_or_else(String::new, |(_, h)| h.trim_end_matches('/').to_owned())
        } else {
            style.footer.clone()
        },
        logo: if style.logo { asset(&logo_path(state, style), false).await } else { None },
        background: if wants_background { asset(&style.background_image, true).await } else { None },
    };
    let permit = tokio::time::timeout(Duration::from_secs(5), DRAWING.clone().acquire_owned())
        .await
        .map_err(|_| AppError::RateLimited { retry_after_secs: 5 })?
        .map_err(|_| AppError::NotFound)?;
    let png = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        draw(&card)
    })
    .await
    .ok()
    .flatten()
    .ok_or(AppError::NotFound)?;
    Ok(axum::body::Bytes::from(png))
}

fn image_response(bytes: axum::body::Bytes, jpeg: bool, cache: &'static str) -> Response {
    let mut res = (StatusCode::OK, bytes).into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(if jpeg { "image/jpeg" } else { "image/png" }));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    h.insert("content-security-policy", HeaderValue::from_static("default-src 'none'; sandbox"));
    res
}

async fn picture_png(state: State<AppState>, q: Query<HashMap<String, String>>) -> Result<Response, AppError> {
    picture(state, q, "png").await
}

async fn picture_jpg(state: State<AppState>, q: Query<HashMap<String, String>>) -> Result<Response, AppError> {
    picture(state, q, "jpg").await
}

/// `/og.png` or `/og.jpg` `?k=<kind>&t=<title>&d=<description>&v=<look>&s=<signature>`. The file
/// type must be the one the current look draws, so the address says what it returns.
async fn picture(State(state): State<AppState>, Query(q): Query<HashMap<String, String>>, ext: &str) -> Result<Response, AppError> {
    let get = |k: &str| q.get(k).map_or("", String::as_str);
    let (kind, title, description) = (get("k"), get("t"), get("d"));
    if !bf_core::og::verify(kind, title, description, get("s")) {
        return Err(AppError::NotFound);
    }
    let style = Style::from_settings(&state.settings());
    if !style.covers(kind) || style.extension() != ext {
        return Err(AppError::NotFound);
    }
    // The look is part of the key: a changed look draws again, though the address is the same.
    let key = format!("{}|{kind}|{title}|{description}", style.version());
    if let Some(done) = DONE.get(&key) {
        return Ok(image_response(done, ext == "jpg", "public, max-age=86400"));
    }
    let drawn = build(&state, &style, kind, title.to_owned(), description.to_owned()).await?;
    DONE.insert(key, drawn.clone());
    Ok(image_response(drawn, ext == "jpg", "public, max-age=86400"))
}

/// A sample in the saved look, for the admin panel; works while the pictures are switched off.
async fn preview(State(state): State<AppState>, v: crate::http::viewer::Viewer, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Result<Response, AppError> {
    crate::admin::api::admin(&state, &v, &headers, false)?;
    let mut style = Style::from_settings(&state.settings());
    style.enabled = true;
    let kind = q.get("kind").map_or("topic", String::as_str);
    let kind = if bf_core::og::KINDS.contains(&kind) { kind } else { "topic" };
    let t = v.t();
    let (title, description) = bf_core::og::text_of(
        &t.msg("How do I share a link that looks good everywhere?"),
        &t.msg("A short summary of the page appears here, under the title, the way people see it in a chat or a feed."),
    );
    let drawn = build(&state, &style, kind, title, description).await?;
    Ok(image_response(drawn, style.extension() == "jpg", "no-store"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(style: Style, title: &str, description: &str) -> Card {
        Card {
            style,
            kind_label: "Topic".into(),
            title: title.into(),
            description: description.into(),
            site_name: "Boring Forum".into(),
            footer: "forum.example".into(),
            logo: None,
            background: None,
        }
    }

    fn style(template: &str) -> Style {
        let mut s = Style::from_settings(&bf_core::settings::Settings::default());
        s.enabled = true;
        s.template = template.into();
        s
    }

    #[test]
    fn text_is_cleaned_and_direction_found() {
        assert_eq!(clean("  a \u{1F600}\u{200D} b\n\tc\u{FE0F} "), "a b c");
        assert!(is_rtl("مرحبا بالعالم") && is_rtl("123 مرحبا") && !is_rtl("Hello مرحبا") && !is_rtl("123"));
    }

    #[test]
    fn long_text_is_wrapped_and_cut() {
        let (lines, cut) = wrap("alpha beta gamma delta epsilon zeta eta theta iota kappa", 60.0, true, 500.0, 2);
        assert_eq!(lines.len(), 2);
        assert!(cut && lines[1].ends_with('…'), "{lines:?}");
        let (lines, cut) = wrap(&"w".repeat(30), 60.0, true, 500.0, 5);
        assert!(lines.len() > 1 && !cut, "one long word breaks: {lines:?}");
        assert!(lines.iter().all(|l| measure(l, 60.0, true) <= 500.5));
    }

    /// Writes samples to `target/og-samples` for a look at them, and checks each is a PNG.
    #[test]
    fn pictures_are_drawn() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/og-samples");
        std::fs::create_dir_all(&dir).unwrap();
        let samples = [
            ("classic", "How do I set up a share picture that looks good everywhere on the internet?", "A short summary of the page appears here, under the title, the way people see it in a chat or a feed."),
            ("centered", "Zażółć gęślą jaźń: jak ustawić obrazek do udostępniania?", "Krótki opis strony pojawia się pod tytułem."),
            ("poster", "كيف أضبط صورة المشاركة بحيث تظهر بشكل جيد في كل مكان على الإنترنت؟", "ملخص قصير للصفحة يظهر هنا تحت العنوان."),
            ("classic", "Short", ""),
        ];
        for (i, (template, title, description)) in samples.iter().enumerate() {
            let mut s = style(template);
            if i == 1 {
                s.background_end = "#1e3a5f".into();
            }
            let png = draw(&card(s, title, description)).expect("drawn");
            assert!(png.starts_with(b"\x89PNG"));
            std::fs::write(dir.join(format!("{i}-{template}.png")), png).unwrap();
        }
    }
}
