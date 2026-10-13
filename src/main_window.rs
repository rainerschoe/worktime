//! Main egui application — day view + tray icon.
//!
//! Threading model:
//!   - Main thread: eframe/egui event loop (this file)
//!   - GTK/tray thread: dedicated thread running gtk_main (tray icon)
//!   - Monitoring thread: idle detection, auto-save (monitoring.rs)
//!
//! Communication:
//!   - Monitoring → UI:   mpsc channel of IdleGapInfo + `wake_ui()`
//!   - Popup:             deferred sub-viewport with its own repaint cycle
//!   - Tray → UI:         tray_icon event channels (try_recv each frame)

use chrono::{Local, NaiveDate};
use eframe::egui;
use crate::database::DbHandle;
use crate::idle_popup::{IdleGapInfo, IdleGapAnswer, IdlePopupState, draw_idle_popup};
use std::sync::{Arc, Mutex, OnceLock};

// ── Waking the UI thread from background threads ──────────────────────────────

/// Set once the egui context exists (see `MainApp::new`).
static UI_CTX: OnceLock<egui::Context> = OnceLock::new();

/// Ask the egui event loop to run a frame now.
///
/// Background threads must call this after pushing an idle-gap event, otherwise
/// the event is only picked up on the next frame that happens for some other
/// reason — which, when the main window is minimized, can be many seconds away.
pub fn wake_ui() {
    if let Some(ctx) = UI_CTX.get() {
        ctx.request_repaint();
    }
}

// ── Tray icon ─────────────────────────────────────────────────────────────────

static QUIT_ITEM_ID: once_cell::sync::OnceCell<tray_icon::menu::MenuId> =
    once_cell::sync::OnceCell::new();
static OPEN_ITEM_ID: once_cell::sync::OnceCell<tray_icon::menu::MenuId> =
    once_cell::sync::OnceCell::new();

/// Spawn a dedicated GTK thread that owns the tray icon.
/// GTK must run on its own thread; eframe/winit owns the main thread.
fn spawn_tray_thread() {
    // Initialise the badge slot before spawning so the GTK thread can find it.
    PENDING_BADGE.get_or_init(|| Mutex::new(None));

    std::thread::spawn(|| {
        gtk::init().expect("Failed to initialise GTK for tray icon");

        let open_item = tray_icon::menu::MenuItem::new("Open worktime", true, None);
        let quit_item = tray_icon::menu::MenuItem::new("Quit", true, None);
        let tray_menu = tray_icon::menu::Menu::new();
        tray_menu.append(&open_item).unwrap();
        tray_menu.append(&quit_item).unwrap();
        let _ = QUIT_ITEM_ID.set(quit_item.id().clone());
        let _ = OPEN_ITEM_ID.set(open_item.id().clone());

        let icon = solid_icon(32, 32, 80, 80, 80);
        let tray = tray_icon::TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("worktime")
            .with_menu(Box::new(tray_menu))
            .build()
            .expect("Failed to build tray icon");

        // Poll badge updates from the egui thread every second via a GLib timeout.
        glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
            if let Some(cell) = PENDING_BADGE.get() {
                if let Some(payload) = cell.lock().unwrap().take() {
                    if let Ok(icon) = tray_icon::Icon::from_rgba(payload.rgba, 32, 32) {
                        let _ = tray.set_icon(Some(icon));
                    }
                    let _ = tray.set_tooltip(Some(payload.tooltip));
                }
            }
            glib::ControlFlow::Continue
        });

        // GTK main loop — drives tray icon, its menu, and the badge timeout.
        gtk::main();
    });
}

fn solid_icon(w: u32, h: u32, r: u8, g: u8, b: u8) -> tray_icon::Icon {
    let rgba: Vec<u8> = std::iter::repeat([r, g, b, 255u8])
        .take((w * h) as usize)
        .flatten()
        .collect();
    tray_icon::Icon::from_rgba(rgba, w, h).expect("icon")
}

// ── MainApp ───────────────────────────────────────────────────────────────────

pub struct MainApp {
    db:           DbHandle,
    /// Receives idle-gap events from the monitoring thread.
    gap_rx:       Option<std::sync::mpsc::Receiver<IdleGapInfo>>,
    /// Pending idle-gap popup, if any.  Shared with the deferred viewport that
    /// renders it and that commits the user's answer (see `show_idle_popup`).
    idle_popup:   Option<Arc<Mutex<IdlePopupState>>>,

    selected_date:      NaiveDate,
    last_badge_label:   Option<String>,
    daily_target_hours: u32,
}

