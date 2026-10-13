//! Graphical popup asking the user whether an idle gap was actually work time.
//!
//! The popup is rendered as a *deferred* sub-viewport from the main egui app
//! (see `main_window::MainApp::show_idle_popup`), so it runs on the main thread
//! but has its own repaint cycle: it stays responsive even while the main
//! window is minimized to the tray or obscured and therefore gets no frames
//! from the compositor. The old `run_native`-based `ask_idle_gap` is kept for
//! the `test_popup` binary but is no longer used by the main app.

use chrono::{DateTime, Local, Duration, TimeZone};
use eframe::egui;
use crate::utils::format_chrono_duration;

/// The user's answer to the idle-gap question.
#[derive(Debug, Clone, PartialEq)]
pub enum IdleGapAnswer {
    /// The gap was entirely work time — insert the full gap into the database.
    WorkTime { gap_start: DateTime<Local>, gap_end: DateTime<Local> },
    /// The gap was genuinely idle — do nothing.
    Idle,
    /// The user selected a sub-range of the gap as work time.
    Adjusted {
        work_start: DateTime<Local>,
        work_end:   DateTime<Local>,
    },
}

/// Description of the idle gap shown to the user.
#[derive(Debug, Clone)]
pub struct IdleGapInfo {
    pub gap_start: DateTime<Local>,
    pub gap_end:   DateTime<Local>,
}

impl IdleGapInfo {
    pub fn duration(&self) -> Duration {
        self.gap_end - self.gap_start
    }
}

// ── Public viewport-based popup state ────────────────────────────────────────

/// Mutable state for one pending idle-gap popup.
/// Lives in `MainApp` behind an `Arc<Mutex<_>>`, shared with the deferred
/// viewport that renders it.
#[derive(Clone)]
pub struct IdlePopupState {
    pub gap:        IdleGapInfo,
    pub work_start: DateTime<Local>,
    pub work_end:   DateTime<Local>,
    /// Set when the user clicks a button (or closes the popup window).
    pub answer:     Option<IdleGapAnswer>,
    /// True once `answer` has been written to the database.  The popup repaints
    /// itself after answering, so this keeps the write from happening twice.
    pub committed:  bool,
}

impl IdlePopupState {
    pub fn new(gap: IdleGapInfo) -> Self {
        let work_start = gap.gap_start;
        let work_end   = gap.gap_end;
        Self { gap, work_start, work_end, answer: None, committed: false }
    }
}

/// Draw the popup UI into `ui`. Returns true when the user has answered
/// (i.e. `state.answer` has been set). Caller should close the viewport.
pub fn draw_idle_popup(ui: &mut egui::Ui, state: &mut IdlePopupState) {
    ui.add_space(12.0);
    ui.heading("Were you working?");
    ui.add_space(8.0);

    let dur = state.gap.duration();
    ui.label(format!(
        "No activity was detected for {}.",
        format_chrono_duration(&dur),
    ));
    ui.label(format!(
        "Gap:  {}  →  {}",
        state.gap.gap_start.format("%H:%M:%S"),
        state.gap.gap_end.format("%H:%M:%S"),
    ));

    ui.add_space(10.0);
    ui.label("Were you in a meeting or otherwise working away from the keyboard?");
    ui.add_space(12.0);

    ui.horizontal(|ui| {
        if ui.add_sized([145.0, 36.0],
            egui::Button::new(egui::RichText::new("✔  All Work Time").size(14.0))
        ).clicked() {
            state.answer = Some(IdleGapAnswer::WorkTime {
                gap_start: state.gap.gap_start,
                gap_end:   state.gap.gap_end,
            });
        }
        ui.add_space(8.0);
        if ui.add_sized([130.0, 36.0],
            egui::Button::new(egui::RichText::new("✘  All Idle").size(14.0))
        ).clicked() {
            state.answer = Some(IdleGapAnswer::Idle);
        }
        ui.add_space(8.0);
        if ui.add_sized([180.0, 36.0],
            egui::Button::new(egui::RichText::new("⏱  Save adjusted work time").size(14.0))
        ).clicked() {
            state.answer = Some(IdleGapAnswer::Adjusted {
                work_start: state.work_start,
                work_end:   state.work_end,
            });
        }
    });

    ui.add_space(16.0);
    ui.label("Click and drag to paint the work window:");
    ui.add_space(6.0);

    let changed = draw_range_slider(ui, &state.gap, &mut state.work_start, &mut state.work_end);
    if changed {
        ui.ctx().request_repaint();
    }

    ui.add_space(20.0);

    let work_dur = state.work_end - state.work_start;
    ui.label(
        egui::RichText::new(format!(
            "Work: {}  →  {}   ({})",
            state.work_start.format("%H:%M"),
            state.work_end.format("%H:%M"),
            format_chrono_duration(&work_dur),
        ))
        .strong(),
    );

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);
    ui.weak("worktime idle-gap tracker");
}

