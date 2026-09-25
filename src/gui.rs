//! GTK4/libadwaita/WebKitGTK shell: window, tab strip, and lazy page realization.

use gtk4 as gtk;
use gtk4::glib::Propagation;
use gtk4::prelude::*;
use libadwaita as adw;
use rbrowse::{config::Config, downloads, navigation, readability, storage};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use webkit6::prelude::*;
use webkit6::{Download, LoadEvent, NetworkSession, WebContext, WebView};

/// Attach download handling to a network session.
///
/// WebKit asks for a destination with a server-supplied name, so the answer is
/// always a path inside the profile's download directory. A name that cannot be
/// made safe is refused rather than guessed at, and the user sees why.
fn watch_downloads(session: &NetworkSession, download_dir: PathBuf) {
    let dir = download_dir.clone();
    session.connect_download_started(move |_, download| {
        let dir = dir.clone();
        watch_one_download(download, &dir);
    });
}

fn watch_one_download(download: &Download, download_dir: &std::path::Path) {
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
            match downloads::destination_for(&dir, suggested, |_| false) {
                Some(path) => {
                    let path = path.to_string_lossy().into_owned();
                    owner.set_destination(&path);
                    true
                }
                // Refusing is better than writing outside the profile or
                // inventing a name the server did not ask for.
                None => false,
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
    for_finished.connect_finished(move |_| {
        println!("download finished");
    });
}

pub const APPLICATION_ID: &str = "io.github.rbrowse.RBrowse";

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
    address: gtk::Entry,
    status: gtk::Label,
    url: RefCell<String>,
    title: RefCell<String>,
    mode: TabMode,
    page: RefCell<Option<adw::TabPage>>,
    network: NetworkHandle,
    context: Rc<WebContext>,
    view: RefCell<Option<WebView>>,
    /// Toolbar button that reflects and toggles the bookmark state.
    bookmark: gtk::Button,
    /// Toolbar button that reflects and toggles the readability pass.
    readable: gtk::Button,
    /// Whether this tab is showing the readability stylesheet.
    readable_on: Cell<bool>,
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
}

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
        realize_page(&state);
    }

    /// Open a URL in the selected tab, reporting a rejection in its status line.
    fn navigate_selected(&self, input: &str) {
        let Some(state) = self.selected() else {
            return;
        };
        if let Err(error) = navigate_state(&state, input, self.browser.search_endpoint.as_deref()) {
            self.note(&state, &error);
        }
    }

    /// Toggle the bookmark for the selected tab and reflect the result.
    fn toggle_bookmark(&self) {
        let Some(state) = self.selected() else {
            return;
        };
        toggle_bookmark(&state, &self.browser.store);
    }

    /// Reflect the bookmark state of a tab after a page load.
    fn refresh_bookmark(&self, state: &PageRef) {
        let url = current_state_url(state);
        let bookmarked = if url == "about:blank" {
            false
        } else {
            self.store.borrow().is_bookmarked(&url).unwrap_or(false)
        };
        set_bookmark_icon(state, bookmarked);
    }

    fn close_selected(&self) {
        if let Some(page) = self.tab_view.selected_page() {
            self.tab_view.close_page(&page);
        }
    }

    fn select_relative(&self, next: bool) {
        // AdwTabView owns the ordering, so let it do the wrapping rather than
        // reimplementing index arithmetic over the page list.
        if next {
            self.tab_view.select_next_page();
        } else {
            self.tab_view.select_previous_page();
        }
    }

    fn save(&self) {
        save_sessions(&self.states, &self.tab_view, &self.store);
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
        if let Some(state) = self.selected() {
            state.address.grab_focus();
            state.address.select_region(0, -1);
        }
    }

    fn toggle_readability(&self) {
        if let Some(state) = self.selected() {
            toggle_readability(&state);
        }
    }

    fn copy_address(&self) {
        let Some(state) = self.selected() else {
            return;
        };
        let url = current_state_url(&state);
        state.address.clipboard().set_text(&url);
        self.note(&state, "Address copied");
    }
}

/// Add or remove the bookmark for a tab, reporting the outcome in its status
/// line. Shared by the toolbar button and the Ctrl+D action so both behave
/// identically, including the refusals for private and blank tabs.
fn toggle_bookmark(state: &PageRef, store: &Rc<RefCell<storage::SessionStore>>) {
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
            set_bookmark_icon(state, true);
            state.status.set_text("Bookmarked");
        }
        Ok(false) => {
            set_bookmark_icon(state, false);
            state.status.set_text("Bookmark removed");
        }
        Err(error) => state.status.set_text(&format!("bookmark failed: {error}")),
    }
}