impl MainApp {
    pub fn new(
        db: DbHandle,
        gap_rx: Option<std::sync::mpsc::Receiver<IdleGapInfo>>,
        daily_target_hours: u32,
        cc: &eframe::CreationContext<'_>,
    ) -> Self {
        // Let background threads wake the event loop (idle-gap events).
        let _ = UI_CTX.set(cc.egui_ctx.clone());
        spawn_tray_thread();
        Self {
            db,
            gap_rx,
            idle_popup: None,
            selected_date: Local::now().date_naive(),
            last_badge_label: None,
            daily_target_hours,
        }
    }
}

// ── eframe::App ───────────────────────────────────────────────────────────────

impl eframe::App for MainApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {

        // ── Hide to tray on close ─────────────────────────────────────────────
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }

        // ── Tray events ───────────────────────────────────────────────────────
        while let Ok(event) = tray_icon::TrayIconEvent::receiver().try_recv() {
            if matches!(event,
                tray_icon::TrayIconEvent::Click { button: tray_icon::MouseButton::Left,
                    button_state: tray_icon::MouseButtonState::Up, .. }
                | tray_icon::TrayIconEvent::DoubleClick { .. })
            {
                self.show_window(ui.ctx());
            }
        }
        while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            if QUIT_ITEM_ID.get().map_or(false, |id| event.id == *id) {
                std::process::exit(0);
            }
            if OPEN_ITEM_ID.get().map_or(true, |id| event.id == *id) {
                self.show_window(ui.ctx());
            }
        }

        // ── Drain idle-gap channel → queue popup (only one at a time) ─────────
        if let Some(ref rx) = self.gap_rx {
            while let Ok(gap) = rx.try_recv() {
                // If a popup is already visible, discard older gaps so we never
                // stack multiple windows.  (In practice only one gap fires at a
                // time, but guard it anyway.)
                if self.idle_popup.is_none() {
                    self.idle_popup = Some(Arc::new(Mutex::new(IdlePopupState::new(gap))));
                }
            }
        }

        // ── Idle-gap popup ────────────────────────────────────────────────────
        self.show_idle_popup(ui.ctx());

        // ── Update tray badge ─────────────────────────────────────────────────
        self.update_tray_badge();

        // ── Day view ──────────────────────────────────────────────────────────
        egui::ScrollArea::vertical().show(ui, |ui| {
            crate::day_view::draw_day_view(ui, &mut self.selected_date, &self.db);
        });
    }
}

impl MainApp {
    /// Show the pending idle-gap popup as a **deferred** viewport.
    ///
    /// Why deferred and not `show_viewport_immediate`: an immediate viewport can
    /// only be laid out, receive input and react while *this* (root) window runs
    /// a frame — eframe bounces every event on an immediate viewport's window
    /// into "repaint the parent window instead".  But the popup shows up exactly
    /// when the main window is minimized to the tray (or obscured), and a
    /// Wayland surface in that state gets frame callbacks only sporadically, so
    /// winit emits `RedrawRequested` for the root only every few seconds.  The
    /// popup then appeared to freeze: a click was not *processed* until the next
    /// root frame, seconds later.
    ///
    /// A deferred viewport has its own repaint cycle driven by its own (visible,
    /// focused) window, so it runs at full speed regardless of the root.  For the
    /// same reason everything that must happen promptly — including the database
    /// write — is done inside the popup's own callback; the root viewport is only
    /// needed afterwards to drop the viewport and thereby close the window.
    fn show_idle_popup(&mut self, ctx: &egui::Context) {
        let Some(state) = self.idle_popup.clone() else { return };

        let (answered, gap) = {
            let popup = state.lock().unwrap();
            (popup.committed, popup.gap.clone())
        };

        // The popup already handled and committed the answer in its own pass:
        // stop listing the viewport so eframe destroys the window.
        if answered {
            self.idle_popup = None;
            return;
        }

        let title = format!(
            "Idle {}  ({} → {})",
            crate::utils::format_chrono_duration(&gap.duration()),
            gap.gap_start.format("%H:%M"),
            gap.gap_end.format("%H:%M"),
        );

        let db = self.db.clone();
        ctx.show_viewport_deferred(
            egui::ViewportId::from_hash_of("idle_popup"),
            egui::ViewportBuilder::default()
                .with_title(title)
                .with_inner_size([520.0, 320.0])
                .with_resizable(false)
                .with_always_on_top(),
            move |ui, _class| {
                let popup_ctx = ui.ctx().clone();
                let mut popup = state.lock().unwrap();

                // Closing the window counts as "it was all idle time".
                if popup.answer.is_none()
                    && popup_ctx.input(|i| i.viewport().close_requested())
                {
                    popup.answer = Some(IdleGapAnswer::Idle);
                }

                egui::CentralPanel::default().show(ui, |ui| {
                    if popup.committed {
                        // Answer is already in the database; we are only waiting
                        // for the main window to run a frame and close us.
                        ui.add_space(40.0);
                        ui.vertical_centered(|ui| {
                            ui.heading("Saved ✔");
                        });
                        return;
                    }
                    draw_idle_popup(ui, &mut popup);
                });

                if let Some(answer) = popup.answer.clone() {
                    if !popup.committed {
                        popup.committed = true;
                        commit_idle_answer(&db, answer);
                        // Ask to close, and wake the root viewport so it drops
                        // this viewport (which is what actually closes it).
                        popup_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        popup_ctx.request_repaint_of(egui::ViewportId::ROOT);
                    }
                }
            },
        );
    }

