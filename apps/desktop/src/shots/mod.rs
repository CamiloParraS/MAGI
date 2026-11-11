//! `just shots`: photographs every screen in fixed states, in light and dark,
//! English and Spanish, and compares them with the run before.
//!
//! Each window is real (the app's own views over a real host with sample
//! files, `MAGI_FAKE_EMBEDDER` and `MAGI_FAKE_ASK`), but hidden: it is drawn
//! on demand and read back with `Window::render_to_image`, so nothing shows
//! on screen and a locked screen does not matter. Windows and macOS only.
//!
//! Output, under `<target>/shots/`: `latest/` (this run), `previous/` (the
//! run before), `diff/` (where they differ) and `index.html`.

mod demo;
mod diff;
mod fixtures;
mod scenes;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use gpui_kit::*;

use crate::app::AppAssets;
use crate::ask::view::AskView;
use crate::i18n::Lang;
use crate::live::Live;
use crate::search::view::SearchView;
use diff::{Entry, compare, report};
use scenes::{Kit, Scene};

/// Between draws: long enough for a search, an answer from the fake server
/// or a thumbnail to arrive.
const SETTLE: Duration = Duration::from_millis(200);
const DRAWS: usize = 3;
/// A focused box's caret blinks every 500 ms: three pictures this far apart
/// always see it both shown and hidden, which [`diff::steady`] needs.
const CARET_SAMPLES: usize = 3;
const CARET_GAP: Duration = Duration::from_millis(300);

const THEMES: [&str; 2] = ["light", "dark"];
const LANGS: [(Lang, &str); 2] = [(Lang::En, "en"), (Lang::Es, "es")];

#[derive(Default)]
struct Filter {
    only: Vec<String>,
    theme: Option<String>,
    lang: Option<String>,
    /// `--demo <models>`: the demo video's screens, over real models.
    demo: Option<PathBuf>,
}

impl Filter {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut filter = Self::default();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let mut value = || args.next().cloned().ok_or(format!("{arg} needs a value"));
            match arg.as_str() {
                "--only" => filter.only.extend(value()?.split(',').map(str::to_owned)),
                "--theme" => filter.theme = Some(value()?),
                "--lang" => filter.lang = Some(value()?),
                "--demo" => filter.demo = Some(PathBuf::from(value()?)),
                other => return Err(format!("unknown argument {other}")),
            }
        }
        Ok(filter)
    }

    fn is_partial(&self) -> bool {
        !self.only.is_empty() || self.theme.is_some() || self.lang.is_some()
    }

    fn takes(&self, scene: &str, theme: &str, lang: &str) -> bool {
        (self.only.is_empty() || self.only.iter().any(|o| scene.starts_with(o.as_str())))
            && self.theme.as_deref().is_none_or(|t| t == theme)
            && self.lang.as_deref().is_none_or(|l| l == lang)
    }
}

