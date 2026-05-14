use std::io::Write;
use std::mem::size_of;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};

use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        System::{LibraryLoader::GetModuleHandleA, Threading::GetCurrentThreadId},
        UI::{
            Input::KeyboardAndMouse::{
                INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
                VIRTUAL_KEY, VK_LSHIFT, VK_RSHIFT,
            },
            WindowsAndMessaging::{
                CallNextHookEx, DispatchMessageA, GetForegroundWindow, GetMessageA, HHOOK,
                KBDLLHOOKSTRUCT, MSG, PostMessageA, PostThreadMessageA, SetWindowsHookExA,
                TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN,
                WM_KEYUP, WM_LBUTTONDOWN, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_QUIT, WM_SYSKEYUP,
            },
        },
    },
    core::{Error, PCSTR, Result},
};

// Repeat=1, scanCode=0, extended=0, context=0, prevState=1, transition=1.
const KEYUP_LPARAM: LPARAM = LPARAM(0xC0000001isize);

static SHIFT_DOWN: AtomicBool = AtomicBool::new(false);
static SHIFT_SELECT_GUARD_ARMED: AtomicBool = AtomicBool::new(false);
static TARGET_HWND: AtomicIsize = AtomicIsize::new(0);
static SUPPRESS_COUNT: AtomicU64 = AtomicU64::new(0);

struct HookGuard(HHOOK);

impl Drop for HookGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.0.0.is_null() {
                let _ = UnhookWindowsHookEx(self.0);
            }
        }
    }
}

#[cfg(debug_assertions)]
fn log_win32_error(context: &str, err: &Error) {
    eprintln!("{context} failed: {err}");
}

#[cfg(not(debug_assertions))]
fn log_win32_error(_context: &str, _err: &Error) {}

unsafe fn post_shift_keyup(hwnd: HWND, vk_code: u32) {
    if hwnd.is_invalid() {
        return;
    }

    if let Err(err) =
        unsafe { PostMessageA(Some(hwnd), WM_KEYUP, WPARAM(vk_code as usize), KEYUP_LPARAM) }
    {
        log_win32_error("PostMessageA(WM_KEYUP)", &err);
    }
    if let Err(err) = unsafe {
        PostMessageA(
            Some(hwnd),
            WM_SYSKEYUP,
            WPARAM(vk_code as usize),
            KEYUP_LPARAM,
        )
    } {
        log_win32_error("PostMessageA(WM_SYSKEYUP)", &err);
    }
}

unsafe fn inject_shift_keyup(vk_code: u32) {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk_code as u16),
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let sent = unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
    if sent == 0 {
        log_win32_error("SendInput", &Error::from_thread());
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let kb_struct = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let vk_code = kb_struct.vkCode;
        if vk_code == VK_LSHIFT.0 as u32 || vk_code == VK_RSHIFT.0 as u32 {
            match wparam.0 as u32 {
                WM_KEYDOWN => SHIFT_DOWN.store(true, Ordering::SeqCst),
                WM_KEYUP | WM_SYSKEYUP => {
                    let should_suppress = SHIFT_SELECT_GUARD_ARMED.load(Ordering::SeqCst);
                    let target_hwnd_raw = TARGET_HWND.load(Ordering::SeqCst);
                    let target_hwnd = if target_hwnd_raw != 0 {
                        HWND(target_hwnd_raw as *mut _)
                    } else {
                        unsafe { GetForegroundWindow() }
                    };

                    SHIFT_DOWN.store(false, Ordering::SeqCst);

                    if should_suppress {
                        let count = SUPPRESS_COUNT.fetch_add(1, Ordering::SeqCst);
                        let msg = "[Shift+Click] Selection IME mode protected";
                        if count == 0 {
                            println!("{msg}");
                        } else {
                            print!("\x1b[1A\r{msg} (+{count})\n");
                            let _ = std::io::stdout().flush();
                        }

                        SHIFT_SELECT_GUARD_ARMED.store(false, Ordering::SeqCst);
                        TARGET_HWND.store(0, Ordering::SeqCst);

                        unsafe { post_shift_keyup(target_hwnd, vk_code) };
                        unsafe { inject_shift_keyup(vk_code) };

                        return LRESULT(1);
                    }

                    SHIFT_SELECT_GUARD_ARMED.store(false, Ordering::SeqCst);
                    TARGET_HWND.store(0, Ordering::SeqCst);
                    return unsafe { CallNextHookEx(None, code, wparam, lparam) };
                }
                _ => {}
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        if SHIFT_DOWN.load(Ordering::SeqCst) && wparam.0 as u32 == WM_LBUTTONDOWN {
            SHIFT_SELECT_GUARD_ARMED.store(true, Ordering::SeqCst);
            let hwnd = unsafe { GetForegroundWindow() };
            TARGET_HWND.store(hwnd.0 as isize, Ordering::SeqCst);

            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }
        if wparam.0 as u32 == WM_MOUSEWHEEL || wparam.0 as u32 == WM_MOUSEHWHEEL {
            SHIFT_SELECT_GUARD_ARMED.store(false, Ordering::SeqCst);
            TARGET_HWND.store(0, Ordering::SeqCst);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

pub fn run(register_ctrlc: bool) -> Result<()> {
    unsafe {
        let main_thread_id = GetCurrentThreadId();
        let h_instance = Some(HINSTANCE(GetModuleHandleA(PCSTR(ptr::null()))?.0));

        let keyboard_hook = SetWindowsHookExA(WH_KEYBOARD_LL, Some(keyboard_proc), h_instance, 0)?;
        if keyboard_hook.0.is_null() {
            return Err(Error::from_thread());
        }
        let _keyboard_guard = HookGuard(keyboard_hook);

        let mouse_hook = SetWindowsHookExA(WH_MOUSE_LL, Some(mouse_proc), h_instance, 0)?;
        if mouse_hook.0.is_null() {
            return Err(Error::from_thread());
        }
        let _mouse_guard = HookGuard(mouse_hook);

        println!("Hooks set successfully");

        if register_ctrlc {
            ctrlc::set_handler(move || {
                if let Err(err) = PostThreadMessageA(main_thread_id, WM_QUIT, WPARAM(0), LPARAM(0))
                {
                    log_win32_error("PostThreadMessageA(WM_QUIT)", &err);
                }
            })
            .expect("Error setting Ctrl-C handler");
        }

        let mut msg = MSG::default();
        loop {
            let ret = GetMessageA(&mut msg, Some(HWND::default()), 0, 0).0;
            match ret {
                -1 => return Err(Error::from_thread()),
                0 => break,
                _ => {
                    let _ = TranslateMessage(&msg);
                    let _ = DispatchMessageA(&msg);
                }
            }
        }

        println!("Hooks unset successfully");
    }
    Ok(())
}
