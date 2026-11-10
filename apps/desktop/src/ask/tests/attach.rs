//! Attaching files to the next question and choosing how each is sent.

use magi_core::ask::SendAs;
use magi_core::discovery::Kind;
use magi_core::dto::AskFileInfo;

use super::*;
use crate::ask::attach::{
    Added, MAX_IMAGES, Notice, file_info, notice_for, notice_text, send_choices, send_text,
    send_tip, whole_note,
};
use magi_core::dto::WholeSize;

fn info(file_id: i64, kind: Kind) -> AskFileInfo {
    AskFileInfo {
        file_id,
        file_name: format!("{file_id}.bin"),
        kind,
        thumb_path: None,
        preview: "some text".into(),
    }
}

fn photo(file_id: i64) -> SearchResult {
    result(file_id, Kind::Image, None)
}

#[test]
fn a_photo_with_text_in_it_still_goes_as_its_picture() {
    let taken = request("receipt", &[result(1, Kind::Image, Some("Pago aprobado"))]);
    assert_eq!(taken.sources[0].send, SendAs::Image);
    // The text stays, for the tooltip and if the picture cannot be read.
    assert_eq!(taken.sources[0].text, "Pago aprobado");
}

#[test]
fn past_three_pictures_a_photo_goes_as_its_text_or_not_at_all() {
    let mut results: Vec<SearchResult> = (1..=3).map(photo).collect();
    results.push(result(4, Kind::Image, Some("text in it")));
    results.push(photo(5));
    results.push(result(6, Kind::Text, Some("rent")));
    let taken = request("q", &results).sources;
    let sends: Vec<(i64, SendAs)> = taken.iter().map(|s| (s.file_id, s.send)).collect();
    assert_eq!(
        sends,
        [
            (1, SendAs::Image),
            (2, SendAs::Image),
            (3, SendAs::Image),
            (4, SendAs::Passages),
            (6, SendAs::Passages),
        ]
    );
}

#[test]
fn what_each_kind_of_file_can_be_sent_as() {
    let text = request("q", &texts(1)).sources.remove(0);
    assert_eq!(send_choices(&text), [SendAs::Passages, SendAs::Whole]);
    let bare_photo = request("q", &[photo(1)]).sources.remove(0);
    assert_eq!(send_choices(&bare_photo), [SendAs::Image]);
    let photo_with_text = request("q", &[result(1, Kind::Image, Some("hi"))])
        .sources
        .remove(0);
    assert_eq!(
        send_choices(&photo_with_text),
        [SendAs::Image, SendAs::Passages]
    );
}

#[test]
fn switching_a_chip_to_whole_file_is_what_is_sent() {
    let mut model = chat(2);
    assert!(model.set_send(1, SendAs::Whole));
    let out = model.start("summarise it").unwrap();
    assert_eq!(out.files[0].send, SendAs::Passages);
    assert_eq!(out.files[1].send, SendAs::Whole);
}

#[test]
fn a_choice_the_file_does_not_have_is_refused() {
    let mut model = chat(1);
    assert!(!model.set_send(0, SendAs::Image));
    assert_eq!(model.sources()[0].send, SendAs::Passages);
}

#[test]
fn no_more_than_three_ticked_pictures_at_once() {
    let mut results: Vec<SearchResult> = (1..=3).map(photo).collect();
    results.push(result(4, Kind::Image, Some("text")));
    let mut model = AskModel::new(signed_in(), request("q", &results));
    assert_eq!(MAX_IMAGES, 3);
    assert!(!model.set_send(3, SendAs::Image));
    // Unticking one makes room.
    model.toggle(0);
    assert!(model.set_send(3, SendAs::Image));
    // And it cannot be ticked back while three are ticked.
    model.toggle(0);
    assert!(!model.sources()[0].ticked);
}

#[test]
fn the_chip_says_briefly_how_its_file_is_sent() {
    let s = Lang::En.strings();
    assert_eq!(send_text(SendAs::Passages, s), s.ask_send_passages);
    assert_eq!(send_text(SendAs::Whole, s), s.ask_send_whole);
    assert_eq!(send_text(SendAs::Image, s), s.ask_send_image);
    for lang in [Lang::En, Lang::Es] {
        let s = lang.strings();
        for text in [s.ask_send_passages, s.ask_send_whole, s.ask_send_image] {
            assert!(!text.is_empty());
        }
    }
}

#[test]
fn a_dropped_or_picked_file_joins_the_chips_ticked() {
    let mut model = chat(2);
    assert_eq!(model.add(info(9, Kind::Pdf)), Added::Yes);
    let last = model.sources().last().unwrap();
    assert_eq!((last.file_id, last.ticked), (9, true));
    assert_eq!(last.send, SendAs::Passages);
    assert_eq!(last.text, "some text");
    assert_eq!(model.add(info(10, Kind::Image)), Added::Yes);
    assert_eq!(model.sources().last().unwrap().send, SendAs::Image);
}

#[test]
fn the_same_file_twice_is_not_added_again() {
    let mut model = chat(2);
    model.toggle(0);
    assert_eq!(model.add(info(1, Kind::Text)), Added::AlreadyThere);
    assert_eq!(model.sources().len(), 2);
    // Adding it again ticks it.
    assert!(model.sources()[0].ticked);
}

