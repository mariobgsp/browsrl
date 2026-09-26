# Design language

Brwsl is a tool, not a product. Everything below is a rule the code already
follows, kept here so the next change to the window is made with the reasons in
hand instead of rediscovering them. There are no colours, no spacing scale and
no type scale in this project, and adding them is a design change with its own
plan, not a side effect of reading this file.

## Palette

The system theme decides every colour. The app names Adwaita and libadwaita and
the `*-symbolic` icon names, and defines no colour of its own, so it looks the
same as the desktop it runs on and follows it when the desktop changes.

The readability pass sets no colours either, on purpose: a page that sets its
own background fights a style sheet that sets one and ends up with unreadable
text. Measure, type size and hiding page furniture are the parts that help
(`src/readability.rs`).

## Controls

Every control in the chrome is an Adwaita symbolic icon whose meaning lives in
its tooltip. A text "Reload" button once sat in a row of symbolic buttons and
looked like a different kind of control, and the rule came from that: the row has
to read as one thing, so the icons are all the same kind and the words are in
the tooltips.

The one exception is a destructive action, which keeps its word, because it is
about what it will lose rather than what it will do: `Clear history` in the
history panel.

## Chrome

The window is two bands and then the page:

1. the bar, carrying the tab strip and the window's controls, and
2. the field row, carrying the one address field.

The page is the ground. The bar used to be a header bar with a tab strip stacked
under it, and each tab carried its own row of buttons above its page: four bands
and 202 px of a 1157 px window, measured on the running app, none of it the page.
It is now 103 px.

The tab strip shares the bar rather than having a band of its own, and it is
always shown: a strip that appears and disappears with the number of tabs moves
the whole bar every time a tab opens. It cannot be the window's *title bar* on
this stack, and that is a platform limit rather than a choice:
`gtk_window_set_titlebar()` is refused for an `AdwApplicationWindow`, and
libadwaita 0.9.2 exposes no `AdwWindow` titlebar property of its own.

## The field

There is one address field, in the window, not one per tab. It shows the tab in
front and typing in it navigates the tab in front, so a field that belonged to a
tab would have to be rebuilt every time the front tab changed.

It is also the only place a page-level state is written, which is why nothing in
it is a button: Enter goes, `Ctrl+L` returns to it, and the text follows the page
that finishes loading.

### What the field offers

The addresses this profile has been to, most recent first, as a band between the
field and the page while somebody is typing in it. Three rules, and each of them
is a measurement rather than a preference:

- **Local.** The candidates are the profile's own history. Nothing is looked up,
  and the search path still only goes out when Return is pressed.
- **Read rarely, matched in memory.** The addresses are read at most once every
  thirty seconds, and always when there are none to offer, so typing a URL costs no
  database work per letter. The throttle must not also make the field look broken:
  an empty list is read straight away, because the first use in a window is often
  before anything has been visited.
- **Offered to a person, not to the shell.** The field is also written by the app —
  a page finishing a load, a tab switching, a navigation — and none of those is
  somebody asking. So the shell marks its own writes and the rows stay down for
  them. Neither the focus nor the keyboard can tell the two apart on this desktop:
  the field reports no focus while it is being filled, and the characters arrive as
  text rather than as key presses. That is worth knowing before anything is
  built here on either of them.

The rows are plain labels rather than a list, and the row the keyboard is on is
marked in the text. A list claims the keyboard the moment one of its rows is
selected, and a keyboard that arrives somewhere else is a keyboard this browser has
lost: Return stopped reaching the address, and the address kept typing into a field
nobody was typing in.

## State

The status line under the page is where the app says what it is doing, and it is
visible only when it has something to say: a tab with nothing to report gives the
height back to the page instead of holding an empty strip.

A row of controls carries no permanent words. The zoom percentage is the one
value shown at all times, and it is a readout, not a control: it is not
sensitive, and `Ctrl+0` is the way back to 100%.

## Titles

A window title reads `<what> — Brwsl`. The browsing window's `<what>` is the
page in front, so the window list and the taskbar entry are worth reading. A
panel's is the panel's name.

Only the tab in front may write the window's own controls — the field, the star,
the reading mark, the percentage. A tab that finishes loading in the background
must not move them out from under the page someone is reading.

## Copy

User-facing copy never names a class, an enum variant, a session, a directory or
a mechanism. "The page crashed", "not an address to reload", "Private tabs are
not bookmarked" — the words are about the person's situation. That is why
`describe_termination` exists: to stop WebKit's own variant names reaching the
screen, and why the same rule applies to everything else this app says.

## Windows

Every window is a header bar over its content, including the bookmark and history
panels. A window that falls back to a plain title bar is a different visual
language from the window it was opened from, and the panel's only control ends up
in a strip under the list because there is nowhere else for it to go.

A prompt that grants something is a window too, and its granting control cannot be
reached by the keyboard. An alert dialog whose default response was "Deny" still
answered "Allow" to a stray Return in roughly one run in eight, because the
affirmative response takes initial focus while the dialog's first frame is mapped;
a page got a real camera stream that way. So a default that can lose a race is not
a default, and granting takes a click.

## Where the model comes from

`driceroland/Search` is the browser this project is in the spirit of, and its
`Design.swift` is where the shape comes from: the page is the ground, everything
the browser draws gets out of its way, and every colour and metric lives in one
file. Brwsl has no such file of its own yet — this one is the small version of
it, written in the rules rather than the values, because the values belong to the
system theme.
