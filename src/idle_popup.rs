//! Graphical popup asking the user whether an idle gap was actually work time.
//!
//! Uses eframe/egui 0.35 which exposes `fn ui()` on the `App` trait and requires
//! the `glow` feature for `run_native`.

use chrono::DateTime;
use chrono::Local;
use eframe::egui;
use crate::utils::format_chrono_duration;

/// The user's answer to the idle-gap question.
#[derive(Debug, Clone, PartialEq)]
pub enum IdleGapAnswer {
    /// The gap was work time — insert it into the database.
    WorkTime,
    /// The gap was genuinely idle — do nothing.
    Idle,
}

/// Description of the idle gap shown to the user.
#[derive(Debug, Clone)]
pub struct IdleGapInfo {
    pub gap_start: DateTime<Local>,
    pub gap_end:   DateTime<Local>,
}

impl IdleGapInfo {
    pub fn duration(&self) -> chrono::Duration {
        self.gap_end - self.gap_start
    }
}

// ── Internal egui app ────────────────────────────────────────────────────────

struct IdlePopupApp {
    gap: IdleGapInfo,
    /// Set when the user clicks a button; causes the window to close next frame.
    answer: Option<IdleGapAnswer>,
}

impl IdlePopupApp {
    fn new(gap: IdleGapInfo, _cc: &eframe::CreationContext<'_>) -> Self {
        Self { gap, answer: None }
    }
}

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

        ui.add_space(12.0);
        ui.label("Were you in a meeting or otherwise working away from the keyboard?");
        ui.add_space(16.0);

        ui.horizontal(|ui| {
            let yes_btn = ui.add_sized(
                [150.0, 36.0],
                egui::Button::new(egui::RichText::new("✔  Yes, work time").size(15.0)),
            );
            if yes_btn.clicked() {
                self.answer = Some(IdleGapAnswer::WorkTime);
            }

            ui.add_space(16.0);

            let no_btn = ui.add_sized(
                [150.0, 36.0],
                egui::Button::new(egui::RichText::new("✘  No, was idle").size(15.0)),
            );
            if no_btn.clicked() {
                self.answer = Some(IdleGapAnswer::Idle);
            }
        });

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
            .with_inner_size([400.0, 220.0])
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