/// Apply or remove the readability pass for a tab.
///
/// The stylesheet is attached to a per-tab content manager, so no script runs
/// in the page and no other tab is affected.
fn toggle_readability(state: &PageRef) {
    let next = !state.readable_on.get();
    state.readable_on.set(next);
    set_readability_icon(state, next);
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

fn set_readability_icon(state: &PageRef, on: bool) {
    state.readable.set_icon_name(if on {
        "view-reading-mode-checked-symbolic"
    } else {
        "view-reading-mode-symbolic"
    });
    state.readable.set_tooltip_text(Some(if on {
        "Turn off the readability pass (Ctrl+Shift+R)"
    } else {
        "Apply the readability pass (Ctrl+Shift+R)"
    }));
}

fn connect_readability_button(state: &PageRef, button: &gtk::Button) {
    let weak: Weak<PageState> = Rc::downgrade(state);
    button.connect_clicked(move |_| {
        let Some(state) = weak.upgrade() else {
            return;
        };
        toggle_readability(&state);
    });
}

fn set_bookmark_icon(state: &PageRef, bookmarked: bool) {
    state.bookmark.set_icon_name(if bookmarked {
        "starred-symbolic"
    } else {
        "non-starred-symbolic"
    });
    state.bookmark.set_tooltip_text(Some(if bookmarked {
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
            eprintln!("open R Browse window: {error}");
            *error_slot.borrow_mut() = Some(error);
            application.quit();
        }
    });
    // R Browse parses its own flags, so the GTK option parser only ever sees
    // the program name; unknown-flag handling and exit codes stay ours.
    let program = std::env::args()
        .next()
        .unwrap_or_else(|| "rbrowse".to_string());
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
    // One explicit WebKit context for the whole window. Owning it here keeps
    // the process pool, the network sessions, and any future per-window
    // settings in one place instead of implicit defaults.
    let browser = Browser {
        context: Rc::new(WebContext::new()),
        store: Rc::clone(&store),
        normal_network: Rc::new(NetworkSession::new(
            Some(data_path(&config)?),
            Some(cache_path(&config)?),
        )),
        private_network: Rc::new(NetworkSession::new_ephemeral()),
        search_endpoint: config.search_endpoint.clone(),
        download_dir: config.download_dir().to_path_buf(),
    };

    watch_downloads(&browser.normal_network, browser.download_dir.clone());
    watch_downloads(&browser.private_network, browser.download_dir.clone());

    let tab_view = adw::TabView::new();
    let tab_bar = adw::TabBar::new();
    tab_bar.set_view(Some(&tab_view));
    tab_bar.set_autohide(false);

    let header = adw::HeaderBar::new();
    let new_tab = gtk::Button::from_icon_name("tab-new-symbolic");
    new_tab.set_tooltip_text(Some("New tab"));
    let private_tab = gtk::Button::with_label("Private");
    private_tab.set_tooltip_text(Some("New private tab"));
    header.pack_start(&new_tab);
    header.pack_start(&private_tab);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&header);
    root.append(&tab_bar);
    root.append(&tab_view);
    tab_view.set_vexpand(true);

    let window = adw::ApplicationWindow::builder()
        .application(application)
        .title("R Browse")
        .default_width(1100)
        .default_height(760)
        .content(&root)
        .build();

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
    }
    if let Some(page) = selected_page.clone()
        && let Some(state) = find_state(&states, &page)
    {
        tab_view.set_selected_page(&page);
        realize_page(&state);
    }

    let shell = Shell {
        states: Rc::clone(&states),
        tab_view: tab_view.clone(),
        store: Rc::clone(&store),
        browser: browser.clone(),
    };

    // Tab selection realizes the page and syncs its bookmark indicator.
    let shell_for_select = shell.clone();
    tab_view.connect_selected_page_notify(move |view| {
        let Some(page) = view.selected_page() else {
            return;
        };
        if let Some(state) = find_state(&shell_for_select.states, &page) {
            realize_page(&state);
            shell_for_select.refresh_bookmark(&state);
        }
    });

    let shell_for_new = shell.clone();
    new_tab.connect_clicked(move |_| shell_for_new.add_tab(TabMode::Normal, "about:blank"));

    let shell_for_private = shell.clone();
    private_tab
        .connect_clicked(move |_| shell_for_private.add_tab(TabMode::Private, "about:blank"));

    let shell_for_close = shell.clone();
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
            let _ = navigate_state(&state, "about:blank", endpoint_for_close.as_deref());
            shell_for_close.save();
            return Propagation::Stop;
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
        Propagation::Proceed
    });

    install_actions(application, &shell);
    window.present();
    Ok(())
}