#[test]
fn no_more_than_five_chips() {
    let mut model = chat(5);
    assert_eq!(model.add(info(9, Kind::Text)), Added::Full);
    assert_eq!(model.sources().len(), 5);
    model.remove(0);
    assert_eq!(model.add(info(9, Kind::Text)), Added::Yes);
}

#[test]
fn a_chip_can_be_removed_but_not_while_its_question_is_on_its_way() {
    let mut model = chat(3);
    model.remove(1);
    let ids: Vec<i64> = model.sources().iter().map(|s| s.file_id).collect();
    assert_eq!(ids, [1, 3]);
    model.start("q").unwrap();
    model.remove(0);
    assert_eq!(model.sources().len(), 2);
    assert_eq!(model.add(info(9, Kind::Text)), Added::Busy);
}

#[test]
fn a_photo_past_three_pictures_is_added_as_text_when_it_has_some() {
    let results: Vec<SearchResult> = (1..=3).map(photo).collect();
    let mut model = AskModel::new(signed_in(), request("q", &results));
    assert_eq!(model.add(info(4, Kind::Image)), Added::Yes);
    assert_eq!(model.sources()[3].send, SendAs::Passages);
    let mut bare = info(5, Kind::Image);
    bare.preview.clear();
    assert_eq!(model.add(bare), Added::TooManyPictures);
}

#[test]
fn a_picked_result_becomes_a_file_with_its_excerpt_but_never_its_path() {
    let taken = file_info(&result(7, Kind::Text, Some("the tenant shall pay rent")));
    assert_eq!((taken.file_id, taken.file_name.as_str()), (7, "7.bin"));
    assert_eq!(taken.preview, "the tenant shall pay rent");
    // A snippet that only repeats the path says nothing about the file.
    let repeats = file_info(&result(8, Kind::Text, Some("private folder 8 bin")));
    assert_eq!(repeats.preview, "");
    assert_eq!(file_info(&result(9, Kind::Image, None)).preview, "");
}

#[test]
fn only_a_limit_or_a_stranger_is_worth_saying_when_adding() {
    assert_eq!(notice_for(Added::Yes), None);
    assert_eq!(notice_for(Added::AlreadyThere), None);
    assert_eq!(notice_for(Added::Busy), None);
    assert_eq!(notice_for(Added::Full), Some(Notice::Full));
    assert_eq!(
        notice_for(Added::TooManyPictures),
        Some(Notice::TooManyPictures)
    );
    for lang in [Lang::En, Lang::Es] {
        let s = lang.strings();
        for notice in [Notice::Full, Notice::TooManyPictures, Notice::NotIndexed] {
            assert!(!notice_text(notice, s).is_empty());
        }
    }
}

#[test]
fn dropped_files_join_and_the_first_problem_is_reported() {
    let mut model = chat(3);
    let said = model.add_dropped(vec![Some(info(9, Kind::Text)), None]);
    assert_eq!(said, Some(Notice::NotIndexed));
    assert_eq!(model.sources().len(), 4);
    // A full dock is the more useful thing to say than a stranger.
    let said = model.add_dropped(vec![
        Some(info(10, Kind::Text)),
        Some(info(11, Kind::Text)),
        None,
    ]);
    assert_eq!(said, Some(Notice::Full));
    assert_eq!(model.sources().len(), 5);
    assert_eq!(model.add_dropped(vec![Some(info(1, Kind::Text))]), None);
}

#[test]
fn how_much_goes_whole_is_a_rounded_number_and_a_cut_is_said() {
    let s = Lang::En.strings();
    let size = WholeSize {
        chars: 23_412,
        cut: false,
    };
    assert_eq!(whole_note(size, Lang::En, s), "About 23,400 characters");
    let short = WholeSize {
        chars: 380,
        cut: false,
    };
    assert_eq!(whole_note(short, Lang::En, s), "About 380 characters");
    let long = WholeSize {
        chars: 200_000,
        cut: true,
    };
    assert_eq!(whole_note(long, Lang::En, s), s.ask_whole_cut_short);
}

#[test]
fn the_tooltip_explains_the_way_and_warns_of_a_cut() {
    let s = Lang::En.strings();
    assert_eq!(send_tip(SendAs::Passages, None, s), s.ask_send_passages_tip);
    assert_eq!(send_tip(SendAs::Image, None, s), s.ask_send_image_tip);
    let fits = WholeSize {
        chars: 5,
        cut: false,
    };
    assert_eq!(send_tip(SendAs::Whole, Some(fits), s), s.ask_send_whole_tip);
    let long = WholeSize {
        chars: 99_999,
        cut: true,
    };
    assert!(send_tip(SendAs::Whole, Some(long), s).ends_with(s.ask_whole_cut));
}

#[test]
fn a_click_on_the_send_pill_goes_to_the_next_way_and_back() {
    use crate::ask::attach::next_send;
    let mut text = request("q", &texts(1)).sources.remove(0);
    assert_eq!(next_send(&text), Some(SendAs::Whole));
    text.send = SendAs::Whole;
    assert_eq!(next_send(&text), Some(SendAs::Passages));
    let bare_photo = request("q", &[photo(1)]).sources.remove(0);
    assert_eq!(next_send(&bare_photo), None);
}
