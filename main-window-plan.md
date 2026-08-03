# Main Window + Tray Icon Plan

## Overview

Add a persistent system tray icon and a main application window to the worktime tracker.
The tray icon stays alive for the lifetime of the process. The main window can be opened
and closed without quitting the app. The monitoring loop moves off the main thread so that
eframe owns the main thread permanently.

The first deliverable covered in this plan is the complete feature set:
tray icon, main window with day view (calendar + timeline painter with interactive editing).

---

## Architecture

```
Main Thread (eframe)
  └─ MainApp (egui App)
       ├─ tray icon (tray-icon crate, polled each frame)
       ├─ main window viewport (open/close independently)
       └─ idle-gap popup viewport (shown when gap arrives via channel)

Background Thread: monitoring loop (moved from main thread)
  └─ idle detector, auto-save, SIGINT handler (unchanged logic)
  └─ sends IdleGapInfo over channel → MainApp receives and shows popup
  └─ sends DB-change notifications back via another channel

Shared state: Arc<Mutex<Database>>
```

Key decisions:
- eframe owns the main thread; the monitoring loop moves to a background thread.
- The tray icon is managed via the `tray-icon` crate. On Linux it uses `libappindicator`
  under the hood, which works on Sway and Niri (both support the StatusNotifierItem protocol
  via `xdg-desktop-portal` or a compatible bar like waybar).
- The main window is shown/hidden by toggling an egui viewport or by opening a second
  `ViewportId`-based sub-viewport. Closing it does **not** close the root eframe window.
- The root eframe window is hidden/zero-size (acts as a message pump only); the main window
  and popup are separate viewports.
- The idle-gap popup continues to work as today, but is triggered from the MainApp's `update()`
  loop instead of the monitoring loop directly.
- The calendar in the main window is a simple always-visible prev/next-day navigation row
  (no hidden button). A full month-grid calendar with day colourization is deferred — the
  data model and selected_date state are designed to support it later.
- Tray icon badge text is rendered via the `ab_glyph` crate (already pulled in transitively
  by egui/eframe) into a 32×32 RGBA pixel buffer, avoiding an extra crate dependency.

---

## Sub-Tasks

---

### Sub-Task 1 — Move monitoring loop to background thread

**Status:** [x] done

**Intent:**
Decouple the monitoring loop from the main thread so eframe can own the main thread.
No user-visible change; all existing monitoring behaviour is preserved.

**Expected Outcomes:**
- `run_interactive_monitoring` runs on a spawned `std::thread`.
- The main thread immediately enters `eframe::run_native`.
- Existing idle popup channel still works (sent from background, received on main thread via channel).
- SIGINT handler and auto-save threads are unchanged.

**Todo List:**
1. In `src/main.rs`, wrap the `run_interactive_monitoring(database, &cfg)` call inside
   `std::thread::spawn(move || { ... })`.
2. The channel receiver (`idle_gap_rx`) must be moved out of `run_interactive_monitoring`
   and returned to the caller so the main thread (eframe) can poll it.
   - Change `run_interactive_monitoring` signature to return the
     `Option<std::sync::mpsc::Receiver<IdleGapInfo>>` instead of consuming it internally.
   - Remove the `while let Ok(gap) = rx.try_recv()` popup-handling block from
     `run_interactive_monitoring`; the main egui App will handle it instead.
3. Spawn the monitoring thread from `main()` before calling `eframe::run_native`.
4. Confirm the app still compiles and the monitoring loop still auto-saves / detects idle.

**Relevant Context:**
- [`src/monitoring.rs`](src/monitoring.rs) — `run_interactive_monitoring`, lines 11–178
- [`src/main.rs`](src/main.rs) — entry point, lines 87–98
- [`src/idle_popup.rs`](src/idle_popup.rs) — `IdleGapInfo`, `ask_idle_gap`

---

### Sub-Task 2 — Add tray icon crate and skeleton MainApp

**Status:** [x] done

