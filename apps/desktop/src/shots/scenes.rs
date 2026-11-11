//! Every screen the shots photograph, each in one fixed state: how to open
//! it, and what to do to it before the picture.

use std::time::Instant;

use gpui_kit::*;
use magi_core::ask::SendAs;
use magi_core::config::Onboarding;
use magi_core::dto::{IndexState, IndexStatus, Install, SearchRequest, SearchResult};
use magi_core::host::Host;

use crate::app::Events;
use crate::ask::view::AskView;
use crate::ask::{self, Request};
use crate::live::Live;
use crate::onboarding::{self, OnboardingView};
use crate::search::view::{self as search, Opening, SearchView};
use crate::settings::view::{self as settings, SECTIONS, Section, SettingsView};
use crate::theme::Backdrop;

/// What every scene opens its window over.
pub struct Kit {
    pub host: Host,
    pub live: Entity<Live>,
    pub events: Events,
    /// The idle status after the samples were indexed.
    pub status: IndexStatus,
    /// The profile's `config.toml`.
    pub config: std::path::PathBuf,
}

pub type Open = Box<dyn Fn(&Kit, &mut App) -> anyhow::Result<AnyWindowHandle>>;
/// Done to the open window; the picture waits until it settles.
pub type Step = Box<dyn Fn(&Kit, AnyWindowHandle, &mut App)>;

pub struct Scene {
    pub name: String,
    pub open: Open,
    pub steps: Vec<Step>,
}

pub(super) fn scene(name: &str, open: Open, steps: Vec<Step>) -> Scene {
    Scene {
        name: name.to_owned(),
        open,
        steps,
    }
}

/// Every scene, in the order they are taken.
pub fn all() -> Vec<Scene> {
    let mut scenes = vec![
        scene("search-empty", search_window("", None, false), vec![]),
        scene(
            "search-shortcut",
            search_window("", Some("Alt+Space"), false),
            vec![],
        ),
        scene("search-results", search_window("rent", None, false), vec![]),
        scene(
            "search-results-pane",
            search_window("rent", None, true),
            vec![],
        ),
        scene(
            "search-nothing",
            search_window("qqzzx vvbnm", None, false),
            vec![],
        ),
        scene("search-indexing", search_indexing(), vec![]),
    ];
    for section in SECTIONS {
        let name = format!("settings-{}", slug(section));
        scenes.push(scene(&name, settings_window(section, true), vec![]));
    }
    scenes.push(scene(
        "settings-account-signed-out",
        settings_window(Section::Account, false),
        vec![],
    ));
    scenes.push(scene(
        "onboarding-setup",
        onboarding_window(Onboarding::Folders),
        vec![],
    ));
    scenes.push(scene(
        "onboarding-finish",
        onboarding_window(Onboarding::Done),
        vec![],
    ));
    scenes.push(scene(
        "onboarding-finish-downloading",
        onboarding_window(Onboarding::Done),
        vec![Box::new(|kit, _, cx| {
            kit.live.update(cx, |live, cx| {
                if let Some(f) = live.features.iter_mut().find(|f| f.enabled) {
                    f.install = Install::Downloading {
                        // The meaning model's size, as the setup screen lists it.
                        bytes: 54_000_000,
                        total: 135_000_000,
                    };
                }
                cx.notify();
            });
        })],
    ));
    scenes.push(scene(
        "onboarding-no-folders",
        onboarding_window(Onboarding::Folders),
        vec![Box::new(|kit, _, cx| {
            kit.live.update(cx, |live, cx| {
                if let Some(status) = live.status.as_mut() {
                    status.roots.clear();
                }
                cx.notify();
            });
        })],
    ));
    scenes.push(scene(
        "onboarding-ask-on",
        onboarding_window(Onboarding::Folders),
        vec![Box::new(|_, handle, cx| {
            on_onboarding(handle, cx, |view, _, cx| view.set_ask(true, cx));
        })],
    ));
    scenes.push(scene(
        "onboarding-account",
        onboarding_window(Onboarding::Folders),
        vec![Box::new(|_, handle, cx| {
            on_onboarding(handle, cx, |view, _, cx| {
                view.set_ask(true, cx);
                view.go(onboarding::Step::Account, cx);
            });
        })],
    ));
    scenes.extend(ask_scenes());
    scenes
}

fn slug(section: Section) -> &'static str {
    match section {
        Section::Folders => "folders",
        Section::Searched => "whats-searched",
        Section::General => "general",
        Section::Appearance => "appearance",
        Section::Index => "index",
        Section::Account => "account",
    }
}

