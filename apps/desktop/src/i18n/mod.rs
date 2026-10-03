//! UI strings and ICU4X formatting (SPEC.md M6 Localization). One
//! [`Strings`] value per language; a field missing from `es.rs` is a compile
//! error, so Spanish always covers English. The tray uses the same table.

mod en;
mod es;

use chrono::NaiveDate;
use icu_calendar::{Date, Gregorian};
use icu_datetime::FixedCalendarDateTimeFormatter;
use icu_datetime::fieldsets::YMD;
use icu_decimal::DecimalFormatter;
use icu_decimal::input::Decimal;
use icu_locale_core::{Locale, locale};
use icu_plurals::{PluralCategory, PluralRules};
use magi_core::config::Language;
use magi_core::dto::MatchSource;
use magi_core::features::Feature;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Es,
}

/// A count-dependent string; `{n}` is replaced by the formatted count.
pub struct Plural {
    pub one: &'static str,
    pub other: &'static str,
}

/// A search feature as the user sees it (ADR-0010).
pub struct FeatureText {
    pub name: &'static str,
    /// Follows "<name> is off." in the search hint.
    pub pitch: &'static str,
}

pub struct Strings {
    pub placeholder: &'static str,
    /// The window with no query yet.
    pub intro: Plural,
    /// `{q}` is the query.
    pub no_match: &'static str,
    pub search_failed: &'static str,
    pub try_again: &'static str,
    pub still_indexing: Plural,
    /// `{n}` is the page number.
    pub page: &'static str,
    pub open: &'static str,
    pub reveal: &'static str,
    pub copy_path: &'static str,
    /// Replaces [`copy_path`](Self::copy_path) for a moment after a copy.
    pub copied: &'static str,
    pub move_selection: &'static str,
    pub close: &'static str,
    pub today: &'static str,
    pub yesterday: &'static str,
    pub days_ago: Plural,
    pub source_keyword: &'static str,
    pub source_semantic: &'static str,
    pub source_ocr: &'static str,
    pub source_visual: &'static str,
    pub source_qr: &'static str,
    pub source_filename: &'static str,
    pub meaning: FeatureText,
    pub image_text: FeatureText,
    pub image_visual: FeatureText,
    /// "<name> is off."; `{name}` is the feature name.
    pub feature_off: &'static str,
    /// `{size}` is the download size.
    pub turn_on: &'static str,
    /// `{name}` is the feature name, `{done}`/`{total}` are sizes.
    pub downloading: &'static str,
    /// `{name}` is the feature name.
    pub updating: Plural,
    pub dismiss: &'static str,
    pub tray_starting: &'static str,
    pub tray_open: &'static str,
    pub tray_pause: &'static str,
    pub tray_resume: &'static str,
    pub tray_quit: &'static str,
    pub status_paused: &'static str,
    pub status_scanning: &'static str,
    pub status_indexing: Plural,
    pub status_idle: &'static str,
    pub status_idle_errors: Plural,
    pub settings: &'static str,
    pub folders: &'static str,
    pub add_folder: &'static str,
    pub remove: &'static str,
    pub root_watching: &'static str,
    pub root_polling: &'static str,
    pub root_disabled: &'static str,
    pub root_files: Plural,
    pub root_missing: &'static str,
    pub root_denied: &'static str,
    pub no_folders: &'static str,
    /// `{path}` is the folder.
    pub root_not_found: &'static str,
    /// `{path}` is the folder.
    pub root_exists: &'static str,
    /// `{path}` is the folder, `{other}` the root that already covers it.
    pub root_nested: &'static str,
    pub engine_starting: &'static str,
    pub change_failed: &'static str,
    pub what_to_index: &'static str,
    pub file_types: &'static str,
    pub file_types_note: &'static str,
    /// Under the file types when none is checked.
    pub kind_none: &'static str,
    /// Under Images, unchecked while an image feature is on.
    pub images_needed: &'static str,
    /// Under an image feature that is on while Images is unchecked.
    pub images_off: &'static str,
    pub kind_text: &'static str,
    pub kind_code: &'static str,
    pub kind_pdf: &'static str,
    pub kind_office: &'static str,
    pub kind_image: &'static str,
    pub max_size: &'static str,
    pub max_size_note: &'static str,
    pub excludes: &'static str,
    pub excludes_note: &'static str,
    pub save: &'static str,
    /// `{glob}` is the pattern.
    pub invalid_glob: &'static str,
    pub invalid_size: &'static str,
    pub appearance: &'static str,
    pub language: &'static str,
    /// `{lang}` is the language `system` resolves to, in that language.
    pub language_system: &'static str,
    pub window_background: &'static str,
    pub background_match_system: &'static str,
    pub background_always: &'static str,
    pub background_never: &'static str,
    pub see_through_amount: &'static str,
    /// Shown when the amount does nothing (solid window).
    pub see_through_off: &'static str,
    pub more_solid: &'static str,
    pub more_transparent: &'static str,
    pub badge_paused: &'static str,
    pub badge_polling: &'static str,
    pub badge_missing: &'static str,
    pub badge_denied: &'static str,
    pub searched_folders: &'static str,
    pub pause_on_battery: &'static str,
    pub pause_on_battery_note: &'static str,
    pub search_features: &'static str,
    pub features_note: &'static str,
    pub features_footnote: &'static str,
    pub installed: &'static str,
    pub not_downloaded: &'static str,
    pub feature_downloading: &'static str,
    pub download_failed: &'static str,
    pub remove_download: &'static str,
    pub dl_network: &'static str,
    pub dl_checksum: &'static str,
    pub dl_disk_full: &'static str,
    pub dl_denied: &'static str,
    pub dl_write_failed: &'static str,
    /// The sidebar's shorter `search_features`.
    pub search_features_nav: &'static str,
    pub general: &'static str,
    pub shortcut: &'static str,
    pub shortcut_note: &'static str,
    pub results_shown: &'static str,
    pub results_shown_note: &'static str,
    pub theme: &'static str,
    pub theme_light: &'static str,
    pub theme_dark: &'static str,
    pub index: &'static str,
    pub stat_indexed: &'static str,
    pub stat_waiting: &'static str,
    pub stat_skipped: &'static str,
    pub stat_errors: &'static str,
    pub unreadable: &'static str,
    pub all_read: &'static str,
    pub retry_all: &'static str,
    pub read_denied: &'static str,
    pub read_locked: &'static str,
    pub read_failed: &'static str,
    pub read_damaged: &'static str,
    pub read_timed_out: &'static str,
    pub read_crashed: &'static str,
    pub read_embed_failed: &'static str,
    pub read_write_failed: &'static str,
    pub read_other: &'static str,
    pub danger_zone: &'static str,
    pub clear_index: &'static str,
    pub clear_index_note: &'static str,
    pub clear_index_button: &'static str,
    pub clear_confirm_title: &'static str,
    pub cancel: &'static str,
    /// `{n}` is a count.
    pub feature_updating: Plural,
    /// `{n}` is a count.
    pub clear_confirm_body: Plural,
}

