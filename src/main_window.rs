//! Main egui application — owns the tray icon, the main window, and
//! the idle-gap popup dispatch.
//!
//! Architecture:
//! - The root eframe viewport IS the main window (day view).
//! - Closing the window hides it to tray (sets `window_visible = false` and
//!   calls `ViewportCommand::Minimized(true)`); the window is not destroyed.
//! - Clicking the tray icon un-hides the window.
//! - The tray icon lives on a dedicated GTK thread (Linux requirement).

use std::sync::{Arc, Mutex};
use chrono::{Local, NaiveDate};

use eframe::egui;
use crate::database::Database;
use crate::idle_popup::IdleGapInfo;

// ── Tray icon setup ───────────────────────────────────────────────────────────

/// A badge update message sent from the egui thread to the GTK/tray thread.
pub struct BadgeUpdate {
    pub rgba:    Vec<u8>,   // 32×32 RGBA pixels
    pub tooltip: String,
}

/// Spawn a dedicated GTK thread for the tray icon.  Returns a `Sender` the
/// egui thread can use to push badge updates.
fn spawn_tray_thread() -> std::sync::mpsc::Sender<BadgeUpdate> {
    let (badge_tx, badge_rx) = std::sync::mpsc::channel::<BadgeUpdate>();

    std::thread::spawn(move || {
        gtk::init().expect("Failed to initialise GTK for tray icon");

        let icon = make_tray_icon_rgba(32, 32, 80, 80, 80);

        let open_item = tray_icon::menu::MenuItem::new("Open worktime", true, None);
        let quit_item = tray_icon::menu::MenuItem::new("Quit",          true, None);
        let tray_menu = tray_icon::menu::Menu::new();
        tray_menu.append(&open_item).unwrap();
        tray_menu.append(&quit_item).unwrap();

        let _ = QUIT_ITEM_ID.set(quit_item.id().clone());
        let _ = OPEN_ITEM_ID.set(open_item.id().clone());

        let mut tray = tray_icon::TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("worktime")
            .with_menu(Box::new(tray_menu))
            .build()
            .expect("Failed to build tray icon");

        // Drive the GTK event loop ourselves so we can also drain the badge channel.
        loop {
            while unsafe { gtk_sys::gtk_events_pending() } != 0 {
                unsafe { gtk_sys::gtk_main_iteration_do(0) };
            }
            while let Ok(update) = badge_rx.try_recv() {
                if let Ok(new_icon) = tray_icon::Icon::from_rgba(update.rgba, 32, 32) {
                    let _ = tray.set_icon(Some(new_icon));
                }
                let _ = tray.set_tooltip(Some(update.tooltip));
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });

    badge_tx
}

fn make_tray_icon_rgba(w: u32, h: u32, r: u8, g: u8, b: u8) -> tray_icon::Icon {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..(w * h) {
        rgba.extend_from_slice(&[r, g, b, 255]);
    }
    tray_icon::Icon::from_rgba(rgba, w, h).expect("icon")
}

static QUIT_ITEM_ID: once_cell::sync::OnceCell<tray_icon::menu::MenuId> =
    once_cell::sync::OnceCell::new();
static OPEN_ITEM_ID: once_cell::sync::OnceCell<tray_icon::menu::MenuId> =
    once_cell::sync::OnceCell::new();

// ── MainApp ───────────────────────────────────────────────────────────────────

pub struct MainApp {
    pub database:    Arc<Mutex<Database>>,
    pub idle_gap_rx: Option<std::sync::mpsc::Receiver<IdleGapInfo>>,

    popup_answer_rx: std::sync::mpsc::Receiver<(IdleGapInfo, crate::idle_popup::IdleGapAnswer)>,
    popup_answer_tx: std::sync::mpsc::Sender<(IdleGapInfo, crate::idle_popup::IdleGapAnswer)>,

    /// Whether the window should be visible. When false we keep it minimized.
    window_visible:  bool,
    pub selected_date: NaiveDate,

    badge_tx:           std::sync::mpsc::Sender<BadgeUpdate>,
    last_badge_label:   Option<String>,
    daily_target_hours: u32,
}

impl MainApp {
    pub fn new(
        database: Arc<Mutex<Database>>,
        idle_gap_rx: Option<std::sync::mpsc::Receiver<IdleGapInfo>>,
        daily_target_hours: u32,
        _cc: &eframe::CreationContext<'_>,
    ) -> Self {
        let badge_tx = spawn_tray_thread();
        let (popup_tx, popup_rx) = std::sync::mpsc::channel();
        Self {
            database,
            idle_gap_rx,
            popup_answer_rx: popup_rx,
            popup_answer_tx: popup_tx,
            window_visible: true,
            selected_date: Local::now().date_naive(),
            badge_tx,
            last_badge_label: None,
            daily_target_hours,
        }
    }
}

// ── eframe::App impl ─────────────────────────────────────────────────────────

impl eframe::App for MainApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {

        // ── Handle window close → hide to tray instead of quitting ────────────
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            // Cancel the close and minimize instead.
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            self.window_visible = false;
        }

        // ── Poll tray icon click events (non-Linux; Linux uses menu) ──────────
        while let Ok(event) = tray_icon::TrayIconEvent::receiver().try_recv() {
            if matches!(
                event,
                tray_icon::TrayIconEvent::Click {
                    button: tray_icon::MouseButton::Left,
                    button_state: tray_icon::MouseButtonState::Up,
                    ..
                } | tray_icon::TrayIconEvent::DoubleClick { .. }
            ) {
                self.show_window(ui.ctx());
            }
        }

        // ── Poll tray menu events ─────────────────────────────────────────────
        while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            if QUIT_ITEM_ID.get().map_or(false, |id| event.id == *id) {
                std::process::exit(0);
            }
            if OPEN_ITEM_ID.get().map_or(true, |id| event.id == *id) {
                self.show_window(ui.ctx());
            }
        }

        // ── Poll idle-gap channel ─────────────────────────────────────────────
        if let Some(ref rx) = self.idle_gap_rx {
            while let Ok(gap) = rx.try_recv() {
                let tx = self.popup_answer_tx.clone();
                std::thread::spawn(move || {
                    let answer = crate::idle_popup::ask_idle_gap(gap.clone());
                    let _ = tx.send((gap, answer));
                });
            }
        }

        // ── Apply popup answers ───────────────────────────────────────────────
        while let Ok((gap, answer)) = self.popup_answer_rx.try_recv() {
            use crate::idle_popup::IdleGapAnswer;
            use crate::models::WorktimeEntry;
            let mut db = self.database.lock().unwrap();
            match answer {
                IdleGapAnswer::WorkTime => {
                    db.insert_sorted(WorktimeEntry {
                        start: gap.gap_start, end: gap.gap_end,
                        comments: String::new(),
                    });
                    println!("Idle gap {} → {} added as work time.",
                        gap.gap_start.format("%H:%M:%S"), gap.gap_end.format("%H:%M:%S"));
                }
                IdleGapAnswer::Adjusted { work_start, work_end } => {
                    db.insert_sorted(WorktimeEntry {
                        start: work_start, end: work_end,
                        comments: String::new(),
                    });
                    println!("Idle gap adjusted: {} → {} added as work time.",
                        work_start.format("%H:%M:%S"), work_end.format("%H:%M:%S"));
                }
                IdleGapAnswer::Idle => {}
            }
        }

        // ── Update tray badge ─────────────────────────────────────────────────
        self.update_tray_badge();

        // ── Draw day view (fills the rest of the panel) ───────────────────────
        egui::ScrollArea::vertical().show(ui, |ui| {
            crate::day_view::draw_day_view(ui, &mut self.selected_date, &self.database);
        });

        // Wake up twice per second to notice tray/channel events even when idle.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }
}

