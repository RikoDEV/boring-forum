//! Upload scanning.
//!
//! Images are compared with an operator-supplied list of known illegal images (CSAM hash
//! lists from NCMEC, IWF or the Tech Coalition, which are only available to members and are
//! never shipped here): PDQ perceptual hashes within Hamming distance 31, and exact SHA-256.
//!
//! A match stores nothing, answers with a generic refusal, suspends the account pending review,
//! writes an audit entry and tells the administrators. Reporting to the authorities (in the US
//! NCMEC's CyberTipline, in the EU the national authority) is a legal duty of the operator and
//! is left to the people reviewing the case; nothing is sent automatically.

use crate::state::AppState;
use bf_core::AppError;
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

/// The Hamming distance at or below which two PDQ hashes are the same image (Meta's
/// recommended threshold).
const PDQ_THRESHOLD: u32 = 31;

struct HashList {
    pdq: Vec<[u8; 32]>,
    sha256: std::collections::HashSet<String>,
}

static LIST: LazyLock<HashList> = LazyLock::new(load_list);

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    // `from_str_radix` also takes a leading `+`, so check the digits first.
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
    }
    Some(out)
}

/// `BF_IMAGE_HASHLIST`: one entry per line, `pdq:<64 hex>` (or bare 64 hex, the format the PDQ
/// lists use) or `sha256:<64 hex>`; `#` starts a comment. Read once at first use.
fn load_list() -> HashList {
    let mut list = HashList { pdq: Vec::new(), sha256: std::collections::HashSet::default() };
    let Some(path) = std::env::var("BF_IMAGE_HASHLIST").ok().filter(|p| !p.is_empty()) else { return list };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %e, path, "BF_IMAGE_HASHLIST could not be read; known-image matching is off");
            return list;
        }
    };
    let mut bad = 0;
    for line in text.lines().map(|l| l.split('#').next().unwrap_or("").trim()).filter(|l| !l.is_empty()) {
        let lower = line.to_ascii_lowercase();
        if let Some(h) = lower.strip_prefix("sha256:") {
            if parse_hex32(h).is_some() {
                list.sha256.insert(h.to_owned());
                continue;
            }
        } else if let Some(h) = parse_hex32(lower.strip_prefix("pdq:").unwrap_or(&lower)) {
            list.pdq.push(h);
            continue;
        }
        bad += 1;
    }
    tracing::info!(pdq = list.pdq.len(), sha256 = list.sha256.len(), unreadable = bad, "known-image hash list loaded");
    list
}

pub fn hashlist_loaded() -> bool {
    !LIST.pdq.is_empty() || !LIST.sha256.is_empty()
}

fn hamming(a: &[u8; 32], b: &[u8; 32]) -> u32 {
    a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum()
}

/// Refuses an image that is on the list. Run before anything is stored.
pub async fn check_image(state: &AppState, user_id: i64, bytes: &[u8]) -> Result<(), AppError> {
    if !hashlist_loaded() {
        return Ok(());
    }
    let sha: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    let exact = LIST.sha256.contains(&sha);
    let near = if exact || LIST.pdq.is_empty() {
        None
    } else {
        let owned = bytes.to_vec();
        let hash = tokio::task::spawn_blocking(move || pdq::hash_bytes(&owned)).await.ok().flatten();
        hash.and_then(|h| LIST.pdq.iter().map(|k| hamming(&h, k)).min()).filter(|d| *d <= PDQ_THRESHOLD)
    };
    if !exact && near.is_none() {
        return Ok(());
    }
    metrics::counter!("bf_known_image_matches_total").increment(1);
    quarantine_account(state, user_id, if exact { "sha256" } else { "pdq" }, near).await;
    // Deliberately generic: the uploader is not told why.
    Err(AppError::Validation("That file could not be uploaded.".into()))
}

