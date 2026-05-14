use std::env;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use windows::{
    Win32::{
        Foundation::{CloseHandle, E_FAIL, HANDLE, WAIT_OBJECT_0},
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
static USER_AGENT_PROCESS_HANDLE_RAW: AtomicIsize = AtomicIsize::new(0);
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

unsafe fn launch_user_agent_process() -> Result<HandleGuard> {
    let session_id = unsafe { WTSGetActiveConsoleSessionId() };
    if session_id == u32::MAX {
        return Err(Error::new(E_FAIL, "No active console session"));
    }

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
        let process_handle = USER_AGENT_PROCESS_HANDLE_RAW.load(Ordering::SeqCst);
        if process_handle != 0 {
            if let Err(err) = unsafe { TerminateProcess(HANDLE(process_handle as *mut _), 0) } {
                log_win32_error("TerminateProcess(user)", &err);
            }
        }
    }
    0
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

    let result = unsafe { launch_user_agent_process() };
    let user_agent_process = match result {
        Ok(user_agent_process) => user_agent_process,
        Err(err) => {
            log_win32_error("launch_user_agent_process", &err);
            unsafe { report_status(SERVICE_STOPPED, 1) };
            return;
        }
    };
    USER_AGENT_PROCESS_HANDLE_RAW.store(user_agent_process.0.0 as isize, Ordering::SeqCst);
    unsafe { report_status(SERVICE_RUNNING, 0) };

    while !STOP_REQUESTED.load(Ordering::SeqCst) {
        if unsafe { WaitForSingleObject(user_agent_process.0, 1000) } == WAIT_OBJECT_0 {
            break;
        }
    }

    USER_AGENT_PROCESS_HANDLE_RAW.store(0, Ordering::SeqCst);
    unsafe { report_status(SERVICE_STOPPED, 0) };
}
