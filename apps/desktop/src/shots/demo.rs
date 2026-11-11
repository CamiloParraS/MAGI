//! `just shots --demo <models>`: the screens of the demo video, over real
//! models and a part of `fixtures/corpus`, so search by meaning, text in
//! images and finding photos by what they show are the app's own results.
//!
//! The models folder holds `text/`, `ocr/` and `image/` as the app downloads
//! them (a profile's `data/models`). They are linked, not copied, when the
//! folder is on the same drive as the output.

use std::path::{Path, PathBuf};

use gpui_kit::*;
use magi_core::ask::AskAnswer;
use magi_core::dto::{SearchRequest, SearchResult};

use super::scenes::{
    Open, Scene, Step, account, content, fresh_email, on_ask, onboarding_window, open_hidden,
    reset, scene, search_window, settings_window,
};
use crate::ask::view::AskView;
use crate::ask::{self, Request};
use crate::search::view::SearchView;
use crate::settings::view::Section;
use crate::theme::Backdrop;
use magi_core::config::Onboarding;

/// The corpus files the demo indexes, under `fixtures/corpus`. All made or
/// shot by the repo owner (`fixtures/README.md`); nothing that shows a
/// person, a brand or someone else's art.
pub const FILES: &[&str] = &[
    "en/lease_agreement.txt",
    "en/arepas_recipe.txt",
    "en/travel_itinerary.txt",
    "en/car_maintenance.txt",
    "en/doctor_appointment.txt",
    "en/electrician_invoice.txt",
    "en/meeting_notes.txt",
    "en/birthday_party.txt",
    "en/book_club.txt",
    "en/grocery_list.txt",
    "en/onboarding_notes.txt",
    "es/contrato_alquiler.txt",
    "es/receta_arepas.txt",
    "es/itinerario_viaje.txt",
    "es/informe_trimestral.txt",
    "es/cita_medica.txt",
    "pdf/factura_electricista.pdf",
    "pdf/report.pdf",
    "office/notes.docx",
    "office/kickoff.pptx",
    "office/inventory.xlsx",
    "code/sample.rs",
    "code/muestra.py",
    "images/dog_on_beach.jpg",
    "images/dog_on_beach2.jpg",
    "images/cat_on_beach.jpg",
    "images/mountain_sunrise.jpg",
    "images/mountain_sunset.jpg",
    "images/shelf_christmas.heic",
    "images/screenshot_es.png",
    "qr/qr_url.png",
];

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus")
}

/// Copies [`FILES`] into `root`, keeping their folders.
pub fn write_files(root: &Path) -> anyhow::Result<()> {
    let corpus = corpus();
    for file in FILES {
        let to = root.join(file);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(corpus.join(file), &to)?;
    }
    Ok(())
}

/// Puts the models under `data/models`, hard-linked where it can.
pub fn link_models(from: &Path, data: &Path) -> anyhow::Result<()> {
    for entry in walk(from)? {
        let to = data.join("models").join(entry.strip_prefix(from)?);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if std::fs::hard_link(&entry, &to).is_err() {
            std::fs::copy(&entry, &to)?;
        }
    }
    Ok(())
}

fn walk(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(walk(&path)?);
        } else {
            files.push(path);
        }
    }
    Ok(files)
}

/// Every feature on, written before the host starts.
pub const CONFIG: &str =
    "schema_version = 1\n\n[features]\nmeaning = true\nimage_text = true\nimage_visual = true\n";

/// The video's screens, in the order it shows them.
pub fn scenes() -> Vec<Scene> {
    let mut scenes = vec![
        scene("demo-01-open", search_window("", None, false), vec![]),
        scene(
            "demo-02-rent",
            search_window("when do I have to pay the rent", None, true),
            vec![typed()],
        ),
        scene(
            "demo-03-spanish",
            search_window("rental contract deposit", None, true),
            vec![typed()],
        ),
        scene(
            "demo-04-recipe",
            search_window("how to make arepas", None, false),
            vec![typed()],
        ),
        scene(
            "demo-05-photo",
            search_window("dog playing on the sand", None, true),
            vec![typed()],
        ),
        scene(
            "demo-06-sunrise",
            search_window("mountains at dawn", None, true),
            vec![typed()],
        ),
        scene(
            "demo-07-image-text",
            search_window("vocales con acento", None, true),
            vec![typed()],
        ),
        scene(
            "demo-08-qr",
            search_window("example.com", None, true),
            vec![typed()],
        ),
        scene(
            "demo-09-invoice",
            search_window("electrician invoice", None, true),
            vec![typed()],
        ),
        scene(
            "demo-10-code",
            search_window("point origin", None, true),
            vec![typed()],
        ),
        scene(
            "demo-11-slides",
            search_window("project kickoff", None, false),
            vec![typed()],
        ),
        scene(
            "demo-12-onboarding",
            onboarding_window(Onboarding::Folders),
            vec![],
        ),
        scene(
            "demo-13-features",
            settings_window(Section::Searched, false),
            vec![],
        ),
        scene(
            "demo-14-folders",
            settings_window(Section::Folders, false),
            vec![],
        ),
    ];
    scenes.push(scene("demo-15-ask", ask_window(&EN), vec![]));
    scenes.push(scene(
        "demo-16-ask-answer",
        ask_window(&EN),
        vec![answer(&EN)],
    ));
    scenes.extend(spanish());
    scenes
}