async fn quarantine_account(state: &AppState, user_id: i64, kind: &str, distance: Option<u32>) {
    let Some(pool) = &state.pool else { return };
    let details = serde_json::json!({ "match": kind, "distance": distance });
    let steps = async {
        sqlx::query("UPDATE users SET suspended_until = 'infinity' WHERE id = $1 AND NOT EXISTS (SELECT 1 FROM users WHERE id = $1 AND primary_group_id = 4)")
            .bind(user_id)
            .execute(pool)
            .await?;
        sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(user_id).execute(pool).await?;
        bf_db::admin::audit::record(
            pool,
            bf_db::admin::audit::Entry::new("upload.known_image")
                .by(0_i64)
                .on(format!("user:{user_id}"))
                .details(details.clone()),
        )
        .await?;
        bf_db::admin::notices::add_notice(
            pool,
            "safety",
            &format!("An upload by user {user_id} matched the known-image list. The account is suspended pending review; see the audit log."),
        )
        .await
    };
    if let Err(e) = steps.await {
        tracing::error!(error = %e, user_id, "known-image quarantine incomplete");
    }
    state.sessions.invalidate_all();
}

/// PDQ perceptual hash (Meta, 2019; "The TMK+PDQF Video-Hashing Algorithm and the PDQ
/// Image Similarity Algorithm"). Follows the reference: luminance, two Jarosz box-filter passes
/// sized for a 64×64 decimation, a 16×16 slice of the DCT, and a median threshold. The
/// structure follows the Apache-2.0 `pdqhash` crate by Darwinium, written against this
/// workspace's `image` version.
mod pdq {
    use image::GenericImageView;

    const BUFFER: usize = 64;
    const DCT_OUT: usize = 16;
    const DOWNSAMPLE: u32 = 512;
    const MIN_DIM: u32 = 5;

    /// `None` for anything that does not decode as an image, or is too small to hash.
    pub(super) fn hash_bytes(bytes: &[u8]) -> Option<[u8; 32]> {
        let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(16_384);
        limits.max_image_height = Some(16_384);
        limits.max_alloc = Some(256 * 1024 * 1024);
        reader.limits(limits);
        let img = reader.decode().ok()?;
        if img.width() < MIN_DIM || img.height() < MIN_DIM {
            return None;
        }
        let img = if img.width() > DOWNSAMPLE || img.height() > DOWNSAMPLE {
            img.thumbnail_exact(DOWNSAMPLE.min(img.width()), DOWNSAMPLE.min(img.height()))
        } else {
            img
        };
        Some(hash(&img))
    }