**Intent:**
Introduce the `tray-icon` crate, create a new `src/main_window.rs` module containing a
minimal `MainApp` struct that implements `eframe::App`, and wire it up in `main()`.
At this stage the tray icon shows a static icon; clicking it prints to stdout.
The monitoring loop already runs in the background (Sub-Task 1).

**Expected Outcomes:**
- `tray-icon` dependency added to `Cargo.toml`.
- A system tray icon is visible after launch on Sway/Niri with a compatible status bar.
- `eframe::run_native` runs the new `MainApp` on the main thread.
- The root eframe window is invisible (zero inner-size, no decorations, no taskbar entry).
- `MainApp` holds the `Arc<Mutex<Database>>` and the idle-gap channel receiver.
- App does not quit when the (invisible) root window is "closed" — only SIGINT exits.

**Todo List:**
1. Add `tray-icon = "0.20"` (latest stable, check crates.io for actual latest) to
   `[dependencies]` in `Cargo.toml`. No extra image crate needed — icon built from raw RGBA.
2. Create `src/main_window.rs` with a `MainApp` struct:
   ```
   pub struct MainApp {
       database: Arc<Mutex<Database>>,
       idle_gap_rx: Option<Receiver<IdleGapInfo>>,
       tray: TrayIcon,           // tray-icon handle
       show_main_window: bool,   // toggle for day-view window
   }
   ```
3. Implement `eframe::App for MainApp`:
   - In `update()`, poll `idle_gap_rx` for pending gaps; store them to show popup next step.
   - Poll tray icon menu events; set `show_main_window = true` on click.
   - Keep root viewport 1×1 px, no decorations, off-screen position.
4. In `main()`:
   - Build a 16×16 RGBA icon (simple solid-colour square for now).
   - Construct `TrayIcon` with that icon.
   - Call `eframe::run_native` with `MainApp`.
5. Verify the icon appears in the system tray and the process stays alive.

**Relevant Context:**
- [`src/main.rs`](src/main.rs)
- [`Cargo.toml`](Cargo.toml)
- tray-icon crate API: `TrayIconBuilder`, `TrayIconEvent::receiver()`

---

### Sub-Task 3 — Idle-gap popup via MainApp

**Status:** [x] done

**Intent:**
Re-wire the idle-gap popup so it is triggered from `MainApp::update()` via the channel
established in Sub-Task 1, instead of the old blocking call inside the monitoring loop.
The popup itself (`ask_idle_gap`) is still a blocking `eframe::run_native` call, but it
is now invoked from the egui frame, using a deferred mechanism (spawn a thread from
`update()` to avoid blocking the main thread; receive the answer back via a second channel).

**Expected Outcomes:**
- When an idle gap arrives on `idle_gap_rx`, a popup window appears (same UI as today).
- The main egui loop is not blocked while the popup is open.
- After the user answers, the result is applied to the database (same logic as today in
  `monitoring.rs` lines 126–151).
- Existing `IdleGapAnswer` variants all work correctly.

**Todo List:**
1. Add a second channel inside `MainApp`:
   `popup_answer_rx: Option<Receiver<(IdleGapInfo, IdleGapAnswer)>>`
2. In `update()`, when a gap arrives on `idle_gap_rx`:
   - Spawn a `std::thread` that calls `ask_idle_gap(gap)` (blocking is fine on that thread)
     and sends `(gap, answer)` back over `popup_answer_rx`.
3. In `update()`, drain `popup_answer_rx` and apply the database mutations (same match
   arms as in `monitoring.rs` today).
4. Remove the now-dead popup-handling code from `monitoring.rs`.
5. Verify end-to-end: go idle → return → popup appears → answer → DB updated.

**Relevant Context:**
- [`src/idle_popup.rs`](src/idle_popup.rs) — `ask_idle_gap`, `IdleGapAnswer`
- [`src/monitoring.rs`](src/monitoring.rs:126) — old answer-handling code to migrate

---

### Sub-Task 4 — Main window: day view skeleton

**Status:** [x] done

**Intent:**
Open a proper named egui viewport as the "main window" when `show_main_window` is true.
The window contains a date navigation row at the top and a scrollable day-timeline area
below. At this stage the timeline area just renders the existing work entries as blue
blocks; no interaction yet.