/// Register the window's keyboard actions.
///
/// These are `GAction`s on the application rather than key-press handlers, so
/// GTK owns the accelerator table and the bindings work wherever focus is.
fn install_actions(application: &adw::Application, shell: &Shell) {
    // The closures live as long as the application, so they hold their own
    // handle rather than borrowing the caller's.
    let shell = shell.clone();
    let actions: &[ActionBinding] = &[
        ("new-tab", &["<Primary>t", "<Primary><Shift>n"], |shell| {
            shell.add_tab(TabMode::Normal, "about:blank")
        }),
        ("new-private-tab", &["<Primary><Shift>p"], |shell| {
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
        (
            "readability",
            &["<Primary><Shift>r"],
            Shell::toggle_readability,
        ),
        ("focus-address", &["<Primary>l"], Shell::focus_address),
        ("copy-page-address", &["<Primary>c"], Shell::copy_address),
    ];
    for (name, keys, handler) in actions {
        let action = gtk::gio::SimpleAction::new(name, None);
        let handler = *handler;
        let shell = shell.clone();
        action.connect_activate(move |_, _| handler(&shell));
        application.add_action(&action);
        application.set_accels_for_action(&format!("win.{name}"), keys);
    }
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

fn find_state(states: &Rc<RefCell<Vec<PageRef>>>, page: &adw::TabPage) -> Option<PageRef> {
    states
        .borrow()
        .iter()
        .find(|state| state.page.borrow().as_ref() == Some(page))
        .cloned()
}

/// Re-apply positional tab titles so the strip does not keep gaps after a
/// close. A tab that already shows a page title keeps it.
fn renumber_tabs(states: &Rc<RefCell<Vec<PageRef>>>) {
    for (index, state) in states.borrow().iter().enumerate() {
        let number = index + 1;
        if state.title.borrow().as_str() == TabMode::title(state.mode, number + 1)
            || state.title.borrow().as_str() == TabMode::title(state.mode, number)
        {
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
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.set_hexpand(true);
    content.set_vexpand(true);

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    toolbar.set_margin_top(6);
    toolbar.set_margin_end(6);
    toolbar.set_margin_bottom(6);
    toolbar.set_margin_start(6);
    let back = gtk::Button::with_label("←");
    back.set_tooltip_text(Some("Back"));
    let forward = gtk::Button::with_label("→");
    forward.set_tooltip_text(Some("Forward"));
    let reload = gtk::Button::with_label("Reload");
    reload.set_tooltip_text(Some("Reload"));
    let bookmark = gtk::Button::from_icon_name("non-starred-symbolic");
    let readable = gtk::Button::from_icon_name("view-reading-mode-symbolic");
    let address = gtk::Entry::new();
    address.set_text(&url);
    address.set_placeholder_text(Some("Address (use search: for a configured search)"));
    address.set_width_chars(48);
    address.set_hexpand(true);
    for button in [&back, &forward, &reload] {
        toolbar.append(button);
    }
    toolbar.append(&address);
    toolbar.append(&bookmark);
    toolbar.append(&readable);
    content.append(&toolbar);

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

    // A restored tab keeps the title it had; a fresh tab gets its positional
    // label until a page reports a real one.
    let title = restored_title
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| mode.title(number));
    let state = Rc::new(PageState {
        content: content.clone(),
        placeholder: placeholder.clone(),
        address: address.clone(),
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
        bookmark: bookmark.clone(),
        readable: readable.clone(),
        readable_on: Cell::new(false),
        readable_manager: readability::manager(),
        readable_sheet: RefCell::new(None),
        store: browser.store.clone(),
    });

    connect_address(&state, &address, browser.search_endpoint.clone());
    connect_history_button(&state, &back, HistoryAction::Back);
    connect_history_button(&state, &forward, HistoryAction::Forward);
    connect_reload(&state, &reload);
    connect_bookmark_button(&state, &browser.store, &bookmark);
    connect_readability_button(&state, &readable);
    set_bookmark_icon(&state, false);
    set_readability_icon(&state, false);

    let page = tab_view.append(&content);
    page.set_title(&title);
    *state.page.borrow_mut() = Some(page.clone());
    (page, state)
}

fn connect_address(state: &PageRef, address: &gtk::Entry, search_endpoint: Option<String>) {
    let weak: Weak<PageState> = Rc::downgrade(state);
    address.connect_activate(move |entry| {
        let Some(state) = weak.upgrade() else {
            return;
        };
        if let Err(error) = navigate_state(&state, &entry.text(), search_endpoint.as_deref()) {
            state.status.set_text(&error);
        }
    });
}

#[derive(Clone, Copy)]
enum HistoryAction {
    Back,
    Forward,
}

fn connect_history_button(state: &PageRef, button: &gtk::Button, action: HistoryAction) {
    let weak: Weak<PageState> = Rc::downgrade(state);
    button.connect_clicked(move |_| {
        let Some(state) = weak.upgrade() else {
            return;
        };
        let Some(view) = state.view.borrow().clone() else {
            return;
        };
        match action {
            HistoryAction::Back if view.can_go_back() => view.go_back(),
            HistoryAction::Forward if view.can_go_forward() => view.go_forward(),
            _ => {}
        }
    });
}

fn connect_bookmark_button(
    state: &PageRef,
    store: &Rc<RefCell<storage::SessionStore>>,
    button: &gtk::Button,
) {
    // The click target is the tab itself, so the handler re-uses the same
    // toggle the Ctrl+D action runs rather than duplicating the logic.
    let weak: Weak<PageState> = Rc::downgrade(state);
    let store = store.clone();
    button.connect_clicked(move |_| {
        let Some(state) = weak.upgrade() else {
            return;
        };
        toggle_bookmark(&state, &store);
    });
}

fn connect_reload(state: &PageRef, button: &gtk::Button) {
    let weak: Weak<PageState> = Rc::downgrade(state);
    button.connect_clicked(move |_| {
        let Some(state) = weak.upgrade() else {
            return;
        };
        let Some(view) = state.view.borrow().clone() else {
            return;
        };
        match view.uri() {
            Some(uri) => view.load_uri(uri.as_str()),
            None => state.status.set_text("This tab has no address to reload."),
        }
    });
}

fn navigate_state(
    state: &PageRef,
    input: &str,
    search_endpoint: Option<&str>,
) -> Result<(), String> {
    let url = navigation::normalize_input_with_search(input, search_endpoint)?;
    state.address.set_text(&url);
    *state.url.borrow_mut() = url.clone();
    state.status.set_text("");
    if let Some(view) = state.view.borrow().clone() {
        view.load_uri(&url);
    }
    Ok(())
}

fn realize_page(state: &PageRef) {
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
                    state.address.set_text(&uri);
                }
                if let Some(title) = view.title() {
                    let title = title.to_string();
                    *state.title.borrow_mut() = title.clone();
                    if let Some(page) = state.page.borrow().as_ref() {
                        page.set_title(&title);
                    }
                }
                // A completed load is what history records; the shell writes it
                // rather than injecting script into the page.
                record_visit(&state, &store);
                set_bookmark_icon(
                    &state,
                    store
                        .borrow()
                        .is_bookmarked(&current_state_url(&state))
                        .unwrap_or(false),
                );
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
        .unwrap_or_else(|| state.url.borrow().clone())
}

fn save_sessions(
    states: &Rc<RefCell<Vec<PageRef>>>,
    tab_view: &adw::TabView,
    store: &Rc<RefCell<storage::SessionStore>>,
) {
    let selected = tab_view.selected_page();
    let records: Vec<storage::SessionTab> = states
        .borrow()
        .iter()
        .filter(|state| state.mode == TabMode::Normal)
        .enumerate()
        .map(|(position, state)| storage::SessionTab {
            id: position as i64 + 1,
            position: position as i64,
            url: current_state_url(state),
            title: state.title.borrow().clone(),
            selected: state.page.borrow().as_ref() == selected.as_ref(),
        })
        .collect();
    if let Err(error) = store.borrow_mut().save_normal(&records) {
        eprintln!("save session: {error}");
    }
}
