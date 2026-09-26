//! GTK4/libadwaita/WebKitGTK shell: window, tab strip, and lazy page realization.

use crate::library::{self, LibraryKind};
use brwsl::{config::Config, downloads, navigation, readability, storage};
use gtk4 as gtk;
use gtk4::glib;
use gtk4::glib::Propagation;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use webkit6::prelude::*;
use webkit6::{Download, LoadEvent, NetworkSession, WebContext, WebView};

/// Handle the shortcuts whose key is a shifted letter.
///
/// A window-level key controller sees the event before the focus widget does, so
/// these fire whether the address entry, the page, or a dialog holds the
/// keyboard. Anything else is passed straight on, so the accelerator table keeps
/// working for every binding that can be expressed as one.
pub(crate) fn install_shifted_letter_shortcuts(
    window: &impl IsA<gtk::Widget>,
    application: &adw::Application,
) {
    let keys: &[(gtk::gdk::Key, &str)] = &[
        (gtk::gdk::Key::t, "restore-closed"),
        (gtk::gdk::Key::n, "new-tab"),
        (gtk::gdk::Key::p, "new-private-tab"),
        (gtk::gdk::Key::b, "bookmarks"),
        (gtk::gdk::Key::r, "readability"),
        (gtk::gdk::Key::i, "import-bookmarks"),
        (gtk::gdk::Key::q, "quit"),
    ];
    // Owned, so the closure does not borrow the application it looks actions up
    // in; an accelerator key can outlive this function by a long way.
    let application = application.clone();
    let controller = gtk::EventControllerKey::new();
    controller.connect_key_pressed(move |_, keyval, _keycode, state| {
        let wanted = gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::SHIFT_MASK;
        if !state.contains(wanted) || state.contains(gtk::gdk::ModifierType::SUPER_MASK) {
            return glib::Propagation::Proceed;
        }
        let Some((_, name)) = keys.iter().find(|(key, _)| *key == keyval) else {
            return glib::Propagation::Proceed;
        };
        // Activated through the application, so a key and a menu item take the
        // same path rather than two copies of the same behaviour.
        application
            .lookup_action(name)
            .and_then(|action| action.downcast_ref::<gtk::gio::SimpleAction>().cloned())
            .map_or(glib::Propagation::Proceed, |action| {
                action.activate(None);
                glib::Propagation::Stop
            })
    });
    window.add_controller(controller);
}

/// Attach download handling to a network session.
///
/// WebKit asks for a destination with a server-supplied name, so the answer is
/// always a path inside the profile's download directory. A name that cannot be
/// made safe is refused rather than guessed at, and the user sees why.
///
/// Nothing is created here. WebKit opens the destination itself, exclusively, so
/// a placeholder file left behind by this function makes that open fail and the
/// download dies with "File exists" - measured, not assumed: reserving by
/// creating a placeholder broke every download while the name policy itself still
/// passed its tests. Names already handed out in this process are remembered so
/// two downloads of the same file in one session cannot collide, and a finished
/// download is made 0600 because WebKit creates it with the process umask.
fn watch_downloads(session: &NetworkSession, download_dir: PathBuf) {
    let dir = download_dir.clone();
    let handed_out: Rc<RefCell<HashSet<PathBuf>>> = Rc::new(RefCell::new(HashSet::new()));
    session.connect_download_started(move |_, download| {
        let dir = dir.clone();
        let handed_out = Rc::clone(&handed_out);
        watch_one_download(download, &dir, handed_out);
    });
}

fn watch_one_download(
    download: &Download,
    download_dir: &std::path::Path,
    handed_out: Rc<RefCell<HashSet<PathBuf>>>,
) {
    let dir = download_dir.to_path_buf();
    // The suggested name arrives with the destination request rather than from
    // the response, which is not available yet at this point in WebKit 6.0.
    {
        // One reference registers the handler, a second is moved into it: a
        // closure that captured the value it is registered on would not
        // outlive the call.
        let registrar = download.clone();
        let owner = download.clone();
        registrar.connect_decide_destination(move |_, suggested| {
            // Choose rather than merely check the name, so two downloads of the
            // same file in this session cannot be handed the same path.
            let taken = Rc::clone(&handed_out);
            let lookup_dir = dir.clone();
            match downloads::reserve(&dir, suggested, move |name| {
                taken.borrow().contains(&lookup_dir.join(name))
            }) {
                Ok(path) => {
                    handed_out.borrow_mut().insert(path.clone());
                    owner.set_destination(&path.to_string_lossy());
                    true
                }
                // Refusing is better than writing outside the profile or
                // inventing a name the server did not ask for.
                Err(reason) => {
                    eprintln!("refused a download: {reason}");
                    false
                }
            }
        });
    }
    let for_created = download.clone();
    for_created.connect_created_destination(move |_, path| {
        println!("download started: {path}");
    });
    let for_failure = download.clone();
    for_failure.connect_failed(move |_, error| {
        eprintln!("download failed: {error}");
    });
    let for_finished = download.clone();
    for_finished.connect_finished(move |download| {
        // The file exists by now, and this is the only moment its mode can be
        // tightened before a person opens it.
        match download.destination().map(PathBuf::from) {
            Some(path) => {
                if let Err(error) = downloads::restrict_mode(&path) {
                    eprintln!("could not restrict {}: {error}", path.display());
                }
            }
            None => eprintln!("a finished download reported no destination"),
        }
    });
}

pub const APPLICATION_ID: &str = "io.github.brwsl.Brwsl";

type PageRef = Rc<PageState>;
type NetworkHandle = Rc<NetworkSession>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TabMode {
    Normal,
    Private,
}

impl TabMode {
    fn title(self, number: usize) -> String {
        match self {
            Self::Normal => format!("Tab {number}"),
            Self::Private => format!("Private tab {number}"),
        }
    }
}

/// Resources shared by every tab in one window.
#[derive(Clone)]
struct Browser {
    application: adw::Application,
    /// Weak so the application-level actions cannot keep a closed window alive.
    window: glib::WeakRef<adw::ApplicationWindow>,
    context: Rc<WebContext>,
    normal_network: NetworkHandle,
    private_network: NetworkHandle,
    search_endpoint: Option<String>,
    store: Rc<RefCell<storage::SessionStore>>,
    download_dir: PathBuf,
}

struct PageState {
    content: gtk::Box,
    placeholder: gtk::Label,
    status: gtk::Label,
    url: RefCell<String>,
    title: RefCell<String>,
    mode: TabMode,
    page: RefCell<Option<adw::TabPage>>,
    network: NetworkHandle,
    context: Rc<WebContext>,
    view: RefCell<Option<WebView>>,
    /// Whether this tab is showing the readability stylesheet.
    readable_on: Cell<bool>,
    /// Weak window handle, so dialogs parent to the window without keeping it
    /// alive.
    window: glib::WeakRef<adw::ApplicationWindow>,
    /// Weak tab-view handle, so a background tab can tell it is in the back and
    /// leave the window title to the tab in front.
    tab_view: glib::WeakRef<adw::TabView>,
    /// Per-tab zoom level, kept here so switching tabs restores it.
    zoom: Cell<f64>,
    /// Per-tab content manager; the sheet is added to and removed from it.
    readable_manager: webkit6::UserContentManager,
    readable_sheet: RefCell<Option<webkit6::UserStyleSheet>>,
    /// Profile store, shared with the window so a finished load can be recorded
    /// without reaching back into the tab list.
    store: Rc<RefCell<storage::SessionStore>>,
}

/// The window's shared state.
///
/// Toolbar buttons, tab signals and keyboard actions all operate on the same
/// tab list, so they share one handle instead of cloning a separate `Rc` per
/// callback. Every method is fallible-free at the call site: storage errors are
/// reported in the affected tab's status line instead of panicking.
#[derive(Clone)]
struct Shell {
    states: Rc<RefCell<Vec<PageRef>>>,
    tab_view: adw::TabView,
    store: Rc<RefCell<storage::SessionStore>>,
    browser: Browser,
    /// Recently closed normal tabs, newest last.
    closed: Rc<RefCell<std::collections::VecDeque<(String, String)>>>,
    /// The window's own controls, which every action in the shell acts through.
    chrome: Chrome,
}

/// How many closed tabs can be reopened.
const CLOSED_TAB_MEMORY: usize = 16;