The calendar is kept simple for now: an always-visible `← YYYY-MM-DD (Weekday) →` row.
A full month-grid with day colourization is planned for a later sub-task; the `selected_date`
field is already the right hook for it.

**Expected Outcomes:**
- Clicking the tray icon opens a titled, resizable window.
- Closing the window hides it (sets `show_main_window = false`) without quitting the app.
- A prev/next day navigation row is always visible at the top; the selected day defaults
  to today.
- The timeline paints work entries for the selected day as blue filled rectangles on a
  grey background track.
- Default visible time range is 07:00–20:00.
- The timeline is pannable with the mouse wheel and zoomable with Ctrl+wheel.

**Todo List:**
1. In `MainApp::update()`, when `show_main_window` is true, open a sub-viewport via
   `ctx.show_viewport_immediate(ViewportId::from_hash_of("main"), ...)`.
   Set `close_requested` callback to set `show_main_window = false`.
2. Add to `MainApp`:
   ```
   selected_date: NaiveDate,          // defaults to today
   view_start_secs: f32,              // default 7 * 3600
   view_end_secs: f32,                // default 20 * 3600
   ```
   (Pan/zoom state is managed inside the widget via egui temp data, keyed by widget id.)
3. Layout inside the main window viewport:
   - Top: horizontal row with "←" button, date label (e.g. "Mon 2025-06-02"), "→" button,
     and a "Today" button. Changing the date re-queries the DB.
   - Below: full-width day timeline widget (see step 4).
4. Day timeline widget (`draw_day_timeline()`):
  - The widget is **vertical**: time runs top-to-bottom. The visible range is the
    current view window (default 07:00–20:00). This orientation makes the day view
    feel like a calendar/planner and allows easy height-based zoom.
  - Draws the view range as a grey background with horizontal hour/half-hour grid lines
    and time labels on the left margin.
  - Renders each `WorktimeEntry` for the selected day as a blue filled rect spanning
    the full widget width, vertically positioned by time, clipped to the view range.
  - Mouse wheel (no modifier) pans up/down; Ctrl+wheel zooms (adjusts pixels-per-hour).
  - At this stage: no click/drag interaction yet.
