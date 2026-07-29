//! Linux idle detection using evdev
//!
//! This module monitors /dev/input/event* devices to detect keyboard and mouse
//! activity. It works with any Linux compositor (X11, Wayland, Sway, GNOME, KDE).
//!
//! Requirements:
//!   - User must be in the 'input' group: sudo usermod -a -G input $USER
//!   - Then log out and back in

use std::fs;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use evdev::{Device, InputEventKind, Key};
use super::{WorkSession, IdleDetector, SessionCallback, SessionStateMachine};

/// Returns true if the device is a relevant input device (keyboard, mouse, touchpad)
/// Filters out audio devices, microphones, video buses, and other non-input devices.
fn is_relevant_input_device(device: &Device) -> bool {
    let supported = device.supported_events();
    let has_keys = supported.contains(evdev::EventType::KEY);
    let has_rel = supported.contains(evdev::EventType::RELATIVE);

    if !has_keys && !has_rel {
        return false;
    }

    // If only keys (no relative axes), verify it has actual keyboard/mouse keys
    // This filters out audio volume keys, media controls, etc.
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

/// Open all relevant input devices in non-blocking mode.
/// Returns the opened devices and a flag indicating whether any were blocked by permissions.
fn open_input_devices() -> (Vec<(PathBuf, Device)>, bool) {
    let mut devices = Vec::new();
    let mut any_permission_denied = false;
    let input_dir = PathBuf::from("/dev/input");

    if let Ok(entries) = fs::read_dir(&input_dir) {
        let mut paths: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().starts_with("event"))
                    .unwrap_or(false)
            })
            .collect();
        paths.sort();

        for path in paths {
            match Device::open(&path) {
                Ok(device) => {
                    if is_relevant_input_device(&device) {
                        // Set non-blocking mode
                        let fd = device.as_raw_fd();
                        unsafe {
                            let flags = libc::fcntl(fd, libc::F_GETFL, 0);
                            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
                        }
                        devices.push((path, device));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    any_permission_denied = true;
                }
                Err(_) => {
                    // Skip other errors (device vanished, etc.)
                }
            }
        }
    }

    (devices, any_permission_denied)
}

/// Linux idle detector using evdev input device monitoring
pub struct LinuxIdleDetector {
    session_machine: Arc<Mutex<SessionStateMachine>>,
    last_activity_time: Arc<Mutex<Option<chrono::DateTime<chrono::Local>>>>,
}

impl LinuxIdleDetector {
    /// Create a new Linux evdev idle detector
    ///
    /// # Arguments
    /// * `timeout_minutes` - Idle timeout in minutes
    ///
    /// # Returns
    /// * `Ok(LinuxIdleDetector)` - If at least one input device is accessible
    /// * `Err` - If no input devices are accessible (user not in 'input' group)
    pub fn new(timeout_minutes: u64) -> Result<Self, Box<dyn std::error::Error>> {
        // Check that we can open at least one input device
        let (devices, any_permission_denied) = open_input_devices();
        if devices.is_empty() {
            if any_permission_denied {
                return Err(
                    "Permission denied opening /dev/input/event* devices.\n\
                     Your user is not in the 'input' group.\n\
                     Fix: sudo usermod -a -G input $USER\n\
                     Then log out and back in (a new terminal is not enough)."
                        .into(),
                );
            }
            return Err(
                "No accessible input devices found. \
                 Add your user to the 'input' group: sudo usermod -a -G input $USER \
                 (then log out and back in)"
                    .into(),
            );
        }

        if any_permission_denied {
            eprintln!(
                "evdev: WARNING - some /dev/input/event* devices could not be opened (permission denied).\n\
                 Your keyboard or mouse may not be monitored.\n\
                 Fix: sudo usermod -a -G input $USER  then log out and back in."
            );
        }

        eprintln!(
            "evdev: found {} accessible input device(s)",
            devices.len()
        );

        Ok(LinuxIdleDetector {
            session_machine: Arc::new(Mutex::new(SessionStateMachine::new(timeout_minutes))),
            last_activity_time: Arc::new(Mutex::new(None)),
        })
    }
}