// ── Internal egui app (kept for test_popup binary) ───────────────────────────

struct IdlePopupApp {
    gap:        IdleGapInfo,
    work_start: DateTime<Local>,
    work_end:   DateTime<Local>,
    answer:     Option<IdleGapAnswer>,
}

impl IdlePopupApp {
    fn new(gap: IdleGapInfo, _cc: &eframe::CreationContext<'_>) -> Self {
        let work_start = gap.gap_start;
        let work_end   = gap.gap_end;
        Self { gap, work_start, work_end, answer: None }
    }
}

// ── Paint-to-select range widget ─────────────────────────────────────────────

/// Draw a horizontal timeline strip for `gap`.
///
/// The user click-drags anywhere on (or slightly outside) the track to paint
/// the work window.  The painted range snaps to 5-minute boundaries when the
/// gap is > 10 minutes.  Returns `true` if the selection changed.
///
/// Extra horizontal padding lets the user start/finish a drag slightly outside
/// the track to reach exact gap_start / gap_end.
fn draw_range_slider(
    ui:         &mut egui::Ui,
    gap:        &IdleGapInfo,
    work_start: &mut DateTime<Local>,
    work_end:   &mut DateTime<Local>,
) -> bool {
    let total_secs = gap.duration().num_seconds().max(1) as f32;

    // Extra pixels beyond the track ends where dragging is still accepted,
    // making it easy to paint all the way to gap_start / gap_end.
    let overhang = 18.0_f32;
    let track_h  = 16.0_f32; // taller bar — easier to hit, no handles needed
    let label_h  = 16.0_f32; // height reserved below track for grid labels

    let desired_size = egui::vec2(ui.available_width(), track_h + label_h + 6.0);
    let (rect, resp) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());

    // The visual track sits inset by overhang; the interaction area is the full rect.
    let track_left  = rect.left()  + overhang;
    let track_right = rect.right() - overhang;
    let track_width = (track_right - track_left).max(1.0);
    let track_y_top = rect.top()   + 2.0;
    let track_y_bot = track_y_top  + track_h;

    // ── Snap intervals ────────────────────────────────────────────────────────
    // Same candidate list as the grid.  The grid picks the coarsest interval
    // that yields ≤8 lines; snapping uses one level finer (previous entry).
    let candidates: &[i64] = &[60, 300, 600, 900, 1800, 3600, 7200, 10800];
    let gap_secs = gap.duration().num_seconds().max(1);
    let grid_idx = candidates.iter().position(|&iv| gap_secs / iv <= 8)
        .unwrap_or(candidates.len() - 1);
    let grid_interval = candidates[grid_idx];
    // One finer level: index - 1, minimum 60 s (1 minute).
    let snap_interval = if grid_idx > 0 { candidates[grid_idx - 1] } else { candidates[0] };

    // How many pixels wide one snap unit is — used to detect "near edge".
    let _snap_px = (snap_interval as f32 / total_secs) * track_width;
    // Snap a raw pixel x to the nearest snap boundary, with edge-snapping.
    let x_to_time = |x: f32| -> DateTime<Local> {
        let frac = ((x - track_left) / track_width).clamp(0.0, 1.0);
        let secs = (frac * total_secs) as i64;

        // If within half a snap-unit of gap_start or gap_end, snap to the edge.
        let half = snap_interval / 2;
        if secs <= half {
            return gap.gap_start;
        }
        if secs >= gap_secs - half {
            return gap.gap_end;
        }

        // Otherwise round to nearest snap_interval boundary (absolute clock time).
        let abs_secs = gap.gap_start.timestamp() + secs;
        let snapped_abs = ((abs_secs + half) / snap_interval) * snap_interval;
        let snapped_rel = (snapped_abs - gap.gap_start.timestamp())
            .clamp(0, gap_secs);
        gap.gap_start + Duration::seconds(snapped_rel)
    };
    // DateTime → x pixel on track.
    let time_to_x = |t: DateTime<Local>| -> f32 {
        let offset = (t - gap.gap_start).num_seconds().max(0) as f32;
        track_left + (offset / total_secs) * track_width
    };

    // ── Drag: record press_x in memory, update selection live ─────────────────
    let mem_id = resp.id.with("paint_anchor");

    if resp.drag_started() {
        if let Some(pos) = resp.interact_pointer_pos() {
            ui.ctx().data_mut(|d| d.insert_temp::<f32>(mem_id, pos.x));
        }
    }
    let anchor_x: Option<f32> = ui.ctx().data(|d| d.get_temp(mem_id));
    if resp.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove_temp::<f32>(mem_id));
    }

    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }

    let mut changed = false;
    if resp.dragged() {
        if let (Some(anchor), Some(cur)) = (anchor_x, resp.interact_pointer_pos()) {
            let lo = anchor.min(cur.x);
            let hi = anchor.max(cur.x);
            let new_start = x_to_time(lo);
            let new_end   = x_to_time(hi);
            // Ensure at least 1 snap interval of width.
            let min_dur = Duration::seconds(snap_interval);
            let new_end = new_end.max(new_start + min_dur).min(gap.gap_end);
            if new_start != *work_start || new_end != *work_end {
                *work_start = new_start;
                *work_end   = new_end;
                changed = true;
            }
        }
    }

    // ── Paint ─────────────────────────────────────────────────────────────────
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let accent  = egui::Color32::from_rgb(59, 130, 212);

        // Background track
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(track_left,  track_y_top),
                egui::pos2(track_right, track_y_bot),
            ),
            4.0,
            egui::Color32::from_gray(55),
        );

        // ── Grid lines ───────────────────────────────────────────────────────
        // (grid_interval already computed above alongside snap_interval)
        let gap_start_secs  = gap.gap_start.timestamp();
        let first_tick_secs = ((gap_start_secs / grid_interval) + 1) * grid_interval;
        let font_grid       = egui::FontId::proportional(10.0);
        let label_grid_y    = track_y_bot + 3.0;
        let weak_color      = ui.visuals().weak_text_color();
        let tick_color      = egui::Color32::from_rgba_unmultiplied(
            weak_color.r(), weak_color.g(), weak_color.b(), 180);

        let mut tick_secs = first_tick_secs;
        while tick_secs < gap_start_secs + gap_secs {
            let frac = (tick_secs - gap_start_secs) as f32 / gap_secs as f32;
            let tx   = track_left + frac * track_width;

            painter.line_segment(
                [egui::pos2(tx, track_y_top), egui::pos2(tx, track_y_bot)],
                egui::Stroke::new(1.0, tick_color),
            );

            let dt    = chrono::Local.timestamp_opt(tick_secs, 0).unwrap();
            let label = if grid_interval < 3600 {
                dt.format(":%M").to_string()
            } else {
                dt.format("%H:%M").to_string()
            };
            painter.text(
                egui::pos2(tx, label_grid_y),
                egui::Align2::CENTER_TOP,
                label,
                font_grid.clone(),
                weak_color,
            );

            tick_secs += grid_interval;
        }

        // Work window highlight (on top of grid)
        let sel_left  = time_to_x(*work_start);
        let sel_right = time_to_x(*work_end);
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(sel_left,  track_y_top),
                egui::pos2(sel_right, track_y_bot),
            ),
            3.0,
            accent,
        );

        // Live drag ghost: show the raw painted region before snap settles
        if resp.dragged() {
            if let (Some(anchor), Some(cur)) = (anchor_x, resp.interact_pointer_pos()) {
                let lo = anchor.min(cur.x).max(track_left);
                let hi = anchor.max(cur.x).min(track_right);
                if hi > lo {
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(lo, track_y_top),
                            egui::pos2(hi, track_y_bot),
                        ),
                        3.0,
                        egui::Color32::from_rgba_unmultiplied(120, 180, 255, 80),
                    );
                }
            }
        }

        // End-point time labels — anchored to the widget rect edges with a
        // small inset so they are never clipped by the window border.
        let font_edge   = egui::FontId::proportional(11.0);
        let edge_color  = ui.visuals().text_color();
        let left_inset  = rect.left() + 4.0;
        let right_inset = rect.right() - 4.0;
        painter.text(
            egui::pos2(left_inset,  label_grid_y),
            egui::Align2::LEFT_TOP,
            gap.gap_start.format("%H:%M").to_string(),
            font_edge.clone(),
            edge_color,
        );
        painter.text(
            egui::pos2(right_inset, label_grid_y),
            egui::Align2::RIGHT_TOP,
            gap.gap_end.format("%H:%M").to_string(),
            font_edge.clone(),
            edge_color,
        );
    }

    changed
}

