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

use chrono::Datelike;
use clap::Parser;

use crate::cli::Args;
use crate::config::Config;
use crate::database::DbHandle;
use crate::models::SpecialDayType;
use crate::monitoring::run_monitoring;
use crate::utils::format_chrono_duration;

fn main() {
    let args = Args::parse();
    let cfg: Config = confy::load("worktime", None).unwrap();

    let data_path = expanduser::expanduser(cfg.data_file.as_str()).unwrap();
    println!("Using data file {}", data_path.display());

    let special_day_path = expanduser::expanduser(cfg.special_day_file.as_str()).unwrap();
    println!("Using special_day file {}", special_day_path.display());

    let db = DbHandle::open(data_path, special_day_path).unwrap();

    if args.overtime {
        let overtime_end: chrono::DateTime<chrono::offset::Local> =
            std::time::SystemTime::now().into();
        let overtime = db.with(|d| d.calculate_overtime(
            chrono::Duration::hours(cfg.weekly_hours),
            (cfg.cutoff_datetime, overtime_end),
        )) + chrono::Duration::seconds((cfg.cutoff_day_overtime_hours * 3600.0) as i64);
        println!("overtime: {}", format_chrono_duration(&overtime));

    } else if let Some(days) = args.daysums {
        let daysums = db.with(|d| d.get_day_sums(days));
        let expected_per_day_secs = (cfg.weekly_hours as f64 / 5.0) * 3600.0;
        let expected_per_day = chrono::Duration::seconds(expected_per_day_secs.round() as i64);
        db.with(|d| {
            for (time, sum) in &daysums {
                let weekday = time.weekday();
                let special = d.special_days.iter().find(|sd| {
                    sd.day == time.date_naive() && matches!(sd.day_type,
                        SpecialDayType::Vacation | SpecialDayType::Sick | SpecialDayType::Holiday)
                });
                let expected = match (weekday, special) {
                    (chrono::Weekday::Sat | chrono::Weekday::Sun, _) => chrono::Duration::zero(),
                    (_, Some(_)) => chrono::Duration::zero(),
                    _ => expected_per_day,
                };
                let deviation = *sum - expected;
                let deviation_secs = deviation.num_seconds();
                let color = if deviation_secs > 0 { "\x1b[32m" }
                    else if deviation_secs < -3*60*60 { "\x1b[31m" }
                    else if deviation_secs < -1*60*60 { "\x1b[38;5;208m" }
                    else { "\x1b[33m" };
                let mut reason = String::new();
                if expected == chrono::Duration::zero() {
                    if let Some(s) = special {
                        reason = format!(" ({:?})", s.day_type);
                    } else if matches!(weekday, chrono::Weekday::Sat | chrono::Weekday::Sun) {
                        reason = " (Weekend)".to_string();
                    }
                }
                println!("{}: {}  deviation: {}{}\x1b[0m{}",
                    time.format("%a %Y-%m-%d"),
                    format_chrono_duration(sum),
                    color,
                    format_chrono_duration(&deviation),
                    reason);
            }
        });

    } else {
        // Only one monitoring process at a time.
        let monitoring_lock = named_lock::NamedLock::create("worktime_monitoring").unwrap();
        let _guard = monitoring_lock.try_lock();
        if _guard.is_ok() {
            // Start monitoring on a background thread; it returns the gap receiver.
            let db_bg  = db.clone();
            let cfg_bg = cfg.clone();
            let (gap_tx, gap_rx) = std::sync::mpsc::channel::<crate::idle_popup::IdleGapInfo>();
            let gap_tx_test = gap_tx.clone();

            std::thread::spawn(move || {
                if let Some(rx) = run_monitoring(db_bg, &cfg_bg) {
                    // Forward gaps from the monitoring channel to the UI channel.
                    for gap in rx {
                        let _ = gap_tx.send(gap);
                    }
                }
            });

            // ── Test: inject a fake idle-gap after 2 s ────────────────────────
            if args.test_idle_popup {
                let tx = gap_tx_test;
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    let now = chrono::Local::now();
                    let _ = tx.send(crate::idle_popup::IdleGapInfo {
                        gap_start: now - chrono::Duration::hours(1),
                        gap_end:   now,
                    });
                });
            }

            // Run the egui main app on the main thread.
            let options = eframe::NativeOptions {
                viewport: egui::ViewportBuilder::default()
                    .with_title("worktime")
                    .with_inner_size([700.0, 600.0])
                    .with_min_inner_size([400.0, 300.0]),
                ..Default::default()
            };
            let daily_target_hours = (cfg.weekly_hours as f64 / 5.0).round() as u32;
            let _ = eframe::run_native(
                "worktime",
                options,
                Box::new(move |cc| {
                    Ok(Box::new(main_window::MainApp::new(
                        db, Some(gap_rx), daily_target_hours, cc,
                    )))
                }),
            );
        } else {
            println!("Another process is already monitoring worktime. exiting...");
        }
    }
}
