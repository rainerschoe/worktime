//! Day view: date navigation bar + interactive vertical day timeline.

use std::sync::{Arc, Mutex};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime,
             NaiveTime, TimeZone, Timelike, Weekday};
use eframe::egui;
use crate::database::Database;
use crate::models::WorktimeEntry;

// ── Duration formatting helper ────────────────────────────────────────────────

/// Format a chrono Duration as "Xh:MMm" (e.g. "1h:23m") for display inside
/// timeline blocks.  For very short durations shows just minutes.
fn fmt_duration(d: Duration) -> String {
    let total_mins = d.num_minutes().abs();
    let h = total_mins / 60;
    let m = total_mins % 60;
    if h == 0 {
        format!("{}m", m)
    } else {
        format!("{}h {:02}m", h, m)
    }
}

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

    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    // ── Timeline widget ───────────────────────────────────────────────────────
    draw_day_timeline(ui, *selected_date, database);
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

/// Snap to the nearest grid boundary, entry edge, or current time when close.
fn snap_secs_with_targets(secs: f32, snap_interval: i64, targets: &[f32]) -> f32 {
    let snapped = snap_secs(secs, snap_interval);
    let threshold = (snap_interval as f32 / 2.0).max(60.0);
    let mut best = snapped;
    let mut best_dist = (secs - snapped).abs();

    for &target in targets {
        let dist = (secs - target).abs();
        if dist <= threshold && dist < best_dist {
            best = target;
            best_dist = dist;
        }
    }

    best
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
    ui:       &mut egui::Ui,
    date:      NaiveDate,
    database: &Arc<Mutex<Database>>,
) {
    // Always read fresh so the live session end-time is never stale.
    let (day_start, day_end) = day_bounds(date);
    let entries: Vec<WorktimeEntry> = database.lock().unwrap()
        .query((day_start, day_end))
        .collect();
    let entries = entries.as_slice();

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

    // ── "Now" — current time in seconds-from-midnight (None on other days) ────
    let is_today = date == Local::now().date_naive();
    let now_secs: Option<f32> = if is_today {
        let now = Local::now();
        Some(now.num_seconds_from_midnight() as f32)
    } else {
        None
    };

    let mut snap_targets: Vec<f32> = entries.iter()
        .flat_map(|e| [
            e.start.num_seconds_from_midnight() as f32,
            e.end.num_seconds_from_midnight() as f32,
        ])
        .collect();
    if let Some(now) = now_secs {
        snap_targets.push(now);
    }

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

    // Gate interactions: pointer position must be in the past (or not today).
    let is_past_or_not_today = |secs: f32| -> bool {
        match now_secs {
            None => true,               // not today → all times are "past"
            Some(now) => secs <= now,   // today → only allow up to now
        }
    };

    if resp.drag_started() {
        if let Some(pos) = resp.interact_pointer_pos() {
            if pos.x >= entry_left {
                let secs = snap_secs_with_targets(y_to_secs(pos.y), snap_interval, &snap_targets);
                if is_past_or_not_today(secs) {
                    let mode = if shift { DragMode::Replace } else { DragMode::Draw };
                    state.drag = Some(DragState {
                        anchor_secs:  secs,
                        current_secs: secs,
                        mode,
                    });
                }
            }
        }
    }

    if resp.dragged() {
        if let Some(ref mut drag) = state.drag {
            if let Some(pos) = resp.interact_pointer_pos() {
                let raw = snap_secs_with_targets(y_to_secs(pos.y), snap_interval, &snap_targets);
                // Clamp current end to "now" on today.
                drag.current_secs = match now_secs {
                    Some(now) => raw.min(now),
                    None => raw,
                };
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
                        if !extend_live_entry_for_day(&mut db.rows, &new_entry, date, now_secs) {
                            // Only clip against entries for this day.
                            clip_and_insert_for_day(&mut db.rows, new_entry, date);
                        }
                    }
                    DragMode::Replace => {
                        // Remove overlapping entries for this day only.
                        remove_overlapping_for_day(&mut db.rows, &new_entry, date);
                        db.insert_sorted(new_entry);
                    }
                }
                db.normalize(60);
                let _ = db.store_file();
                db_changed = true;
            }
        }
    }

    // Click (no drag) on existing entry → schedule delete confirmation.
    // Only allow clicking entries that are fully in the past (or not today).
    if resp.clicked() && state.drag.is_none() {
        if let Some(pos) = resp.interact_pointer_pos() {
            if pos.x >= entry_left {
                let secs = y_to_secs(pos.y);
                if is_past_or_not_today(secs) {
                    if let Some(idx) = entry_at(secs) {
                        state.pending_delete = Some(idx);
                    }
                }
            }
        }
    }

    // ── Refresh entries if DB was mutated this frame ──────────────────────────
    // (The Vec at the top is already fresh for the normal path; we only need
    //  to re-query when the user just drew/deleted something this very frame.)
    let entries_updated: Vec<WorktimeEntry>;
    let entries = if db_changed {
        entries_updated = database.lock().unwrap().query((day_start, day_end)).collect();
        entries_updated.as_slice()
    } else {
        entries
    };

    // ── Delete confirmation popup ─────────────────────────────────────────────
    if let Some(del_idx) = state.pending_delete {
        if let Some(entry) = entries.get(del_idx) {
            let popup_id = ui.id().with("del_popup");
            let label = format!(
                "Delete  {}–{}?",
                entry.start.format("%H:%M"),
                entry.end.format("%H:%M"),
            );
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
                            let end   = entry.end;
                            let mut db = database.lock().unwrap();
                            // `entry` may be a day-clipped view of a row that spans midnight.
                            // Match the row that fully contains [start, end] rather than
                            // requiring an exact timestamp match on both ends.
                            db.rows.retain(|r| !(r.start <= start && r.end >= end));
                            db.normalize(60);
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

    let entries_ref: &[WorktimeEntry] = entries;

    // ── Paint ─────────────────────────────────────────────────────────────────
    if ui.is_rect_visible(rect) {
        let painter  = ui.painter_at(rect);
        let accent   = egui::Color32::from_rgb(59, 130, 212);
        let bg       = egui::Color32::from_gray(35);
        let bg_future= egui::Color32::from_gray(28); // slightly darker tint for future
        let grid_h   = egui::Color32::from_gray(60);
        let grid_hh  = egui::Color32::from_gray(48);
        let text_c   = ui.visuals().text_color();
        let gap_text = egui::Color32::from_gray(130);
        let font_lbl = egui::FontId::proportional(11.0);

        // Background (split at "now" on today)
        painter.rect_filled(rect, 0.0, bg);
        if let Some(now) = now_secs {
            let y_now = secs_to_y(now).clamp(rect.top(), rect.bottom());
            // Shade the future area slightly darker
            if y_now < rect.bottom() {
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(entry_left, y_now),
                        egui::pos2(rect.right(), rect.bottom()),
                    ),
                    0.0, bg_future,
                );
            }
        }

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
                    font_lbl.clone(),
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

        // ── Gap duration labels (between consecutive work entries) ────────────
        let hover_pos = resp.hover_pos();
        let entry_mid_x = (entry_left + entry_right) / 2.0;
        for window in entries_ref.windows(2) {
            let gap_start_s = window[0].end.num_seconds_from_midnight() as f32;
            let gap_end_s   = window[1].start.num_seconds_from_midnight() as f32;
            let gap_dur     = window[1].start - window[0].end;
            if gap_dur > Duration::zero() {
                let yt_gap = secs_to_y(gap_start_s).max(rect.top());
                let yb_gap = secs_to_y(gap_end_s).min(rect.bottom());
                let mid_y  = (yt_gap + yb_gap) / 2.0;
                let height = yb_gap - yt_gap;
                let label  = fmt_duration(gap_dur);

                // Measure text to see if it fits in the gap height.
                // Approx: font size ~11px, needs at least 14px to be readable.
                let fits = height >= 14.0;
                if fits {
                    // Centred horizontally in the entry column.
                    painter.text(
                        egui::pos2(entry_mid_x, mid_y),
                        egui::Align2::CENTER_CENTER,
                        &label,
                        font_lbl.clone(),
                        gap_text,
                    );
                } else if let Some(hp) = hover_pos {
                    // Show as tooltip when hovered over this gap band.
                    if hp.y >= yt_gap && hp.y <= yb_gap && hp.x >= entry_left {
                        egui::Tooltip::always_open(
                            ui.ctx().clone(),
                            ui.layer_id(),
                            egui::Id::new("gap_tooltip").with(window[0].end),
                            egui::pos2(hp.x + 12.0, hp.y),
                        )
                        .show(|ui| { ui.label(format!("Gap: {}", label)); });
                    }
                }
            }
        }

        // ── Work entry blocks ─────────────────────────────────────────────────
        // Helper: paint a small boxed label with a dark background.
        let paint_badge = |painter: &egui::Painter,
                           pos: egui::Pos2,
                           align: egui::Align2,
                           text: &str,
                           font: egui::FontId,
                           text_color: egui::Color32,
                           bg_color: egui::Color32| {
            let galley = painter.layout_no_wrap(
                text.to_string(), font, text_color,
            );
            let size   = galley.size();
            let origin = align.anchor_rect(
                egui::Rect::from_min_size(pos, size)
            ).min;
            let bg_rect = egui::Rect::from_min_size(origin, size)
                .expand(3.0);
            painter.rect_filled(bg_rect, 3.0, bg_color);
            painter.rect_stroke(bg_rect, 3.0, egui::Stroke::new(1.0, egui::Color32::from_gray(80)), egui::StrokeKind::Inside);
            painter.galley(origin, galley, text_color);
        };

        let mut hover_badges: Vec<(egui::Pos2, egui::Align2, String)> = Vec::new();

        for (idx, entry) in entries_ref.iter().enumerate() {
            let s = entry.start.num_seconds_from_midnight() as f32;
            let e = entry.end.num_seconds_from_midnight()   as f32;
            let yt = secs_to_y(s).max(rect.top());
            let yb = secs_to_y(e).min(rect.bottom());
            if yb <= yt { continue; }

            let col = if state.pending_delete == Some(idx) {
                egui::Color32::from_rgb(40, 90, 150)
            } else {
                accent
            };
            let block_rect = egui::Rect::from_min_max(
                egui::pos2(entry_left, yt),
                egui::pos2(entry_right, yb),
            );
            painter.rect_filled(block_rect, 3.0, col);

            // Duration label centred in the block.
            let dur     = entry.duration();
            let dur_lbl = fmt_duration(dur);
            let block_h = yb - yt;
            let mid_y   = (yt + yb) / 2.0;

            if block_h >= 14.0 {
                // Enough room — paint duration directly.
                painter.text(
                    egui::pos2(entry_mid_x, mid_y),
                    egui::Align2::CENTER_CENTER,
                    &dur_lbl,
                    font_lbl.clone(),
                    egui::Color32::WHITE,
                );
            } else if let Some(hp) = hover_pos {
                // Too small — show tooltip on hover.
                if block_rect.contains(hp) {
                    egui::Tooltip::always_open(
                        ui.ctx().clone(),
                        ui.layer_id(),
                        egui::Id::new("block_tooltip").with(entry.start),
                        egui::pos2(hp.x + 12.0, hp.y),
                    )
                    .show(|ui| {
                        ui.label(format!(
                            "{}–{}  ({})",
                            entry.start.format("%H:%M"),
                            entry.end.format("%H:%M"),
                            dur_lbl,
                        ));
                    });
                }
            }

            // ── Hovered: start badge above, end badge below the block ─────────
            if let Some(hp) = hover_pos {
                if block_rect.contains(hp) {
                    hover_badges.push((
                        egui::pos2(entry_mid_x, yt - 2.0),
                        egui::Align2::CENTER_BOTTOM,
                        entry.start.format("%H:%M").to_string(),
                    ));
                    hover_badges.push((
                        egui::pos2(entry_mid_x, yb + 2.0),
                        egui::Align2::CENTER_TOP,
                        entry.end.format("%H:%M").to_string(),
                    ));
                }
            }
        }

        // ── Total worked time label at the bottom of the canvas ───────────────
        {
            let total: Duration = entries_ref.iter()
                .map(|e| e.duration())
                .fold(Duration::zero(), |a, d| a + d);
            let th = total.num_hours();
            let tm = (total.num_minutes() % 60).abs();
            let total_txt = format!("Total: {}h {:02}m", th, tm);
            let font_tot  = egui::FontId::proportional(13.0);
            let pos       = egui::pos2(entry_mid_x, rect.bottom() - 6.0);
            // Boxed badge so it reads even when sitting on top of a work block.
            paint_badge(
                &painter,
                pos,
                egui::Align2::CENTER_BOTTOM,
                &total_txt,
                font_tot,
                egui::Color32::WHITE,
                egui::Color32::from_black_alpha(180),
            );
        }

        // ── Ghost drag preview ────────────────────────────────────────────────
        if let Some(drag) = state.drag {
            let lo = drag.anchor_secs.min(drag.current_secs);
            let hi = drag.anchor_secs.max(drag.current_secs);
            let yt = secs_to_y(lo).max(rect.top());
            let yb = secs_to_y(hi).min(rect.bottom());
            if yb > yt {
                let ghost_col = if drag.mode == DragMode::Replace {
                    egui::Color32::from_rgba_unmultiplied(220, 100, 60, 80)
                } else {
                    egui::Color32::from_rgba_unmultiplied(59, 130, 212, 80)
                };
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(entry_left, yt),
                        egui::pos2(entry_right, yb),
                    ),
                    3.0, ghost_col,
                );
                let label = format!("{}–{}", fmt_secs(lo), fmt_secs(hi));
                painter.text(
                    egui::pos2(entry_left + 4.0, yt + 2.0),
                    egui::Align2::LEFT_TOP,
                    label,
                    font_lbl.clone(),
                    egui::Color32::WHITE,
                );
            }
        }

        for (pos, align, text) in hover_badges {
            paint_badge(
                &painter,
                pos,
                align,
                &text,
                egui::FontId::proportional(11.0),
                egui::Color32::WHITE,
                egui::Color32::from_black_alpha(180),
            );
        }

        // ── Red "now" line ────────────────────────────────────────────────────
        if let Some(now) = now_secs {
            let y_now = secs_to_y(now);
            if y_now >= rect.top() && y_now <= rect.bottom() {
                painter.circle_filled(
                    egui::pos2(entry_left, y_now),
                    4.0,
                    egui::Color32::RED,
                );
                painter.line_segment(
                    [egui::pos2(entry_left, y_now), egui::pos2(entry_right, y_now)],
                    egui::Stroke::new(2.0, egui::Color32::RED),
                );
                paint_badge(
                    &painter,
                    egui::pos2(rect.left() + 2.0, y_now - 1.0),
                    egui::Align2::LEFT_BOTTOM,
                    &Local::now().format("%H:%M").to_string(),
                    egui::FontId::proportional(10.0),
                    egui::Color32::RED,
                    egui::Color32::from_black_alpha(220),
                );
            }
        }

        if let Some(pos) = resp.hover_pos() {
            if pos.x >= entry_left {
                let hovered_secs = y_to_secs(pos.y);
                let y_hover = pos.y.clamp(rect.top(), rect.bottom());
                painter.line_segment(
                    [egui::pos2(entry_left, y_hover), egui::pos2(entry_right, y_hover)],
                    egui::Stroke::new(1.0, egui::Color32::WHITE),
                );
                paint_badge(
                    &painter,
                    egui::pos2(rect.left() + 2.0, y_hover - 1.0),
                    egui::Align2::LEFT_BOTTOM,
                    &fmt_secs(hovered_secs),
                    egui::FontId::proportional(10.0),
                    egui::Color32::WHITE,
                    egui::Color32::from_black_alpha(220),
                );
            }
        }

        // ── Cursor: crosshair in past area, not-allowed in future area ────────
        if resp.hovered() {
            if let Some(pos) = resp.hover_pos() {
                if pos.x >= entry_left {
                    let hovered_secs = y_to_secs(pos.y);
                    if is_past_or_not_today(hovered_secs) {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                    } else {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::NotAllowed);
                    }
                }
            }
        }
    }

    // Persist state
    ui.ctx().data_mut(|d| d.insert_temp(state_id, state));

    // Request repaint while drag is live so ghost updates smoothly.
    // On today's view, also schedule a repaint in ~30 s so the now-line and
    // the live session block advance without requiring mouse movement.
    if resp.dragged() {
        ui.ctx().request_repaint();
    }
    if is_today {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(30));
    }
}

