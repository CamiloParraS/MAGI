//! Comparing two runs: which screens changed, by how much, and a picture of
//! where. Pure functions over images and names; `mod.rs` does the files.

use image::{Rgba, RgbaImage};

#[cfg(test)]
mod tests;

/// A channel must move more than this to count: below it is antialiasing
/// noise between two draws of the same frame.
const THRESHOLD: u8 = 8;
/// Changed pixels in the diff picture.
pub const MARK: Rgba<u8> = Rgba([255, 0, 90, 255]);

pub struct Comparison {
    pub changed: u64,
    pub total: u64,
    /// The screen is a different size: every pixel counts as changed.
    pub resized: bool,
    /// The new picture, faded, with every changed pixel in [`MARK`].
    pub image: RgbaImage,
}

pub fn compare(before: &RgbaImage, after: &RgbaImage) -> Comparison {
    let (w, h) = after.dimensions();
    let total = u64::from(w) * u64::from(h);
    let faded = |p: &Rgba<u8>| {
        let grey = (u16::from(p[0]) + u16::from(p[1]) + u16::from(p[2])) / 3;
        let v = (200 + grey / 5) as u8;
        Rgba([v, v, v, 255])
    };
    if before.dimensions() != after.dimensions() {
        let image = RgbaImage::from_fn(w, h, |x, y| faded(after.get_pixel(x, y)));
        return Comparison {
            changed: total,
            total,
            resized: true,
            image,
        };
    }
    let mut changed = 0;
    let image = RgbaImage::from_fn(w, h, |x, y| {
        let (a, b) = (before.get_pixel(x, y), after.get_pixel(x, y));
        if a.0.iter().zip(b.0).any(|(a, b)| a.abs_diff(b) > THRESHOLD) {
            changed += 1;
            MARK
        } else {
            faded(b)
        }
    });
    Comparison {
        changed,
        total,
        resized: false,
        image,
    }
}

/// One picture from several draws of the same screen: each pixel's
/// lightest value. A blinking caret is the only thing that differs between
/// them, so whichever phase each draw caught, the result is the same.
pub fn steady(frames: &[RgbaImage]) -> RgbaImage {
    let mut out = frames[0].clone();
    for frame in &frames[1..] {
        if frame.dimensions() != out.dimensions() {
            continue;
        }
        for (o, f) in out.pixels_mut().zip(frame.pixels()) {
            for (o, f) in o.0.iter_mut().zip(f.0) {
                *o = (*o).max(f);
            }
        }
    }
    out
}

/// One screen in the report.
pub struct Entry {
    pub name: String,
    pub state: State,
}

pub enum State {
    Changed {
        changed: u64,
        total: u64,
        resized: bool,
    },
    New,
    Gone,
    Same,
}

impl Entry {
    pub fn changed(name: &str, changed: u64, total: u64, resized: bool) -> Self {
        let state = State::Changed {
            changed,
            total,
            resized,
        };
        Self {
            name: name.into(),
            state,
        }
    }
    pub fn new_shot(name: &str) -> Self {
        Self {
            name: name.into(),
            state: State::New,
        }
    }
    pub fn gone(name: &str) -> Self {
        Self {
            name: name.into(),
            state: State::Gone,
        }
    }
    pub fn same(name: &str) -> Self {
        Self {
            name: name.into(),
            state: State::Same,
        }
    }

    /// Changed screens first, the most changed on top; then new, gone and
    /// unchanged ones, each by name.
    fn rank(&self) -> (u8, std::cmp::Reverse<u64>) {
        match self.state {
            State::Changed { changed, total, .. } => {
                // Parts per million, so a small screen's big change ranks fairly.
                (0, std::cmp::Reverse(changed * 1_000_000 / total.max(1)))
            }
            State::New => (1, std::cmp::Reverse(0)),
            State::Gone => (2, std::cmp::Reverse(0)),
            State::Same => (3, std::cmp::Reverse(0)),
        }
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The page: a summary line, then one card per screen. Pictures are read
/// from `previous/`, `latest/` and `diff/` next to the page.
pub fn report(entries: &[Entry]) -> String {
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.rank().cmp(&b.rank()).then_with(|| a.name.cmp(&b.name)));
    let count = |f: fn(&State) -> bool| sorted.iter().filter(|e| f(&e.state)).count();
    let changed = count(|s| matches!(s, State::Changed { .. }));
    let new = count(|s| matches!(s, State::New));
    let gone = count(|s| matches!(s, State::Gone));
    let same = count(|s| matches!(s, State::Same));
    let mut cards = String::new();
    for entry in sorted {
        let name = escape(&entry.name);
        let png = format!("{name}.png");
        let (label, body) = match entry.state {
            State::Changed {
                changed,
                total,
                resized,
            } => {
                let pct = changed as f64 * 100.0 / total.max(1) as f64;
                let label = if resized {
                    "changed size".to_owned()
                } else {
                    format!("{pct:.2}% of pixels changed")
                };
                let body = format!(
                    "<div class=row><figure><img src=\"previous/{png}\"><figcaption>before</figcaption></figure>\
                     <figure><img src=\"latest/{png}\"><figcaption>after</figcaption></figure>\
                     <figure><img src=\"diff/{png}\"><figcaption>where</figcaption></figure></div>"
                );
                (label, body)
            }
            State::New => (
                "new".to_owned(),
                format!("<div class=row><figure><img src=\"latest/{png}\"></figure></div>"),
            ),
            State::Gone => (
                "gone".to_owned(),
                format!("<div class=row><figure><img src=\"previous/{png}\"></figure></div>"),
            ),
            State::Same => (
                "unchanged".to_owned(),
                format!("<details><summary>show</summary><img src=\"latest/{png}\"></details>"),
            ),
        };
        cards.push_str(&format!(
            "<section><h2>{name} <span>{label}</span></h2>{body}</section>\n"
        ));
    }
    format!(
        "<!doctype html><html><head><meta charset=utf-8><title>Magi screens</title>\n<style>\
         :root{{color-scheme:light dark;--bg:#f6f6f4;--fg:#1d1d1f;--mute:#76767c;--card:#fff;--line:#e2e2e0}}\
         @media (prefers-color-scheme:dark){{:root{{--bg:#151517;--fg:#ececee;--mute:#9a9aa2;--card:#1f1f22;--line:#2e2e33}}}}\
         body{{margin:0;padding:24px 16px;background:var(--bg);color:var(--fg);font:14px/1.45 system-ui,sans-serif}}\
         h1{{font-size:20px;margin:0 0 4px}} p{{color:var(--mute);margin:0 0 20px}}\
         section{{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:12px 14px;margin:0 0 14px}}\
         h2{{font-size:14px;margin:0 0 10px;font-weight:600}} h2 span{{color:var(--mute);font-weight:400;margin-left:8px}}\
         .row{{display:flex;gap:10px;overflow-x:auto}} figure{{margin:0;flex:1;min-width:240px}}\
         img{{max-width:100%;border:1px solid var(--line);border-radius:6px;display:block}}\
         figcaption{{color:var(--mute);font-size:12px;margin-top:4px}}</style></head><body>\n\
         <h1>Magi screens</h1><p>{changed} changed · {new} new · {gone} gone · {same} unchanged</p>\n{cards}</body></html>\n"
    )
}