fn ask_scenes() -> Vec<Scene> {
    let ask = |signed_in: bool, disclosed: bool| ask_window(signed_in, disclosed);
    let send = |question: &'static str| -> Step {
        Box::new(move |_, handle, cx| {
            on_ask(handle, cx, |view, window, cx| {
                view.set_question(question, window, cx);
                view.send(window, cx);
            });
        })
    };
    vec![
        scene("ask-sign-in", ask(false, false), vec![]),
        scene("ask-disclosure", ask(true, false), vec![]),
        scene("ask-ready", ask(true, true), vec![]),
        scene(
            "ask-send-modes",
            ask(true, true),
            vec![Box::new(|_, handle, cx| {
                on_ask(handle, cx, |view, _, cx| {
                    view.switch_send(0, SendAs::Whole, cx)
                });
            })],
        ),
        scene(
            "ask-answer",
            ask(true, true),
            vec![send("When is the rent due?")],
        ),
        scene(
            "ask-conversation-scrolled",
            ask(true, true),
            vec![
                send("When is the rent due?"),
                send("And the deposit?"),
                send("Who repairs the heating?"),
                send("How do I end the lease?"),
                Box::new(|_, handle, cx| {
                    on_ask(handle, cx, |view, _, cx| view.scroll_to_top(cx));
                }),
            ],
        ),
        scene(
            "ask-picker",
            ask(true, true),
            vec![Box::new(|_, handle, cx| {
                on_ask(handle, cx, |view, window, cx| {
                    view.toggle_picker(window, cx)
                });
            })],
        ),
        scene(
            "ask-picker-results",
            ask(true, true),
            vec![Box::new(|_, handle, cx| {
                on_ask(handle, cx, |view, window, cx| {
                    view.toggle_picker(window, cx);
                    view.set_picker_query("lease", window, cx);
                });
            })],
        ),
    ]
}

pub(super) fn on_ask(
    handle: AnyWindowHandle,
    cx: &mut App,
    f: impl FnOnce(&mut AskView, &mut Window, &mut Context<AskView>),
) {
    let _ = handle.update(cx, |root, window, cx| {
        if let Some(view) = content::<AskView>(root, cx) {
            view.update(cx, |view, cx| f(view, window, cx));
        }
    });
}

fn on_onboarding(
    handle: AnyWindowHandle,
    cx: &mut App,
    f: impl FnOnce(&mut OnboardingView, &mut Window, &mut Context<OnboardingView>),
) {
    let _ = handle.update(cx, |root, window, cx| {
        if let Some(view) = content::<OnboardingView>(root, cx) {
            view.update(cx, |view, cx| f(view, window, cx));
        }
    });
}

/// The view inside a window's Root.
pub(super) fn content<V: 'static>(root: AnyView, cx: &mut App) -> Option<Entity<V>> {
    root.downcast::<base::Root>()
        .ok()?
        .read(cx)
        .view()
        .clone()
        .downcast::<V>()
        .ok()
}

/// A hidden window: nothing is shown or focused, and the picture is drawn
/// off screen.
fn options(size: Size<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            // Far from the pointer: wherever it is on screen, nothing hovers.
            origin: point(px(-30_000.), px(-30_000.)),
            size,
        })),
        focus: false,
        show: false,
        ..Default::default()
    }
}

/// Opens `build` hidden at `size`. A hidden window's drawing surface stays
/// 1x1 until the window is resized, so it is resized once at the start.
pub(super) fn open_hidden<V: Render + 'static>(
    size: Size<Pixels>,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> anyhow::Result<(AnyWindowHandle, Entity<V>)> {
    let (handle, view) = gpui_kit::open_window(options(size), cx, build)?;
    handle.update(cx, |_, window, _| window.resize(size))?;
    Ok((handle, view))
}

/// Back to the shared state every scene starts from.
pub(super) fn reset(kit: &Kit, cx: &mut App) {
    let status = kit.status.clone();
    kit.live.update(cx, |live, cx| {
        live.status = Some(status);
        live.scan_left = None;
        live.last_query.clear();
        live.dismissed.clear();
        live.shortcut_dismissed = false;
        cx.notify();
    });
}

pub(super) fn search_window(query: &'static str, teach: Option<&'static str>, pane: bool) -> Open {
    Box::new(move |kit, cx| {
        reset(kit, cx);
        kit.host
            .update_settings(&serde_json::json!({ "ui": { "detail_pane": pane } }))?;
        kit.live
            .update(cx, |live, _| live.last_query = query.to_owned());
        open_search(kit, teach, cx)
    })
}

fn search_indexing() -> Open {
    Box::new(|kit, cx| {
        reset(kit, cx);
        kit.host
            .update_settings(&serde_json::json!({ "ui": { "detail_pane": false } }))?;
        let status = IndexStatus {
            state: IndexState::Indexing,
            queued: 1_204,
            current_file: Some("Quarterly report.pdf".into()),
            ..kit.status.clone()
        };
        kit.live.update(cx, |live, _| live.status = Some(status));
        open_search(kit, None, cx)
    })
}