/// One keyboard action: its name, its accelerators, and what it does.
type ActionBinding = (&'static str, &'static [&'static str], fn(&Shell));

impl Shell {
    fn selected(&self) -> Option<PageRef> {
        let page = self.tab_view.selected_page()?;
        find_state(&self.states, &page)
    }

    fn note(&self, state: &PageRef, message: &str) {
        state.status.set_text(message);
    }

    fn add_tab(&self, mode: TabMode, url: &str) {
        let number = self.states.borrow().len() + 1;
        let (page, state) = create_page(
            &self.tab_view,
            &self.browser,
            url.to_string(),
            mode,
            number,
            None,
        );
        self.states.borrow_mut().push(state.clone());
        self.tab_view.set_selected_page(&page);
        realize_page(&state, &self.chrome);
    }

    /// Open a URL in the selected tab, reporting a rejection in its status line.
    fn navigate_selected(&self, input: &str) {
        let Some(state) = self.selected() else {
            return;
        };
        if let Err(error) = navigate_state(
            &state,
            input,
            self.browser.search_endpoint.as_deref(),
            &self.chrome.address,
            &self.chrome.suggestions,
        ) {
            self.note(&state, &error);
        }
    }

    /// Toggle the bookmark for the selected tab and reflect the result.
    fn toggle_bookmark(&self) {
        let Some(state) = self.selected() else {
            return;
        };
        toggle_bookmark(&state, &self.browser.store, &self.chrome.bookmark);
    }

    /// Reflect the bookmark state of a tab after a page load.
    fn refresh_bookmark(&self, state: &PageRef) {
        let url = current_state_url(state);
        let bookmarked = if url == "about:blank" {
            false
        } else {
            self.store.borrow().is_bookmarked(&url).unwrap_or(false)
        };
        set_bookmark_icon(&self.chrome.bookmark, bookmarked);
    }

    /// Bring the window's own controls in line with the tab in front.
    ///
    /// The field, the percentage, the star and the reading mark belong to the
    /// window, and each of them shows the tab someone is looking at, so they are
    /// written whenever the front tab changes and not only when a control is used.
    fn sync_chrome(&self, state: &PageRef) {
        self.chrome
            .suggestions
            .write(&self.chrome.address, &current_state_url(state));
        let percent = (state.zoom.get() * 100.0).round() as i64;
        self.chrome.zoom_label.set_text(&format!("{percent}%"));
        set_readability_icon(&self.chrome.readable, state.readable_on.get());
        self.refresh_bookmark(state);
    }

    fn close_selected(&self) {
        if let Some(page) = self.tab_view.selected_page() {
            self.tab_view.close_page(&page);
        }
    }

    /// Move to the next or previous tab, wrapping at either end.
    ///
    /// AdwTabView's own `select_next_page` and `select_previous_page` stop at the
    /// ends, which was measured rather than assumed: pressing next on the last tab
    /// left the saved session byte-for-byte unchanged. A shortcut named "next tab"
    /// that does nothing on the last tab is a dead shortcut, so the wrap is done
    /// here against the page list.
    fn select_relative(&self, next: bool) {
        let count = self.states.borrow().len();
        if count < 2 {
            return;
        }
        let Some(current) = self.tab_view.selected_page() else {
            return;
        };
        let index = {
            let states = self.states.borrow();
            states
                .iter()
                .position(|state| state.page.borrow().as_ref() == Some(&current))
        };
        let Some(index) = index else {
            return;
        };
        let target = if next {
            (index + 1) % count
        } else {
            (index + count - 1) % count
        };
        let page = self
            .states
            .borrow()
            .get(target)
            .and_then(|state| state.page.borrow().as_ref().map(|page| page.clone()));
        if let Some(page) = page {
            self.tab_view.set_selected_page(&page);
        }
    }

    fn save(&self) {
        save_sessions(&self.states, &self.tab_view, &self.store);
    }

    /// Open the bookmark or history window.
    ///
    /// The lists are read on demand rather than mirrored in the shell, so a
    /// window opened after a page load shows the current database.
    fn open_library(&self, kind: LibraryKind) {
        let shell = self.clone();
        library::open(
            kind,
            &self.browser.store,
            &self.browser.application,
            move |url| {
                shell.add_tab(TabMode::Normal, &url);
            },
        );
    }

    /// Change the selected tab's zoom, clamped to a usable range.
    fn zoom_by(&self, steps: f64) {
        let Some(state) = self.selected() else {
            return;
        };
        set_zoom(&state, state.zoom.get() + steps, &self.chrome.zoom_label);
    }

    fn reset_zoom(&self) {
        let Some(state) = self.selected() else {
            return;
        };
        set_zoom(&state, 1.0, &self.chrome.zoom_label);
    }

    /// Open the print dialog for the selected tab, parented to the window.
    fn print(&self) {
        let Some(state) = self.selected() else {
            return;
        };
        let Some(view) = state.view.borrow().clone() else {
            self.note(&state, "This tab has nothing to print yet");
            return;
        };
        let operation = webkit6::PrintOperation::new(&view);
        let parent = self.browser.window.upgrade();
        operation.run_dialog(parent.as_ref());
    }

    /// Reopen the most recently closed normal tab.
    fn restore_closed(&self) {
        let Some((url, title)) = self.closed.borrow_mut().pop_back() else {
            return;
        };
        let number = self.states.borrow().len() + 1;
        let (page, state) = create_page(
            &self.tab_view,
            &self.browser,
            url,
            TabMode::Normal,
            number,
            Some(title),
        );
        self.states.borrow_mut().push(state.clone());
        self.tab_view.set_selected_page(&page);
        realize_page(&state, &self.chrome);
    }

    fn remember_closed(&self, url: String, title: String) {
        // A small ring buffer: enough to undo a few mistakes, bounded so a long
        // session cannot grow it without limit.
        if url == "about:blank" {
            return;
        }
        let mut closed = self.closed.borrow_mut();
        closed.push_back((url, title));
        while closed.len() > CLOSED_TAB_MEMORY {
            closed.pop_front();
        }
    }

    /// Erase the profile's own history and WebKit's site data.
    ///
    /// Both are cleared: the SQLite history rows, and the cookies, local
    /// storage and caches WebKit holds for this profile's network session.
    fn clear_browsing_data(&self) {
        let history_removed = match self.store.borrow().clear_history() {
            Ok(removed) => removed,
            Err(error) => {
                self.report(&format!("could not clear history: {error}"));
                return;
            }
        };
        let Some(manager) = self.browser.normal_network.website_data_manager() else {
            self.report("no website data manager for this profile");
            return;
        };
        // The callback runs off the main thread, so it carries a plain string
        // rather than the session object.
        let location = manager
            .base_data_directory()
            .map(|dir| dir.to_string())
            .unwrap_or_else(|| "this profile".to_string());
        manager.clear(
            webkit6::WebsiteDataTypes::ALL,
            glib::TimeSpan::from_seconds(0),
            None::<&gtk::gio::Cancellable>,
            move |result| match result {
                Ok(()) => println!("cleared site data under {location}"),
                Err(error) => eprintln!("clearing site data failed: {error}"),
            },
        );
        // Clearing site data leaves the cookie file alone, because the cookie
        // store is a file of its own rather than part of the data manager's
        // directories. Without this, the next launch would sign the person back
        // in to exactly what they just erased.
        if let Some(dir) = manager.base_data_directory() {
            let _ = std::fs::remove_file(PathBuf::from(dir.to_string()).join("cookies.txt"));
        }
        self.report(&format!(
            "cleared {history_removed} history entries and site data"
        ));
    }

    /// Put a message in the selected tab's status line, if a tab is selected.
    fn report(&self, message: &str) {
        match self.selected() {
            Some(state) => state.status.set_text(message),
            None => eprintln!("{message}"),
        }
    }

    /// Ask for another browser's bookmark file and merge it in.
    ///
    /// The picker is a `FileDialog` rather than a hand-rolled path entry, so it
    /// gets the platform's own file chooser - a portal dialog where the session
    /// has one - and the person can see the file before choosing it. The import
    /// itself is the same code the `--import-bookmarks` command runs, so the two
    /// cannot drift apart.
    fn choose_bookmark_file(&self) {
        let Some(window) = self.browser.window.upgrade() else {
            self.report("the window is gone, so there is nothing to import into");
            return;
        };
        let dialog = gtk::FileDialog::new();
        dialog.set_title("Import bookmarks");
        let store = self.store.clone();
        let report_target = self.browser.window.clone();
        dialog.open(
            Some(&window),
            None::<&gtk::gio::Cancellable>,
            move |chosen| {
                let file = match chosen {
                    Ok(file) => file,
                    Err(error) => {
                        // A dismissed dialog is not a failure and says nothing.
                        let dismissed = matches!(
                            error.kind::<gtk::DialogError>(),
                            Some(gtk::DialogError::Dismissed | gtk::DialogError::Cancelled)
                        );
                        if !dismissed {
                            let message = error.to_string();
                            log_event(&format!("bookmark import could not start: {message}"));
                            tell(
                                report_target.upgrade().as_ref(),
                                "Bookmarks could not be imported",
                                &message,
                            );
                        }
                        return;
                    }
                };
                let Some(path) = file.path() else {
                    tell(
                        report_target.upgrade().as_ref(),
                        "Bookmarks could not be imported",
                        "that file has no location on disk, so it cannot be read",
                    );
                    return;
                };
                let entries = match brwsl::import::read_file(&path) {
                    Ok(entries) => entries,
                    Err(error) => {
                        let message = error.to_string();
                        log_event(&format!("bookmark import failed: {message}"));
                        tell(
                            report_target.upgrade().as_ref(),
                            "Bookmarks could not be imported",
                            &message,
                        );
                        return;
                    }
                };
                let report = brwsl::import::apply(&store.borrow(), &entries);
                let summary = report.summary.to_string();
                log_event(&format!("bookmark import: {summary}"));
                tell(
                    report_target.upgrade().as_ref(),
                    "Bookmarks imported",
                    &summary,
                );
            },
        );
    }

    fn open_bookmarks(&self) {
        self.open_library(LibraryKind::Bookmarks);
    }

    fn open_history(&self) {
        self.open_library(LibraryKind::History);
    }

    fn reload_selected(&self) {
        let Some(state) = self.selected() else {
            return;
        };
        let Some(view) = state.view.borrow().clone() else {
            self.navigate_selected("about:blank");
            return;
        };
        match view.uri() {
            Some(uri) => view.load_uri(uri.as_str()),
            None => self.note(&state, "This tab has no address to reload."),
        }
    }

    fn focus_address(&self) {
        self.chrome.suggestions.mark_active();
        self.chrome.address.grab_focus();
        self.chrome.address.select_region(0, -1);
    }

    fn toggle_readability(&self) {
        if let Some(state) = self.selected() {
            toggle_readability(&state, &self.chrome.readable);
        }
    }

    fn copy_address(&self) {
        let Some(state) = self.selected() else {
            return;
        };
        let url = current_state_url(&state);
        self.chrome.address.clipboard().set_text(&url);
        self.note(&state, "Address copied");
    }
}

/// Add or remove the bookmark for a tab, reporting the outcome in its status
/// line. Shared by the toolbar button and the Ctrl+D action so both behave
/// identically, including the refusals for private and blank tabs.
fn toggle_bookmark(
    state: &PageRef,
    store: &Rc<RefCell<storage::SessionStore>>,
    button: &gtk::Button,
) {
    if state.mode == TabMode::Private {
        state.status.set_text("Private tabs are not bookmarked");
        return;
    }
    let url = current_state_url(state);
    if url == "about:blank" {
        state
            .status
            .set_text("There is nothing to bookmark on a blank tab");
        return;
    }
    let title = state.title.borrow().clone();
    let outcome = {
        let store = store.borrow();
        match store.is_bookmarked(&url) {
            Ok(true) => store.remove_bookmark(&url).map(|_| false),
            Ok(false) => store.add_bookmark(&url, &title).map(|_| true),
            Err(error) => Err(error),
        }
    };
    match outcome {
        Ok(true) => {
            set_bookmark_icon(button, true);
            state.status.set_text("Bookmarked");
        }
        Ok(false) => {
            set_bookmark_icon(button, false);
            state.status.set_text("Bookmark removed");
        }
        Err(error) => state.status.set_text(&format!("bookmark failed: {error}")),
    }
}

/// Apply or remove the readability pass for a tab.
///
/// The stylesheet is attached to a per-tab content manager, so no script runs
/// in the page and no other tab is affected.
fn toggle_readability(state: &PageRef, button: &gtk::Button) {
    let next = !state.readable_on.get();
    state.readable_on.set(next);
    set_readability_icon(button, next);
    apply_readability_sheet(state);
    state.status.set_text(if next {
        "Readability pass on"
    } else {
        "Readability pass off"
    });
}

/// Reconcile the tab's manager with the readability flag.
///
/// Idempotent, so it is safe to call both from the toggle and when a view is
/// realized: the sheet is attached exactly once and removed exactly once.
fn apply_readability_sheet(state: &PageRef) {
    let wanted = state.readable_on.get();
    let attached = state.readable_sheet.borrow().is_some();
    match (wanted, attached) {
        (true, false) => {
            let sheet = readability::sheet();
            state.readable_manager.add_style_sheet(&sheet);
            *state.readable_sheet.borrow_mut() = Some(sheet);
        }
        (false, true) => {
            if let Some(sheet) = state.readable_sheet.borrow_mut().take() {
                state.readable_manager.remove_style_sheet(&sheet);
            }
        }
        // Already in the requested state.
        (true, true) | (false, false) => {}
    }
}

fn set_readability_icon(button: &gtk::Button, on: bool) {
    button.set_icon_name(if on {
        "view-reading-mode-checked-symbolic"
    } else {
        "view-reading-mode-symbolic"
    });
    button.set_tooltip_text(Some(if on {
        "Turn off the readability pass (Ctrl+Shift+R)"
    } else {
        "Apply the readability pass (Ctrl+Shift+R)"
    }));
}

/// The window's own controls, and the one field every tab shares.
///
/// Each of these acts on whichever tab is in front, which is why they belong to
/// the window rather than to a tab: the field shows that tab's address, the
/// back button moves that tab's history, the star reflects that tab's bookmark
/// and the percentage is that tab's zoom.
/// How many past addresses the field is willing to offer.
///
/// The history table holds five thousand rows, and offering all of them is not
/// helpful: eight or so good ones is a list a person can read, and a thousand is
/// a list to scroll past. The most recent few hundred are the ones anybody is
/// likely to want anyway.
const SUGGESTION_LIMIT: i64 = 500;

/// How many rows are shown at once.
const SUGGESTION_ROWS: usize = 8;

/// What a picked address should do: go there.
type Navigate = Rc<dyn Fn(&str)>;

/// What the field is offering: where the addresses come from, what is on show,
/// and which row the keyboard is on.
///
/// The matching is done here rather than by a completion widget, because the
/// obvious one only matches the start of a string, and a person types "youtu"
/// where the address is "https://www.youtube.com/". The candidates are read in
/// one go, at most every half minute, so typing costs no queries per letter.
struct Suggestions {
    /// Every address the profile has been to, most recent first, deduplicated.
    candidates: RefCell<Vec<String>>,
    /// Indexes into `candidates` for the rows on show, in the order shown.
    shown: RefCell<Vec<usize>>,
    /// The row the keyboard is on, if any. None means the typed text stands on
    /// its own, which is what Return navigates.
    cursor: Cell<Option<usize>>,
    /// The text the shell last wrote into the field, if it was the shell.
    ///
    /// The field is written by the shell as well as typed into - a tab
    /// switching, a page finishing a load, a navigation, a suggestion being
    /// taken - and none of those is somebody asking for suggestions, so rows that
    /// opened on every page load would be unbearable. Neither the focus nor
    /// the keyboard can tell the two apart here: on Wayland the field reports no
    /// focus while it is being filled, and the characters arrive as text rather
    /// than as key presses. So the shell marks its own writes here, and a change
    /// that matches the mark is the shell talking to itself.
    written: Cell<Option<String>>,
    /// Whether the rows have been asked for.
    ///
    /// Typing does not bring them up. It narrows them, so that asking afterwards
    /// shows the addresses that match what has been typed - and nothing else
    /// appears under the page until the field is asked, with the arrow key or the
    /// control at the end of it. A field that offers itself on every letter is a
    /// field that is talking over the page.
    asked: Cell<bool>,
    /// Whether the field is the thing being typed into right now.
    ///
    /// The arrow keys that walk the rows and the Escape that puts them away are
    /// taken from the field, and only while it is the subject: with this false
    /// every key passes on, so a page keeps its own arrow keys and its Escape. It
    /// is set from the field's own changes and cleared as soon as the field stops
    /// being the subject - when the shell writes it, and when the rows go away.
    active: Cell<bool>,
    /// What to do with an address a person picked. Set by the window once it
    /// exists, because the shell that navigates is built after the chrome is.
    navigate: RefCell<Option<Navigate>>,
    /// This, for the closures the rows carry.
    me: RefCell<Weak<Suggestions>>,
    /// The rows, in a band of the window between the field and the page.
    rows: gtk::Box,
    /// The label of each row on show, so the one the keyboard is on can be marked.
    labels: RefCell<Vec<gtk::Label>>,
}

impl Suggestions {
    /// Read the addresses this profile has been to.
    ///
    /// Called when the field is asked and not while it is being typed into, which
    /// is the whole of the "no database work per letter" promise: one deliberate
    /// question, one query. A throttle on top of that was tried and removed,
    /// because it hid exactly the address somebody had just visited - they would
    /// ask for suggestions and their own last page would not be among them.
    ///
    /// A read that fails leaves the previous list alone rather than emptying it
    /// under a person who is halfway through choosing.
    fn refresh(&self, store: &Rc<RefCell<storage::SessionStore>>) {
        let Ok(entries) = store.borrow().history(SUGGESTION_LIMIT) else {
            return;
        };
        let mut seen = HashSet::new();
        let urls: Vec<String> = entries
            .into_iter()
            .map(|entry| entry.url)
            .filter(|url| seen.insert(url.clone()))
            .collect();
        // Reading them is worth a line of its own: a field that offers nothing
        // and a field that was never given anything look identical from outside.
        log_event(&format!(
            "read {} addresses from this profile's history",
            urls.len()
        ));
        *self.candidates.borrow_mut() = urls;
    }

    /// Work out what the typed text offers, and show the rows only if they were
    /// asked for.
    fn update(&self, typed: &str) {
        // The mark is always cleared, so a shell write that produced no change of
        // its own cannot swallow a later keystroke. The one thing it can swallow
        // is a person retyping exactly what the shell just put there, which costs
        // one missing set of rows.
        let written = self.written.take();
        if needle_is_shells(typed, written.as_deref()) {
            self.hide();
            return;
        }
        self.active.set(true);
        let typed = typed.trim();
        let needle = typed.to_lowercase();
        let candidates = self.candidates.borrow();
        let shown: Vec<usize> = candidates
            .iter()
            .enumerate()
            .filter(|(_, url)| contains_ci(url.as_str(), typed))
            .map(|(index, _)| index)
            .take(SUGGESTION_ROWS)
            .collect();
        drop(candidates);
        if shown.is_empty() {
            // A line here is worth having when a person asked and got nothing,
            // because that is otherwise indistinguishable from a field that was
            // never asked. It is not worth having when the shell wrote the
            // field, which is the common case and would turn typing into a run
            // of stderr writes.
            if self.asked.get() {
                log_event(&format!(
                    "nothing in this profile's history matches {needle:?}"
                ));
            }
            self.asked.set(false);
            self.rows.set_visible(false);
            self.set_cursor(None);
            return;
        }
        while let Some(row) = self.rows.first_child() {
            self.rows.remove(&row);
        }
        let mut labels = Vec::new();
        for index in shown.iter() {
            let Some(url) = self.candidates.borrow().get(*index).cloned() else {
                continue;
            };
            let label = gtk::Label::new(Some(&url));
            label.set_xalign(0.0);
            label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            // A click takes the address, and takes nothing else: a click
            // controller on a label leaves the keyboard where it was, where a
            // list row would have taken it.
            let clicked = gtk::GestureClick::new();
            let url_for_click = url.clone();
            let suggestions_for_click = self.handle();
            clicked.connect_released(move |_, _, _, _| {
                if let Some(suggestions) = suggestions_for_click.clone() {
                    suggestions.choose(&url_for_click);
                }
            });
            label.add_controller(clicked);
            self.rows.append(&label);
            labels.push(label);
        }
        *self.labels.borrow_mut() = labels;
        let count = shown.len();
        *self.shown.borrow_mut() = shown;
        self.set_cursor(None);
        if self.asked.get() {
            self.rows.set_visible(true);
            // Worth a line in the log: rows that are not where they should be, or
            // that never came up when they were asked for, look identical from
            // outside the window.
            log_event(&format!("showing {count} addresses for {needle:?}"));
        }
    }

    /// The field is the thing being typed into: the cursor has been put in it.
    ///
    /// This is what makes the arrow key ask rather than scroll the page, and it
    /// is set by `focus_address` as well as by typing: putting the cursor in the
    /// field is putting it in the field, whether or not a letter follows.
    fn mark_active(&self) {
        self.active.set(true);
    }

    /// Bring the rows up for whatever the field holds, having been asked.
    fn ask(&self, typed: &str, store: &Rc<RefCell<storage::SessionStore>>) {
        self.asked.set(true);
        self.refresh(store);
        // Asking is a person acting, so it is never the shell's own write, whatever
        // the field happens to be holding. The mark is dropped rather than obeyed:
        // a page has just loaded, the field holds its address, and the mark is
        // still on it, so asking without dropping it would be taken for the shell
        // talking to itself and nothing would come up.
        self.written.set(None);
        self.update(typed);
    }

    /// Move the keyboard on the list of rows, or off it.
    fn set_cursor(&self, cursor: Option<usize>) {
        self.cursor.set(cursor);
        // The row the keyboard is on is marked in the text, with the same arrow a
        // list would have highlighted, because a plain label cannot be selected -
        // and cannot take the keyboard on its way either.
        let labels = self.labels.borrow();
        for (position, label) in labels.iter().enumerate() {
            let shown = self.shown.borrow();
            let Some(index) = shown.get(position) else {
                continue;
            };
            let Some(url) = self.candidates.borrow().get(*index).cloned() else {
                continue;
            };
            let text = if cursor == Some(position) {
                format!("MARK {url}")
            } else {
                format!("  {url}")
            };
            label.set_text(&text.replace("MARK", "›"));
        }
    }

    /// Move the keyboard down or up the list, stopping at the ends.
    fn move_cursor(&self, steps: isize) {
        let count = self.shown.borrow().len();
        if count == 0 {
            return;
        }
        let next = match self.cursor.get() {
            None if steps > 0 => 0,
            None => (count - 1) as isize,
            Some(current) => (current as isize + steps).clamp(0, count as isize - 1),
        };
        self.set_cursor(Some(next as usize));
    }

    /// The address the keyboard is on, if it is on one.
    fn cursor_url(&self) -> Option<String> {
        let cursor = self.cursor.get()?;
        let shown = self.shown.borrow();
        let index = *shown.get(cursor)?;
        self.candidates.borrow().get(index).cloned()
    }

    /// A handle on this, for the closures a row carries.
    fn handle(&self) -> Option<Rc<Suggestions>> {
        self.me.borrow().upgrade()
    }

    /// Set what a picked address should do: navigate, in this app.
    fn set_navigator(&self, navigate: Navigate) {
        *self.navigate.borrow_mut() = Some(navigate);
    }

    /// Take the address a person picked and go there.
    fn choose(&self, url: &str) {
        self.hide();
        let go = self.navigate.borrow().clone();
        if let Some(go) = go {
            go(url);
        }
    }

    /// Take the rows away and forget where the keyboard was.
    ///
    /// The rows are in the window rather than floating above it on purpose. A
    /// popover is a surface of its own, and on this desktop one takes the
    /// keyboard with it: the field stopped receiving the letters that were typed
    /// after the rows appeared, and Return went to the popover instead of to the
    /// address. A band cannot do that, and while a person is typing the page
    /// moving down a little is the honest way to show they are being offered
    /// something.
    fn hide(&self) {
        if self.rows.is_visible() {
            log_event("put the addresses this profile has been to away");
        }
        self.asked.set(false);
        self.rows.set_visible(false);
        self.set_cursor(None);
        self.active.set(false);
    }

    /// Write the field from the shell, offering nothing for it.
    fn write(&self, entry: &gtk::Entry, text: &str) {
        self.written.set(Some(text.to_string()));
        self.asked.set(false);
        self.rows.set_visible(false);
        self.set_cursor(None);
        self.active.set(false);
        entry.set_text(text);
    }
}

/// Whether this change to the field was the shell's own write.
fn needle_is_shells(typed: &str, written: Option<&str>) -> bool {
    written == Some(typed)
}

/// Offer the addresses this profile has already been to, from the field.
///
/// Returns the state, so the field's own Return can ask whether the keyboard is
/// on a row before it navigates what was typed.
///
/// Nothing here leaves the machine: the candidates are the profile's own history,
/// and the search path still only goes out when Return is pressed. The addresses
/// are read at most once every half minute and matched in memory, so typing a
/// URL costs no database work per letter.
/// Case-insensitive substring test that folds as it compares.
///
/// The candidate list is walked on every keystroke, so lowercasing each address
/// would allocate once per address in the profile's history per key. URLs are
/// ASCII in practice, so folding byte by byte is allocation-free and sufficient.
fn contains_ci(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let needle = needle.as_bytes();
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

fn offer_history(address: &gtk::Entry) -> Rc<Suggestions> {
    // A box of labels, not a list: a list claims the keyboard the moment a row is
    // selected, and a keyboard that arrives somewhere else is a keyboard this
    // browser has lost. Labels cannot be selected, so the row the keyboard is on
    // is marked in the text instead, and the keyboard stays in the field.
    let rows_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rows_box.set_margin_start(6);
    rows_box.set_margin_end(6);

    let suggestions = Rc::new(Suggestions {
        candidates: RefCell::new(Vec::new()),
        shown: RefCell::new(Vec::new()),
        cursor: Cell::new(None),
        written: Cell::new(None),
        asked: Cell::new(false),
        active: Cell::new(false),
        navigate: RefCell::new(None),
        me: RefCell::new(Weak::new()),
        rows: rows_box.clone(),
        labels: RefCell::new(Vec::new()),
    });
    suggestions.me.replace(Rc::downgrade(&suggestions));
    rows_box.set_visible(false);

    let for_typed = suggestions.clone();
    address.connect_changed(move |entry| {
        for_typed.update(&entry.text());
    });

    suggestions
}

#[derive(Clone)]
struct Chrome {
    address: gtk::Entry,
    new_tab: gtk::Button,
    private_tab: gtk::Button,
    back: gtk::Button,
    forward: gtk::Button,
    reload: gtk::Button,
    zoom_out: gtk::Button,
    zoom_label: gtk::Label,
    zoom_in: gtk::Button,
    bookmark: gtk::Button,
    readable: gtk::Button,
    /// The control that asks for the addresses the field can offer.
    ask_button: gtk::Button,
    /// What the field is offering, which writes to the field go through.
    suggestions: Rc<Suggestions>,
}

/// Build the window chrome: the title bar with the tab strip inside it, and the
/// field row below.
///
/// Two bands rather than four, because the tab strip is the title bar instead of
/// a bar stacked under one, and the row that holds the address field holds
/// nothing else. The names are the Adwaita symbolic ones, so they follow the
/// system icon theme instead of shipping artwork.
///
/// Icons, not words. A text "Reload" button sat in a row of symbolic buttons
/// and looked like a different kind of control; every button here is an icon
/// with a tooltip, so the meaning lives in the tooltip and the row reads as one
/// thing.
fn build_chrome(tab_view: &adw::TabView) -> (adw::HeaderBar, gtk::Box, Chrome) {
    let tab_bar = adw::TabBar::new();
    tab_bar.set_view(Some(tab_view));
    // Always shown: a strip that appears and disappears with the number of tabs
    // moves the whole bar every time a tab opens, and a bar that moves is a bar
    // nobody can aim at.
    tab_bar.set_autohide(false);
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&tab_bar));

    let new_tab = gtk::Button::from_icon_name("tab-new-symbolic");
    new_tab.set_tooltip_text(Some("New tab"));
    let private_tab = gtk::Button::from_icon_name("view-conceal-symbolic");
    private_tab.set_tooltip_text(Some("New private tab"));
    let back = gtk::Button::from_icon_name("go-previous-symbolic");
    back.set_tooltip_text(Some("Back"));
    let forward = gtk::Button::from_icon_name("go-next-symbolic");
    forward.set_tooltip_text(Some("Forward"));
    let reload = gtk::Button::from_icon_name("view-refresh-symbolic");
    reload.set_tooltip_text(Some("Reload (Ctrl+R)"));
    for button in [&new_tab, &private_tab, &back, &forward, &reload] {
        header.pack_start(button);
    }

    let zoom_out = gtk::Button::from_icon_name("zoom-out-symbolic");
    zoom_out.set_tooltip_text(Some("Zoom out (Ctrl+-)"));
    // The percentage is a label, not a button: Ctrl+0 is the way back to 100%,
    // and the label keeps the current level visible at all times.
    let zoom_label = gtk::Label::new(Some("100%"));
    zoom_label.set_width_chars(5);
    zoom_label.set_xalign(0.5);
    zoom_label.set_sensitive(false);
    zoom_label.set_tooltip_text(Some("Reset zoom (Ctrl+0)"));
    let zoom_in = gtk::Button::from_icon_name("zoom-in-symbolic");
    zoom_in.set_tooltip_text(Some("Zoom in (Ctrl++)"));
    let bookmark = gtk::Button::from_icon_name("non-starred-symbolic");
    let readable = gtk::Button::from_icon_name("view-reading-mode-symbolic");
    for button in [&zoom_out, &zoom_in, &bookmark, &readable] {
        header.pack_end(button);
    }
    header.pack_end(&zoom_label);

    let address = gtk::Entry::new();
    address.set_placeholder_text(Some("Address (use search: for a configured search)"));
    address.set_width_chars(48);
    address.set_hexpand(true);
    // The field spans the window, so the address would otherwise sit against the
    // left edge with most of the band empty beside it. Named in full because
    // both the entry and the editable trait offer set_alignment.
    gtk::prelude::EditableExt::set_alignment(&address, 0.5);
    let field = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    field.set_margin_top(6);
    field.set_margin_end(6);
    field.set_margin_bottom(6);
    field.set_margin_start(6);
    field.append(&address);
    // The control that asks for the addresses and puts them away again, so being
    // able to see them does not depend on knowing the arrow key.
    let ask_button = gtk::Button::from_icon_name("open-menu-symbolic");
    ask_button.set_tooltip_text(Some("Show the addresses this profile has been to (Down)"));
    ask_button.set_focusable(false);
    field.append(&ask_button);

    let suggestions = offer_history(&address);
    let chrome = Chrome {
        address,
        new_tab,
        private_tab,
        back,
        forward,
        reload,
        zoom_out,
        zoom_label,
        zoom_in,
        bookmark,
        readable,
        ask_button,
        suggestions,
    };
    (header, field, chrome)
}

/// Report a page-level event on stderr as well as in the status line.
///
/// A browser that fails quietly is hard to tell apart from one that is merely
/// idle, and the status line is visible to nobody but the person at the screen.
/// These are the events worth having in a terminal or a bug report, so they are
/// the ones written down.
fn log_event(message: &str) {
    eprintln!("brwsl: {message}");
}

/// Name a web-process death in words a person can act on.
///
/// The enum's own variant names are developer-facing, so a crash, a page that
/// outgrew its memory limit and an intentional teardown are told apart here.
fn describe_termination(reason: webkit6::WebProcessTerminationReason) -> &'static str {
    use webkit6::WebProcessTerminationReason as Reason;
    match reason {
        Reason::Crashed => "the page crashed",
        Reason::ExceededMemoryLimit => "the page used too much memory",
        Reason::TerminatedByApi => "the page was closed by the browser",
        _ => "the page was stopped for an unknown reason",
    }
}

