//! The capability prompt: the one window a page cannot refuse to be shown.
//!
//! A camera, microphone, screen or location request opens a window of this
//! application whose "Allow" control the keyboard cannot reach, so Return and
//! Escape both refuse and a grant takes a deliberate click and nothing else.
//!
//! The prompt knows nothing about tabs, the session or the window's controls.
//! It is handed the origin to show and a weak handle to parent itself to, and
//! it builds everything else itself.

use gtk4 as gtk;
use gtk4::glib;
use gtk4::glib::Propagation;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use webkit6::prelude::*;

use crate::gui::log_event;

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
pub fn ask(
    window: &glib::WeakRef<adw::ApplicationWindow>,
    current: &str,
    request: &webkit6::PermissionRequest,
) {
    // The request object exposes no URI, so the origin shown is the tab's own
    // current address, which is what the person is looking at anyway.
    let origin = if current == "about:blank" {
        "this page".to_string()
    } else {
        current.to_string()
    };
    // The prompt is a window of this application, and it reports itself to the
    // compositor under the application's class. That class is what decides
    // whether the keyboard can reach the prompt: a plain window reports the
    // binary's name instead, and anything that aims keys at this application -
    // the end-to-end harness among them, which refuses to type into a window it
    // does not recognise - then treats the prompt as somebody else's, and
    // Return and Escape go nowhere.
    let prompt = window.upgrade().and_then(|parent| {
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