pub fn main(args: Vec<String>) -> ExitCode {
    let filter = match Filter::parse(&args) {
        Ok(filter) => filter,
        Err(error) => {
            eprintln!(
                "{error}\nusage: magi-shots [--only a,b] [--theme light|dark] [--lang en|es] [--demo <models>]"
            );
            return ExitCode::FAILURE;
        }
    };
    let out = std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| PathBuf::from("target"), PathBuf::from)
        .join(if filter.demo.is_some() {
            "shots-demo"
        } else {
            "shots"
        });
    match prepare(&out, &filter) {
        Ok(profile) => run(out, filter, profile),
        Err(error) => {
            eprintln!("magi-shots: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn prepare(out: &Path, filter: &Filter) -> anyhow::Result<fixtures::Profile> {
    for dir in ["latest", "previous"] {
        std::fs::create_dir_all(out.join(dir))?;
    }
    // A full run leaves `latest` holding exactly this run: the last one moves
    // to `previous` whole. A partial run swaps only what it retakes.
    if !filter.is_partial() {
        std::fs::remove_dir_all(out.join("previous"))?;
        std::fs::rename(out.join("latest"), out.join("previous"))?;
        std::fs::create_dir_all(out.join("latest"))?;
    }
    let _ = std::fs::remove_dir_all(out.join("diff"));
    std::fs::create_dir_all(out.join("diff"))?;
    eprintln!("indexing the sample files...");
    fixtures::start(&out.join("profile"), filter.demo.as_deref())
}

fn run(out: PathBuf, filter: Filter, profile: fixtures::Profile) -> ExitCode {
    let host = profile.host.clone();
    let status = match host.engine().and_then(|e| e.status()) {
        Ok(status) => status,
        Err(error) => {
            eprintln!("magi-shots: no status: {error}");
            return ExitCode::FAILURE;
        }
    };
    gpui_kit::application()
        .with_assets(AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            SearchView::bind_keys(cx);
            AskView::bind_keys(cx);
            crate::onboarding::bind_keys(cx);
            // Animations draw their end state: the same picture every run.
            cx.set_reduce_motion(true);
            // Each shot closes its window; the next one opens after.
            cx.set_quit_mode(QuitMode::Explicit);
            let mut live = Live::new(Lang::En, "light".into());
            live.features = host.features().status().unwrap_or_default();
            let kit = Kit {
                host: host.clone(),
                live: cx.new(|_| live),
                // Nothing listens: a window asking the shell for something is a no-op.
                events: async_channel::unbounded().0,
                status,
                config: profile.config,
            };
            cx.spawn(async move |cx| {
                let taken = take_all(&kit, &out, &filter, cx).await;
                let code = finish(&out, &filter, &taken);
                host.shutdown();
                // GPUI's test-support build checks for leaked entities when the
                // app quits; these windows are throwaway, so skip the quit.
                std::process::exit(code);
            })
            .detach();
        });
    ExitCode::SUCCESS
}

/// Takes every scene the filter allows; returns the names written.
async fn take_all(kit: &Kit, out: &Path, filter: &Filter, cx: &mut AsyncApp) -> Vec<String> {
    let scenes = match filter.demo {
        Some(_) => demo::scenes(),
        None => scenes::all(),
    };
    let mut taken = Vec::new();
    for theme in THEMES {
        for (lang, lang_code) in LANGS {
            let wanted: Vec<&Scene> = scenes
                .iter()
                .filter(|s| filter.takes(&s.name, theme, lang_code))
                .collect();
            if wanted.is_empty() {
                continue;
            }
            let ui = serde_json::json!({ "ui": { "theme": theme, "language": lang_code } });
            if let Err(error) = kit.host.update_settings(&ui) {
                eprintln!("could not set {theme}/{lang_code}: {error}");
                continue;
            }
            cx.update(|cx| {
                kit.live.update(cx, |live, cx| {
                    live.lang = lang;
                    live.theme = theme.to_owned();
                    cx.notify();
                });
            });
            for scene in wanted {
                let name = format!("{}.{theme}.{lang_code}", scene.name);
                match take(kit, scene, cx).await {
                    Ok(image) => match save(out, &name, &image, filter.is_partial()) {
                        Ok(()) => {
                            eprintln!("  {name}");
                            taken.push(name);
                        }
                        Err(error) => eprintln!("  {name}: could not save: {error:#}"),
                    },
                    Err(error) => eprintln!("  {name}: FAILED: {error:#}"),
                }
            }
        }
    }
    taken
}

async fn take(kit: &Kit, scene: &Scene, cx: &mut AsyncApp) -> anyhow::Result<image::RgbaImage> {
    // A scene's steps may change the features (a download in progress);
    // the next scene starts from the real ones again.
    let features = kit.host.features().status().unwrap_or_default();
    cx.update(|cx| kit.live.update(cx, |live, _| live.features = features));
    let handle = cx.update(|cx| (scene.open)(kit, cx))?;
    settle(handle, cx).await;
    for step in &scene.steps {
        cx.update(|cx| step(kit, handle, cx));
        settle(handle, cx).await;
    }
    let mut frames = Vec::new();
    for i in 0..CARET_SAMPLES {
        if i > 0 {
            cx.background_executor().timer(CARET_GAP).await;
        }
        let frame = cx.update(|cx| {
            handle.update(cx, |_, window, cx| {
                window.draw(cx).clear(cx);
                window.render_to_image()
            })
        });
        match frame {
            Ok(Ok(frame)) => frames.push(frame),
            Ok(Err(error)) => return Err(error),
            Err(error) => return Err(error),
        }
    }
    cx.update(|cx| {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    });
    Ok(diff::steady(&frames))
}

/// Draws a few times with pauses between: each draw lets deferred work (a
/// window fitting its content, a search, a fake answer) land before the next.
async fn settle(handle: AnyWindowHandle, cx: &mut AsyncApp) {
    for _ in 0..DRAWS {
        cx.update(|cx| {
            let _ = handle.update(cx, |_, window, cx| {
                pointer_away(window, cx);
                window.draw(cx).clear(cx);
            });
        });
        cx.background_executor().timer(SETTLE).await;
    }
}

/// A window takes the real pointer's position when it opens, so whatever
/// is under the pointer would show hovered. A move far outside clears it.
fn pointer_away(window: &mut Window, cx: &mut App) {
    window.dispatch_event(
        PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(-10_000.), px(-10_000.)),
            pressed_button: None,
            modifiers: Modifiers::default(),
        }),
        cx,
    );
}