/// Ask before granting a site a device capability.
///
/// The default is refusal, and it is refusal by construction rather than by
/// setting: "Allow" is an ordinary button the keyboard cannot reach, so no stray
/// Return can turn a camera on, and the only thing the default key and Escape
/// can do is deny.
///
/// This started as an `AdwAlertDialog` with `set_default_response("deny")` and a
/// suggested "Allow", which sounds sufficient and is not. Measured on this
/// machine, one Return in about eight still answered "allow" - libadwaita gives
/// the affirmative response initial focus while the dialog's first frame is
/// being mapped, and the default response did not always win that race - and the
/// page received a real stream. A property that loses a race is not a safety
/// property, so the prompt is a window where the grant is not a response at all.
fn ask_for_permission(state: &PageRef, request: &webkit6::PermissionRequest) {
    // The request object exposes no URI, so the origin shown is the tab's own
    // current address, which is what the person is looking at anyway.
    let origin = current_state_url(state);
    let origin = if origin == "about:blank" {
        "this page".to_string()
    } else {
        origin
    };
    // The prompt is a window of this application, and it reports itself to the
    // compositor under the application's class. That class is what decides
    // whether the keyboard can reach the prompt: a plain window reports the
    // binary's name instead, and anything that aims keys at this application -
    // the end-to-end harness among them, which refuses to type into a window it
    // does not recognise - then treats the prompt as somebody else's, and
    // Return and Escape go nowhere.
    let prompt = state.window.upgrade().and_then(|parent| {
        parent
            .application()
            .map(|application| (parent, application))
    });
    let Some((parent, application)) = prompt else {
        // No window, or no application behind it: there is nobody to ask, so the
        // request is refused rather than left waiting on a prompt no one sees.
        request.deny();
        log_event("refused a capability: the window it belongs to is gone");
        return;
    };
    let dialog = adw::ApplicationWindow::builder()
        .application(&application)
        .title("Allow this capability?")
        .modal(true)
        .default_width(420)
        .build();
    dialog.set_transient_for(Some(&parent));

    let deny = gtk::Button::with_label("Deny");
    let allow = gtk::Button::with_label("Allow");
    // Unfocusable, so Return, Tab and the space bar have nothing to land on but
    // Deny. Granting a device takes a click on this one button.
    allow.set_focusable(false);
    deny.set_focusable(true);

    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new("Allow this capability?", "")));
    header.pack_start(&deny);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    let message = gtk::Label::new(Some(&format!(
        "{origin} is asking for a capability such as the camera or microphone."
    )));
    message.set_xalign(0.0);
    message.set_wrap(true);
    body.append(&message);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    actions.set_halign(gtk::Align::End);
    actions.set_margin_top(12);
    actions.append(&allow);
    body.append(&actions);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&header);
    root.append(&body);
    dialog.set_content(Some(&root));
    // Return and Escape both land here: Return on the default widget, Escape by
    // way of the close request below.
    dialog.set_default_widget(Some(&deny));

    // The Deny button and dismissing the window are two paths to the same end,
    // and one Return takes both: Deny is the default widget, and closing is
    // what a dismissed default does. So whichever arrives first answers the
    // request and the other finds it already answered, rather than denying
    // twice and reporting it twice.
    let answered = Rc::new(Cell::new(false));

    // Escape refuses, stated rather than assumed. The prompt is a window, and a
    // window is not closed by a keystroke the way a dialog is, so the refusal it
    // stands for is wired here - the same key controller the window's shifted
    // letter shortcuts use.
    let controller = gtk::EventControllerKey::new();
    let request_for_escape = request.clone();
    let for_log_escape = origin.clone();
    let window_for_escape = dialog.clone();
    let answered_for_escape = answered.clone();
    controller.connect_key_pressed(move |_, keyval, _, _| {
        if keyval != gtk::gdk::Key::Escape {
            return glib::Propagation::Proceed;
        }
        if !answered_for_escape.replace(true) {
            request_for_escape.deny();
            log_event(&format!("refused a capability for {for_log_escape}"));
        }
        window_for_escape.close();
        glib::Propagation::Stop
    });
    dialog.add_controller(controller);

    let request_for_deny = request.clone();
    let for_log_deny = origin.clone();
    let window_for_deny = dialog.clone();
    let answered_for_deny = answered.clone();
    deny.connect_clicked(move |_| {
        if answered_for_deny.replace(true) {
            return;
        }
        request_for_deny.deny();
        log_event(&format!("refused a capability for {for_log_deny}"));
        window_for_deny.close();
    });

    let request_for_close = request.clone();
    let for_log_close = origin.clone();
    let answered_for_close = answered.clone();
    dialog.connect_close_request(move |_| {
        // Dismissing the prompt is a refusal, never a grant: the request is
        // still outstanding here, and a WebKit permission request has no state
        // that survives being dropped.
        if !answered_for_close.replace(true) {
            request_for_close.deny();
            log_event(&format!("refused a capability for {for_log_close}"));
        }
        Propagation::Proceed
    });

    let request_for_allow = request.clone();
    let for_log_allow = origin.clone();
    let window_for_allow = dialog.clone();
    let answered_for_allow = answered.clone();
    allow.connect_clicked(move |_| {
        if answered_for_allow.replace(true) {
            return;
        }
        request_for_allow.allow();
        log_event(&format!("granted a capability to {for_log_allow}"));
        window_for_allow.close();
    });

    log_event(&format!(
        "a page at {origin} is asking for a capability; the answer defaults to deny"
    ));
    dialog.present();
}

