//! GTK4/libadwaita/WebKitGTK shell: window, tab strip, and lazy page realization.

use gtk4 as gtk;
use gtk4::glib::Propagation;
use gtk4::prelude::*;
use libadwaita as adw;
use rbrowse::{config::Config, navigation, storage};
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use webkit6::prelude::*;
use webkit6::{LoadEvent, NetworkSession, WebView};

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
    view: RefCell<Option<WebView>>,
}

pub fn run(config: Config) -> Result<(), String> {
    adw::init().map_err(|error| format!("initialize GTK/libadwaita: {error}"))?;
    let application = adw::Application::builder()
        .application_id(APPLICATION_ID)
        .flags(gtk::gio::ApplicationFlags::empty())
        .build();
    let activate_config = config.clone();
    application.connect_activate(move |application| {
        if !application.windows().is_empty() {
            return;
        }
        if let Err(error) = build_window(application, activate_config.clone()) {
            eprintln!("open R Browse window: {error}");
            application.quit();
        }
    });
    // R Browse parses its own flags, so the GTK option parser only ever sees
    // the program name; unknown-flag handling and exit codes stay ours.
    let program = std::env::args()
        .next()
        .unwrap_or_else(|| "rbrowse".to_string());
    application.run_with_args(&[program]);
    Ok(())
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
    let normal_network = Rc::new(NetworkSession::new(
        Some(data_path(&config)?),
        Some(cache_path(&config)?),
    ));
    let private_network = Rc::new(NetworkSession::new_ephemeral());

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
        let (page, state) = create_page(
            &tab_view,
            tab.url,
            TabMode::Normal,
            states.borrow().len() + 1,
            normal_network.clone(),
            config.search_endpoint.clone(),
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
        let (page, state) = create_page(
            &tab_view,
            url,
            TabMode::Normal,
            1,
            normal_network.clone(),
            config.search_endpoint.clone(),
        );
        states.borrow_mut().push(state);
        selected_page = Some(page);
    }
    if let Some(page) = selected_page.clone()
        && let Some(state) = find_state(&states, &page)
    {
        tab_view.set_selected_page(&page);
        realize_page(&state);
    }

    let states_for_select = Rc::clone(&states);
    tab_view.connect_selected_page_notify(move |view| {
        let Some(page) = view.selected_page() else {
            return;
        };
        if let Some(state) = find_state(&states_for_select, &page) {
            realize_page(&state);
        }
    });

    let states_for_new = Rc::clone(&states);
    let view_for_new = tab_view.clone();
    let network_for_new = normal_network.clone();
    let endpoint_for_new = config.search_endpoint.clone();
    new_tab.connect_clicked(move |_| {
        let number = states_for_new.borrow().len() + 1;
        let (page, state) = create_page(
            &view_for_new,
            "about:blank".to_string(),
            TabMode::Normal,
            number,
            network_for_new.clone(),
            endpoint_for_new.clone(),
        );
        states_for_new.borrow_mut().push(state.clone());
        view_for_new.set_selected_page(&page);
        realize_page(&state);
    });

    let states_for_private = Rc::clone(&states);
    let view_for_private = tab_view.clone();
    let network_for_private = private_network.clone();
    let endpoint_for_private = config.search_endpoint.clone();
    private_tab.connect_clicked(move |_| {
        let number = states_for_private.borrow().len() + 1;
        let (page, state) = create_page(
            &view_for_private,
            "about:blank".to_string(),
            TabMode::Private,
            number,
            network_for_private.clone(),
            endpoint_for_private.clone(),
        );
        states_for_private.borrow_mut().push(state.clone());
        view_for_private.set_selected_page(&page);
        realize_page(&state);
    });

    let states_for_close = Rc::clone(&states);
    let view_for_close = tab_view.clone();
    let store_for_close = Rc::clone(&store);
    let endpoint_for_close = config.search_endpoint.clone();
    tab_view.connect_close_page(move |_, page| {
        let position = {
            let states = states_for_close.borrow();
            states
                .iter()
                .position(|state| state.page.borrow().as_ref() == Some(page))
        };
        let Some(position) = position else {
            return Propagation::Proceed;
        };
        let remaining = {
            let states = states_for_close.borrow();
            let remaining = states.len() - 1;
            let last = (remaining == 0)
                .then(|| states.get(position).cloned())
                .flatten();
            (remaining, last)
        };
        if let Some(state) = remaining.1 {
            // Never leave the window without a page: the last tab resets to
            // about:blank instead of closing.
            let _ = navigate_state(&state, "about:blank", endpoint_for_close.as_deref());
            save_sessions(&states_for_close, &view_for_close, &store_for_close);
            return Propagation::Stop;
        }
        states_for_close.borrow_mut().remove(position);
        save_sessions(&states_for_close, &view_for_close, &store_for_close);
        Propagation::Proceed
    });

    let states_for_window = Rc::clone(&states);
    let view_for_window = tab_view.clone();
    let store_for_window = Rc::clone(&store);
    window.connect_close_request(move |_| {
        save_sessions(&states_for_window, &view_for_window, &store_for_window);
        Propagation::Proceed
    });

    window.present();
    Ok(())
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

fn create_page(
    tab_view: &adw::TabView,
    url: String,
    mode: TabMode,
    number: usize,
    network: NetworkHandle,
    search_endpoint: Option<String>,
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
    let address = gtk::Entry::new();
    address.set_text(&url);
    address.set_placeholder_text(Some("Address (use search: for a configured search)"));
    address.set_width_chars(48);
    address.set_hexpand(true);
    for button in [&back, &forward, &reload] {
        toolbar.append(button);
    }
    toolbar.append(&address);
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

    let state = Rc::new(PageState {
        content: content.clone(),
        placeholder: placeholder.clone(),
        address: address.clone(),
        status: status.clone(),
        url: RefCell::new(url.clone()),
        title: RefCell::new(mode.title(number)),
        mode,
        page: RefCell::new(None),
        network,
        view: RefCell::new(None),
    });

    connect_address(&state, &address, search_endpoint);
    connect_history_button(&state, &back, HistoryAction::Back);
    connect_history_button(&state, &forward, HistoryAction::Forward);
    connect_reload(&state, &reload);

    let page = tab_view.append(&content);
    page.set_title(&mode.title(number));
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
    let view = WebView::builder()
        .network_session(state.network.as_ref())
        .build();
    let weak: Weak<PageState> = Rc::downgrade(state);
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