// ── App impl ─────────────────────────────────────────────────────────────────

impl eframe::App for IdlePopupApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Close as soon as an answer was recorded (next frame after click)
        if self.answer.is_some() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        ui.add_space(12.0);
        ui.heading("Were you working?");
        ui.add_space(8.0);

        let dur = self.gap.duration();
        ui.label(format!(
            "No activity was detected for {}.",
            format_chrono_duration(&dur),
        ));
        ui.label(format!(
            "Gap:  {}  →  {}",
            self.gap.gap_start.format("%H:%M:%S"),
            self.gap.gap_end.format("%H:%M:%S"),
        ));

        ui.add_space(10.0);
        ui.label("Were you in a meeting or otherwise working away from the keyboard?");
        ui.add_space(12.0);

        // ── Three action buttons ─────────────────────────────────────────────
        ui.horizontal(|ui| {
            let all_work_btn = ui.add_sized(
                [145.0, 36.0],
                egui::Button::new(egui::RichText::new("✔  All Work Time").size(14.0)),
            );
            if all_work_btn.clicked() {
                self.answer = Some(IdleGapAnswer::WorkTime {
                    gap_start: self.gap.gap_start,
                    gap_end:   self.gap.gap_end,
                });
            }

            ui.add_space(8.0);

            let all_idle_btn = ui.add_sized(
                [130.0, 36.0],
                egui::Button::new(egui::RichText::new("✘  All Idle").size(14.0)),
            );
            if all_idle_btn.clicked() {
                self.answer = Some(IdleGapAnswer::Idle);
            }

            ui.add_space(8.0);

            let save_btn = ui.add_sized(
                [180.0, 36.0],
                egui::Button::new(egui::RichText::new("⏱  Save adjusted work time").size(14.0)),
            );
            if save_btn.clicked() {
                self.answer = Some(IdleGapAnswer::Adjusted {
                    work_start: self.work_start,
                    work_end:   self.work_end,
                });
            }
        });

        ui.add_space(16.0);

        // ── Timeline slider ──────────────────────────────────────────────────
        ui.label("Click and drag to paint the work window:");
        ui.add_space(6.0);

        let changed = draw_range_slider(
            ui,
            &self.gap,
            &mut self.work_start,
            &mut self.work_end,
        );
        if changed {
            ui.ctx().request_repaint();
        }

        ui.add_space(20.0); // room for the time labels painted below the track

        // Live summary label
        let work_dur = self.work_end - self.work_start;
        ui.label(
            egui::RichText::new(format!(
                "Work: {}  →  {}   ({})",
                self.work_start.format("%H:%M"),
                self.work_end.format("%H:%M"),
                format_chrono_duration(&work_dur),
            ))
            .strong(),
        );

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        ui.weak("worktime idle-gap tracker");
    }
}

