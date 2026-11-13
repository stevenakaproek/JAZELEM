//! Windows services through the Service Control Manager API - no sc.exe, no PowerShell.
//! Listing, start type changes, start / stop, and (for services Windows protects even from administrators)
//! registry writes under the SYSTEM account, all from Rust.

use std::ffi::c_void;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, BOOL, HANDLE, INVALID_HANDLE_VALUE, LUID};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, DuplicateTokenEx, ImpersonateLoggedOnUser, LookupPrivilegeValueW, RevertToSelf, SecurityImpersonation, TokenImpersonation,
    LUID_AND_ATTRIBUTES, SE_DEBUG_NAME, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_ALL_ACCESS, TOKEN_DUPLICATE, TOKEN_IMPERSONATE,
    TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
use windows_sys::Win32::System::Services::*;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};

const ERROR_MORE_DATA: u32 = 234;
const ERROR_SERVICE_ALREADY_RUNNING: u32 = 1056;
const ERROR_SERVICE_NOT_ACTIVE: u32 = 1062;
/// Asked for: Win32 services (own / shared process) and per-user services; list() then drops the per-user
/// templates, leaving the same set Win32_Service lists.
const LIST_TYPES: u32 = SERVICE_WIN32 | 0x40 | 0x80;
/// How long a stop may take (Stop-Service waits too).
const STOP_WAIT: Duration = Duration::from_secs(30);

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// "Access is denied. (os error 5)" for the last Win32 error.
fn last_error() -> String {
    std::io::Error::from_raw_os_error(unsafe { GetLastError() } as i32).to_string()
}

fn ok(b: BOOL, what: &str) -> Result<(), String> {
    if b != 0 {
        Ok(())
    } else {
        Err(format!("{what}: {}", last_error()))
    }
}

/// A service manager or service handle, closed on drop.
struct Sc(SC_HANDLE);

impl Drop for Sc {
    fn drop(&mut self) {
        unsafe { CloseServiceHandle(self.0) };
    }
}

fn manager(access: u32) -> Result<Sc, String> {
    let h = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), access) };
    if h.is_null() {
        Err(format!("cannot open the service manager: {}", last_error()))
    } else {
        Ok(Sc(h))
    }
}

fn service(name: &str, access: u32) -> Result<Sc, String> {
    let m = manager(SC_MANAGER_CONNECT)?;
    let h = unsafe { OpenServiceW(m.0, wide(name).as_ptr(), access) };
    if h.is_null() {
        Err(format!("cannot open service {name}: {}", last_error()))
    } else {
        Ok(Sc(h))
    }
}

/// What Win32_Service.State says for a SERVICE_STATUS.dwCurrentState.
pub fn state_name(s: u32) -> &'static str {
    match s {
        1 => "Stopped",
        2 => "Start Pending",
        3 => "Stop Pending",
        4 => "Running",
        5 => "Continue Pending",
        6 => "Pause Pending",
        7 => "Paused",
        _ => "Unknown",
    }
}

unsafe fn from_wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

/// Every service: (name, display name, state).
pub fn list() -> Result<Vec<(String, String, String)>, String> {
    let m = manager(SC_MANAGER_ENUMERATE_SERVICE)?;
    let mut out = vec![];
    let mut resume: u32 = 0;
    // u64 storage keeps the buffer aligned for the structs the API writes into it
    let mut buf: Vec<u64> = vec![0; 64 * 1024 / 8];
    loop {
        let (mut needed, mut count) = (0u32, 0u32);
        let r = unsafe {
            EnumServicesStatusExW(
                m.0,
                SC_ENUM_PROCESS_INFO,
                LIST_TYPES,
                SERVICE_STATE_ALL,
                buf.as_mut_ptr() as *mut u8,
                (buf.len() * 8) as u32,
                &mut needed,
                &mut count,
                &mut resume,
                std::ptr::null(),
            )
        };
        let more = r == 0 && unsafe { GetLastError() } == ERROR_MORE_DATA;
        if r == 0 && !more {
            return Err(format!("cannot list the services: {}", last_error()));
        }
        let items = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW, count as usize) };
        for it in items {
            // per-user templates (0x40 without the 0x80 instance bit) are not listed by Win32_Service either:
            // the list shows their per-user instances, and changes to those go to the template anyway
            let ty = it.ServiceStatusProcess.dwServiceType;
            if ty & 0x40 != 0 && ty & 0x80 == 0 {
                continue;
            }
            let (name, display) = unsafe { (from_wide(it.lpServiceName), from_wide(it.lpDisplayName)) };
            out.push((name, display, state_name(it.ServiceStatusProcess.dwCurrentState).to_string()));
        }
        if !more {
            break;
        }
        // grow when not even one more entry fitted
        if count == 0 {
            buf.resize((needed as usize / 8 + 1).max(buf.len() * 2), 0);
        }
    }
    Ok(out)
}

fn status(h: &Sc) -> Result<SERVICE_STATUS, String> {
    let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
    ok(unsafe { QueryServiceStatus(h.0, &mut st) }, "cannot read the service state")?;
    Ok(st)
}

