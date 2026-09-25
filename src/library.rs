//! The bookmark and history windows.
//!
//! Both lists are read from the profile when the window is opened rather than
//! mirrored anywhere, so a window opened after a page load shows what is
//! actually stored. Choosing an entry hands the URL back to the caller, which
//! opens it in the browsing window, so navigation still goes through the normal
//! address policy.

use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use rbrowse::{bookmarks, history, storage};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LibraryKind {
    Bookmarks,
    History,
}

impl LibraryKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Bookmarks => "Bookmarks",
            Self::History => "History",
        }
    }

    fn empty_hint(self) -> &'static str {
        match self {
            Self::Bookmarks => "No bookmarks yet. Press Ctrl+D on a page.",
            Self::History => "No history yet. Visit a page.",
        }
    }
}

type Entry = (String, String, String);

/// Open the window for `kind`.
///
/// `on_open` is called with the URL a person chose; the window closes itself
/// afterwards. Storage failures surface as an empty list plus a message rather
/// than a panic.
pub fn open(
    kind: LibraryKind,
    store: &Rc<RefCell<storage::SessionStore>>,
    application: &adw::Application,
    on_open: impl Fn(String) + 'static,
) {
    let on_open: Rc<dyn Fn(String)> = Rc::new(on_open);
    // Registering the window with the application keeps it in
    // `application.windows()`, so a second Ctrl+Shift+B raises this one instead
    // of stacking copies, and quitting tears it down.
    let window = adw::Window::builder()
        .application(application)
        .title(format!("R Browse — {}", kind.title()))
        .default_width(560)
        .default_height(520)
        .build();
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    populate(&list, kind, store, &on_open, &window);

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_child(Some(&list));
    scroller.set_vexpand(true);
    scroller.set_margin_top(12);
    scroller.set_margin_bottom(12);
    scroller.set_margin_start(12);
    scroller.set_margin_end(12);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&scroller);
    if kind == LibraryKind::History {
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        toolbar.set_margin_top(6);
        toolbar.set_margin_bottom(6);
        toolbar.set_margin_start(12);
        toolbar.set_margin_end(12);
        let clear = gtk::Button::with_label("Clear history");
        let store_for_clear = store.clone();
        let list_for_clear = list.clone();
        let window_for_clear = window.clone();
        let on_open_for_clear = on_open.clone();
        clear.connect_clicked(move |_| {
            match store_for_clear.borrow().clear_history() {
                Ok(removed) => eprintln!("cleared {removed} history entries"),
                Err(error) => eprintln!("clear history failed: {error}"),
            }
            populate(
                &list_for_clear,
                LibraryKind::History,
                &store_for_clear,
                &on_open_for_clear,
                &window_for_clear,
            );
        });
        toolbar.append(&clear);
        root.append(&toolbar);
    }
    window.set_content(Some(&root));
    window.present();
}

fn populate(
    list: &gtk::ListBox,
    kind: LibraryKind,
    store: &Rc<RefCell<storage::SessionStore>>,
    on_open: &Rc<dyn Fn(String)>,
    window: &adw::Window,
) {
    clear(list);
    let entries = read(kind, store);
    if entries.is_empty() {
        let empty = gtk::Label::new(Some(kind.empty_hint()));
        empty.set_margin_top(24);
        empty.set_margin_bottom(24);
        list.append(&empty);
        return;
    }
    for (label, detail, url) in entries {
        let row = adw::ActionRow::new();
        row.set_title(&label);
        row.set_subtitle(&detail);
        row.set_tooltip_text(Some(&url));
        row.set_activatable(true);
        let target = url.clone();
        let on_open = on_open.clone();
        let window = window.clone();
        row.connect_activated(move |_| {
            on_open(target.clone());
            window.close();
        });
        list.append(&row);
    }
}

fn clear(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn read(kind: LibraryKind, store: &Rc<RefCell<storage::SessionStore>>) -> Vec<Entry> {
    let store = store.borrow();
    match kind {
        LibraryKind::Bookmarks => store
            .bookmarks()
            .unwrap_or_default()
            .into_iter()
            .map(|bookmark: bookmarks::Bookmark| {
                let url = bookmark.url;
                (bookmark.title, url.clone(), url)
            })
            .collect(),
        LibraryKind::History => store
            .history(500)
            .unwrap_or_default()
            .into_iter()
            .map(|entry: history::HistoryEntry| {
                let url = entry.url;
                let visited_at = entry.visited_at;
                (entry.title, format!("{url} · {visited_at}"), url)
            })
            .collect(),
    }
}
