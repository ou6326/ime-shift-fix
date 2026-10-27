use std::env;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use windows::{
    Win32::{
        Foundation::{CloseHandle, E_FAIL, HANDLE, WAIT_FAILED, WAIT_OBJECT_0},
        System::{
            Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock},
            RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken},
            Services::{
                CloseServiceHandle, ControlService, CreateServiceW, DeleteService, OpenSCManagerW,
                OpenServiceW, QueryServiceStatus, RegisterServiceCtrlHandlerExW, SC_HANDLE,
                SC_MANAGER_CONNECT, SC_MANAGER_CREATE_SERVICE, SERVICE_ACCEPT_STOP,
                SERVICE_AUTO_START, SERVICE_CONTROL_STOP, SERVICE_ERROR_NORMAL,
                SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START, SERVICE_START_PENDING,
                SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STATUS_HANDLE, SERVICE_STOP,
                SERVICE_STOP_PENDING, SERVICE_STOPPED, SERVICE_TABLE_ENTRYW,
                SERVICE_WIN32_OWN_PROCESS, SetServiceStatus, StartServiceCtrlDispatcherW,
                StartServiceW,
            },
            Threading::{
                CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW,
                PROCESS_INFORMATION, STARTF_USESHOWWINDOW, STARTUPINFOW, TerminateProcess,
                WaitForSingleObject,
            },
        },
        UI::WindowsAndMessaging::SW_HIDE,
    },
    core::{Error, HRESULT, PCWSTR, PWSTR, Result, w},
};

pub const NAME: &str = "ime_shift_fix";
pub const DISPLAY_NAME: &str = "IME Shift Fix";

const NAME_W: PCWSTR = w!("ime_shift_fix");
const DISPLAY_NAME_W: PCWSTR = w!("IME Shift Fix");
const SERVICE_DELETE_ACCESS: u32 = 0x0001_0000;
const ERROR_SERVICE_ALREADY_RUNNING: u32 = 1056;
const ERROR_SERVICE_DOES_NOT_EXIST: u32 = 1060;
const ERROR_SERVICE_NOT_ACTIVE: u32 = 1062;
const SERVICE_STATE_TIMEOUT: Duration = Duration::from_secs(15);

static STATUS_HANDLE_RAW: AtomicIsize = AtomicIsize::new(0);
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

struct ServiceHandle(SC_HANDLE);

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_invalid() {
                let _ = CloseServiceHandle(self.0);
            }
        }
    }
}

struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_invalid() {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

struct EnvironmentBlock(*mut core::ffi::c_void);

impl Drop for EnvironmentBlock {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                let _ = DestroyEnvironmentBlock(self.0);
            }
        }
    }
}

fn hresult_from_io(err: &std::io::Error) -> HRESULT {
    err.raw_os_error()
        .map(|code| HRESULT::from_win32(code as u32))
        .unwrap_or(E_FAIL)
}

#[cfg(debug_assertions)]
fn log_win32_error(context: &str, err: &Error) {
    eprintln!("{context} failed: {err}");
}

#[cfg(not(debug_assertions))]
fn log_win32_error(_context: &str, _err: &Error) {}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn is_win32_error(err: &Error, code: u32) -> bool {
    err.code() == HRESULT::from_win32(code)
}

unsafe fn query_status(service: SC_HANDLE) -> Result<SERVICE_STATUS> {
    let mut status = SERVICE_STATUS::default();
    unsafe { QueryServiceStatus(service, &mut status)? };
    Ok(status)
}

unsafe fn wait_for_state(
    service: SC_HANDLE,
    target: SERVICE_STATUS_CURRENT_STATE,
    timeout: Duration,
) -> Result<SERVICE_STATUS> {
    let started = Instant::now();
    loop {
        let status = unsafe { query_status(service)? };
        if status.dwCurrentState == target {
            return Ok(status);
        }
        if started.elapsed() >= timeout {
            return Ok(status);
        }
        thread::sleep(Duration::from_millis(250));
    }
}

