use super::*;

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

#[test]
fn system_resolves_any_spanish_locale_to_spanish() {
    use Language::*;
    for tag in ["es", "es-CO", "es-419", "es_ES.UTF-8"] {
        assert_eq!(Lang::resolve(System, Some(tag)), Lang::Es, "{tag}");
    }
    for tag in [Some("en-US"), Some("pt-BR"), Some("estonian"), None] {
        assert_eq!(Lang::resolve(System, tag), Lang::En, "{tag:?}");
    }
    assert_eq!(Lang::resolve(En, Some("es-CO")), Lang::En);
    assert_eq!(Lang::resolve(Es, Some("en-US")), Lang::Es);
}

#[test]
fn numbers_and_plurals_follow_the_language() {
    let s = Lang::En.strings();
    assert_eq!(
        Lang::En.plural(&s.status_indexing, 1),
        "Indexing, 1 file to go"
    );
    assert_eq!(
        Lang::En.plural(&s.status_indexing, 1284),
        "Indexing, 1,284 files to go"
    );
    let s = Lang::Es.strings();
    assert_eq!(
        Lang::Es.plural(&s.status_indexing, 1),
        "Indexando, falta 1 archivo"
    );
    assert_eq!(Lang::Es.number(12345), "12.345");
}

#[test]
fn dates_are_today_or_yesterday_then_absolute_with_the_year_only_when_not_this_year() {
    let today = day(2026, 9, 29);
    assert_eq!(Lang::En.date(today, today), "today");
    assert_eq!(Lang::En.date(day(2026, 9, 28), today), "yesterday");
    assert_eq!(Lang::En.date(day(2026, 9, 17), today), "Sep 17");
    assert_eq!(Lang::En.date(day(2026, 8, 18), today), "Aug 18");
    assert_eq!(Lang::En.date(day(2025, 8, 18), today), "Aug 18, 2025");
    assert_eq!(Lang::Es.date(day(2026, 9, 26), today), "26 sept");
    assert_eq!(Lang::Es.date(day(2026, 8, 18), today), "18 ago");
    assert_eq!(Lang::Es.date(day(2025, 8, 18), today), "18 ago 2025");
    // A file dated in the future (clock skew) shows its date.
    assert_eq!(Lang::En.date(day(2026, 10, 2), today), "Oct 2");
}

#[test]
fn sizes_are_megabytes_or_tenths_of_gigabytes() {
    assert_eq!(Lang::En.size(812_300_000), "812 MB");
    assert_eq!(Lang::En.size(1_240_000_000), "1.2 GB");
    assert_eq!(Lang::Es.size(1_240_000_000), "1,2 GB");
}

#[test]
fn file_sizes_scale_from_bytes_to_gigabytes() {
    assert_eq!(Lang::En.file_size(900), "900 B");
    assert_eq!(Lang::En.file_size(59_400), "59 KB");
    assert_eq!(Lang::En.file_size(2_440_000), "2.4 MB");
    assert_eq!(Lang::En.file_size(23_400_000), "23 MB");
    assert_eq!(Lang::En.file_size(1_240_000_000), "1.2 GB");
    assert_eq!(Lang::Es.file_size(2_440_000), "2,4 MB");
}