/// Writes `latest/<name>.png`. A partial run first moves the picture it
/// replaces to `previous/`, or drops a `previous/` one with nothing in
/// `latest/` (left from an older run, not the last one).
fn save(out: &Path, name: &str, image: &image::RgbaImage, partial: bool) -> anyhow::Result<()> {
    let file = format!("{name}.png");
    let (latest, previous) = (
        out.join("latest").join(&file),
        out.join("previous").join(&file),
    );
    if partial {
        if latest.exists() {
            std::fs::rename(&latest, &previous)?;
        } else if previous.exists() {
            std::fs::remove_file(&previous)?;
        }
    }
    image.save(&latest)?;
    Ok(())
}

/// Compares this run with the last, writes the diff pictures and the page,
/// and says where it is. Exit code 1 when any scene failed.
fn finish(out: &Path, filter: &Filter, taken: &[String]) -> i32 {
    let mut entries = Vec::new();
    for name in taken {
        let file = format!("{name}.png");
        let after = image::open(out.join("latest").join(&file)).map(|i| i.to_rgba8());
        let before = image::open(out.join("previous").join(&file)).map(|i| i.to_rgba8());
        entries.push(match (before, after) {
            (Ok(before), Ok(after)) => {
                let c = compare(&before, &after);
                if c.changed == 0 {
                    Entry::same(name)
                } else {
                    if let Err(error) = c.image.save(out.join("diff").join(&file)) {
                        eprintln!("could not save the diff of {name}: {error}");
                    }
                    Entry::changed(name, c.changed, c.total, c.resized)
                }
            }
            _ => Entry::new_shot(name),
        });
    }
    if !filter.is_partial() {
        let gone = std::fs::read_dir(out.join("previous"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                e.file_name()
                    .to_str()?
                    .strip_suffix(".png")
                    .map(str::to_owned)
            })
            .filter(|name| !taken.contains(name));
        entries.extend(gone.map(|name| Entry::gone(&name)));
    }
    let page = out.join("index.html");
    if let Err(error) = std::fs::write(&page, report(&entries)) {
        eprintln!("could not write {}: {error}", page.display());
        return 1;
    }
    let changed = entries
        .iter()
        .filter(|e| matches!(e.state, diff::State::Changed { .. }))
        .count();
    let expected = expected_count(filter);
    eprintln!(
        "\n{} screens, {changed} changed since the last run\n{}",
        taken.len(),
        page.display()
    );
    if taken.len() < expected {
        eprintln!("{} scene(s) failed; see above", expected - taken.len());
        return 1;
    }
    0
}

fn expected_count(filter: &Filter) -> usize {
    let scenes = match filter.demo {
        Some(_) => demo::scenes(),
        None => scenes::all(),
    };
    THEMES
        .iter()
        .flat_map(|t| LANGS.iter().map(move |(_, l)| (*t, *l)))
        .map(|(t, l)| {
            scenes
                .iter()
                .filter(|s| filter.takes(&s.name, t, l))
                .count()
        })
        .sum()
}