unsafe fn start_service(service: SC_HANDLE) -> Result<()> {
    match unsafe { StartServiceW(service, None) } {
        Ok(()) => {}
        Err(err) if is_win32_error(&err, ERROR_SERVICE_ALREADY_RUNNING) => {}
        Err(err) => return Err(err),
    }

    let status = unsafe { wait_for_state(service, SERVICE_RUNNING, SERVICE_STATE_TIMEOUT)? };
    if status.dwCurrentState == SERVICE_RUNNING {
        println!("Service started: {DISPLAY_NAME}");
    } else {
        println!(
            "Service start requested, current state: {}",
            status.dwCurrentState.0
        );
    }
    Ok(())
}

unsafe fn stop_service(service: SC_HANDLE) -> Result<bool> {
    let status = unsafe { query_status(service)? };
    if status.dwCurrentState == SERVICE_STOPPED {
        return Ok(false);
    }

    let mut stop_status = SERVICE_STATUS::default();
    match unsafe { ControlService(service, SERVICE_CONTROL_STOP, &mut stop_status) } {
        Ok(()) => {}
        Err(err) if is_win32_error(&err, ERROR_SERVICE_NOT_ACTIVE) => return Ok(false),
        Err(err) => return Err(err),
    }

    let status = unsafe { wait_for_state(service, SERVICE_STOPPED, SERVICE_STATE_TIMEOUT)? };
    if status.dwCurrentState == SERVICE_STOPPED {
        println!("Service stopped: {DISPLAY_NAME}");
    } else {
        println!(
            "Service stop requested, current state: {}",
            status.dwCurrentState.0
        );
    }
    Ok(status.dwCurrentState == SERVICE_STOPPED)
}

pub fn install() -> Result<()> {
    let exe =
        env::current_exe().map_err(|err| Error::new(hresult_from_io(&err), err.to_string()))?;
    let binary_path = to_wide(&format!("\"{}\" --service", exe.display()));

    unsafe {
        let manager = ServiceHandle(OpenSCManagerW(
            PCWSTR::null(),
            PCWSTR::null(),
            SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE,
        )?);

        match OpenServiceW(manager.0, NAME_W, SERVICE_START | SERVICE_QUERY_STATUS) {
            Ok(service) => {
                let service = ServiceHandle(service);
                println!("Service already installed: {DISPLAY_NAME}");
                start_service(service.0)?;
                return Ok(());
            }
            Err(err) if is_win32_error(&err, ERROR_SERVICE_DOES_NOT_EXIST) => {}
            Err(err) => return Err(err),
        }

        let service = CreateServiceW(
            manager.0,
            NAME_W,
            DISPLAY_NAME_W,
            SERVICE_START | SERVICE_QUERY_STATUS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            PCWSTR(binary_path.as_ptr()),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
        )?;
        let service = ServiceHandle(service);
        println!("Service installed: {DISPLAY_NAME}");
        start_service(service.0)?;
    }

    Ok(())
}

pub fn is_installed() -> Result<bool> {
    unsafe {
        let manager = ServiceHandle(OpenSCManagerW(
            PCWSTR::null(),
            PCWSTR::null(),
            SC_MANAGER_CONNECT,
        )?);

        match OpenServiceW(manager.0, NAME_W, SERVICE_QUERY_STATUS) {
            Ok(service) => {
                let _service = ServiceHandle(service);
                Ok(true)
            }
            Err(err) if is_win32_error(&err, ERROR_SERVICE_DOES_NOT_EXIST) => Ok(false),
            Err(err) => Err(err),
        }
    }
}