impl MainApp {
    fn show_window(&mut self, ctx: &egui::Context) {
        self.window_visible = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    }
}

// ── Tray badge rendering ──────────────────────────────────────────────────────

impl MainApp {
    fn update_tray_badge(&mut self) {
        let today = Local::now().date_naive();
        let today_mins = {
            let db = self.database.lock().unwrap();
            db.get_day_sum(Local::now()).num_minutes() as u32
        };

        let label = badge_label(today_mins);
        if self.last_badge_label.as_deref() == Some(label.as_str()) {
            return;
        }
        self.last_badge_label = Some(label.clone());

        let target_mins = self.daily_target_hours * 60;
        let (r, g, b): (u8, u8, u8) = if today_mins < 60 {
            (80, 80, 80)
        } else if today_mins < target_mins {
            (210, 120, 0)
        } else {
            (40, 160, 80)
        };

        let rgba    = render_badge_rgba(32, 32, &label, r, g, b);
        let tooltip = format!("worktime — {} today", format_mins(today_mins));
        let _ = self.badge_tx.send(BadgeUpdate { rgba, tooltip });

        if self.selected_date != today {
            self.selected_date = today;
        }
    }
}

// ── Badge helpers ─────────────────────────────────────────────────────────────

/// Badge label: whole hours worked. "0" when < 1h, "6" at 6h, "12" at 12h+.
fn badge_label(total_mins: u32) -> String {
    format!("{}", total_mins / 60)
}

