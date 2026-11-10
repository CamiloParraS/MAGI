//! How each chip's file is sent (ADR-0014), and adding and removing chips:
//! files dropped on the window or picked from the index join the next
//! question. Plain functions over [`AskModel`], so `cargo test` covers them.

use magi_core::ask::SendAs;
use magi_core::discovery::Kind;
use magi_core::dto::{AskFileInfo, SearchResult, WholeSize};

use super::{AskModel, MAX_FILE_NAME_LEN, MAX_SOURCE_LEN, MAX_SOURCES, Source, clip, excerpt};
use crate::i18n::{Lang, Strings};

/// The most pictures one question sends; the server refuses more.
pub const MAX_IMAGES: usize = magi_core::ask::image::MAX_IMAGES;

/// What a chip's file can be sent as, the default first.
pub fn send_choices(source: &Source) -> Vec<SendAs> {
    let has_text = !source.text.trim().is_empty();
    match (source.kind == Kind::Image, has_text) {
        (true, true) => vec![SendAs::Image, SendAs::Passages],
        (true, false) => vec![SendAs::Image],
        (false, _) => vec![SendAs::Passages, SendAs::Whole],
    }
}

/// The few words on a chip that say how its file goes.
pub fn send_text(send: SendAs, s: &Strings) -> &'static str {
    match send {
        SendAs::Passages => s.ask_send_passages,
        SendAs::Whole => s.ask_send_whole,
        SendAs::Image => s.ask_send_image,
    }
}

fn ticked_images(sources: &[Source]) -> usize {
    sources
        .iter()
        .filter(|s| s.ticked && s.send == SendAs::Image)
        .count()
}

/// A picture goes as itself while there is room for one; past that, as its
/// text, or not at all when it has none.
pub(super) fn default_send(kind: Kind, has_text: bool, images: usize) -> Option<SendAs> {
    match kind {
        Kind::Image if images < MAX_IMAGES => Some(SendAs::Image),
        Kind::Image if has_text => Some(SendAs::Passages),
        Kind::Image => None,
        _ => Some(SendAs::Passages),
    }
}

/// What [`AskModel::add`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Added {
    Yes,
    /// It was a chip already; now it is ticked.
    AlreadyThere,
    /// [`MAX_SOURCES`] chips already.
    Full,
    /// A picture with no text while [`MAX_IMAGES`] are ticked.
    TooManyPictures,
    /// A question is on its way.
    Busy,
}

impl AskModel {
    /// Sends a chip's file as `send`, when the file can go that way and,
    /// for a picture, there is room for one more.
    pub fn set_send(&mut self, ix: usize, send: SendAs) -> bool {
        if self.is_asking() {
            return false;
        }
        let images = ticked_images(&self.request.sources);
        let Some(source) = self.request.sources.get_mut(ix) else {
            return false;
        };
        if !send_choices(source).contains(&send) {
            return false;
        }
        let adds_image = send == SendAs::Image && source.send != SendAs::Image && source.ticked;
        if adds_image && images >= MAX_IMAGES {
            return false;
        }
        source.send = send;
        true
    }

    /// Whether ticking chip `ix` would pass the picture limit.
    pub(super) fn tick_allowed(&self, ix: usize) -> bool {
        self.request.sources.get(ix).is_some_and(|source| {
            source.ticked
                || source.send != SendAs::Image
                || ticked_images(&self.request.sources) < MAX_IMAGES
        })
    }

    /// A file dropped on the window or picked from the index, as a ticked
    /// chip after the others.
    pub fn add(&mut self, file: AskFileInfo) -> Added {
        if self.is_asking() {
            return Added::Busy;
        }
        if let Some(ix) = self
            .request
            .sources
            .iter()
            .position(|s| s.file_id == file.file_id)
        {
            if self.tick_allowed(ix) {
                self.request.sources[ix].ticked = true;
            }
            return Added::AlreadyThere;
        }
        if self.request.sources.len() >= MAX_SOURCES {
            return Added::Full;
        }
        let text = clip(file.preview.trim(), MAX_SOURCE_LEN);
        let images = ticked_images(&self.request.sources);
        let Some(send) = default_send(file.kind, !text.is_empty(), images) else {
            return Added::TooManyPictures;
        };
        self.request.sources.push(Source {
            file_id: file.file_id,
            file_name: clip(&file.file_name, MAX_FILE_NAME_LEN),
            kind: file.kind,
            text,
            thumb: file.thumb_path.map(std::path::PathBuf::from),
            ticked: true,
            send,
        });
        Added::Yes
    }