pub fn uninstall() -> Result<()> {
    unsafe {
        let manager = ServiceHandle(OpenSCManagerW(
            PCWSTR::null(),
            PCWSTR::null(),
            SC_MANAGER_CONNECT,
        )?);
        let service = match OpenServiceW(
            manager.0,
            NAME_W,
            SERVICE_DELETE_ACCESS | SERVICE_STOP | SERVICE_QUERY_STATUS,
        ) {
            Ok(service) => ServiceHandle(service),
            Err(err) if is_win32_error(&err, ERROR_SERVICE_DOES_NOT_EXIST) => {
                eprintln!("Service {DISPLAY_NAME} is not installed.");
                return Ok(());
            }
            Err(err) => return Err(err),
        };
        let stopped = stop_service(service.0)?;
        let status = query_status(service.0)?;
        if !stopped && status.dwCurrentState != SERVICE_STOPPED {
            eprintln!(
                "Service {DISPLAY_NAME} is not stopped; uninstall aborted. Current state: {}",
                status.dwCurrentState.0
            );
            return Ok(());
        }
        DeleteService(service.0)?;
    }

    println!("Service {DISPLAY_NAME} uninstalled.");
    Ok(())
}

unsafe fn launch_user_agent_process(session_id: u32) -> Result<HandleGuard> {
    let mut token = HANDLE::default();
    unsafe { WTSQueryUserToken(session_id, &mut token)? };
    let token = HandleGuard(token);

    let mut environment = core::ptr::null_mut();
    unsafe { CreateEnvironmentBlock(&mut environment, Some(token.0), false)? };
    let environment = EnvironmentBlock(environment);

    let exe =
        env::current_exe().map_err(|err| Error::new(hresult_from_io(&err), err.to_string()))?;
    let exe_path = exe.display().to_string();
    let mut exe_w = to_wide(&exe_path);
    let mut command_line = to_wide(&format!("\"{exe_path}\" --user"));
    let mut desktop = to_wide("winsta0\\default");
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        lpDesktop: PWSTR(desktop.as_mut_ptr()),
        dwFlags: STARTF_USESHOWWINDOW,
        wShowWindow: SW_HIDE.0 as u16,
        ..Default::default()
    };
    let mut process_info = PROCESS_INFORMATION::default();

    unsafe {
        CreateProcessAsUserW(
            Some(token.0),
            PCWSTR(exe_w.as_mut_ptr()),
            Some(PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            Some(environment.0),
            PCWSTR::null(),
            &startup,
            &mut process_info,
        )?;
    }

    if !process_info.hThread.is_invalid() {
        let _thread = HandleGuard(process_info.hThread);
    }

    println!(
        "User session process started: pid {}",
        process_info.dwProcessId
    );
    Ok(HandleGuard(process_info.hProcess))
}

pub fn run() -> Result<()> {
    let service_table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR(NAME_W.as_ptr() as *mut _),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];

    unsafe { StartServiceCtrlDispatcherW(service_table.as_ptr()) }
}

unsafe fn report_status(state: SERVICE_STATUS_CURRENT_STATE, exit_code: u32) {
    let handle = STATUS_HANDLE_RAW.load(Ordering::SeqCst);
    if handle == 0 {
        return;
    }

    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP
        } else {
            0
        },
        dwWin32ExitCode: exit_code,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: 0,
    };
    if let Err(err) = unsafe { SetServiceStatus(SERVICE_STATUS_HANDLE(handle as *mut _), &status) }
    {
        log_win32_error("SetServiceStatus", &err);
    }
}

unsafe extern "system" fn service_ctrl_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut core::ffi::c_void,
    _context: *mut core::ffi::c_void,
) -> u32 {
    if control == SERVICE_CONTROL_STOP {
        unsafe { report_status(SERVICE_STOP_PENDING, 0) };
        STOP_REQUESTED.store(true, Ordering::SeqCst);
    }
    0
}

