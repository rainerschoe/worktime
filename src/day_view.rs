//! Day view: date navigation bar + interactive vertical day timeline.

use std::sync::{Arc, Mutex};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime,
             NaiveTime, TimeZone, Timelike, Weekday};
use eframe::egui;
use crate::database::Database;
use crate::models::WorktimeEntry;

// ── Public entry point ────────────────────────────────────────────────────────

pub fn draw_day_view(
    ui: &mut egui::Ui,
    selected_date: &mut NaiveDate,
    database: &Arc<Mutex<Database>>,
) {
    // ── Date navigation row ───────────────────────────────────────────────────
    ui.horizontal(|ui| {
        if ui.button("◀").clicked() {
            *selected_date = selected_date.pred_opt().unwrap_or(*selected_date);
        }
        let weekday = match selected_date.weekday() {
            Weekday::Mon => "Mon", Weekday::Tue => "Tue", Weekday::Wed => "Wed",
            Weekday::Thu => "Thu", Weekday::Fri => "Fri",
            Weekday::Sat => "Sat", Weekday::Sun => "Sun",
        };
        ui.label(
            egui::RichText::new(format!("{}  {}", weekday, selected_date.format("%Y-%m-%d")))
                .size(16.0)
                .strong(),
        );
        if ui.button("▶").clicked() {
            *selected_date = selected_date.succ_opt().unwrap_or(*selected_date);
        }
        if ui.button("Today").clicked() {
            *selected_date = Local::now().date_naive();
        }
    });

    ui.add_space(6.0);

    // ── Load entries for selected day ─────────────────────────────────────────
    let (day_start, day_end) = day_bounds(*selected_date);
    let entries: Vec<WorktimeEntry> = database.lock().unwrap()
        .query((day_start, day_end))
        .collect();

    // Total label
    let total: Duration = entries.iter()
        .map(|e| e.duration())
        .fold(Duration::zero(), |a, d| a + d);
    let h = total.num_hours();
    let m = (total.num_minutes() % 60).abs();
    ui.label(format!("Total: {}h {:02}m", h, m));

    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    // ── Timeline widget ───────────────────────────────────────────────────────
    draw_day_timeline(ui, *selected_date, &entries, database);
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn day_bounds(date: NaiveDate) -> (DateTime<Local>, DateTime<Local>) {
    let start = Local.from_local_datetime(
        &NaiveDateTime::new(date, NaiveTime::from_hms_opt(0, 0, 0).unwrap())
    ).unwrap();
    let end = Local.from_local_datetime(
        &NaiveDateTime::new(date, NaiveTime::from_hms_nano_opt(23, 59, 59, 999_999_999).unwrap())
    ).unwrap();
    (start, end)
}

/// Convert seconds-from-midnight on `date` into an absolute `DateTime<Local>`.
fn secs_to_dt(date: NaiveDate, secs: f32) -> DateTime<Local> {
    let s = secs.round() as i64;
    let naive = NaiveDateTime::new(date, NaiveTime::from_hms_opt(0, 0, 0).unwrap())
        + Duration::seconds(s);
    Local.from_local_datetime(&naive).single()
        .unwrap_or_else(|| Local::now())
}

/// Snap `secs` to the nearest `snap_interval` seconds boundary.
fn snap_secs(secs: f32, snap_interval: i64) -> f32 {
    let iv = snap_interval as f32;
    (secs / iv).round() * iv
}

// ── Timeline state (stored in egui temp data) ─────────────────────────────────

#[derive(Clone, Default)]
struct TimelineState {
    pixels_per_hour:    f32,
    /// Seconds-from-view-start currently scrolled (pan offset).
    scroll_offset_secs: f32,
    /// Active drag: (anchor_secs, current_secs, mode).
    drag: Option<DragState>,
    /// Index into the day's entry list of the entry awaiting delete confirmation.
    pending_delete: Option<usize>,
}

#[derive(Clone, Copy, PartialEq)]
enum DragMode { Draw, Replace }

#[derive(Clone, Copy)]
struct DragState {
    anchor_secs:  f32,
    current_secs: f32,
    mode:         DragMode,
}

// ── Constants ─────────────────────────────────────────────────────────────────

const VIEW_START_H: f32  = 7.0;
const VIEW_END_H:   f32  = 20.0;
const LABEL_W:      f32  = 46.0;    // left margin for hour labels
const PPH_DEFAULT:  f32  = 60.0;    // pixels per hour default
const PPH_MIN:      f32  = 20.0;
const PPH_MAX:      f32  = 400.0;

// ── draw_day_timeline ─────────────────────────────────────────────────────────

fn draw_day_timeline(
    ui:        &mut egui::Ui,
    date:       NaiveDate,
    entries:   &[WorktimeEntry],
    database:  &Arc<Mutex<Database>>,
) {
    let state_id = ui.id().with("tl_state");
    let mut state: TimelineState = ui.ctx()
        .data(|d| d.get_temp::<TimelineState>(state_id))
        .unwrap_or_else(|| TimelineState {
            pixels_per_hour: PPH_DEFAULT,
            ..Default::default()
        });

    let view_start_s = VIEW_START_H * 3600.0;
    let view_end_s   = VIEW_END_H   * 3600.0;
    let view_span_s  = view_end_s - view_start_s;

    let visible_h  = view_span_s / 3600.0 * state.pixels_per_hour;
    let total_w    = ui.available_width();

    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(total_w, visible_h.max(200.0)),
        egui::Sense::click_and_drag(),
    );

    // ── Scroll / zoom input ───────────────────────────────────────────────────
    if resp.hovered() || resp.dragged() {
        let scroll = ui.input(|i| i.smooth_scroll_delta);
        let ctrl   = ui.input(|i| i.modifiers.ctrl);
        if ctrl {
            let factor = 1.0 + scroll.y * 0.01;
            state.pixels_per_hour = (state.pixels_per_hour * factor).clamp(PPH_MIN, PPH_MAX);
        } else if scroll.y.abs() > 0.0 {
            let secs_per_px = 3600.0 / state.pixels_per_hour;
            state.scroll_offset_secs =
                (state.scroll_offset_secs - scroll.y * secs_per_px)
                    .clamp(0.0, view_span_s * 0.8);
        }
    }

    // ── Coordinate helpers ────────────────────────────────────────────────────

    // Seconds-from-midnight → y pixel.
    let secs_to_y = |secs: f32| -> f32 {
        let rel = secs - view_start_s - state.scroll_offset_secs;
        rect.top() + rel / 3600.0 * state.pixels_per_hour
    };

    // y pixel → seconds-from-midnight, clamped to view range.
    let y_to_secs = |y: f32| -> f32 {
        let rel = (y - rect.top()) * 3600.0 / state.pixels_per_hour;
        (view_start_s + state.scroll_offset_secs + rel).clamp(0.0, 86400.0)
    };

    // Pixel width of the entry area (right of label column).
    let entry_left  = rect.left() + LABEL_W + 2.0;
    let entry_right = rect.right() - 2.0;

    // ── Snapping ──────────────────────────────────────────────────────────────
    // Use 5-min snap when view span > 10 min (always true here).
    let snap_interval: i64 = 300; // 5 minutes

    // ── Drag interaction ──────────────────────────────────────────────────────
    let shift = ui.input(|i| i.modifiers.shift);

    // Determine which entry index (if any) is under the pointer.
    let entry_at = |secs: f32| -> Option<usize> {
        entries.iter().position(|e| {
            let s = e.start.num_seconds_from_midnight() as f32;
            let t = e.end.num_seconds_from_midnight()   as f32;
            secs >= s && secs < t
        })
    };

    if resp.drag_started() {
        if let Some(pos) = resp.interact_pointer_pos() {
            if pos.x >= entry_left {
                let secs = snap_secs(y_to_secs(pos.y), snap_interval);
                let mode = if shift { DragMode::Replace } else { DragMode::Draw };
                // Start drag regardless of whether we're on an entry.
                state.drag = Some(DragState {
                    anchor_secs:  secs,
                    current_secs: secs,
                    mode,
                });
            }
        }
    }

    if resp.dragged() {
        if let Some(ref mut drag) = state.drag {
            if let Some(pos) = resp.interact_pointer_pos() {
                drag.current_secs = snap_secs(y_to_secs(pos.y), snap_interval);
            }
        }
    }

    let mut db_changed = false;
    if resp.drag_stopped() {
        if let Some(drag) = state.drag.take() {
            let lo = drag.anchor_secs.min(drag.current_secs);
            let hi = drag.anchor_secs.max(drag.current_secs);
            // Only commit if range is at least one snap interval.
            if hi - lo >= snap_interval as f32 {
                let start_dt = secs_to_dt(date, lo);
                let end_dt   = secs_to_dt(date, hi);
                let new_entry = WorktimeEntry { start: start_dt, end: end_dt, comments: String::new() };

                let mut db = database.lock().unwrap();
                match drag.mode {
                    DragMode::Draw => {
                        clip_and_insert(&mut db.rows, new_entry);
                    }
                    DragMode::Replace => {
                        remove_overlapping(&mut db.rows, &new_entry);
                        db.insert_sorted(new_entry);
                    }
                }
                let _ = db.store_file();
                db_changed = true;
            }
        }
    }

    // Click (no drag) on existing entry → schedule delete confirmation.
    if resp.clicked() && state.drag.is_none() {
        if let Some(pos) = resp.interact_pointer_pos() {
            if pos.x >= entry_left {
                let secs = y_to_secs(pos.y);
                if let Some(idx) = entry_at(secs) {
                    state.pending_delete = Some(idx);
                }
            }
        }
    }

    // ── Delete confirmation popup ─────────────────────────────────────────────
    if let Some(del_idx) = state.pending_delete {
        if let Some(entry) = entries.get(del_idx) {
            let popup_id = ui.id().with("del_popup");
            let label = format!(
                "Delete  {}–{}?",
                entry.start.format("%H:%M"),
                entry.end.format("%H:%M"),
            );
            // Position the popup near the entry's y midpoint.
            let mid_y = (secs_to_y(entry.start.num_seconds_from_midnight() as f32)
                + secs_to_y(entry.end.num_seconds_from_midnight() as f32)) / 2.0;
            let popup_pos = egui::pos2(entry_left + 8.0, mid_y - 20.0);

            egui::Window::new("Delete entry")
                .id(popup_id)
                .fixed_pos(popup_pos)
                .collapsible(false)
                .resizable(false)
                .title_bar(false)
                .show(ui.ctx(), |ui| {
                    ui.label(&label);
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            let start = entry.start;
                            let mut db = database.lock().unwrap();
                            db.rows.retain(|r| r.start != start);
                            let _ = db.store_file();
                            state.pending_delete = None;
                            db_changed = true;
                        }
                        if ui.button("Cancel").clicked() {
                            state.pending_delete = None;
                        }
                    });
                });
        } else {
            state.pending_delete = None;
        }
    }

    // Reload entries if we just mutated the DB.
    let entries_live: Vec<WorktimeEntry>;
    let entries_ref: &[WorktimeEntry] = if db_changed {
        let (ds, de) = day_bounds(date);
        entries_live = database.lock().unwrap().query((ds, de)).collect();
        &entries_live
    } else {
        entries
    };

    // ── Paint ─────────────────────────────────────────────────────────────────
    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect);
        let accent   = egui::Color32::from_rgb(59, 130, 212);
        let bg       = egui::Color32::from_gray(35);
        let grid_h   = egui::Color32::from_gray(60);
        let grid_hh  = egui::Color32::from_gray(48);
        let text_c   = ui.visuals().text_color();

        // Background
        painter.rect_filled(rect, 0.0, bg);

        // Hour / half-hour grid lines + labels
        let mut h = VIEW_START_H as i32;
        while h <= VIEW_END_H as i32 {
            let secs = h as f32 * 3600.0;
            let y    = secs_to_y(secs);
            if y >= rect.top() - 1.0 && y <= rect.bottom() + 1.0 {
                painter.line_segment(
                    [egui::pos2(rect.left() + LABEL_W, y), egui::pos2(rect.right(), y)],
                    egui::Stroke::new(1.0, grid_h),
                );
                painter.text(
                    egui::pos2(rect.left() + 2.0, y + 2.0),
                    egui::Align2::LEFT_TOP,
                    format!("{:02}:00", h),
                    egui::FontId::proportional(11.0),
                    text_c,
                );
            }
            let yh = secs_to_y(secs + 1800.0);
            if yh >= rect.top() && yh <= rect.bottom() {
                painter.line_segment(
                    [egui::pos2(rect.left() + LABEL_W, yh), egui::pos2(rect.right(), yh)],
                    egui::Stroke::new(1.0, grid_hh),
                );
            }
            h += 1;
        }

        // Existing work entry blocks
        for (idx, entry) in entries_ref.iter().enumerate() {
            let s = entry.start.num_seconds_from_midnight() as f32;
            let e = entry.end.num_seconds_from_midnight()   as f32;
            let yt = secs_to_y(s).max(rect.top());
            let yb = secs_to_y(e).min(rect.bottom());
            if yb > yt {
                // Dim if this entry is pending deletion.
                let col = if state.pending_delete == Some(idx) {
                    egui::Color32::from_rgb(40, 90, 150)
                } else {
                    accent
                };
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(entry_left, yt),
                        egui::pos2(entry_right, yb),
                    ),
                    3.0, col,
                );
            }
        }

        // Ghost drag preview
        if let Some(drag) = state.drag {
            let lo = drag.anchor_secs.min(drag.current_secs);
            let hi = drag.anchor_secs.max(drag.current_secs);
            let yt = secs_to_y(lo).max(rect.top());
            let yb = secs_to_y(hi).min(rect.bottom());
            if yb > yt {
                let ghost_col = if drag.mode == DragMode::Replace {
                    egui::Color32::from_rgba_unmultiplied(220, 100, 60, 80) // orange-ish for replace
                } else {
                    egui::Color32::from_rgba_unmultiplied(59, 130, 212, 80) // blue ghost for draw
                };
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(entry_left, yt),
                        egui::pos2(entry_right, yb),
                    ),
                    3.0, ghost_col,
                );
                // Time label on ghost
                let label = format!(
                    "{}–{}",
                    fmt_secs(lo),
                    fmt_secs(hi),
                );
                painter.text(
                    egui::pos2(entry_left + 4.0, yt + 2.0),
                    egui::Align2::LEFT_TOP,
                    label,
                    egui::FontId::proportional(11.0),
                    egui::Color32::WHITE,
                );
            }
        }

        // Cursor crosshair while dragging inside the entry area
        if resp.hovered() {
            if let Some(pos) = resp.hover_pos() {
                if pos.x >= entry_left {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                }
            }
        }
    }

    // Persist state
    ui.ctx().data_mut(|d| d.insert_temp(state_id, state));

    // Request repaint while drag is live so ghost updates smoothly
    if resp.dragged() {
        ui.ctx().request_repaint();
    }
}