fn set_bookmark_icon(button: &gtk::Button, bookmarked: bool) {
    button.set_icon_name(if bookmarked {
        "starred-symbolic"
    } else {
        "non-starred-symbolic"
    });
    button.set_tooltip_text(Some(if bookmarked {
        "Remove bookmark (Ctrl+D)"
    } else {
        "Bookmark this page (Ctrl+D)"
    }));
}

pub fn run(config: Config) -> Result<(), String> {
    adw::init().map_err(|error| format!("initialize GTK/libadwaita: {error}"))?;
    let application = adw::Application::builder()
        .application_id(APPLICATION_ID)
        .flags(gtk::gio::ApplicationFlags::empty())
        .build();
    let activate_config = config.clone();
    // A window that cannot be built is a startup failure, so the error is kept
    // and returned after the main loop stops instead of exiting 0.
    let startup_error: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let error_slot = startup_error.clone();
    application.connect_activate(move |application| {
        if !application.windows().is_empty() {
            return;
        }
        if let Err(error) = build_window(application, activate_config.clone()) {
            eprintln!("open Brwsl window: {error}");
            *error_slot.borrow_mut() = Some(error);
            application.quit();
        }
    });
    // Brwsl parses its own flags, so the GTK option parser only ever sees
    // the program name; unknown-flag handling and exit codes stay ours.
    let program = std::env::args()
        .next()
        .unwrap_or_else(|| "brwsl".to_string());
    application.run_with_args(&[program]);
    match startup_error.borrow_mut().take() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn build_window(application: &adw::Application, config: Config) -> Result<(), String> {
    config.ensure_profile_dirs()?;
    let store = Rc::new(RefCell::new(storage::SessionStore::open(
        config.database_path(),
    )?));
    let restored = if config.restore_session {
        store.borrow().load()?
    } else {
        Vec::new()
    };
    let tab_view = adw::TabView::new();
    // The tab strip shares one band with the window's controls instead of having
    // a band of its own under a second one. The bar cannot be the window's
    // title bar on this stack: gtk_window_set_titlebar is refused for an
    // AdwApplicationWindow, and libadwaita 0.9.2 exposes no AdwWindow
    // titlebar property of its own, so the bar is the first band of the content
    // rather than the frame. Two bands in all - this one, then the field row -
    // where there were four.
    let (header, field, chrome) = build_chrome(&tab_view);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&header);
    root.append(&field);
    root.append(&chrome.suggestions.rows);
    root.append(&tab_view);
    tab_view.set_vexpand(true);

    let window = adw::ApplicationWindow::builder()
        .application(application)
        .title("Brwsl")
        .default_width(1100)
        .default_height(760)
        .content(&root)
        .build();

    // A weak handle, so the application-level actions can parent a dialog to
    // the window without keeping it alive after it closes.
    let window_ref: glib::WeakRef<adw::ApplicationWindow> = glib::WeakRef::new();
    window_ref.set(Some(&window));

    // One explicit WebKit context for the whole window. Owning it here keeps
    // the process pool, the network sessions, and any future per-window
    // settings in one place instead of implicit defaults.
    // WebKit does not keep cookies by itself. A session that has not been told
    // where its cookie store lives holds them in the network process's memory,
    // which is why a profile directory, a data directory and a clean shutdown
    // all looked right while every restart signed the person out again. The
    // store is a file inside the profile's own data directory, in the Netscape
    // text format WebKit reads and writes, and the private session is
    // deliberately left without one: a private tab must leave nothing behind.
    let data_dir = data_path(&config)?.to_string();
    let normal_network = NetworkSession::new(Some(&data_dir), Some(cache_path(&config)?));
    if let Some(cookies) = normal_network.cookie_manager() {
        let store = PathBuf::from(&data_dir).join("cookies.txt");
        // The store is created empty first, because WebKit makes the file with
        // whatever the umask allows and the cookie jar is the one place in the
        // profile where a session token sits in a file. A placeholder is safe
        // here where it is not for a download: WebKit is about to be handed
        // this path and writes to the file it finds.
        if !store.exists()
            && let Err(error) = std::fs::File::create(&store).map_err(|error| error.to_string())
        {
            eprintln!("preparing the cookie store failed: {error}");
        }
        if let Err(error) = downloads::restrict_mode(&store) {
            eprintln!("restricting the cookie store failed: {error}");
        }
        cookies.set_persistent_storage(
            store.to_str().unwrap_or_default(),
            webkit6::CookiePersistentStorage::Text,
        );
    }

    let browser = Browser {
        application: application.clone(),
        window: window_ref,
        context: Rc::new(WebContext::new()),
        store: Rc::clone(&store),
        normal_network: Rc::new(normal_network),
        private_network: Rc::new(NetworkSession::new_ephemeral()),
        search_endpoint: config.search_endpoint.clone(),
        download_dir: config.download_dir().to_path_buf(),
    };

    // A private tab's download must not outlive the session, so it goes to a
    // temporary directory that is removed when the window closes.
    let private_download_dir = private_download_dir();
    watch_downloads(&browser.normal_network, browser.download_dir.clone());
    watch_downloads(&browser.private_network, private_download_dir.clone());

    let states: Rc<RefCell<Vec<PageRef>>> = Rc::new(RefCell::new(Vec::new()));
    let mut selected_page = None;
    for tab in restored {
        let number = states.borrow().len() + 1;
        let (page, state) = create_page(
            &tab_view,
            &browser,
            tab.url,
            TabMode::Normal,
            number,
            Some(tab.title),
        );
        states.borrow_mut().push(state);
        if tab.selected {
            selected_page = Some(page);
        }
    }
    if states.borrow().is_empty() {
        let url = navigation::normalize_input_with_search(
            &config.start_url,
            config.search_endpoint.as_deref(),
        )?;
        let (page, state) = create_page(&tab_view, &browser, url, TabMode::Normal, 1, None);
        states.borrow_mut().push(state);
        selected_page = Some(page);
    } else if config.start_url_given {
        // A saved session *and* a URL means someone clicked a link, and a link
        // has to open. Restoring the session instead of the URL is how a launch
        // could look like it did nothing: the old tabs came back and the page
        // that was asked for never appeared. So the session is kept and the URL
        // opens on top of it, which is what the other browsers do.
        let url = navigation::normalize_input_with_search(
            &config.start_url,
            config.search_endpoint.as_deref(),
        )?;
        let number = states.borrow().len() + 1;
        let (page, state) = create_page(&tab_view, &browser, url, TabMode::Normal, number, None);
        states.borrow_mut().push(state);
        selected_page = Some(page);
    }
    if let Some(page) = selected_page.clone()
        && let Some(state) = find_state(&states, &page)
    {
        tab_view.set_selected_page(&page);
        realize_page(&state, &chrome);
    }

    let shell = Shell {
        states: Rc::clone(&states),
        tab_view: tab_view.clone(),
        store: Rc::clone(&store),
        browser: browser.clone(),
        closed: Rc::new(RefCell::new(std::collections::VecDeque::new())),
        chrome: chrome.clone(),
    };

    // Tab selection realizes the page and brings the window's own controls in
    // line with the tab now in front.
    let shell_for_select = shell.clone();
    let chrome_for_select = chrome.clone();
    tab_view.connect_selected_page_notify(move |view| {
        let Some(page) = view.selected_page() else {
            return;
        };
        if let Some(state) = find_state(&shell_for_select.states, &page) {
            realize_page(&state, &chrome_for_select);
            shell_for_select.sync_chrome(&state);
        }
        // The selected tab is part of the session, so it is written down when it
        // changes rather than only when a tab closes. Without this, a browser
        // that ended without a clean shutdown reopened on the wrong tab, and the
        // selection was the only part of the session that could be stale.
        save_sessions(
            &shell_for_select.states,
            &shell_for_select.tab_view,
            &shell_for_select.store,
        );
    });

    let shell_for_new = shell.clone();
    let new_tab = chrome.new_tab.clone();
    new_tab.connect_clicked(move |_| shell_for_new.add_tab(TabMode::Normal, "about:blank"));

    let shell_for_private = shell.clone();
    let private_tab = chrome.private_tab.clone();
    private_tab
        .connect_clicked(move |_| shell_for_private.add_tab(TabMode::Private, "about:blank"));

    // Every other control in the chrome goes through the same methods the
    // keyboard actions use, so a click and a shortcut cannot drift apart.
    let shell_for_back = shell.clone();
    let back = chrome.back.clone();
    back.connect_clicked(move |_| {
        if let Some(state) = shell_for_back.selected() {
            history_step(&state, HistoryAction::Back);
        }
    });
    let shell_for_forward = shell.clone();
    let forward = chrome.forward.clone();
    forward.connect_clicked(move |_| {
        if let Some(state) = shell_for_forward.selected() {
            history_step(&state, HistoryAction::Forward);
        }
    });
    let shell_for_reload = shell.clone();
    let reload = chrome.reload.clone();
    reload.connect_clicked(move |_| shell_for_reload.reload_selected());
    let shell_for_address = shell.clone();
    let address = chrome.address.clone();
    let suggestions_for_return = chrome.suggestions.clone();
    address.connect_activate(move |entry| {
        // Return goes to the row the keyboard has been walked onto, and to what
        // was typed otherwise, so finishing a suggestion and typing an address
        // both do the obvious thing.
        let target = suggestions_for_return.cursor_url();
        suggestions_for_return.hide();
        match target {
            Some(url) => shell_for_address.navigate_selected(&url),
            None => shell_for_address.navigate_selected(&entry.text()),
        }
    });
    let shell_for_zoom_out = shell.clone();
    let zoom_out = chrome.zoom_out.clone();
    zoom_out.connect_clicked(move |_| shell_for_zoom_out.zoom_by(-ZOOM_STEP));
    let shell_for_zoom_in = shell.clone();
    let zoom_in = chrome.zoom_in.clone();
    zoom_in.connect_clicked(move |_| shell_for_zoom_in.zoom_by(ZOOM_STEP));
    let shell_for_star = shell.clone();
    let bookmark = chrome.bookmark.clone();
    bookmark.connect_clicked(move |_| shell_for_star.toggle_bookmark());
    let shell_for_read = shell.clone();
    let readable = chrome.readable.clone();
    readable.connect_clicked(move |_| shell_for_read.toggle_readability());

    let shell_for_close = shell.clone();
    let chrome_for_close = chrome.clone();
    let endpoint_for_close = config.search_endpoint.clone();
    tab_view.connect_close_page(move |_, page| {
        let position = {
            let states = shell_for_close.states.borrow();
            states
                .iter()
                .position(|state| state.page.borrow().as_ref() == Some(page))
        };
        let Some(position) = position else {
            return Propagation::Proceed;
        };
        let last = {
            let states = shell_for_close.states.borrow();
            (states.len() == 1)
                .then(|| states.get(position).cloned())
                .flatten()
        };
        if let Some(state) = last {
            // Never leave the window without a page: the last tab resets to
            // about:blank instead of closing.
            let _ = navigate_state(
                &state,
                "about:blank",
                endpoint_for_close.as_deref(),
                &chrome_for_close.address,
                &chrome_for_close.suggestions,
            );
            shell_for_close.save();
            return Propagation::Stop;
        }
        if let Some(state) = shell_for_close.states.borrow().get(position).cloned() {
            let title = state.title.borrow().clone();
            shell_for_close.remember_closed(current_state_url(&state), title);
        }
        shell_for_close.states.borrow_mut().remove(position);
        // Renumber so the strip stays dense ("Tab 1", "Tab 2") after a close.
        renumber_tabs(&shell_for_close.states);
        shell_for_close.save();
        Propagation::Proceed
    });

    let shell_for_window = shell.clone();
    window.connect_close_request(move |_| {
        shell_for_window.save();
        let _ = std::fs::remove_dir_all(&private_download_dir);
        Propagation::Proceed
    });

    for (label, session) in [
        ("normal", &browser.normal_network),
        ("private", &browser.private_network),
    ] {
        let manager = session.website_data_manager();
        eprintln!(
            "DIAG {label}: session.is_ephemeral={} manager.is_ephemeral={:?} base_data={:?}",
            session.is_ephemeral(),
            manager.as_ref().map(|m| m.is_ephemeral()),
            manager
                .as_ref()
                .and_then(|m| m.base_data_directory())
                .map(|value| value.to_string()),
        );
    }

    install_actions(application, &shell);
    install_shifted_letter_shortcuts(&window, application);
    // The field, not the window, watches the keyboard while rows are showing.
    // Key events arrive at the focus widget first, and the rows are a sibling of
    // the field: once one is selected the list claims Return for itself and
    // activates the row instead of letting the address be typed, so the keys are
    // taken here, where they are seen before the list ever is. While nothing is
    // showing every key passes straight on, so the page keeps its own arrow keys,
    // its Return and its Escape.
    // A row that is clicked goes the same way as a row the keyboard is on: the
    // window tells the suggestions how to navigate, because the shell that does
    // it is built after the chrome is.
    let shell_for_rows = shell.clone();
    chrome.suggestions.set_navigator(Rc::new(move |url: &str| {
        shell_for_rows.navigate_selected(url);
    }));

    // The control at the end of the field: it asks for the addresses, and it puts
    // them away when they are already up.
    let shell_for_ask = shell.clone();
    let ask_button = chrome.ask_button.clone();
    ask_button.connect_clicked(move |_| {
        let suggestions = &shell_for_ask.chrome.suggestions;
        if suggestions.rows.is_visible() {
            suggestions.hide();
        } else {
            let typed = shell_for_ask.chrome.address.text();
            let store_for_ask = shell_for_ask.browser.store.clone();
            suggestions.ask(&typed, &store_for_ask);
        }
    });

    let suggestion_keys = gtk::EventControllerKey::new();
    let for_suggestion_keys = chrome.suggestions.clone();
    let field_for_keys = chrome.address.clone();
    let store_for_keys = store.clone();
    suggestion_keys.connect_key_pressed(move |_, keyval, _, _| {
        let suggestions = &for_suggestion_keys;
        if !suggestions.rows.is_visible() && !suggestions.active.get() {
            return glib::Propagation::Proceed;
        }
        match keyval {
            gtk::gdk::Key::Down => {
                if suggestions.rows.is_visible() {
                    suggestions.move_cursor(1);
                } else {
                    // Asked for: the rows come up for what has been typed, with the
                    // first one already under the keyboard.
                    let typed = field_for_keys.text();
                    suggestions.ask(&typed, &store_for_keys);
                    suggestions.set_cursor(Some(0));
                }
                glib::Propagation::Stop
            }
            gtk::gdk::Key::Up => {
                suggestions.move_cursor(-1);
                glib::Propagation::Stop
            }
            gtk::gdk::Key::Escape => {
                suggestions.hide();
                glib::Propagation::Stop
            }
            // Return is left alone. The field's own activate signal is what takes
            // an address, and it already asks whether the keyboard is on a row
            // before it reads the text, so finishing a suggestion and typing an
            // address both work through the one path the rest of the keyboard
            // uses.
            _ => glib::Propagation::Proceed,
        }
    });
    chrome.address.add_controller(suggestion_keys);
    window.present();
    // The first tab is selected and realized *before* the selected-page handler
    // is connected, so nothing has put that tab into the window's controls yet:
    // without this the field opens empty and waits for a load to fill it, which
    // a page that never finishes would never do.
    if let Some(state) = shell.selected() {
        shell.sync_chrome(&state);
    }
    // Write the session down once, here. The first tab is selected *before* the
    // selected-page handler is connected, so nothing else saves it: a browser
    // that was opened, used and then ended without a tab switch, a tab close or a
    // clean quit left nothing to restore. It also means the session on disk
    // always matches the window that is actually open, rather than catching up
    // at some later, unrelated moment.
    shell.save();
    Ok(())
}

