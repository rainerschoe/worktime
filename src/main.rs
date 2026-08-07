mod utils;
mod models;
mod config;
mod database;
mod idle_detection;
mod idle_popup;
mod monitoring;
mod main_window;
mod day_view;
mod cli;

use std::sync::*;
use chrono::Datelike;
use clap::Parser;
use gtk;

use crate::cli::Args;
use crate::config::Config;
use crate::database::Database;
use crate::models::SpecialDayType;
use crate::monitoring::run_interactive_monitoring;
use crate::utils::format_chrono_duration;

fn main() {
    let args = Args::parse();
    let cfg: Config = confy::load("worktime", None).unwrap();

    let data_path = expanduser::expanduser(cfg.data_file.as_str()).unwrap();
    println!("Using data file {}", data_path.display());

    let special_day_path = expanduser::expanduser(cfg.special_day_file.as_str()).unwrap();
    println!("Using special_day file {}", special_day_path.display());

    let database = Arc::new(Mutex::new(
        Database::init(data_path, special_day_path).unwrap(),
    ));
    if args.overtime {
        let overtime_end: chrono::DateTime<chrono::offset::Local> =
            std::time::SystemTime::now().into();
        let overtime = database.lock().unwrap().calculate_overtime(
            chrono::Duration::hours(cfg.weekly_hours),
            (cfg.cutoff_datetime, overtime_end),
        ) + chrono::Duration::seconds((cfg.cutoff_day_overtime_hours * 3600.0) as i64);
        println!("overtime: {}", format_chrono_duration(&overtime));
    } else if let Some(days) = args.daysums {
        let daysums = database.lock().unwrap().get_day_sums(days);
        // Use floating-point division for accurate expected_per_day
        let expected_per_day_secs = (cfg.weekly_hours as f64 / 5.0) * 3600.0;
        let expected_per_day = chrono::Duration::seconds(expected_per_day_secs.round() as i64);
        let db = database.lock().unwrap();
        for (time, sum) in daysums {
            let weekday = time.weekday();
            // Check for special day (Vacation, Sick, Holiday)
            let special = db.special_days.iter().find(|sd| {
                sd.day == time.date_naive() && matches!(sd.day_type, SpecialDayType::Vacation | SpecialDayType::Sick | SpecialDayType::Holiday)
            });
            let expected = match (weekday, special) {
                (chrono::Weekday::Sat | chrono::Weekday::Sun, _) => chrono::Duration::zero(),
                (_, Some(_)) => chrono::Duration::zero(),
                _ => expected_per_day,
            };
            let deviation = sum - expected;
            let deviation_secs = deviation.num_seconds();
            let color = if deviation_secs > 0 {
                "\x1b[32m" // green
            } else if deviation_secs < -3*60*60 {
                "\x1b[31m" // red
            } else if deviation_secs < -1*60*60 {
                "\x1b[38;5;208m" // orange
            } else {
                "\x1b[33m" // yellow
            };
            let mut reason = String::new();
            if expected == chrono::Duration::zero() {
                if let Some(s) = special {
                    reason = format!(" ({:?})", s.day_type);
                } else if matches!(weekday, chrono::Weekday::Sat | chrono::Weekday::Sun) {
                    reason = " (Weekend)".to_string();
                }
            }
            println!(
                "{}: {}  deviation: {}{}\x1b[0m{}",
                time.format("%a %Y-%m-%d"),
                format_chrono_duration(&sum),
                color,
                format_chrono_duration(&deviation),
                reason
            );
        }
    } else {
        // No two processes are allowed to monitor worktime at the same time.
        // It would lead to races in writing database file.
        let monitoring_lock = named_lock::NamedLock::create("worktime_monitoring").unwrap();
        let _guard = monitoring_lock.try_lock();
        if _guard.is_ok() {
            // One-shot channel: the monitoring thread sends the idle-gap receiver
            // back to us before it enters its blocking loop.
            let (rx_out_tx, rx_out_rx) = std::sync::mpsc::sync_channel::<
                Option<std::sync::mpsc::Receiver<crate::idle_popup::IdleGapInfo>>,
            >(1);

            // Spawn monitoring loop on a background thread.
            let database_bg = database.clone();
            let cfg_clone   = cfg.clone();
            std::thread::spawn(move || {
                run_interactive_monitoring(database_bg, &cfg_clone, rx_out_tx);
            });

            // Wait for the monitoring thread to hand us the idle-gap receiver.
            let idle_gap_rx = rx_out_rx.recv().ok().flatten();

            // GTK must be initialised on the main thread before eframe starts,
            // because libappindicator (tray icon) requires the GTK event loop to
            // run on the same thread that created it.  eframe/winit own the main
            // thread event loop, so we init GTK here and pump it from inside
            // the egui update() frame callback.
            gtk::init().expect("Failed to initialise GTK");

            // Run the egui main app on the main thread (required by eframe).
            // The root viewport IS the main window; close hides it (minimize to tray).
            let options = eframe::NativeOptions {
                viewport: egui::ViewportBuilder::default()
                    .with_title("worktime")
                    .with_inner_size([700.0, 600.0])
                    .with_min_inner_size([400.0, 300.0]),
                ..Default::default()
            };
            let database_ui         = database.clone();
            let daily_target_hours  = (cfg.weekly_hours as f64 / 5.0).round() as u32;
            let _ = eframe::run_native(
                "worktime",
                options,
                Box::new(move |cc| {
                    Ok(Box::new(main_window::MainApp::new(
                        database_ui, idle_gap_rx, daily_target_hours, cc,
                    )))
                }),
            );
        } else {
            println!("Another process is already monitoring worktime. exiting...")
        }
    }
}