// ── Overlap helpers ───────────────────────────────────────────────────────────

/// Insert `new` into `rows`, clipping away any time that already belongs to
/// existing entries (draw mode: new entry must not overlap existing ones).
///
/// Rules:
/// - Any existing entry fully inside `new`'s range → deleted.
/// - Existing entry whose end falls inside `new`'s range → trimmed: end = new.start.
/// - Existing entry whose start falls inside `new`'s range → trimmed: start = new.end.
/// - Existing entry that fully contains `new` → split into two (not implemented for
///   simplicity; new entry is simply not inserted in that case to avoid overlap).
fn clip_and_insert(rows: &mut Vec<WorktimeEntry>, new: WorktimeEntry) {
    let mut to_add: Vec<WorktimeEntry> = Vec::new();

    rows.retain_mut(|existing| {
        let overlaps = existing.start < new.end && existing.end > new.start;
        if !overlaps {
            return true;
        }
        // Fully inside new → drop.
        if existing.start >= new.start && existing.end <= new.end {
            return false;
        }
        // Existing fully contains new → keep existing, don't insert new.
        if existing.start <= new.start && existing.end >= new.end {
            // Signal: abort insertion by marking new as zero-length (handled below).
            return true; // keep existing as-is
        }
        // Existing start inside new range → push start to new.end.
        if existing.start >= new.start && existing.start < new.end {
            existing.start = new.end;
            return true;
        }
        // Existing end inside new range → trim end to new.start.
        if existing.end > new.start && existing.end <= new.end {
            existing.end = new.start;
            return existing.end > existing.start;
        }
        true
    });

    // Only insert if there's no existing entry that fully contains new.
    let contained = rows.iter().any(|e| e.start <= new.start && e.end >= new.end);
    if !contained {
        to_add.push(new);
    }

    for entry in to_add {
        let pos = rows.partition_point(|r| r.start <= entry.start);
        rows.insert(pos, entry);
    }
}

/// Remove all entries from `rows` that overlap with `new`.
fn remove_overlapping(rows: &mut Vec<WorktimeEntry>, new: &WorktimeEntry) {
    rows.retain(|e| !(e.start < new.end && e.end > new.start));
}

/// Format seconds-from-midnight as "HH:MM".
fn fmt_secs(secs: f32) -> String {
    let s = secs as u32;
    format!("{:02}:{:02}", s / 3600, (s % 3600) / 60)
}