unsafe fn terminate_agent(process: HANDLE) -> Result<()> {
    if unsafe { WaitForSingleObject(process, 0) } == WAIT_OBJECT_0 {
        return Ok(());
    }
    unsafe { TerminateProcess(process, 0)? };
    match unsafe { WaitForSingleObject(process, 1000) } {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_FAILED => Err(Error::from_thread()),
        _ => Err(Error::new(E_FAIL, "User agent did not terminate in time")),
    }
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let handle =
        match unsafe { RegisterServiceCtrlHandlerExW(NAME_W, Some(service_ctrl_handler), None) } {
            Ok(handle) => handle,
            Err(_) => return,
        };
    STATUS_HANDLE_RAW.store(handle.0 as isize, Ordering::SeqCst);
    STOP_REQUESTED.store(false, Ordering::SeqCst);
    unsafe { report_status(SERVICE_START_PENDING, 0) };

    // RUNNING means the supervisor is ready, including while waiting for login.
    // Only this thread owns process handles; the control callback only signals stop.
    let mut agent: Option<(u32, HandleGuard)> = None;
    let mut exit_code = 0;
    unsafe { report_status(SERVICE_RUNNING, 0) };

    while !STOP_REQUESTED.load(Ordering::SeqCst) {
        let session = unsafe { WTSGetActiveConsoleSessionId() };
        if let Some((agent_session, process)) = agent.as_ref() {
            let status = unsafe { WaitForSingleObject(process.0, 0) };
            if status == WAIT_FAILED {
                log_win32_error("WaitForSingleObject(user)", &Error::from_thread());
                exit_code = 1;
                break;
            }
            if replace_agent(session, *agent_session, status == WAIT_OBJECT_0) {
                if status != WAIT_OBJECT_0 {
                    if let Err(err) = unsafe { terminate_agent(process.0) } {
                        log_win32_error("TerminateProcess(user)", &err);
                        exit_code = 1;
                        break;
                    }
                }
                agent = None;
            }
        }
        if agent.is_none() && session != u32::MAX && !STOP_REQUESTED.load(Ordering::SeqCst) {
            match unsafe { launch_user_agent_process(session) } {
                Ok(process) => agent = Some((session, process)),
                // A console session can exist before a user token is available.
                // Retry after login instead of permanently stopping the service.
                Err(err) => log_win32_error("launch_user_agent_process (will retry)", &err),
            }
        }
        thread::sleep(Duration::from_secs(1));
    }

    if let Some((_, process)) = agent {
        if let Err(err) = unsafe { terminate_agent(process.0) } {
            log_win32_error("TerminateProcess(user)", &err);
            exit_code = 1;
        }
    }
    unsafe { report_status(SERVICE_STOPPED, exit_code) };
}

fn replace_agent(active_session: u32, agent_session: u32, exited: bool) -> bool {
    exited || active_session != agent_session
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_termination_waits_for_exit() {
        use std::os::windows::{io::AsRawHandle, process::CommandExt};
        use std::process::Command;

        // The child runs only this test as a harmless stand-in for the agent.
        if env::var_os("IME_SHIFT_FIX_TEST_AGENT").is_some() {
            thread::sleep(Duration::from_secs(10));
            return;
        }
        let mut child = Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "service::tests::agent_termination_waits_for_exit",
            ])
            .env("IME_SHIFT_FIX_TEST_AGENT", "1")
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()
            .unwrap();
        let handle = HANDLE(child.as_raw_handle());
        let result = unsafe { terminate_agent(handle) };
        if result.is_err() {
            let _ = child.kill();
        }
        child.wait().unwrap();
        result.unwrap();
        // An already exited agent must also be safe to clean up.
        unsafe { terminate_agent(handle) }.unwrap();
    }

    #[test]
    fn console_agent_is_replaced_on_exit_or_session_change() {
        assert!(!replace_agent(1, 1, false));
        assert!(replace_agent(1, 1, true));
        assert!(replace_agent(2, 1, false));
        assert!(replace_agent(u32::MAX, 1, false));
    }
}
