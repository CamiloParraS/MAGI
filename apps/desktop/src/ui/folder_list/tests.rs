use super::{FolderList, RootRow};
use crate::i18n::Lang;
use magi_core::db::roots::Health;
use magi_core::dto::{Guidance, IndexState, IndexStatus, PermissionIssue, RootStatus};

fn root(id: i64, enabled: bool, status: Health) -> RootStatus {
    RootStatus {
        id,
        path: format!("/home/ana/{id}"),
        enabled,
        status,
        indexed: 0,
    }
}

fn status(roots: Vec<RootStatus>) -> IndexStatus {
    IndexStatus {
        state: IndexState::Idle,
        paused_for: None,
        scanned: 0,
        queued: 0,
        indexed: 0,
        skipped: 0,
        errors: 0,
        current_file: None,
        roots,
    }
}

#[test]
fn the_opening_roots_show_until_the_first_status() {
    let list = FolderList::with_opening(Some(vec![root(1, true, Health::Ok)]));
    assert_eq!(list.roots(None).unwrap()[0].id, 1);
    let live = status(vec![root(2, true, Health::Ok)]);
    assert_eq!(list.roots(Some(&live)).unwrap()[0].id, 2);
    assert!(FolderList::with_opening(None).roots(None).is_none());
}

#[test]
fn a_removal_ends_when_the_list_no_longer_has_the_root() {
    let mut list = FolderList::with_opening(None);
    list.start_removing(1);
    list.prune(Some(&status(vec![root(1, true, Health::Ok)])));
    assert!(list.is_removing(1), "the list still has it");
    list.prune(Some(&status(Vec::new())));
    assert!(!list.is_removing(1));
}

#[test]
fn a_failed_removal_frees_only_its_own_row() {
    let mut list = FolderList::with_opening(None);
    list.start_removing(1);
    list.start_removing(2);
    list.removal_failed(1);
    assert!(!list.is_removing(1));
    assert!(list.is_removing(2));
    list.all_removals_failed();
    assert!(!list.is_removing(2));
}

#[test]
fn a_row_links_to_system_settings_only_for_an_enabled_root() {
    let s = Lang::En.strings();
    let list = FolderList::with_opening(None);
    let issue = PermissionIssue {
        root_id: 1,
        path: "/home/ana/1".into(),
        status: Health::PermissionDenied,
        guidance: Guidance::MacFullDiskAccess,
    };
    let issues = [issue];
    let enabled = root(1, true, Health::PermissionDenied);
    let row = RootRow::new(&enabled, None, &issues, &list, s);
    assert!(row.settings_url.is_some());
    assert!(row.has_trouble());
    let paused = root(1, false, Health::PermissionDenied);
    let row = RootRow::new(&paused, None, &issues, &list, s);
    assert!(row.settings_url.is_none());
    assert!(!row.has_trouble(), "paused is not a problem");
}