impl Strings {
    pub fn source(&self, source: MatchSource) -> &'static str {
        match source {
            MatchSource::Keyword => self.source_keyword,
            MatchSource::Semantic => self.source_semantic,
            MatchSource::Ocr => self.source_ocr,
            MatchSource::Visual => self.source_visual,
            MatchSource::Qr => self.source_qr,
            MatchSource::Filename => self.source_filename,
        }
    }

    pub fn feature(&self, feature: Feature) -> &FeatureText {
        match feature {
            Feature::Meaning => &self.meaning,
            Feature::ImageText => &self.image_text,
            Feature::ImageVisual => &self.image_visual,
        }
    }
}

impl Lang {
    /// `system` resolves any `es-*` locale to Spanish, everything else to
    /// English (ADR-0010).
    pub fn resolve(setting: Language, system: Option<&str>) -> Self {
        match setting {
            Language::En => Self::En,
            Language::Es => Self::Es,
            Language::System => match system {
                Some(tag) if tag == "es" || tag.starts_with("es-") || tag.starts_with("es_") => {
                    Self::Es
                }
                _ => Self::En,
            },
        }
    }

    /// Reads the OS UI language for `system`.
    pub fn current(setting: Language) -> Self {
        Self::resolve(setting, sys_locale::get_locale().as_deref())
    }