fn format_mins(total_mins: u32) -> String {
    let h = total_mins / 60;
    let m = total_mins % 60;
    if h == 0 { format!("{}m", m) } else { format!("{}h {:02}m", h, m) }
}

/// Render the badge into a 32×32 RGBA buffer.
///
/// Uses a 5×7 pixel bitmap font with 2× pixel scaling so each logical pixel
/// maps to a 2×2 block of screen pixels — giving ~10px-wide digits on a 32×32
/// icon, which is clearly legible in a system tray.
fn render_badge_rgba(w: u32, h: u32, label: &str, bg_r: u8, bg_g: u8, bg_b: u8) -> Vec<u8> {
    let mut buf = vec![0u8; (w * h * 4) as usize];

    // ── Fill background ───────────────────────────────────────────────────────
    for py in 0..h {
        for px in 0..w {
            let idx = ((py * w + px) * 4) as usize;
            buf[idx]     = bg_r;
            buf[idx + 1] = bg_g;
            buf[idx + 2] = bg_b;
            buf[idx + 3] = 255;
        }
    }

    // ── Render text with 2× pixel scaling ────────────────────────────────────
    // Each glyph is 5 logical pixels wide × 7 tall.
    // At scale=2: each glyph occupies 10×14 screen pixels, +2px gap = 12px/char.
    let scale:   u32 = 2;
    let glyph_w: u32 = 5;  // logical pixels wide
    let glyph_h: u32 = 7;  // logical pixels tall
    let gap:     u32 = 1;  // logical gap between chars
    let char_count = label.chars().count() as u32;

    if char_count == 0 { return buf; }
    // Total scaled text width: N glyphs + (N-1) gaps, all scaled
    let text_w_px = (char_count * (glyph_w + gap) - gap) * scale;
    let text_h_px = glyph_h * scale;

    // Centre in the icon
    let start_x = ((w.saturating_sub(text_w_px)) / 2) as i32;
    let start_y = ((h.saturating_sub(text_h_px)) / 2) as i32;

    for (i, ch) in label.chars().enumerate() {
        let glyph = char_bitmap_5x7(ch);
        let char_x = start_x + (i as i32) * ((glyph_w + gap) * scale) as i32;

        for (row, &row_bits) in glyph.iter().enumerate() {
            for col in 0..glyph_w {
                // MSB of the 5-bit row mask = leftmost column
                if row_bits & (1 << (glyph_w - 1 - col)) != 0 {
                    // Paint a scale×scale block
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let px = char_x + (col * scale + dx) as i32;
                            let py = start_y + (row as u32 * scale + dy) as i32;
                            if px >= 0 && px < w as i32 && py >= 0 && py < h as i32 {
                                let idx = ((py as u32 * w + px as u32) * 4) as usize;
                                buf[idx]     = 255;
                                buf[idx + 1] = 255;
                                buf[idx + 2] = 255;
                                buf[idx + 3] = 255;
                            }
                        }
                    }
                }
            }
        }
    }

    buf
}

/// 5×7 pixel bitmap font, one byte per row (5 bits used, MSB = leftmost column).
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