impl IdleDetector for LinuxIdleDetector {
    fn start_monitoring(
        &mut self,
        on_session_end: SessionCallback,
        on_session_start: super::SessionStartCallback,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let on_session_end   = Arc::new(Mutex::new(on_session_end));
        let on_session_start = Arc::new(Mutex::new(on_session_start));
        let session_machine = self.session_machine.clone();
        let last_activity_time = self.last_activity_time.clone();

        // Initialize with current time - starting the app is activity!
        let now = chrono::Local::now();
        *last_activity_time.lock().unwrap() = Some(now);
        session_machine.lock().unwrap().on_activity(now);
        // Fire session-start for the initial session
        on_session_start.lock().unwrap()(now);

        thread::spawn(move || {
            let mut devices = Vec::new();
            let mut last_device_refresh = std::time::Instant::now();
            let device_refresh_interval = std::time::Duration::from_secs(60); // Refresh devices every 60 seconds

            loop {
                // Refresh device list periodically to handle device changes (USB devices, lid close/open, etc.)
                if devices.is_empty() || last_device_refresh.elapsed() >= device_refresh_interval {
                    let (new_devices, permission_denied) = open_input_devices();
                    devices = new_devices;
                    last_device_refresh = std::time::Instant::now();
                    
                    if devices.is_empty() {
                        if permission_denied {
                            eprintln!(
                                "evdev: ERROR - permission denied on all /dev/input/event* devices.\n\
                                 Fix: sudo usermod -a -G input $USER  then log out and back in."
                            );
                        } else {
                            eprintln!("evdev: WARNING - no input devices accessible, will retry");
                        }
                        thread::sleep(Duration::from_secs(5));
                    } else {
                        eprintln!(
                            "evdev: monitoring {} input device(s) for activity",
                            devices.len()
                        );
                    }
                }

                // Poll all devices for events (non-blocking)
                let mut had_activity = false;
                let mut need_refresh = false;
                let mut total_events_processed = 0;
                const MAX_EVENTS_PER_POLL: usize = 100; // Prevent infinite loop on event flood

                for (path, device) in &mut devices {
                    match device.fetch_events() {
                        Ok(events) => {
                            for event in events {
                                total_events_processed += 1;
                                
                                // Safety limit: if we've processed too many events in one poll cycle,
                                // break out to allow the sleep to happen and prevent CPU spinning
                                if total_events_processed > MAX_EVENTS_PER_POLL {
                                    eprintln!("evdev: WARNING - event flood detected, processed {} events. Breaking to prevent CPU spin.", total_events_processed);
                                    had_activity = true;
                                    break;
                                }
                                
                                match event.kind() {
                                    InputEventKind::Key(_)
                                    | InputEventKind::RelAxis(_)
                                    | InputEventKind::AbsAxis(_) => {
                                        had_activity = true;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            // No events available - normal
                        }
                        Err(e) => {
                            // Any device error (disconnect, ENODEV, EOVERFLOW after suspend/resume)
                            // forces an immediate device list refresh so we never get stuck with
                            // stale file descriptors that will never deliver events again.
                            eprintln!("evdev: error reading from {:?}: {} — will refresh device list", path, e);
                            need_refresh = true;
                        }
                    }
                }

                // Any device error triggers an immediate refresh on the next iteration.
                // This handles ENODEV/EOVERFLOW that occur after suspend/resume or USB reconnect,
                // which would otherwise cause the polling loop to silently stop seeing events.
                if need_refresh {
                    devices.clear();
                    // Also reset the refresh timer so the new devices are reported
                    last_device_refresh = std::time::Instant::now() - device_refresh_interval;
                }

                let now = chrono::Local::now();

                // Update last activity time if we had activity
                if had_activity {
                    *last_activity_time.lock().unwrap() = Some(now);
                }

                // Process activity and check timeout in a single lock acquisition
                let (completed_session, session_just_started) = {
                    let mut sm = session_machine.lock().unwrap();
                    let was_idle = sm.get_current_session().is_none();

                    // Process activity through the state machine
                    let activity_result = if had_activity {
                        sm.on_activity(now)
                    } else {
                        None
                    };

                    // Check for timeout (no activity)
                    let timeout_result = sm.check_timeout(now);

                    // Detect Idle→Active transition: was idle before, has session now
                    let is_now_active = sm.get_current_session().is_some();
                    let session_just_started = was_idle && is_now_active && had_activity;

                    (activity_result.or(timeout_result), session_just_started)
                };

                // Fire session-start callback when user returns from idle
                if session_just_started {
                    eprintln!("evdev: new session started (user returned from idle)");
                    on_session_start.lock().unwrap()(now);
                }

                // Handle completed session outside the lock
                if let Some(session) = completed_session {
                    eprintln!("evdev: session completed due to timeout");
                    on_session_end.lock().unwrap()(session);
                }

                // Poll every 5 seconds - sufficient for multi-second accuracy
                // The kernel buffers all events so we won't miss any activity
                thread::sleep(Duration::from_secs(5));
            }
        });

        Ok(())
    }

    fn stop(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Thread will be terminated when the process exits
        Ok(())
    }

    fn get_current_session(&self) -> Option<WorkSession> {
        self.session_machine.lock().unwrap().get_current_session()
    }
    
    fn get_idle_duration(&self) -> Option<chrono::Duration> {
        let last_activity = self.last_activity_time.lock().unwrap();
        last_activity.map(|last| chrono::Local::now() - last)
    }
}