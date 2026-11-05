//! Running changes and saving settings, with their busy, saved and failed
//! states.

use gpui_kit::component::select::SelectEvent;
use gpui_kit::*;
use magi_core::dto::ErrorCode;
use magi_core::host::Host;

use super::choices::{Choice, Dropdown};
use super::{Field, SAVED_FOR, Section, SettingsView};
use crate::app::AppEvent;

impl SettingsView {
    /// Saves `patch(choice)` when a dropdown confirms a choice.
    pub(super) fn save_on_confirm<T>(
        dropdown: &Dropdown<T>,
        field: Field,
        patch: fn(&T) -> serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription
    where
        T: Clone + PartialEq + 'static,
    {
        cx.subscribe_in(
            dropdown,
            window,
            move |this, _, event: &SelectEvent<Vec<Choice<T>>>, window, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    this.save(field, patch(value), window, cx);
                }
            },
        )
    }

    /// Runs a change on the background executor, `field` busy meanwhile,
    /// and keeps its error. Public for the headless tests.
    pub fn run(
        &mut self,
        field: Field,
        job: impl FnOnce(&Host) -> magi_core::Result<()> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.busy.push(field);
        cx.notify();
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            let done = cx
                .background_executor()
                .spawn(async move { job(&host) })
                .await;
            let _ = this.update(cx, |view, cx| {
                if let Some(ix) = view.busy.iter().position(|&f| f == field) {
                    view.busy.remove(ix);
                }
                // A failed removal leaves its row working again.
                if let (Field::Root(id), Err(_)) = (field, &done) {
                    view.removing.retain(|&r| r != id);
                }
                view.error = done.err().map(|error| {
                    tracing::warn!(%error, ?field, "a change failed");
                    (field, ErrorCode::from(&error))
                });
                if view.section == Section::Index {
                    view.load_unreadable(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Saves a settings patch; on success the shell applies the new `ui`, on
    /// failure `field`'s row shows why. Public for the headless tests.
    pub fn save(
        &mut self,
        field: Field,
        patch: serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let host = self.host.clone();
        cx.spawn_in(window, async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn(async move { host.update_settings(&patch) })
                .await;
            let _ = this.update_in(cx, |view, window, cx| {
                match saved {
                    Ok(config) => {
                        view.error = None;
                        if matches!(field, Field::MaxSize | Field::Excludes) {
                            view.flash_saved(field, cx);
                        }
                        view.indexing = config.indexing;
                        view.ui = config.ui.clone();
                        let _ = view.events.try_send(AppEvent::Ui(config.ui));
                    }
                    Err(error) => {
                        tracing::warn!(%error, "a settings change failed");
                        view.error = Some((field, ErrorCode::from(&error)));
                    }
                }
                // The controls show what is saved: the rounded amount, or the
                // old values when the save failed.
                view.sync_controls(window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows `field` as saved for a moment.
    pub(super) fn flash_saved(&mut self, field: Field, cx: &mut Context<Self>) {
        self.saved = Some(field);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVED_FOR).await;
            let _ = this.update(cx, |view, cx| {
                if view.saved == Some(field) {
                    view.saved = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