    fn show_window(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    }

    fn update_tray_badge(&mut self) {
        let today         = Local::now().date_naive();
        let today_mins    = self.db.today_minutes();
        let label         = format!("{}", today_mins / 60);

        if self.last_badge_label.as_deref() == Some(label.as_str()) {
            return;
        }
        self.last_badge_label = Some(label.clone());

        let target_mins = self.daily_target_hours * 60;
        let (r, g, b): (u8, u8, u8) = if today_mins < 60 { (80, 80, 80) }
            else if today_mins < target_mins { (210, 120, 0) }
            else { (40, 160, 80) };

        // Store the badge update for the GTK tray thread to pick up.
        let rgba    = render_badge_rgba(32, 32, &label, r, g, b);
        let tooltip = format!("worktime — {}h {:02}m today", today_mins / 60, today_mins % 60);
        if let Some(cell) = PENDING_BADGE.get() {
            *cell.lock().unwrap() = Some(BadgePayload { rgba, tooltip });
        }

        if self.selected_date != today {
            self.selected_date = today;
        }
    }
}

// ── Idle-gap answer → database ────────────────────────────────────────────────

/// Write the user's answer to the database.  Called from the popup viewport's
/// own pass, so it does not depend on the main window running a frame.
fn commit_idle_answer(db: &DbHandle, answer: IdleGapAnswer) {
    use crate::models::WorktimeEntry;
    match answer {
        IdleGapAnswer::WorkTime { gap_start, gap_end } => {
            db.insert_and_normalize(WorktimeEntry {
                start: gap_start, end: gap_end, comments: String::new(),
            });
        }
        IdleGapAnswer::Idle => {}
        IdleGapAnswer::Adjusted { work_start, work_end } => {
            db.insert_and_normalize(WorktimeEntry {
                start: work_start, end: work_end, comments: String::new(),
            });
        }
    }
}

// ── Tray badge cross-thread update ────────────────────────────────────────────
// The egui thread writes a pending badge here; the GTK tray thread polls it.

pub(crate) struct BadgePayload { pub rgba: Vec<u8>, pub tooltip: String }

pub(crate) static PENDING_BADGE: OnceLock<Mutex<Option<BadgePayload>>> = OnceLock::new();

// ── Badge helpers ─────────────────────────────────────────────────────────────

fn render_badge_rgba(w: u32, h: u32, label: &str, bg_r: u8, bg_g: u8, bg_b: u8) -> Vec<u8> {
    let mut buf = vec![0u8; (w * h * 4) as usize];
    for py in 0..h { for px in 0..w {
        let i = ((py * w + px) * 4) as usize;
        buf[i] = bg_r; buf[i+1] = bg_g; buf[i+2] = bg_b; buf[i+3] = 255;
    }}
    let scale = 2u32; let gw = 5u32; let gh = 7u32; let gap = 1u32;
    let n = label.chars().count() as u32;
    if n == 0 { return buf; }
    let tw = (n * (gw + gap) - gap) * scale;
    let sx = ((w.saturating_sub(tw)) / 2) as i32;
    let sy = ((h.saturating_sub(gh * scale)) / 2) as i32;
    for (i, ch) in label.chars().enumerate() {
        let glyph = char_bitmap_5x7(ch);
        let cx = sx + (i as i32) * ((gw + gap) * scale) as i32;
        for (row, &bits) in glyph.iter().enumerate() {
            for col in 0..gw {
                if bits & (1 << (gw - 1 - col)) != 0 {
                    for dy in 0..scale { for dx in 0..scale {
                        let px = cx + (col * scale + dx) as i32;
                        let py = sy + (row as u32 * scale + dy) as i32;
                        if px >= 0 && px < w as i32 && py >= 0 && py < h as i32 {
                            let i = ((py as u32 * w + px as u32) * 4) as usize;
                            buf[i]=255; buf[i+1]=255; buf[i+2]=255; buf[i+3]=255;
                        }
                    }}
                }
            }
        }
    }
    buf
}

fn char_bitmap_5x7(ch: char) -> [u8; 7] {
    match ch {
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111],
        '3' => [0b11111, 0b00010, 0b00100, 0b00110, 0b00001, 0b10001, 0b01110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        _   => [0b00000; 7],
    }
}