    /// The language's name in itself, the same in every table.
    pub fn name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Es => "Español",
        }
    }

    pub fn strings(self) -> &'static Strings {
        match self {
            Self::En => &en::STRINGS,
            Self::Es => &es::STRINGS,
        }
    }

    fn locale(self) -> Locale {
        match self {
            Self::En => locale!("en"),
            Self::Es => locale!("es"),
        }
    }

    pub fn number(self, n: u64) -> String {
        self.decimal(Decimal::from(n))
    }

    fn decimal(self, d: Decimal) -> String {
        match DecimalFormatter::try_new(self.locale().into(), Default::default()) {
            Ok(f) => f.format(&d).to_string(),
            Err(_) => d.to_string(),
        }
    }

    /// Picks `one` or `other` by the language's plural rules and fills `{n}`.
    pub fn plural(self, p: &Plural, n: u64) -> String {
        let one = PluralRules::try_new_cardinal(self.locale().into())
            .map(|r| r.category_for(n) == PluralCategory::One)
            .unwrap_or(n == 1);
        (if one { p.one } else { p.other }).replace("{n}", &self.number(n))
    }

    /// "today", "yesterday", "N days ago" within a month, else the date.
    pub fn date(self, date: NaiveDate, today: NaiveDate) -> String {
        let s = self.strings();
        match (today - date).num_days() {
            0 => s.today.into(),
            1 => s.yesterday.into(),
            days @ 2..31 => self.plural(&s.days_ago, days as u64),
            _ => self.absolute_date(date),
        }
    }

    fn absolute_date(self, date: NaiveDate) -> String {
        use chrono::Datelike as _;
        let formatter = FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new(
            self.locale().into(),
            YMD::medium(),
        );
        let icu_date = Date::try_new_gregorian(date.year(), date.month() as u8, date.day() as u8);
        match (formatter, icu_date) {
            (Ok(f), Ok(d)) => f.format(&d).to_string(),
            _ => date.to_string(),
        }
    }

    /// Download sizes: "812 MB", "1.2 GB" (decimal units, like the OS).
    pub fn size(self, bytes: u64) -> String {
        if bytes >= 1_000_000_000 {
            let mut tenths = Decimal::from((bytes + 50_000_000) / 100_000_000);
            tenths.multiply_pow10(-1);
            format!("{} GB", self.decimal(tenths))
        } else {
            format!("{} MB", self.number((bytes + 500_000) / 1_000_000))
        }
    }
}

#[cfg(test)]
mod tests {
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
        assert_eq!(Lang::Es.plural(&s.days_ago, 1), "hace 1 día");
        assert_eq!(Lang::Es.number(12345), "12.345");
    }

    #[test]
    fn dates_are_relative_within_a_month_then_absolute() {
        let today = day(2026, 9, 29);
        assert_eq!(Lang::En.date(today, today), "today");
        assert_eq!(Lang::En.date(day(2026, 9, 28), today), "yesterday");
        assert_eq!(Lang::En.date(day(2026, 9, 17), today), "12 days ago");
        assert_eq!(Lang::En.date(day(2026, 8, 18), today), "Aug 18, 2026");
        assert_eq!(Lang::Es.date(day(2026, 9, 26), today), "hace 3 días");
        assert_eq!(Lang::Es.date(day(2026, 8, 18), today), "18 ago 2026");
        // A file dated in the future (clock skew) shows its date.
        assert_eq!(Lang::En.date(day(2026, 10, 2), today), "Oct 2, 2026");
    }

    #[test]
    fn sizes_are_megabytes_or_tenths_of_gigabytes() {
        assert_eq!(Lang::En.size(812_300_000), "812 MB");
        assert_eq!(Lang::En.size(1_240_000_000), "1.2 GB");
        assert_eq!(Lang::Es.size(1_240_000_000), "1,2 GB");
    }
}
