//! Idle detection abstraction layer
//!
//! This module provides a platform-agnostic interface for detecting user activity.
//! Platform-specific implementations:
//! - Linux: Uses evdev (/dev/input/event*) for input monitoring - works with any
//!          compositor (X11, Wayland, Sway, GNOME, KDE). Requires user to be in
//!          the 'input' group. Falls back to rdev if no devices are accessible.
//! - Windows/macOS: Uses rdev for input event monitoring

pub mod recorder;
mod session_state;

#[cfg(target_os = "linux")]
mod linux;

mod rdev_detector;

pub use session_state::SessionStateMachine;

pub use recorder::ActivityRecorder;

/// Represents a completed work session ready to be committed
#[derive(Debug, Clone, PartialEq)]
pub struct WorkSession {
    /// When the work session started (user became active after being high-level idle)
    pub start: chrono::DateTime<chrono::Local>,
    /// When the work session ended (user became high-level idle)
    pub end: chrono::DateTime<chrono::Local>,
}

/// Callback type for work session completion
pub type SessionCallback = Box<dyn Fn(WorkSession) + Send + 'static>;

/// Callback type fired when a new session starts (user returns from idle).
/// Argument is the session start time.
pub type SessionStartCallback = Box<dyn Fn(chrono::DateTime<chrono::Local>) + Send + 'static>;

/// Trait for platform-specific idle detection
pub trait IdleDetector: Send {
    /// Start monitoring for activity
    ///
    /// `on_session_end` is called with the completed session when the user goes idle.
    /// `on_session_start` is called with the start timestamp when the user returns from idle.
    fn start_monitoring(
        &mut self,
        on_session_end: SessionCallback,
        on_session_start: SessionStartCallback,
    ) -> Result<(), Box<dyn std::error::Error>>;
    
    /// Stop monitoring (cleanup)
    fn stop(&mut self) -> Result<(), Box<dyn std::error::Error>>;
    
    /// Get the current session (if any) for manual commit
    fn get_current_session(&self) -> Option<WorkSession>;
    
    /// Get the time since last activity (for live status display)
    ///
    /// Returns the duration since the last detected activity, or None if no activity has been detected yet.
    /// This provides a "live" view of the low-level idle state (most recent poll from the detector).
    fn get_idle_duration(&self) -> Option<chrono::Duration>;
}

/// Create a platform-specific idle detector
/// 
/// # Arguments
/// * `timeout_minutes` - Idle timeout in minutes (from config)
/// 
/// # Returns
/// * `Ok(Box<dyn IdleDetector>)` - Platform-specific detector
/// * `Err` - If detector initialization fails
/// 
/// # Platform-specific behavior
/// - **Linux**: Uses evdev to monitor /dev/input/event* devices. Requires user to be
///              in the 'input' group (`sudo usermod -a -G input $USER`, then log out/in).
///              Falls back to rdev if no evdev devices are accessible.
/// - **Windows/macOS**: Uses rdev for input event monitoring
pub fn create_idle_detector(timeout_minutes: u64) -> Result<Box<dyn IdleDetector>, Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    {
        match linux::LinuxIdleDetector::new(timeout_minutes) {
            Ok(detector) => {
                eprintln!("Using evdev for idle detection (Wayland/X11 compatible)");
                Ok(Box::new(detector))
            }
            Err(e) => {
                // rdev uses XRecord which only works under X11, not Wayland.
                // Allow the fallback only if DISPLAY is set and WAYLAND_DISPLAY is not.
                let has_x11 = std::env::var("DISPLAY").is_ok();
                let has_wayland = std::env::var("WAYLAND_DISPLAY").is_ok();
                if has_x11 && !has_wayland {
                    eprintln!("Warning: evdev not available: {}", e);
                    eprintln!("Falling back to rdev for idle detection (X11 detected)");
                    Ok(Box::new(rdev_detector::RdevIdleDetector::new(timeout_minutes)))
                } else {
                    let context = if has_wayland {
                        "Wayland session detected — rdev (X11-only) will not work here."
                    } else {
                        "No X11 or Wayland session detected — rdev will not work here."
                    };
                    eprintln!("Error: evdev not available: {}", e);
                    eprintln!("{context}");
                    Err(e)
                }
            }
        }
    }

    // On macOS and Windows rdev works natively — no evdev available anyway.
    #[cfg(not(target_os = "linux"))]
    {
        Ok(Box::new(rdev_detector::RdevIdleDetector::new(timeout_minutes)))
    }
}