/// Some(true) running, Some(false) any other state, None when it cannot be read.
pub fn is_running(name: &str) -> Option<bool> {
    let h = service(name, SERVICE_QUERY_STATUS).ok()?;
    status(&h).ok().map(|s| s.dwCurrentState == SERVICE_RUNNING)
}

/// Start type: 2 automatic, 3 manual, 4 disabled; `delayed` = Some(true/false) also sets Delayed start
/// (what `sc config start= delayed-auto` / `auto` do), None leaves it.
pub fn set_start(name: &str, start: u32, delayed: Option<bool>) -> Result<(), String> {
    let h = service(name, SERVICE_CHANGE_CONFIG)?;
    let none = std::ptr::null();
    ok(
        unsafe { ChangeServiceConfigW(h.0, SERVICE_NO_CHANGE, start, SERVICE_NO_CHANGE, none, none, std::ptr::null_mut(), none, none, none, none) },
        "ChangeServiceConfig failed",
    )?;
    if let Some(d) = delayed {
        let info = SERVICE_DELAYED_AUTO_START_INFO { fDelayedAutostart: d as BOOL };
        ok(
            unsafe { ChangeServiceConfig2W(h.0, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, &info as *const _ as *const c_void) },
            "setting Delayed start failed",
        )?;
    }
    Ok(())
}

/// Start a service (already running = Ok).
pub fn start(name: &str) -> Result<(), String> {
    let h = service(name, SERVICE_START)?;
    if unsafe { StartServiceW(h.0, 0, std::ptr::null()) } == 0 {
        let e = unsafe { GetLastError() };
        if e != ERROR_SERVICE_ALREADY_RUNNING {
            return Err(format!("cannot start {name}: {}", std::io::Error::from_raw_os_error(e as i32)));
        }
    }
    Ok(())
}

/// Running services that depend on `h`, in the order they can be stopped.
fn active_dependents(h: &Sc) -> Vec<String> {
    let (mut needed, mut count) = (0u32, 0u32);
    unsafe { EnumDependentServicesW(h.0, SERVICE_ACTIVE, std::ptr::null_mut(), 0, &mut needed, &mut count) };
    if needed == 0 {
        return vec![];
    }
    let mut buf: Vec<u64> = vec![0; needed as usize / 8 + 1];
    let r = unsafe { EnumDependentServicesW(h.0, SERVICE_ACTIVE, buf.as_mut_ptr() as *mut ENUM_SERVICE_STATUSW, (buf.len() * 8) as u32, &mut needed, &mut count) };
    if r == 0 {
        return vec![];
    }
    let items = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const ENUM_SERVICE_STATUSW, count as usize) };
    items.iter().map(|it| unsafe { from_wide(it.lpServiceName) }).collect()
}

/// Send stop and wait until it has stopped. Not running = Ok.
fn stop_one(name: &str, deadline: Instant) -> Result<(), String> {
    let h = service(name, SERVICE_STOP | SERVICE_QUERY_STATUS)?;
    let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
    if unsafe { ControlService(h.0, SERVICE_CONTROL_STOP, &mut st) } == 0 {
        let e = unsafe { GetLastError() };
        if e == ERROR_SERVICE_NOT_ACTIVE {
            return Ok(());
        }
        return Err(format!("cannot stop {name}: {}", std::io::Error::from_raw_os_error(e as i32)));
    }
    loop {
        let s = status(&h)?;
        if s.dwCurrentState == SERVICE_STOPPED {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("{name} did not stop within {} s", STOP_WAIT.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Stop a service and, first, the running services that depend on it (what Stop-Service -Force does).
pub fn stop(name: &str) -> Result<(), String> {
    let h = service(name, SERVICE_QUERY_STATUS | SERVICE_ENUMERATE_DEPENDENTS)?;
    if status(&h)?.dwCurrentState == SERVICE_STOPPED {
        return Ok(());
    }
    let deadline = Instant::now() + STOP_WAIT;
    for d in active_dependents(&h) {
        stop_one(&d, deadline)?;
    }
    stop_one(name, deadline)
}

/// Open a folder in Explorer (the shell does it; no process is started by PC Tuner).
pub fn open_folder(path: &std::path::Path) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let p = wide(&path.display().to_string());
    unsafe { ShellExecuteW(std::ptr::null_mut(), wide("open").as_ptr(), p.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
}

// ------------------------------------------------------------------ SYSTEM

/// A token handle, closed on drop.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(self.0) };
        }
    }
}

fn enable_debug_privilege() -> Result<(), String> {
    let mut tok: HANDLE = std::ptr::null_mut();
    ok(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut tok) }, "cannot open PC Tuner's token")?;
    let tok = Handle(tok);
    let mut luid: LUID = unsafe { std::mem::zeroed() };
    ok(unsafe { LookupPrivilegeValueW(std::ptr::null(), SE_DEBUG_NAME, &mut luid) }, "cannot look up the debug privilege")?;
    let tp = TOKEN_PRIVILEGES { PrivilegeCount: 1, Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }] };
    ok(unsafe { AdjustTokenPrivileges(tok.0, 0, &tp, 0, std::ptr::null_mut(), std::ptr::null_mut()) }, "cannot enable the debug privilege")?;
    // AdjustTokenPrivileges "succeeds" without the privilege (ERROR_NOT_ALL_ASSIGNED = 1300)
    if unsafe { GetLastError() } == 1300 {
        return Err("PC Tuner is not running as administrator".into());
    }
    Ok(())
}

