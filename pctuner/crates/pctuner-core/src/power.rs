//! Power button: restart into the BIOS (UEFI setup), or shut down so the next start opens the BIOS.
//! Uses Windows' own `shutdown /fw` (needs administrator and a UEFI PC).
//!
//! Logic only: the UI draws `Power` and calls `pick`.

use crate::common::{self, Job};

#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Choice {
    RestartToBios,
    ShutdownToBios,
}

impl Choice {
    pub fn title(self) -> &'static str {
        match self {
            Choice::RestartToBios => "Restart into BIOS",
            Choice::ShutdownToBios => "Shut down, BIOS on next start",
        }
    }
    pub fn text(self) -> &'static str {
        match self {
            Choice::RestartToBios => "Windows restarts and opens your motherboard's BIOS (UEFI) setup straight away.",
            Choice::ShutdownToBios => "The PC turns off. The next time you press the power button it goes straight into the BIOS.",
        }
    }
    /// Arguments for shutdown.exe. /fw = next boot goes to the firmware setup; /t 0 = no waiting.
    pub fn args(self) -> [&'static str; 4] {
        match self {
            Choice::RestartToBios => ["/r", "/fw", "/t", "0"],
            Choice::ShutdownToBios => ["/s", "/fw", "/t", "0"],
        }
    }
    /// Headline while Windows is closing programs.
    pub fn working_title(self) -> &'static str {
        match self {
            Choice::RestartToBios => "Restarting into the BIOS...",
            Choice::ShutdownToBios => "Shutting down...",
        }
    }
    /// Headline when the restart / shut down did not happen.
    pub fn failed_title(self) -> &'static str {
        match self {
            Choice::RestartToBios => "Windows did not restart",
            Choice::ShutdownToBios => "Windows did not shut down",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Stage {
    Choose,
    /// The choice and the UI time (seconds) the command was sent.
    Working(Choice, f64),
}

/// Seconds after which a restart / shut down that did not happen is reported (it was cancelled,
/// for example on Windows' "apps are preventing restart" screen, or blocked by a program).
pub const WORKING_TIMEOUT: f64 = 25.0;

/// True when PC Tuner is still running `WORKING_TIMEOUT` seconds after the command was sent.
pub fn working_timed_out(started: f64, now: f64) -> bool {
    now - started >= WORKING_TIMEOUT
}

pub fn is_uefi() -> bool {
    common::run("cmd.exe", &["/c", "echo %firmware_type%"]).map(|(_, out)| out.trim().eq_ignore_ascii_case("UEFI")).unwrap_or(false)
}

/// The Power window's state.
pub struct Power {
    pub stage: Stage,
    /// Some(true) when the PC boots in UEFI mode (needed for /fw); checked when the window opens.
    pub uefi: Option<bool>,
    pub uefi_job: Option<Job<bool>>,
    pub job: Option<Job<Result<(i32, String), String>>>,
    pub error: Option<String>,
}

impl Default for Power {
    fn default() -> Self {
        Self::new()
    }
}

impl Power {
    pub fn new() -> Self {
        Power { stage: Stage::Choose, uefi: None, uefi_job: Some(Job::spawn(is_uefi)), job: None, error: None }
    }

    /// Call often: picks up the firmware check and the result of shutdown.exe.
    pub fn pump(&mut self) {
        if let Some(j) = &self.uefi_job {
            if let Some(v) = j.poll() {
                self.uefi = Some(v);
                self.uefi_job = None;
            }
        }
        if let Some(j) = &self.job {
            if let Some(r) = j.poll() {
                self.job = None;
                match r {
                    // shutdown.exe returns at once; Windows then closes everything
                    Ok((0, _)) => {}
                    Ok((code, out)) => {
                        self.error = Some(format!("Windows refused (code {code}): {}", out.trim()));
                        self.stage = Stage::Choose;
                    }
                    Err(e) => {
                        self.error = Some(e);
                        self.stage = Stage::Choose;
                    }
                }
            }
        }
    }

    /// Why the choices cannot be used, if they cannot.
    pub fn blocked(&self) -> Option<&'static str> {
        if !common::is_admin() {
            Some("Run PC Tuner as administrator to use this.")
        } else if self.uefi == Some(false) {
            Some("This PC does not boot in UEFI mode, so Windows cannot open the BIOS for you. Press Del while the PC starts instead.")
        } else {
            None
        }
    }

    /// May a choice be clicked now (not blocked, and the firmware check is done)?
    pub fn can_pick(&self) -> bool {
        self.blocked().is_none() && self.uefi.is_some()
    }

    /// The user clicked a choice: no second window, the click is the confirmation. `now`: UI time in seconds.
    pub fn pick(&mut self, c: Choice, now: f64) {
        self.error = None;
        self.stage = Stage::Working(c, now);
        self.job = Some(Job::spawn(move || common::run("shutdown.exe", &c.args())));
    }

    /// "Back" on the timed-out screen.
    pub fn back(&mut self) {
        self.stage = Stage::Choose;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_shutdown_args() {
        assert_eq!(Choice::RestartToBios.args(), ["/r", "/fw", "/t", "0"]);
        assert_eq!(Choice::ShutdownToBios.args(), ["/s", "/fw", "/t", "0"]);
    }

    #[test]
    fn power_working_times_out() {
        assert!(!working_timed_out(100.0, 100.0));
        assert!(!working_timed_out(100.0, 124.9));
        assert!(working_timed_out(100.0, 125.0));
        assert!(working_timed_out(100.0, 500.0));
    }
}
