//! Graphical popup asking the user whether an idle gap was actually work time.
//!
//! Uses eframe/egui 0.35 which exposes `fn ui()` on the `App` trait and requires
//! the `glow` feature for `run_native`.

use chrono::{DateTime, Local, Duration, TimeZone};
use eframe::egui;
use crate::utils::format_chrono_duration;

/// The user's answer to the idle-gap question.
#[derive(Debug, Clone, PartialEq)]
pub enum IdleGapAnswer {
    /// The gap was entirely work time — insert the full gap into the database.
    WorkTime,
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

// ── Internal egui app ────────────────────────────────────────────────────────

struct IdlePopupApp {
    gap:        IdleGapInfo,
    /// Current left handle position (inclusive start of work window).
    work_start: DateTime<Local>,
    /// Current right handle position (exclusive end of work window).
    work_end:   DateTime<Local>,
    /// Set when the user clicks a button; causes the window to close next frame.
    answer:     Option<IdleGapAnswer>,
}

impl IdlePopupApp {
    fn new(gap: IdleGapInfo, _cc: &eframe::CreationContext<'_>) -> Self {
        let work_start = gap.gap_start;
        let work_end   = gap.gap_end;
        Self { gap, work_start, work_end, answer: None }
    }
}

// ── Two-handle range slider ──────────────────────────────────────────────────

/// State stored in egui memory across frames during a drag.
#[derive(Clone, Copy, Default)]
struct DragState {
    /// Which handle is being dragged.
    handle: DragHandle,
    /// Accumulated raw pixel offset since drag started (not yet snapped).
    px_accum: f32,
}

/// Which handle is currently being dragged.
#[derive(Clone, Copy, PartialEq, Default)]
enum DragHandle { #[default] Left, Right }

/// Draw a horizontal timeline strip for `gap`.  The two handles define the
/// selected work window `[*work_start, *work_end]`.
///
/// Drag snaps to 5-minute boundaries when the gap is > 10 minutes.
/// Returns `true` if either handle moved.
fn draw_range_slider(
    ui:         &mut egui::Ui,
    gap:        &IdleGapInfo,
    work_start: &mut DateTime<Local>,
    work_end:   &mut DateTime<Local>,
) -> bool {
    let snap       = gap.duration().num_minutes() > 10;
    let total_secs = gap.duration().num_seconds().max(1) as f32;
    let handle_r   = 8.0_f32;
    let pad        = handle_r + 4.0;

    // ── Single allocation owns all pointer events ─────────────────────────────
    let desired_size = egui::vec2(ui.available_width(), 48.0);
    let (rect, resp) = ui.allocate_exact_size(desired_size, egui::Sense::drag());

    let track_left  = rect.left()  + pad;
    let track_right = rect.right() - pad;
    let track_width = (track_right - track_left).max(1.0);
    let track_y     = rect.top() + handle_r + 2.0;
    let track_h     = 8.0_f32;

    // ── Coordinate helpers ────────────────────────────────────────────────────
    // Convert a DateTime to its pixel x on the track.
    let time_to_x = |t: DateTime<Local>| -> f32 {
        let offset = (t - gap.gap_start).num_seconds().max(0) as f32;
        track_left + (offset / total_secs) * track_width
    };
    // Convert a raw (unsnapped) pixel x to a DateTime, applying snap if needed.
    let x_to_time = |x: f32| -> DateTime<Local> {
        let frac = ((x - track_left) / track_width).clamp(0.0, 1.0);
        let secs = (frac * total_secs) as i64;
        let snapped = if snap {
            const SNAP: i64 = 300; // 5 min
            ((secs + SNAP / 2) / SNAP) * SNAP
        } else {
            secs
        };
        gap.gap_start + Duration::seconds(snapped)
    };

    // ── Drag state in egui memory ─────────────────────────────────────────────
    // We accumulate raw pixel deltas so that snapping doesn't eat tiny movements.
    // The visual handle position is (time_to_x(current_time) + px_accum); only
    // when that crosses a snap boundary does the stored DateTime update.
    let mem_id = resp.id.with("drag_state");

    if resp.drag_started() {
        if let Some(press_pos) = resp.interact_pointer_pos() {
            let x_start = time_to_x(*work_start);
            let x_end   = time_to_x(*work_end);
            let handle = if (press_pos.x - x_start).abs() <= (press_pos.x - x_end).abs() {
                DragHandle::Left
            } else {
                DragHandle::Right
            };
            ui.ctx().data_mut(|d| d.insert_temp(mem_id, DragState { handle, px_accum: 0.0 }));
        }
    }

    let drag_state: Option<DragState> = ui.ctx().data(|d| d.get_temp(mem_id));

    if resp.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove_temp::<DragState>(mem_id));
    }

    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }

    // ── Apply accumulated drag to the active handle ───────────────────────────
    let mut changed = false;
    // px_visual_offset: how far the handle circle is drawn from its snapped position.
    let mut px_visual_left  = 0.0_f32;
    let mut px_visual_right = 0.0_f32;

    if resp.dragged() {
        if let Some(mut state) = drag_state {
            state.px_accum += resp.drag_delta().x;

            match state.handle {
                DragHandle::Left => {
                    let base_x   = time_to_x(*work_start);
                    let x_end    = time_to_x(*work_end);
                    let raw_x    = (base_x + state.px_accum).clamp(track_left, x_end - 1.0);
                    let new_time = x_to_time(raw_x)
                        .clamp(gap.gap_start, *work_end - Duration::minutes(1));
                    // Visual offset: distance between snapped position and raw drag position.
                    px_visual_left = raw_x - time_to_x(new_time);
                    if new_time != *work_start {
                        // Snap boundary crossed — commit and reset accum to remainder.
                        state.px_accum = raw_x - time_to_x(new_time);
                        *work_start = new_time;
                        changed = true;
                    }
                }
                DragHandle::Right => {
                    let x_start  = time_to_x(*work_start);
                    let base_x   = time_to_x(*work_end);
                    let raw_x    = (base_x + state.px_accum).clamp(x_start + 1.0, track_right);
                    let new_time = x_to_time(raw_x)
                        .clamp(*work_start + Duration::minutes(1), gap.gap_end);
                    px_visual_right = raw_x - time_to_x(new_time);
                    if new_time != *work_end {
                        state.px_accum = raw_x - time_to_x(new_time);
                        *work_end = new_time;
                        changed = true;
                    }
                }
            }
            // Persist updated accum.
            ui.ctx().data_mut(|d| d.insert_temp(mem_id, state));
        }
    }

    // Recompute positions after possible update
    // Visual handle positions: snapped time + accumulated sub-snap pixel offset.
    let x_start = time_to_x(*work_start) + px_visual_left;
    let x_end   = time_to_x(*work_end)   + px_visual_right;

    // ── Paint ─────────────────────────────────────────────────────────────────
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let accent  = egui::Color32::from_rgb(59, 130, 212);

        // Background track
        painter.rect_filled(
            egui::Rect::from_x_y_ranges(
                track_left..=track_right,
                (track_y - track_h / 2.0)..=(track_y + track_h / 2.0),
            ),
            4.0,
            egui::Color32::from_gray(80),
        );

        // ── Grid lines ───────────────────────────────────────────────────────
        // Pick the coarsest interval that gives 3–8 lines across the gap.
        // Candidate intervals in seconds (1m, 5m, 10m, 15m, 30m, 1h, 2h, 3h).
        let candidates: &[i64] = &[60, 300, 600, 900, 1800, 3600, 7200, 10800];
        let gap_secs = gap.duration().num_seconds().max(1);
        let grid_interval = candidates.iter().copied()
            .find(|&iv| gap_secs / iv <= 8)   // first interval giving ≤8 lines
            .unwrap_or(candidates[candidates.len() - 1]);

        // Snap gap_start up to the first grid boundary strictly inside the gap.
        let gap_start_secs = gap.gap_start.timestamp();
        let first_tick_secs = ((gap_start_secs / grid_interval) + 1) * grid_interval;

        let font_grid  = egui::FontId::proportional(10.0);
        let tick_top   = track_y - track_h / 2.0 - 6.0; // a bit above track
        let tick_bot   = track_y + track_h / 2.0 + 2.0; // a bit below track
        let label_grid_y = tick_bot + 1.0;

        let mut tick_secs = first_tick_secs;
        while tick_secs < gap_start_secs + gap_secs {
            let frac = (tick_secs - gap_start_secs) as f32 / gap_secs as f32;
            let tx   = track_left + frac * track_width;

            // Thin vertical tick spanning the track height
            painter.line_segment(
                [egui::pos2(tx, tick_top), egui::pos2(tx, tick_bot)],
                egui::Stroke::new(1.0, egui::Color32::from_gray(150)),
            );

            // Time label — show HH:MM, or just :MM when hours don't change
            let dt = chrono::Local.timestamp_opt(tick_secs, 0).unwrap();
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
                egui::Color32::from_gray(150),
            );

            tick_secs += grid_interval;
        }

        // Highlight band (drawn on top of grid so it's always visible)
        painter.rect_filled(
            egui::Rect::from_x_y_ranges(
                x_start..=x_end,
                (track_y - track_h / 2.0)..=(track_y + track_h / 2.0),
            ),
            4.0,
            accent,
        );

        // Left handle — highlight when dragging or hovering near it
        let is_dragging_left = drag_state.map_or(false, |s| s.handle == DragHandle::Left && resp.dragged());
        let l_hovered = resp.hovered() && {
            let hit = egui::Rect::from_center_size(
                egui::pos2(x_start, track_y), egui::vec2(handle_r * 3.0, handle_r * 3.0));
            resp.hover_pos().map_or(false, |p| hit.contains(p))
        };
        painter.circle_filled(egui::pos2(x_start, track_y), handle_r,
            if is_dragging_left || l_hovered { egui::Color32::from_rgb(200, 220, 255) }
            else { egui::Color32::WHITE });
        painter.circle_stroke(egui::pos2(x_start, track_y), handle_r,
            egui::Stroke::new(2.0, accent));

        // Right handle
        let is_dragging_right = drag_state.map_or(false, |s| s.handle == DragHandle::Right && resp.dragged());
        let r_hovered = resp.hovered() && {
            let hit = egui::Rect::from_center_size(
                egui::pos2(x_end, track_y), egui::vec2(handle_r * 3.0, handle_r * 3.0));
            resp.hover_pos().map_or(false, |p| hit.contains(p))
        };
        painter.circle_filled(egui::pos2(x_end, track_y), handle_r,
            if is_dragging_right || r_hovered { egui::Color32::from_rgb(200, 220, 255) }
            else { egui::Color32::WHITE });
        painter.circle_stroke(egui::pos2(x_end, track_y), handle_r,
            egui::Stroke::new(2.0, accent));

        // End-point time labels (HH:MM at track edges)
        let font    = egui::FontId::proportional(11.0);
        let label_y = track_y + handle_r + 4.0;
        painter.text(egui::pos2(track_left,  label_y), egui::Align2::LEFT_TOP,
            gap.gap_start.format("%H:%M").to_string(), font.clone(), egui::Color32::GRAY);
        painter.text(egui::pos2(track_right, label_y), egui::Align2::RIGHT_TOP,
            gap.gap_end.format("%H:%M").to_string(),   font.clone(), egui::Color32::GRAY);
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
                self.answer = Some(IdleGapAnswer::WorkTime);
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
        ui.label("Drag handles to select the work window:");
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