/// The same story with Spanish queries, for the Spanish video. Screens that
/// need no query (or whose query is already Spanish) come from the ones above.
fn spanish() -> Vec<Scene> {
    let searches = [
        ("demo-es-02-rent", "cuándo tengo que pagar el alquiler"),
        ("demo-es-03-english", "depósito del contrato de alquiler"),
        ("demo-es-05-photo", "perro jugando en la arena"),
        ("demo-es-06-sunrise", "montañas al amanecer"),
        ("demo-es-10-code", "saludo personalizado"),
    ];
    let mut scenes: Vec<Scene> = searches
        .into_iter()
        .map(|(name, query)| scene(name, search_window(query, None, true), vec![typed()]))
        .collect();
    scenes.push(scene(
        "demo-es-11-slides",
        search_window("kickoff", None, false),
        vec![typed()],
    ));
    scenes.push(scene("demo-es-15-ask", ask_window(&ES), vec![]));
    scenes.push(scene(
        "demo-es-16-ask-answer",
        ask_window(&ES),
        vec![answer(&ES)],
    ));
    scenes
}

/// The restored query is selected; the video shows it as just typed.
fn typed() -> Step {
    Box::new(|_, handle, cx| {
        let _ = handle.update(cx, |root, window, cx| {
            if let Some(view) = content::<SearchView>(root, cx) {
                view.update(cx, |view, cx| view.deselect_query(window, cx));
            }
        });
    })
}

/// The Ask screens' words in one language.
struct AskText {
    search: &'static str,
    question: &'static str,
    answer: &'static str,
}

const EN: AskText = AskText {
    search: "rental contract deposit",
    question: "When is the rent due, and how big is the deposit?",
    answer: "Rent is due on the first day of each month, with a five-day grace period. \
             The security deposit covers two months of rent, refunded when you move out.",
};

const ES: AskText = AskText {
    search: "depósito del contrato de alquiler",
    question: "¿Cuándo se paga el alquiler y de cuánto es el depósito?",
    answer: "El alquiler vence el primer día de cada mes, con cinco días de gracia. \
             El depósito equivale a dos meses de renta y se devuelve al mudarte.",
};

/// Asking about the lease search's results, signed in.
fn ask_window(text: &'static AskText) -> Open {
    Box::new(move |kit, cx| {
        reset(kit, cx);
        account(kit, Some(fresh_email()), true, cx)?;
        let request = request(&kit.host, text)?;
        let (host, live) = (kit.host.clone(), kit.live.clone());
        let (handle, _) = open_hidden(crate::ask::view::SIZE, cx, move |window, cx| {
            cx.new(|cx| AskView::new(host, live, Backdrop::Solid, request, window, cx))
        })?;
        Ok(handle)
    })
}

fn request(host: &magi_core::host::Host, text: &AskText) -> anyhow::Result<Request> {
    let results: Vec<SearchResult> = host
        .search(&SearchRequest {
            query: text.search.into(),
            limit: None,
        })?
        .results
        .into_iter()
        .take(2)
        .collect();
    let mut request = ask::request(text.search, &results, |id| {
        host.ask_preview(id).unwrap_or_default()
    });
    request.question = text.question.into();
    Ok(request)
}

/// The reply the video shows, written from the lease's own text: the shots
/// never reach the real service, and the fake one only echoes.
fn answer(text: &'static AskText) -> Step {
    Box::new(move |_, handle, cx| {
        on_ask(handle, cx, move |view, window, cx| {
            view.show_answer(
                text.question,
                AskAnswer {
                    answer: text.answer.into(),
                    cited: vec!["1".into(), "2".into()],
                    used: 1,
                    limit: 20,
                },
                window,
                cx,
            );
        });
    })
}