/// Register the window's keyboard actions.
///
/// These are `GAction`s on the application rather than key-press handlers, so
/// GTK owns the accelerator table and the bindings work wherever focus is.
/// Zoom is clamped to a range a person can actually read, and the label shows
/// the percentage so the state is never a mystery.
const ZOOM_STEP: f64 = 0.1;
const ZOOM_MIN: f64 = 0.5;
const ZOOM_MAX: f64 = 3.0;

fn set_zoom(state: &PageRef, level: f64, label: &gtk::Label) {
    let level = level.clamp(ZOOM_MIN, ZOOM_MAX);
    state.zoom.set(level);
    let percent = (level * 100.0).round() as i64;
    label.set_text(&format!("{percent}%"));
    if let Some(view) = state.view.borrow().clone() {
        view.set_zoom_level(level);
    }
}

/// Say something in a dialog, because it is the answer to a question the person
/// just asked and the status line is not where they are looking.
fn tell(parent: Option<&impl IsA<gtk::Widget>>, heading: &str, body: &str) {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_response("close", "Close");
    dialog.set_default_response(Some("close"));
    dialog.set_close_response("close");
    dialog.present(parent);
}

/// Quit through the application, so the windows close the way they do when a
/// person closes them.
///
/// This is on the application rather than only on a button, so the session is
/// saved on the way out and so a script can restart the browser. It matters
/// because a process killed from outside never reaches the close handler, and
/// that handler is what writes the session down.
fn quit_application(shell: &Shell) {
    shell.save();
    shell.browser.application.quit();
}

