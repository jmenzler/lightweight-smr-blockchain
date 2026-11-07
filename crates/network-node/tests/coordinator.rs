//! The coordinator must give up with a clear error when fewer participants connect than announced.

use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn a_missing_participant_ends_in_a_timeout_error_not_a_hang() {
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_coordinator"))
        .args([
            "--port",
            "0",
            "--n",
            "3",
            "--rounds",
            "5",
            "--connect-timeout-ms",
            "300",
        ])
        .output()
        .expect("spawn coordinator");
    assert!(started.elapsed() < Duration::from_secs(10), "hung");
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("timed out") && stderr.contains("0 of 3 nodes"),
        "{stderr}"
    );
}