fn open_search(kit: &Kit, teach: Option<&str>, cx: &mut App) -> anyhow::Result<AnyWindowHandle> {
    let (host, live, events) = (kit.host.clone(), kit.live.clone(), kit.events.clone());
    let opening = Opening {
        at: Instant::now(),
        teach: teach.map(str::to_owned),
    };
    let size = size(search::WIDTH, search::OPEN_HEIGHT);
    let (handle, _) = open_hidden(size, cx, move |window, cx| {
        cx.new(|cx| SearchView::new(host, live, events, Backdrop::Solid, opening, window, cx))
    })?;
    Ok(handle)
}

pub(super) fn settings_window(section: Section, signed_in: bool) -> Open {
    Box::new(move |kit, cx| {
        reset(kit, cx);
        let who = signed_in.then(|| SETTINGS_EMAIL.to_owned());
        account(kit, who, true, cx)?;
        let (host, live, events) = (kit.host.clone(), kit.live.clone(), kit.events.clone());
        let (handle, view) = open_hidden(settings::SIZE, cx, move |window, cx| {
            cx.new(|cx| SettingsView::new(host, live, events, window, cx))
        })?;
        view.update(cx, |view, cx| view.show_section(section, cx));
        Ok(handle)
    })
}

pub(super) fn onboarding_window(step: Onboarding) -> Open {
    Box::new(move |kit, cx| {
        reset(kit, cx);
        kit.host
            .update_settings(&serde_json::json!({ "ui": { "onboarding": step } }))?;
        let (host, live, events) = (kit.host.clone(), kit.live.clone(), kit.events.clone());
        let (handle, _) = open_hidden(onboarding::SIZE, cx, move |window, cx| {
            cx.new(|cx| OnboardingView::new(host, live, events, window, cx))
        })?;
        Ok(handle)
    })
}

fn ask_window(signed_in: bool, disclosed: bool) -> Open {
    Box::new(move |kit, cx| {
        reset(kit, cx);
        account(kit, signed_in.then(fresh_email), disclosed, cx)?;
        let request = ask_request(&kit.host)?;
        let (host, live) = (kit.host.clone(), kit.live.clone());
        let (handle, _) = open_hidden(crate::ask::view::SIZE, cx, move |window, cx| {
            cx.new(|cx| AskView::new(host, live, Backdrop::Solid, request, window, cx))
        })?;
        Ok(handle)
    })
}

/// The account Settings shows. It never asks, so its count stays at 0.
const SETTINGS_EMAIL: &str = "ana@example.com";
const PASSWORD: &str = "shots-pass-42";

/// A new account for each Ask scene: the fake server counts questions per
/// account, and a shared one would reach the daily limit partway through.
/// The Ask window never shows the email.
pub(super) fn fresh_email() -> String {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("ask-{n}@example.com")
}

/// Signs the fake account in as `who`, or out (`MAGI_FAKE_ASK`: in memory,
/// never the keychain), and sets whether the first-use disclosure was seen.
pub(super) fn account(
    kit: &Kit,
    who: Option<String>,
    disclosed: bool,
    cx: &mut App,
) -> anyhow::Result<()> {
    let host = &kit.host;
    if host.account_state()?.signed_in_as != who {
        host.sign_out()?;
        if let Some(email) = &who
            && host.sign_in(email, PASSWORD).is_err()
        {
            host.sign_up(email, PASSWORD)?;
        }
    }
    if host.account_state()?.disclosed != disclosed {
        set_disclosed(&kit.config, disclosed)?;
    }
    let state = host.account_state()?;
    kit.live.update(cx, |live, cx| {
        live.account = state;
        cx.notify();
    });
    Ok(())
}

/// The host can only ever set `ask.disclosed` (the first question does), so
/// the throwaway profile's file is edited to show the disclosure again.
fn set_disclosed(config: &std::path::Path, disclosed: bool) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(config)?;
    let (from, to) = if disclosed {
        ("disclosed = false", "disclosed = true")
    } else {
        ("disclosed = true", "disclosed = false")
    };
    anyhow::ensure!(text.contains(from), "no `{from}` in {}", config.display());
    std::fs::write(config, text.replace(from, to))?;
    Ok(())
}

/// What the search for "rent" would ask about, with a picture after it.
fn ask_request(host: &Host) -> anyhow::Result<Request> {
    let search = |query: &str| -> anyhow::Result<Vec<SearchResult>> {
        let request = SearchRequest {
            query: query.into(),
            limit: None,
        };
        Ok(host.search(&request)?.results)
    };
    let mut results: Vec<SearchResult> = search("rent")?.into_iter().take(2).collect();
    results.extend(search("whiteboard")?.into_iter().take(1));
    let mut request = ask::request("rent", &results, |id| {
        host.ask_preview(id).unwrap_or_default()
    });
    request.question = "When is the rent due?".into();
    Ok(request)
}