fn install_actions(application: &adw::Application, shell: &Shell) {
    // The closures live as long as the application, so they hold their own
    // handle rather than borrowing the caller's.
    let shell = shell.clone();
    let actions: &[ActionBinding] = &[
        ("new-tab", &["<Primary>t"], |shell| {
            shell.add_tab(TabMode::Normal, "about:blank")
        }),
        ("new-private-tab", &[], |shell| {
            shell.add_tab(TabMode::Private, "about:blank")
        }),
        ("close-tab", &["<Primary>w"], Shell::close_selected),
        (
            "next-tab",
            &["<Primary>Tab", "<Primary>Page_Down"],
            |shell| shell.select_relative(true),
        ),
        (
            "previous-tab",
            &["<Primary><Shift>Tab", "<Primary>Page_Up"],
            |shell| shell.select_relative(false),
        ),
        ("reload", &["<Primary>r", "F5"], Shell::reload_selected),
        ("bookmark", &["<Primary>d"], Shell::toggle_bookmark),
        ("bookmarks", &[], Shell::open_bookmarks),
        (
            "zoom-in",
            &["<Primary>plus", "<Primary>equal", "F5"],
            |shell| shell.zoom_by(ZOOM_STEP),
        ),
        ("zoom-out", &["<Primary>minus"], |shell| {
            shell.zoom_by(-ZOOM_STEP)
        }),
        ("zoom-reset", &["<Primary>0"], Shell::reset_zoom),
        ("print", &["<Primary>p"], Shell::print),
        (
            "clear-browsing-data",
            &["<Primary><Shift>Delete"],
            Shell::clear_browsing_data,
        ),
        ("restore-closed", &[], Shell::restore_closed),
        ("history", &["<Primary>h"], Shell::open_history),
        ("import-bookmarks", &[], Shell::choose_bookmark_file),
        ("quit", &[], quit_application),
        ("readability", &[], Shell::toggle_readability),
        ("focus-address", &["<Primary>l"], Shell::focus_address),
        ("copy-page-address", &["<Primary>c"], Shell::copy_address),
    ];
    for (name, keys, handler) in actions {
        let action = gtk::gio::SimpleAction::new(name, None);
        let handler = *handler;
        let shell = shell.clone();
        action.connect_activate(move |_, _| handler(&shell));
        application.add_action(&action);
        // The action lives in the application's own group, so the accelerator
        // must name that group. Registering "win.<name>" here would silently
        // match nothing and leave every shortcut dead.
        debug_assert!(application.has_action(name));
        for key in keys.iter().copied() {
            debug_assert!(
                !shifted_letter_is_lowercase(key),
                "{name} is bound to {key}, whose shifted letter is lowercase; \
                 holding shift makes the key event report the uppercase keyval and \
                 GTK compares keyvals exactly, so the shortcut could never fire"
            );
        }
        application.set_accels_for_action(&format!("app.{name}"), keys);
    }
}