// ── Overlap helpers ───────────────────────────────────────────────────────────

/// Insert `new` into `rows`, clipping away overlapping time that already
/// belongs to existing entries **on the same day**.
///
/// Rules:
/// - Existing entry fully inside `new` → deleted.
/// - Existing entry partially overlapping `new` → trimmed.
/// - Existing entry fully containing `new` → `new` is not inserted (no-op).
fn clip_and_insert_for_day(rows: &mut Vec<WorktimeEntry>, new: WorktimeEntry, date: NaiveDate) {
    // If any same-day entry fully contains `new`, do nothing.
    let contained = rows.iter().any(|e| {
        e.start.date_naive() == date
            && e.start <= new.start
            && e.end   >= new.end
    });
    if contained {
        return;
    }

    let mut fragments: Vec<WorktimeEntry> = Vec::new();

    rows.retain_mut(|existing| {
        if existing.start.date_naive() != date {
            return true; // never touch other days
        }
        let overlaps = existing.start < new.end && existing.end > new.start;
        if !overlaps {
            return true;
        }
        // Fully inside new → drop.
        if existing.start >= new.start && existing.end <= new.end {
            return false;
        }
        // Existing entry fully contains new → split into left + right fragments.
        if existing.start < new.start && existing.end > new.end {
            fragments.push(WorktimeEntry {
                start: new.end,
                end: existing.end,
                comments: existing.comments.clone(),
            });
            existing.end = new.start;
            return existing.end > existing.start;
        }
        // Left overhang: trim the right end.
        if existing.start < new.start && existing.end <= new.end {
            existing.end = new.start;
            return existing.end > existing.start;
        }
        // Right overhang: push start forward.
        if existing.start >= new.start && existing.end > new.end {
            existing.start = new.end;
            return existing.end > existing.start;
        }
        true
    });

    rows.push(new);
    rows.extend(fragments);
    rows.sort_by_key(|r| r.start);
}