/// Process id of winlogon.exe (it always runs as SYSTEM).
fn winlogon_pid() -> Option<u32> {
    let snap = Handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) });
    if snap.0 == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut more = unsafe { Process32FirstW(snap.0, &mut e) } != 0;
    while more {
        let n = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
        if String::from_utf16_lossy(&e.szExeFile[..n]).eq_ignore_ascii_case("winlogon.exe") {
            return Some(e.th32ProcessID);
        }
        more = unsafe { Process32NextW(snap.0, &mut e) } != 0;
    }
    None
}

fn system_token() -> Result<Handle, String> {
    enable_debug_privilege()?;
    let pid = winlogon_pid().ok_or("winlogon.exe was not found")?;
    let proc = Handle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) });
    if proc.0.is_null() {
        return Err(format!("cannot open winlogon.exe: {}", last_error()));
    }
    let mut tok: HANDLE = std::ptr::null_mut();
    ok(unsafe { OpenProcessToken(proc.0, TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_IMPERSONATE, &mut tok) }, "cannot open the SYSTEM token")?;
    let tok = Handle(tok);
    let mut dup: HANDLE = std::ptr::null_mut();
    ok(
        unsafe { DuplicateTokenEx(tok.0, TOKEN_ALL_ACCESS, std::ptr::null(), SecurityImpersonation, TokenImpersonation, &mut dup) },
        "cannot copy the SYSTEM token",
    )?;
    Ok(Handle(dup))
}

/// Run `f` as SYSTEM, on a thread of its own (the caller's thread never changes identity), then go back.
/// Used only for the registry writes Windows refuses to administrators (protected services).
pub fn as_system<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> Result<R, String> {
    std::thread::spawn(move || {
        let tok = system_token()?;
        ok(unsafe { ImpersonateLoggedOnUser(tok.0) }, "cannot run as SYSTEM")?;
        let r = f();
        unsafe { RevertToSelf() };
        Ok(r)
    })
    .join()
    .map_err(|_| "the SYSTEM step stopped unexpectedly".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opt-in (`cargo test -- --ignored winsvc_same_as`): the native list against what PowerShell's
    /// Win32_Service returned before - same services, same states. Read-only; PowerShell is used only here.
    #[test]
    #[ignore]
    fn winsvc_same_as_win32_service() {
        let ours: std::collections::BTreeMap<String, String> = list().unwrap().into_iter().map(|(n, _, s)| (n.to_lowercase(), s)).collect();
        let out = crate::common::ps("Get-CimInstance Win32_Service | ForEach-Object { $_.Name + '|' + $_.State }").unwrap();
        let wmi: std::collections::BTreeMap<String, String> = out.lines().filter_map(|l| l.trim().split_once('|')).map(|(n, s)| (n.to_lowercase(), s.to_string())).collect();
        let only_ours: Vec<&String> = ours.keys().filter(|k| !wmi.contains_key(*k)).collect();
        let only_wmi: Vec<&String> = wmi.keys().filter(|k| !ours.contains_key(*k)).collect();
        let state_diff: Vec<(&String, &String, &String)> = ours.iter().filter_map(|(k, s)| wmi.get(k).filter(|w| *w != s).map(|w| (k, s, w))).collect();
        println!("native {} / Win32_Service {}; only native {only_ours:?}; only WMI {only_wmi:?}; state differs {state_diff:?}", ours.len(), wmi.len());
        assert!(only_ours.is_empty() && only_wmi.is_empty(), "the same services");
        // a service can change state between the two reads; anything more is a mapping bug
        assert!(state_diff.len() <= 2, "states agree");
    }

    /// Read-only: lists services and reads one state on this PC (changes nothing).
    #[test]
    fn winsvc_reads_this_pc() {
        let v = list().expect("service list");
        assert!(v.len() > 100, "a Windows install has well over 100 services");
        let wmi = v.iter().find(|s| s.0.eq_ignore_ascii_case("Winmgmt")).expect("Winmgmt is listed");
        assert_eq!(wmi.2, "Running");
        assert!(!wmi.1.is_empty(), "display names are read");
        assert_eq!(is_running("Winmgmt"), Some(true));
        assert_eq!(is_running("No-Such-Service-PCTuner"), None);
        assert_eq!(state_name(1), "Stopped");
        assert!(set_start("No-Such-Service-PCTuner", 3, None).unwrap_err().contains("cannot open service"));
    }
}