/// Whether an accelerator spells its shifted letter in lowercase.
///
/// `Ctrl+Shift+T` cannot be written as an accelerator at all in this GTK, and the
/// reason is not guesswork. The parser folds every `<Primary><Shift>X` to the
/// *lowercase* keyval with SHIFT in the modifier mask, and matching then compares
/// keyval and modifiers exactly - so the accel asks for `t` with Ctrl+Shift while
/// a real Ctrl+Shift+T arrives as `T` with Ctrl+Shift. Probed against GTK
/// directly: `<Primary><Shift>t` and `<Primary><Shift>T` both parse to keyval
/// 0x74 with Ctrl+Shift, and `<Primary>T` parses to 0x54 with Ctrl only, so the
/// keyval or the mask is always wrong. The shortcut is dead while the action
/// behind it works perfectly, and five bindings were dead for that reason.
///
/// The shifted-letter shortcuts are therefore handled by
/// [`install_shifted_letter_shortcuts`], and this guard keeps the spelling that
/// cannot work out of the accelerator table.
///
/// Only the final key is judged: `<Primary>t` is correct as written, and so are
/// `<Primary><Shift>Tab` and `<Primary><Shift>Delete`, whose final keys are not
/// letters at all.
fn shifted_letter_is_lowercase(key: &str) -> bool {
    if !key.contains("<Shift>") {
        return false;
    }
    match key.rsplit('>').next() {
        Some(last) => {
            last.len() == 1 && last.chars().all(|character| character.is_ascii_lowercase())
        }
        None => false,
    }
}

/// A temporary directory for downloads started in a private tab.
///
/// It lives outside the profile, so a private download is never persisted with
/// the browser's data, and it is removed when the window closes.
fn private_download_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("brwsl-private-{}", std::process::id()));
    // Best effort: if it cannot be created the download handler will refuse the
    // write rather than fall back to somewhere less private.
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn data_path(config: &Config) -> Result<&str, String> {
    config
        .data_dir()
        .to_str()
        .ok_or_else(|| "data directory is not valid UTF-8".to_string())
}

fn cache_path(config: &Config) -> Result<&str, String> {
    config
        .cache_dir()
        .to_str()
        .ok_or_else(|| "cache directory is not valid UTF-8".to_string())
}

/// Whether a tab is the one in front.
///
/// The window's own controls show the selected tab: its address in the field,
/// its bookmark in the star, its reading pass in the mark, its zoom in the
/// percentage. A tab that finishes loading in the background must not write any
/// of them, or a quiet background load would move the controls out from under
/// the page someone is reading.
fn in_front(state: &PageState) -> bool {
    state
        .tab_view
        .upgrade()
        .and_then(|view| view.selected_page())
        .zip(state.page.borrow().clone())
        .is_some_and(|(selected, mine)| selected == mine)
}

fn find_state(states: &Rc<RefCell<Vec<PageRef>>>, page: &adw::TabPage) -> Option<PageRef> {
    states
        .borrow()
        .iter()
        .find(|state| state.page.borrow().as_ref() == Some(page))
        .cloned()
}

/// Re-apply positional tab titles so the strip does not keep gaps after a
/// close. Only a tab still showing its own positional label is touched, so a
/// page whose title happens to read "Tab 3" is never overwritten.
fn renumber_tabs(states: &Rc<RefCell<Vec<PageRef>>>) {
    for (index, state) in states.borrow().iter().enumerate() {
        let number = index + 1;
        let owns_label = state.title.borrow().as_str() == TabMode::title(state.mode, number)
            || state.title.borrow().as_str() == TabMode::title(state.mode, number + 1);
        if owns_label {
            let title = TabMode::title(state.mode, number);
            *state.title.borrow_mut() = title.clone();
            if let Some(page) = state.page.borrow().as_ref() {
                page.set_title(&title);
            }
        }
    }
}