fn extend_live_entry_for_day(
    rows: &mut Vec<WorktimeEntry>,
    new: &WorktimeEntry,
    date: NaiveDate,
    now_secs: Option<f32>,
) -> bool {
    let Some(now) = now_secs else {
        return false;
    };

    let live_threshold = 60.0;
    let new_end_secs = new.end.num_seconds_from_midnight() as f32;
    if (new_end_secs - now).abs() > live_threshold {
        return false;
    }

    if let Some(existing) = rows.iter_mut().find(|e| {
        e.start.date_naive() == date
            && (e.end.num_seconds_from_midnight() as f32 - now).abs() <= live_threshold
            && new.start <= e.end
    }) {
        if new.end > existing.end {
            existing.end = new.end;
        }
        return true;
    }

    false
}

/// Remove all entries on `date` that overlap with `new`.
fn remove_overlapping_for_day(rows: &mut Vec<WorktimeEntry>, new: &WorktimeEntry, date: NaiveDate) {
    rows.retain(|e| {
        e.start.date_naive() != date
            || !(e.start < new.end && e.end > new.start)
    });
}

/// Format seconds-from-midnight as "HH:MM".
fn fmt_secs(secs: f32) -> String {
    let s = secs as u32;
    format!("{:02}:{:02}", s / 3600, (s % 3600) / 60)
}