// ── Answer-extraction wrapper ─────────────────────────────────────────────────
// eframe 0.35 destroys the App after run_native returns, so we use a shared
// Arc<Mutex<_>> to ferry the answer back to the caller.

struct AnswerExtractor {
    inner: IdlePopupApp,
    out: std::sync::Arc<std::sync::Mutex<IdleGapAnswer>>,
}

impl eframe::App for AnswerExtractor {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.inner.ui(ui, frame);
        if let Some(ref ans) = self.inner.answer {
            *self.out.lock().unwrap() = ans.clone();
        }
    }
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Spawn a blocking egui popup window that asks the user whether the idle gap
/// was work time.  Returns `IdleGapAnswer::Idle` on any window-close / error.
///
/// **Blocks** the calling thread until the user clicks a button or closes the
/// window.  Must be called from a dedicated thread (not the main UI thread if
/// one is already running).
pub fn ask_idle_gap(gap: IdleGapInfo) -> IdleGapAnswer {
    let dur = gap.duration();
    let title = format!(
        "Idle {}  ({} → {})",
        format_chrono_duration(&dur),
        gap.gap_start.format("%H:%M"),
        gap.gap_end.format("%H:%M"),
    );

    let answer_cell = std::sync::Arc::new(std::sync::Mutex::new(IdleGapAnswer::Idle));
    let answer_out  = answer_cell.clone();
    let gap_clone   = gap.clone();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(&title)
            .with_inner_size([520.0, 320.0])
            .with_resizable(false)
            .with_always_on_top(),
        ..Default::default()
    };

    let _ = eframe::run_native(
        "worktime-idle-popup",
        options,
        Box::new(move |cc| {
            Ok(Box::new(AnswerExtractor {
                inner: IdlePopupApp::new(gap_clone, cc),
                out: answer_out,
            }))
        }),
    );

    let result = answer_cell.lock().unwrap().clone();
    result
}

