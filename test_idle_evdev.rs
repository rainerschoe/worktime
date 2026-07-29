//! Standalone proof of concept for evdev-based idle detection
//!
//! This monitors keyboard/mouse/touchpad input devices in /dev/input/event* and tracks
//! the time since the last input event. It's a generic Linux solution that works
//! with any compositor (X11, Wayland, etc.).
//!
//! Uses non-blocking I/O so all devices can be polled in a single thread.

use std::fs;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use evdev::{Device, InputEventKind, Key};

/// Returns true if the device is a relevant input device (keyboard, mouse, touchpad, etc.)
fn is_relevant_input_device(device: &Device) -> bool {
    let supported = device.supported_events();
    let has_keys = supported.contains(evdev::EventType::KEY);
    let has_rel = supported.contains(evdev::EventType::RELATIVE);
    
    if !has_keys && !has_rel {
        return false;
    }
    
    // If only keys (no relative axes), verify it has actual keyboard/mouse keys
    if has_keys && !has_rel {
        if let Some(keys) = device.supported_keys() {
            let has_keyboard_keys = keys.contains(Key::KEY_A)
                || keys.contains(Key::KEY_SPACE)
                || keys.contains(Key::KEY_ENTER);
            let has_mouse_buttons = keys.contains(Key::BTN_LEFT)
                || keys.contains(Key::BTN_RIGHT)
                || keys.contains(Key::BTN_MIDDLE)
                || keys.contains(Key::BTN_TOUCH);
            
            if !has_keyboard_keys && !has_mouse_buttons {
                return false;
            }
        }
    }
    
    true
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== evdev Idle Detection Proof of Concept ===\n");
    
    let mut devices = Vec::new();
    let mut skipped = Vec::new();
    let input_dir = PathBuf::from("/dev/input");
    
    println!("Scanning for input devices in /dev/input/...");
    
    if let Ok(entries) = fs::read_dir(&input_dir) {
        let mut paths: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name()
                .map(|n| n.to_string_lossy().starts_with("event"))
                .unwrap_or(false))
            .collect();
        paths.sort();
        
        for path in paths {
            match Device::open(&path) {
                Ok(device) => {
                    let device_name = device.name().unwrap_or("Unknown").to_string();
                    if is_relevant_input_device(&device) {
                        // Set non-blocking mode on the file descriptor
                        let fd = device.as_raw_fd();
                        unsafe {
                            let flags = libc::fcntl(fd, libc::F_GETFL, 0);
                            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
                        }
                        println!("  ✓ Monitoring: {} ({})", path.display(), device_name);
                        devices.push((path.clone(), device));
                    } else {
                        skipped.push(format!("{} ({})", path.display(), device_name));
                    }
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::PermissionDenied {
                        println!("  ✗ Permission denied: {} (add yourself to 'input' group)", path.display());
                    } else {
                        println!("  ✗ Failed: {}: {}", path.display(), e);
                    }
                }
            }
        }
    }
    
    if !skipped.is_empty() {
        println!("\n  Skipped (not keyboard/mouse/touchpad):");
        for s in &skipped {
            println!("    - {}", s);
        }
    }
    
    if devices.is_empty() {
        eprintln!("\nError: No relevant input devices accessible!");
        return Ok(());
    }
    
    println!("\nMonitoring {} device(s) for keyboard/mouse activity...", devices.len());
    println!("Idle threshold: 15 seconds (for testing)");
    println!("Poll interval: 5 seconds");
    println!("Press Ctrl+C to exit.\n");

    use std::io::Write;

    let mut last_activity = Instant::now();
    let idle_threshold = Duration::from_secs(15); // 15s for easy manual testing
    let poll_interval = Duration::from_secs(5);
    let mut was_idle = false;

    println!("[  0s] Starting - move mouse or press a key to test activity detection");
    std::io::stdout().flush().ok();

    loop {
        // Sleep first (5s poll interval)
        std::thread::sleep(poll_interval);

        // Poll all devices for events (non-blocking, kernel buffers events between polls)
        let mut had_activity = false;
        let mut activity_device = String::new();

        for (path, device) in &mut devices {
            match device.fetch_events() {
                Ok(events) => {
                    for event in events {
                        match event.kind() {
                            InputEventKind::Key(_)
                            | InputEventKind::RelAxis(_)
                            | InputEventKind::AbsAxis(_) => {
                                if !had_activity {
                                    activity_device = path
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .to_string();
                                }
                                had_activity = true;
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => {
                    eprintln!("Error on {}: {}", path.display(), e);
                }
            }
        }

        if had_activity {
            last_activity = Instant::now();
        }

        let idle_duration = last_activity.elapsed();
        let is_idle = idle_duration >= idle_threshold;

        // Print status on every poll
        let elapsed_total = last_activity.elapsed();
        if is_idle {
            println!(
                "[{:>3}s] 🔴 IDLE  - no activity for {:.0}s (threshold: {}s)",
                elapsed_total.as_secs(),
                idle_duration.as_secs_f64(),
                idle_threshold.as_secs()
            );
        } else if had_activity {
            println!(
                "[  0s] 🟢 ACTIVE - activity detected on {}",
                activity_device
            );
        } else {
            println!(
                "[{:>3}s] 🟢 ACTIVE - last input {:.0}s ago",
                elapsed_total.as_secs(),
                idle_duration.as_secs_f64()
            );
        }
        std::io::stdout().flush().ok();

        // Report state transitions
        if is_idle && !was_idle {
            println!("      ^^^ TRANSITION: Active → Idle");
            was_idle = true;
        } else if !is_idle && was_idle {
            println!("      ^^^ TRANSITION: Idle → Active");
            was_idle = false;
        }
    }
}