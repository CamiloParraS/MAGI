//! Settings › What's searched: the search features.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _};
use gpui_kit::*;
use magi_core::dto::{FeatureStatus, Install};

use super::{Field, SettingsView};
use crate::i18n::Lang;
use crate::settings::{
    FeatureAction, download_error_label, feature_action, images_needed, turn_on,
};
use crate::theme::{Palette, Tone};
use crate::ui::{feature_icon, grouped, note, row_icon, subheading, warn_note};

impl SettingsView {
    pub(super) fn features(&self, p: &Palette, cx: &Context<Self>) -> Div {
        let live = self.live.read(cx);
        let s = live.lang.strings();
        div()
            .child(subheading(s.search_features, p))
            .child(note(p).mb(px(8.)).child(s.features_note))
            .child(grouped(
                p,
                live.features
                    .iter()
                    .map(|f| self.feature_row(f, live.lang, p, cx)),
            ))
            .child(note(p).mt(px(10.)).child(s.features_footnote))
    }

    fn feature_row(&self, f: &FeatureStatus, lang: Lang, p: &Palette, cx: &Context<Self>) -> Div {
        let s = lang.strings();
        let (feature, field) = (f.feature, Field::Feature(f.feature));
        let text = s.feature(feature);
        let progress = |id: &'static str, done: u64, total: u64, label: String| {
            div()
                .mt(px(6.))
                .child(
                    Progress::new((id, feature as usize))
                        .color(p.accent)
                        .value(done as f32 * 100. / total.max(1) as f32),
                )
                .child(note(p).child(label))
        };
        let state = match &f.install {
            // A quiet check and the size in the muted tone: installed is the
            // normal state, not news.
            Install::Installed { size_bytes } => note(p)
                .flex()
                .items_center()
                .gap(px(4.))
                .child(Icon::new(IconName::Check).size(px(12.)).text_color(p.mute))
                .child(s.installed.replace("{size}", &lang.size(*size_bytes)))
                .into_any_element(),
            Install::NotInstalled => note(p)
                .child(
                    s.not_downloaded
                        .replace("{size}", &lang.size(f.download_size)),
                )
                .into_any_element(),
            Install::Downloading { bytes, total } => progress(
                "download",
                *bytes,
                *total,
                s.feature_downloading
                    .replace("{done}", &lang.size(*bytes))
                    .replace("{total}", &lang.size(*total)),
            )
            .into_any_element(),
            Install::Failed { code } => div()
                .mt(px(4.))
                .flex()
                .items_center()
                .gap_2()
                .child(p.badge(s.download_failed, Tone::Err))
                .child(note(p).mt_0().child(download_error_label(*code, s)))
                .into_any_element(),
        };
        let backfill = f.backfill.filter(|b| b.total > b.done).map(|b| {
            progress(
                "backfill",
                b.done,
                b.total,
                lang.plural(&s.feature_updating, b.total - b.done),
            )
        });
        let busy = self.is_busy(field);
        let action = feature_action(f);
        let button = match action {
            FeatureAction::Retry => Some(
                Button::new(("retry", feature as usize))
                    .cursor_pointer()
                    .primary()
                    .small()
                    .label(s.try_again),
            ),
            FeatureAction::Remove => Some(
                Button::new(("remove", feature as usize))
                    .cursor_pointer()
                    .ghost()
                    .small()
                    .label(s.remove_download),
            ),
            FeatureAction::TurnOn | FeatureAction::Nothing => None,
        }
        .map(|b| {
            b.loading(busy).on_click(cx.listener(move |this, _, _, cx| {
                this.run(
                    field,
                    move |host| match action {
                        FeatureAction::Remove => host.features().remove_download(feature),
                        _ => turn_on(host, feature),
                    },
                    cx,
                );
            }))
        });
        // On records the wish and downloads if needed (the size shown is the
        // consent, ADR-0010); off keeps what is installed.
        let switch = Switch::new(("feature", feature as usize))
            .cursor_pointer()
            .checked(f.enabled)
            .disabled(busy)
            .color(p.accent)
            .accessibility_label(text.name)
            .on_click(cx.listener(move |this, &on: &bool, _, cx| {
                this.run(
                    field,
                    move |host| {
                        if on {
                            turn_on(host, feature)
                        } else {
                            host.set_feature_enabled(feature, false)
                        }
                    },
                    cx,
                );
            }));
        self.row(field, p)
            .py(px(10.))
            .child(row_icon(feature_icon(feature), p))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_weight(FontWeight::MEDIUM).child(text.name))
                    .child(note(p).child(text.pitch))
                    .children(
                        (images_needed(&self.indexing.file_types, std::slice::from_ref(f)))
                            .then(|| warn_note(p, s.images_off)),
                    )
                    .child(state)
                    .children(backfill)
                    .children(self.problem(field, s, p)),
            )
            .children(button)
            .child(switch)
    }
}