    fn luma(img: &image::DynamicImage) -> (usize, usize, Vec<f32>) {
        let rgb = img.to_rgb8();
        let (w, h) = img.dimensions();
        let px =
            rgb.pixels().map(|p| 0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2])).collect();
        (w as usize, h as usize, px)
    }

    fn window(old: usize, new: usize) -> usize {
        old.div_ceil(2 * new)
    }

    /// One-dimensional box filter with the reference's edge handling (shrinking windows).
    fn box1d(input: &[f32], start: usize, out: &mut [f32], len: usize, stride: usize, full: usize) {
        let half = (full + 2) / 2;
        let p1 = half - 1;
        let p2 = full - half + 1;
        let (oi_off, li_off) = (p1 * stride, p2 * stride);
        let (mut sum, mut n) = (0.0f32, 0.0f32);
        let p1_end = oi_off + start;
        let mut ri = start;
        while ri < p1_end {
            sum += input[ri];
            n += 1.0;
            ri += stride;
        }
        let p2_end = full * stride + start;
        while ri < p2_end {
            sum += input[ri];
            n += 1.0;
            out[ri - oi_off] = sum / n;
            ri += stride;
        }
        let p3_end = len * stride + start;
        while ri < p3_end {
            let oi = ri - oi_off;
            sum += input[ri];
            sum -= input[oi - li_off];
            out[oi] = sum / n;
            ri += stride;
        }
        let mut oi = (len - half + 1) * stride + start;
        while oi < p3_end {
            sum -= input[oi - li_off];
            n -= 1.0;
            out[oi] = sum / n;
            oi += stride;
        }
    }

    fn jarosz(buf: &mut [f32], rows: usize, cols: usize, wr: usize, wc: usize) {
        let mut tmp = vec![0.0f32; buf.len()];
        for _ in 0..2 {
            for i in 0..rows {
                box1d(buf, i * cols, &mut tmp, cols, 1, wr);
            }
            for j in 0..cols {
                box1d(&tmp, j, buf, rows, cols, wc);
            }
        }
    }

    fn dct_matrix() -> Vec<[f32; BUFFER]> {
        let scale = (2.0f64 / BUFFER as f64).sqrt();
        (1..=DCT_OUT)
            .map(|i| {
                let mut row = [0.0f32; BUFFER];
                for (j, v) in row.iter_mut().enumerate() {
                    let arg = std::f64::consts::PI / 2.0 / BUFFER as f64 * i as f64 * (2 * j + 1) as f64;
                    *v = (scale * arg.cos()) as f32;
                }
                row
            })
            .collect()
    }

    fn median(values: &[f32]) -> f32 {
        let mut v = values.to_vec();
        v.sort_by(f32::total_cmp);
        v[(v.len() - 1) / 2]
    }

    pub(super) fn hash(img: &image::DynamicImage) -> [u8; 32] {
        let (cols, rows, mut px) = luma(img);
        jarosz(&mut px, rows, cols, window(cols, BUFFER), window(rows, BUFFER));
        let mut small = [[0.0f32; BUFFER]; BUFFER];
        for (i, row) in small.iter_mut().enumerate() {
            let ini = ((i * 2 + 1) * rows) / (BUFFER * 2);
            for (j, v) in row.iter_mut().enumerate() {
                let inj = ((j * 2 + 1) * cols) / (BUFFER * 2);
                *v = px[ini * cols + inj];
            }
        }
        let d = dct_matrix();
        let mut mid = [[0.0f32; BUFFER]; DCT_OUT];
        for i in 0..DCT_OUT {
            for j in 0..BUFFER {
                mid[i][j] = (0..BUFFER).map(|k| d[i][k] * small[k][j]).sum();
            }
        }
        let mut out = [0.0f32; DCT_OUT * DCT_OUT];
        for i in 0..DCT_OUT {
            for j in 0..DCT_OUT {
                out[i * DCT_OUT + j] = (0..BUFFER).map(|k| mid[i][k] * d[j][k]).sum();
            }
        }
        let m = median(&out);
        let mut hash = [0u8; 32];
        for i in 0..32 {
            let mut byte = 0u8;
            for j in 0..8 {
                if out[i * 8 + j] > m {
                    byte |= 1 << j;
                }
            }
            hash[31 - i] = byte;
        }
        hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdq_is_stable_under_small_changes_and_differs_across_images() {
        let gradient = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(200, 150, |x, y| {
            image::Rgb([(x % 256) as u8, (y * 2 % 256) as u8, ((x + y) % 256) as u8])
        }));
        let mut tweaked = gradient.to_rgb8();
        tweaked.put_pixel(10, 10, image::Rgb([255, 255, 255]));
        let checker = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(200, 150, |x, y| {
            if (x / 20 + y / 20) % 2 == 0 { image::Rgb([0, 0, 0]) } else { image::Rgb([255, 255, 255]) }
        }));
        let a = pdq::hash(&gradient);
        let b = pdq::hash(&image::DynamicImage::ImageRgb8(tweaked));
        let c = pdq::hash(&checker);
        assert!(hamming(&a, &b) <= PDQ_THRESHOLD, "near-duplicate");
        assert!(hamming(&a, &c) > PDQ_THRESHOLD, "different image");
    }

    #[test]
    fn pdq_survives_every_shape_of_small_and_thin_image() {
        for (w, h) in [(5, 5), (5, 512), (512, 5), (6, 7), (512, 512), (513, 9), (129, 130), (1000, 5), (5, 1000)] {
            let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
                image::Rgb([(x % 251) as u8, (y % 241) as u8, 9])
            }));
            let mut png = std::io::Cursor::new(Vec::new());
            img.write_to(&mut png, image::ImageFormat::Png).unwrap();
            assert!(pdq::hash_bytes(png.get_ref()).is_some(), "{w}x{h}");
        }
        let tiny = image::DynamicImage::ImageRgb8(image::RgbImage::new(4, 4));
        let mut png = std::io::Cursor::new(Vec::new());
        tiny.write_to(&mut png, image::ImageFormat::Png).unwrap();
        assert!(pdq::hash_bytes(png.get_ref()).is_none());
        assert!(pdq::hash_bytes(b"not an image").is_none());
    }
}

#[cfg(test)]
mod hashlist_tests;