5. Query the database for entries for the selected day and pass them to the widget.
6. Store pan/zoom state in egui temp data (keyed off the widget's `Id`) so it persists
  across frames without polluting `MainApp` state.

**Relevant Context:**
- [`src/idle_popup.rs`](src/idle_popup.rs:67) — `draw_range_slider` for painting patterns to reuse
- [`src/database.rs`](src/database.rs:126) — `Database::query` and `get_day_bounds`
- [`src/models.rs`](src/models.rs) — `WorktimeEntry`
- egui docs: `ctx.show_viewport_immediate`, `egui::Sense`, `egui::Painter`

---

### Sub-Task 5 — Day timeline: interactive editing

**Status:** [x] done

**Intent:**
Add the three interaction modes to the day timeline:
1. **Draw** (drag on empty space) → adds a new work entry, clipped to not overlap existing ones.
2. **Delete** (click on existing entry) → shows a small confirmation popup; deletes on confirm.
3. **Replace** (Shift+drag over existing entries) → replaces overlapping entries with the new range.

All changes are committed to the `Database` and saved to file immediately.

**Expected Outcomes:**
- Dragging on empty space shows a blue ghost while dragging; on release a new `WorktimeEntry`
  is inserted (sorted), overlapping time clipped away from existing entries.
- Clicking an existing blue block shows an inline egui popup ("Delete this entry? [Yes] [No]").
- Shift+drag replaces any entries that overlap the painted range with a single new entry
  covering exactly the painted range.
- All mutations call `database.insert_sorted` / remove from `database.rows` and then
  `database.store_file()`.
- The timeline redraws immediately to reflect the change.

**Interaction rules in detail:**
- "empty space" = no existing entry at the click x position.
- Overlap clipping for draw mode: before inserting, trim any existing entry whose end
  falls inside the new range (set its end = new range start) and trim any entry whose
  start falls inside the new range (set its start = new range end). Entries fully inside
  the new range are deleted.
- Replace mode (Shift): delete all entries that overlap the painted range, then insert
  the new entry.
- Snapping: same logic as the idle popup slider (5-minute snap for ranges > 10 min).

**Todo List:**
1. Add interaction state stored in egui temp data (same widget `Id` as pan/zoom):
   ```
   drag_anchor_secs: Option<f32>,
   drag_current_secs: Option<f32>,
   drag_mode: DragMode,   // enum: Draw | Replace
   pending_delete: Option<usize>,   // index into queried entries, pending confirm
   ```
2. In `draw_day_timeline()`, detect pointer events via `egui::Response`:
   - `drag_started()`: determine if click y-position falls inside an existing entry rect
     (→ potential delete intent on click, draw intent if they drag away from it) or
     empty space (→ draw intent); store anchor.
   - `dragged()`: update `drag_current_secs`; show semi-transparent ghost rect.
   - `drag_stopped()`: commit the mutation.
   - Check `ui.input(|i| i.modifiers.shift)` to switch to replace mode.
3. Implement the overlap-clipping helper:
   `fn clip_new_entry(entries: &mut Vec<WorktimeEntry>, new: &WorktimeEntry)`
4. Implement the delete confirmation: small egui `Window` positioned near the clicked
   entry showing "Delete?" with Yes/No buttons.
5. After any mutation: call `database.store_file()` and re-query for the selected day.

**Relevant Context:**
- [`src/idle_popup.rs`](src/idle_popup.rs:131) — drag anchor / x_to_time patterns to reuse
- [`src/database.rs`](src/database.rs:27) — `insert_sorted`, `store_file`
- [`src/models.rs`](src/models.rs) — `WorktimeEntry`

---

### Sub-Task 6 — Tray icon badge: hours worked today (with font rendering)

**Status:** [x] done

**Intent:**
Render the number of hours worked today into the tray icon image each frame so the user
can see their progress at a glance without opening the main window.

**Expected Outcomes:**
- The tray icon shows a small number (e.g. "6h") rendered as white text on a coloured
  background (green ≥ daily target, orange below target, grey < 1h).
- The icon updates at most once per minute (not every frame).
- Works on Linux/Sway/Niri via StatusNotifierItem.

**Todo List:**
1. Use the `ab_glyph` crate (already a transitive dependency of egui) to rasterize a
   digit string into a 32×32 RGBA pixel buffer. No new crate dependency needed.
   - Load a built-in font (e.g. egui's default font bytes, available as a `&[u8]` const)
     via `ab_glyph::FontRef`.
   - Draw coloured background, then render the text centred in the buffer.
2. In `MainApp`, track `last_icon_label: Option<String>` (e.g. `"6h"`).
3. Each frame (or on a 60-second timer via `ctx.request_repaint_after`):
   - Compute `today_minutes = database.get_day_sum(today).num_minutes()`.
   - Format as `"Xh"` (whole hours) or `"Xhm"` if < 1h.
   - If label changed, re-render the 32×32 RGBA buffer and call `tray.set_icon(...)`.
4. Colour scheme: grey `rgb(80,80,80)` when < 1h, orange `rgb(210,120,0)` below daily
   target, green `rgb(40,160,80)` at or above target. Daily target = `weekly_hours / 5`.

**Relevant Context:**
- [`src/database.rs`](src/database.rs:363) — `get_day_sum`
- [`src/config.rs`](src/config.rs) — `weekly_hours`
- tray-icon crate: `TrayIcon::set_icon`

---

## Implementation Order

Sub-tasks must be done in order: 1 → 2 → 3 → 4 → 5 → 6.
Each sub-task should compile and leave the app in a working state before starting the next.

## Out of Scope (for now)

- Full month-grid calendar with per-day colour coding (deferred; `selected_date` field is the hook)
- Week view or multi-day view
- Comments editing in the day view
- Tray icon menu with items beyond "Open" / "Quit"
- Windows / macOS support (Linux X11/Wayland / Sway / Niri is the target)