    /// Takes a chip off, except while a question is on its way.
    pub fn remove(&mut self, ix: usize) {
        if !self.is_asking() && ix < self.request.sources.len() {
            self.request.sources.remove(ix);
        }
    }
}

/// Something worth saying after an attempt to add a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    Full,
    TooManyPictures,
    /// A dropped file outside the user's Magi folders.
    NotIndexed,
}

pub fn notice_text(notice: Notice, s: &Strings) -> &'static str {
    match notice {
        Notice::Full => s.ask_chips_full,
        Notice::TooManyPictures => s.ask_too_many_pictures,
        Notice::NotIndexed => s.ask_not_indexed,
    }
}

/// What to say about an [`Added`]; success needs no words.
pub fn notice_for(added: Added) -> Option<Notice> {
    match added {
        Added::Full => Some(Notice::Full),
        Added::TooManyPictures => Some(Notice::TooManyPictures),
        Added::Yes | Added::AlreadyThere | Added::Busy => None,
    }
}

/// A search result as a file to attach. Its excerpt goes along only when it
/// holds something from inside the file (never a repeat of the path).
pub fn file_info(result: &SearchResult) -> AskFileInfo {
    AskFileInfo {
        file_id: result.file_id,
        file_name: result.file_name.clone(),
        kind: result.kind,
        thumb_path: result.thumb_path.clone(),
        preview: excerpt(result).unwrap_or_default().to_owned(),
    }
}

/// How much "whole file" sends, to the hundred: the exact count is noise.
pub fn whole_note(size: WholeSize, lang: Lang, s: &Strings) -> String {
    if size.cut {
        return s.ask_whole_cut_short.to_owned();
    }
    let n = if size.chars >= 1000 {
        size.chars / 100 * 100
    } else {
        size.chars
    };
    s.ask_whole_size.replace("{n}", &lang.number(n as u64))
}

impl AskModel {
    /// Adds what a drop found (`None`: not in the user's folders). The most
    /// useful thing to say comes back: a full dock over a stranger.
    pub fn add_dropped(&mut self, files: Vec<Option<AskFileInfo>>) -> Option<Notice> {
        let mut said = None;
        for file in files {
            let notice = match file {
                Some(file) => notice_for(self.add(file)),
                None => Some(Notice::NotIndexed),
            };
            said = match (said, notice) {
                (Some(Notice::NotIndexed) | None, notice) => notice.or(said),
                (said, _) => said,
            };
        }
        said
    }
}

/// A chip's tooltip about how it is sent: the way, and for a whole file too
/// long to go whole, that it is cut.
pub fn send_tip(send: SendAs, size: Option<WholeSize>, s: &Strings) -> String {
    match send {
        SendAs::Passages => s.ask_send_passages_tip.to_owned(),
        SendAs::Image => s.ask_send_image_tip.to_owned(),
        SendAs::Whole if size.is_some_and(|size| size.cut) => {
            format!("{} {}", s.ask_send_whole_tip, s.ask_whole_cut)
        }
        SendAs::Whole => s.ask_send_whole_tip.to_owned(),
    }
}

/// What a click on a chip's send pill switches to: the next way it can go,
/// or `None` when there is only one.
pub fn next_send(source: &Source) -> Option<SendAs> {
    let choices = send_choices(source);
    if choices.len() < 2 {
        return None;
    }
    let at = choices.iter().position(|&c| c == source.send).unwrap_or(0);
    Some(choices[(at + 1) % choices.len()])
}
