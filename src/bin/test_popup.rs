// Manual visual test for the idle-gap popup.
//
// Run with:
//   cargo run --bin test_popup
//
// The popup opens with a synthetic 47-minute idle gap (5-minute snapping active).
// Try dragging the two handles, then click any of the three buttons.
// The chosen answer is printed to stdout.

#[path = "../utils.rs"]   mod utils;
#[path = "../idle_popup.rs"] mod idle_popup;

use chrono::{Local, Duration};
use idle_popup::{IdleGapInfo, ask_idle_gap};

fn main() {
    let now   = Local::now();
    let start = now - Duration::minutes(47);

    let gap = IdleGapInfo {
        gap_start: start,
        gap_end:   now,
    };

    println!(
        "Opening idle popup for gap  {} → {}  (47 min)",
        gap.gap_start.format("%H:%M:%S"),
        gap.gap_end.format("%H:%M:%S"),
    );

    let answer = ask_idle_gap(gap);

    println!("Answer: {:?}", answer);
}
