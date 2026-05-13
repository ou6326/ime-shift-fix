mod hook;
mod service;

use std::mem::size_of;
use std::{env, process};

use windows::{
    Win32::{
        Foundation::{CloseHandle, E_FAIL, HANDLE},
        Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
        System::Threading::{
            GetCurrentProcess, GetExitCodeProcess, INFINITE, OpenProcessToken,
            WaitForSingleObject,
        },
        UI::{
            Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW},
            WindowsAndMessaging::SW_HIDE,
        },
    },
    core::{Error, HRESULT, PCWSTR, Result},
};

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

fn print_help(program: &str) {
    println!(
        "Usage: {program} [OPTION]\n\nOptions:\n  --install       Install and start the Windows service\n  --uninstall     Stop and uninstall the Windows service\n  --service       Run as a Windows service (used by SCM)\n  -h, --help      Show this help\n\nWithout options, runs in the foreground."
    );
}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn error_from_io(err: std::io::Error) -> Error {
    let code = err
        .raw_os_error()
        .map(|code| HRESULT::from_win32(code as u32))
        .unwrap_or(E_FAIL);
    Error::new(code, err.to_string())
}

fn is_elevated() -> Result<bool> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let _token_guard = HandleGuard(token);

        let mut elevation = TOKEN_ELEVATION::default();
        let mut ret_len = 0u32;
        GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        )?;

        Ok(elevation.TokenIsElevated != 0)
    }
}

fn run_elevated_and_wait(args: &str) -> Result<u32> {
    let exe = env::current_exe().map_err(error_from_io)?;
    let exe_w = to_wide(&exe.display().to_string());
    let args_w = to_wide(args);
    let op = to_wide("runas");

    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(op.as_ptr()),
        lpFile: PCWSTR(exe_w.as_ptr()),
        lpParameters: PCWSTR(args_w.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };

    unsafe { ShellExecuteExW(&mut info)? };
    let process = HandleGuard(info.hProcess);
    unsafe { WaitForSingleObject(process.0, INFINITE) };

    let mut exit_code = 1u32;
    unsafe { GetExitCodeProcess(process.0, &mut exit_code)? };
    Ok(exit_code)
}

fn report_elevated_service_result(arg: &str, exit_code: u32) {
    let installed = match service::is_installed() {
        Ok(installed) => installed,
        Err(err) => {
            eprintln!(
                "Elevated {arg} finished, but service state could not be queried. Exit code: {exit_code}."
            );
            print_error(&err);
            return;
        }
    };

    match (arg, installed, exit_code) {
        ("--install", true, 0) => println!("Install completed: service {} is installed.", service::DISPLAY_NAME),
        ("--install", true, _) => {
            eprintln!("Install finished with exit code {exit_code}, but service {} is installed.", service::DISPLAY_NAME)
        }
        ("--install", false, _) => {
            eprintln!("Install did not complete: service {} is not installed. Exit code: {exit_code}.", service::DISPLAY_NAME)
        }
        ("--uninstall", false, 0) => println!("Uninstall completed: service {} has been uninstalled.", service::DISPLAY_NAME),
        ("--uninstall", false, _) => {
            eprintln!(
                "Uninstall finished with exit code {exit_code}, and service {} is not installed.", service::DISPLAY_NAME
            )
        }
        ("--uninstall", true, _) => {
            eprintln!("Uninstall did not complete: service {} still exists. Exit code: {exit_code}.", service::DISPLAY_NAME)
        }
        _ => println!("Elevated {arg} finished. Exit code: {exit_code}."),
    }
}

fn service_change_requires_elevation(arg: &str) -> Result<bool> {
    let installed = service::is_installed()?;
    match (arg, installed) {
        ("--install", true) => {
            eprintln!("Service {} already installed", service::DISPLAY_NAME);
            Ok(false)
        }
        ("--uninstall", false) => {
            eprintln!("Service {} is not installed", service::DISPLAY_NAME);
            Ok(false)
        }
        _ => Ok(true),
    }
}

fn ensure_elevated_for_service_change(arg: &str) -> Result<bool> {
    if !service_change_requires_elevation(arg)? {
        return Ok(false);
    }

    if is_elevated()? {
        return Ok(true);
    }

    let exit_code = run_elevated_and_wait(arg)?;
    report_elevated_service_result(arg, exit_code);
    process::exit(exit_code as i32);
}

fn print_error(err: &Error) {
    eprintln!("{}\nHRESULT: {}", err.message(), err.code());
}

fn run_cli() -> Result<()> {
    let mut args = env::args();
    let program = args.next().unwrap_or_else(|| service::NAME.to_string());
    let rest: Vec<String> = args.collect();
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();

    match rest.as_slice() {
        [] => hook::run(true),
        ["-h" | "--help"] => {
            print_help(&program);
            Ok(())
        }
        ["--install"] => {
            if !ensure_elevated_for_service_change("--install")? {
                return Ok(());
            }
            service::install()
        }
        ["--uninstall"] => {
            if !ensure_elevated_for_service_change("--uninstall")? {
                return Ok(());
            }
            service::uninstall()
        }
        ["--service"] => service::run(),
        ["--user"] => hook::run(false),
        _ => {
            eprintln!("Unknown or invalid arguments.\n");
            print_help(&program);
            process::exit(2);
        }
    }
}

fn main() {
    if let Err(err) = run_cli() {
        print_error(&err);
        process::exit(1);
    }
}