fn create_page(
    tab_view: &adw::TabView,
    browser: &Browser,
    url: String,
    mode: TabMode,
    number: usize,
    restored_title: Option<String>,
) -> (adw::TabPage, PageRef) {
    // Weak, like the window handle: a tab must not keep the tab view alive.
    let tab_view_ref: glib::WeakRef<adw::TabView> = glib::WeakRef::new();
    tab_view_ref.set(Some(tab_view));

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.set_hexpand(true);
    content.set_vexpand(true);

    let placeholder = gtk::Label::new(Some("WebView is created when this tab is selected"));
    placeholder.set_hexpand(true);
    placeholder.set_vexpand(true);
    content.append(&placeholder);
    let status = gtk::Label::new(Some(if mode == TabMode::Private {
        "Private tab: sharing an ephemeral network session, never restored"
    } else {
        ""
    }));
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.set_margin_start(8);
    status.set_margin_end(8);
    status.set_margin_bottom(4);
    content.append(&status);
    // The line reports only when it has something to say, so a tab with nothing
    // to report gives the page the height instead of holding a strip of it. The
    // text is written from a dozen places, so the visibility follows the label's
    // own text rather than being set at each of them.
    status.connect_notify_local(Some("label"), |status, _| {
        let empty = status.text().is_empty();
        status.set_visible(!empty);
    });

    // A restored tab keeps the title it had; a fresh tab gets its positional
    // label until a page reports a real one.
    let title = restored_title
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| mode.title(number));
    let state = Rc::new(PageState {
        content: content.clone(),
        placeholder: placeholder.clone(),
        status: status.clone(),
        url: RefCell::new(url.clone()),
        title: RefCell::new(title.clone()),
        mode,
        page: RefCell::new(None),
        network: match mode {
            TabMode::Normal => browser.normal_network.clone(),
            TabMode::Private => browser.private_network.clone(),
        },
        context: browser.context.clone(),
        view: RefCell::new(None),
        window: browser.window.clone(),
        tab_view: tab_view_ref,
        zoom: Cell::new(1.0),
        readable_on: Cell::new(false),
        readable_manager: readability::manager(),
        readable_sheet: RefCell::new(None),
        store: browser.store.clone(),
    });

    let page = tab_view.append(&content);
    page.set_title(&title);
    *state.page.borrow_mut() = Some(page.clone());
    (page, state)
}

/// Move the selected tab back or forward, if it can.
fn history_step(state: &PageRef, action: HistoryAction) {
    let Some(view) = state.view.borrow().clone() else {
        return;
    };
    match action {
        HistoryAction::Back if view.can_go_back() => view.go_back(),
        HistoryAction::Forward if view.can_go_forward() => view.go_forward(),
        _ => {}
    }
}

#[derive(Clone, Copy)]
enum HistoryAction {
    Back,
    Forward,
}

fn navigate_state(
    state: &PageRef,
    input: &str,
    search_endpoint: Option<&str>,
    address: &gtk::Entry,
    suggestions: &Suggestions,
) -> Result<(), String> {
    let url = navigation::normalize_input_with_search(input, search_endpoint)?;
    suggestions.write(address, &url);
    *state.url.borrow_mut() = url.clone();
    state.status.set_text("");
    if let Some(view) = state.view.borrow().clone() {
        view.load_uri(&url);
    }
    Ok(())
}

fn realize_page(state: &PageRef, chrome: &Chrome) {
    // The load callback lives as long as the view, so it takes its own handles
    // on the two window controls it writes instead of borrowing the chrome.
    let address_for_load = chrome.address.clone();
    let bookmark_for_load = chrome.bookmark.clone();
    let suggestions_for_load = chrome.suggestions.clone();
    if state.view.borrow().is_some() {
        return;
    }
    let url = state.url.borrow().clone();
    // The content manager is construct-only, so every view is built with this
    // tab's own manager; readability only adds or removes a sheet on it.
    let view = WebView::builder()
        .web_context(state.context.as_ref())
        .network_session(state.network.as_ref())
        .user_content_manager(&state.readable_manager)
        .build();
    // A tab can be switched to readability before it is ever selected, so the
    // flag is honoured here rather than only in the toggle.
    apply_readability_sheet(state);
    {
        // A failed load used to leave a blank page with no explanation.
        let weak: Weak<PageState> = Rc::downgrade(state);
        view.connect_load_failed(move |_, _, uri, error| {
            // Two of WebKit's load errors are not failures. A page that starts
            // another load cancels the one in flight, and a policy change
            // cancels one outright; the replacement load is the one that
            // matters, and it reports itself. YouTube does the first on every
            // visit - its consent and localisation step replaces the very first
            // request - so treating these as failures reported a broken page over
            // a page that had loaded perfectly.
            if error.matches(webkit6::NetworkError::Cancelled)
                || error.matches(webkit6::PolicyError::FrameLoadInterruptedByPolicyChange)
            {
                log_event(&format!("a load of {uri} was replaced by another one"));
                return false;
            }
            let Some(state) = weak.upgrade() else {
                return false;
            };
            let message = format!("could not load {uri}: {error}");
            state.status.set_text(&message);
            log_event(&message);
            false
        });
        let weak: Weak<PageState> = Rc::downgrade(state);
        view.connect_web_process_terminated(move |_, reason| {
            let Some(state) = weak.upgrade() else {
                return;
            };
            let message = format!(
                "the page stopped responding ({}); use the reload button to try again",
                describe_termination(reason)
            );
            state.status.set_text(&message);
            log_event(&message);
        });
        let weak: Weak<PageState> = Rc::downgrade(state);
        view.connect_permission_request(move |_, request| {
            let Some(state) = weak.upgrade() else {
                // The tab is gone, so there is nobody to ask. Refuse rather than
                // leave the request waiting on a window that no longer exists.
                request.deny();
                return true;
            };
            ask_for_permission(&state, request);
            true
        });
    }

    let weak: Weak<PageState> = Rc::downgrade(state);
    let store = state.store.clone();
    view.connect_load_changed(move |view, event| {
        let Some(state) = weak.upgrade() else {
            return;
        };
        match event {
            LoadEvent::Started | LoadEvent::Redirected | LoadEvent::Committed => {
                if let Some(page) = state.page.borrow().as_ref() {
                    page.set_loading(true);
                }
            }
            LoadEvent::Finished => {
                if let Some(page) = state.page.borrow().as_ref() {
                    page.set_loading(false);
                }
                if let Some(uri) = view.uri() {
                    let uri = uri.to_string();
                    *state.url.borrow_mut() = uri.clone();
                    // The field belongs to the tab in front, so a background
                    // tab finishing its load must not write into it.
                    if in_front(&state) {
                        suggestions_for_load.write(&address_for_load, &uri);
                    }
                }
                if let Some(title) = view.title() {
                    let title = title.to_string();
                    *state.title.borrow_mut() = title.clone();
                    if let Some(page) = state.page.borrow().as_ref() {
                        page.set_title(&title);
                    }
                    // The window title follows the page in front, the way every
                    // browser does. It used to stay "Brwsl" forever, which made
                    // the window list and the taskbar entry useless once a real
                    // page was open. Only the selected tab may retitle the window:
                    // a background tab finishing its load must not steal the title.
                    let is_selected = in_front(&state);
                    if let (true, Some(window)) = (is_selected, state.window.upgrade()) {
                        let label = if title.trim().is_empty() {
                            "Brwsl".to_string()
                        } else {
                            format!("{title} — Brwsl")
                        };
                        window.set_title(Some(&label));
                    }
                }
                // A completed load is what history records; the shell writes it
                // rather than injecting script into the page.
                record_visit(&state, &store);
                // The star is the window's, so only the tab in front may write it.
                if in_front(&state) {
                    set_bookmark_icon(
                        &bookmark_for_load,
                        store
                            .borrow()
                            .is_bookmarked(&current_state_url(&state))
                            .unwrap_or(false),
                    );
                }
            }
            _ => {}
        }
    });
    state.content.remove(&state.placeholder);
    state.content.append(&view);
    view.set_vexpand(true);
    view.set_hexpand(true);
    *state.view.borrow_mut() = Some(view.clone());
    view.load_uri(&url);
}

/// Record a completed page load, skipping private tabs and reporting nothing to
/// the user when the URL is one the navigation layer would not store.
fn record_visit(state: &PageRef, store: &Rc<RefCell<storage::SessionStore>>) {
    if state.mode == TabMode::Private {
        return;
    }
    let url = current_state_url(state);
    let title = state.title.borrow().clone();
    if let Err(error) = store.borrow().record_visit(&url, &title)
        && !error.starts_with("history URL is not allowed")
    {
        eprintln!("record visit: {error}");
    }
}

fn current_state_url(state: &PageRef) -> String {
    state
        .view
        .borrow()
        .as_ref()
        .and_then(|view| view.uri())
        .map(|uri| uri.to_string())
        // An empty URI is not an answer, it is the absence of one. After a
        // renderer crash WebKit reports "" rather than None, and taking that as
        // the current URL is what stopped a crashed tab from ever being saved:
        // the store rightly refuses an empty URL, the refusal failed the whole
        // save, and the session came back empty on the next launch.
        .filter(|uri| !uri.trim().is_empty())
        .unwrap_or_else(|| state.url.borrow().clone())
}

fn save_sessions(
    states: &Rc<RefCell<Vec<PageRef>>>,
    tab_view: &adw::TabView,
    store: &Rc<RefCell<storage::SessionStore>>,
) {
    let selected = tab_view.selected_page();
    let mut records: Vec<storage::SessionTab> = Vec::new();
    let mut skipped = 0usize;
    for state in states.borrow().iter() {
        if state.mode != TabMode::Normal {
            continue;
        }
        let url = current_state_url(state);
        // One unusable tab must not cost the whole session. A tab whose URL is
        // empty - a renderer that died before reporting one - is left out and
        // counted, rather than handed to the store, which would refuse the batch
        // and leave nothing saved at all.
        if url.trim().is_empty() {
            skipped += 1;
            continue;
        }
        let position = records.len();
        records.push(storage::SessionTab {
            id: position as i64 + 1,
            position: position as i64,
            url,
            title: state.title.borrow().clone(),
            selected: state.page.borrow().as_ref() == selected.as_ref(),
        });
    }
    if skipped > 0 {
        eprintln!("save session: {skipped} tab(s) had no URL and were not saved");
    }
    if let Err(error) = store.borrow_mut().save_normal(&records) {
        eprintln!("save session: {error}");
    }
}
