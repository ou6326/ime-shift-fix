use std::cell::Cell;
use std::mem::size_of;
use std::ptr;

use crate::shift_state::{ShiftState, Side};

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
                KBDLLHOOKSTRUCT, LLKHF_ALTDOWN, LLKHF_EXTENDED, LLKHF_INJECTED, LLMHF_INJECTED,
                MSG, MSLLHOOKSTRUCT, PostMessageA, PostThreadMessageA, SetWindowsHookExA,
                TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN,
                WM_KEYUP, WM_LBUTTONDOWN, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_QUIT, WM_SYSKEYDOWN,
                WM_SYSKEYUP,
            },
        },
    },
    core::{Error, PCSTR, Result},
};

#[derive(Clone, Copy, Debug, Default)]
pub enum Compensation {
    #[default]
    Both,
    Post,
    Inject,
}

// Low-level callbacks execute on the installing thread. Copy state out before
// SendInput, which may re-enter the hook; never hold a mutable borrow across it.
thread_local! {
    static SHIFT_STATE: Cell<ShiftState> = Cell::new(ShiftState::default());
    static COMPENSATION: Cell<Compensation> = const { Cell::new(Compensation::Both) };
    static SUPPRESS_COUNT: Cell<u64> = const { Cell::new(0) };
    static COMPENSATION_ERRORS: Cell<u64> = const { Cell::new(0) };
}

fn update_state<T>(update: impl FnOnce(&mut ShiftState) -> T) -> T {
    SHIFT_STATE.with(|cell| {
        let mut state = cell.get();
        let result = update(&mut state);
        cell.set(state);
        result
    })
}

fn record_compensation_error() {
    COMPENSATION_ERRORS.set(COMPENSATION_ERRORS.get().saturating_add(1));
}

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

fn keyup_lparam(kb: &KBDLLHOOKSTRUCT) -> LPARAM {
    let mut bits = 0xC000_0001u32 | ((kb.scanCode & 0xff) << 16);
    if kb.flags.contains(LLKHF_EXTENDED) {
        bits |= 1 << 24;
    }
    if kb.flags.contains(LLKHF_ALTDOWN) {
        bits |= 1 << 29;
    }
    LPARAM(bits as isize)
}

unsafe fn post_shift_keyup(hwnd: HWND, message: u32, kb: &KBDLLHOOKSTRUCT) {
    if hwnd.is_invalid() {
        return;
    }

    if unsafe {
        PostMessageA(
            Some(hwnd),
            message,
            WPARAM(kb.vkCode as usize),
            keyup_lparam(kb),
        )
    }
    .is_err()
    {
        record_compensation_error();
    }
}

unsafe fn inject_shift_keyup(vk_code: u32) -> bool {
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
        record_compensation_error();
    }
    sent != 0
}

fn keyboard_transition(message: u32, kb: &KBDLLHOOKSTRUCT) -> Option<isize> {
    // Synthetic releases must not consume another physical key's guard.
    if kb.flags.contains(LLKHF_INJECTED) {
        return None;
    }
    let side = if kb.vkCode == VK_LSHIFT.0 as u32 {
        Side::Left
    } else if kb.vkCode == VK_RSHIFT.0 as u32 {
        Side::Right
    } else {
        return None;
    };
    match message {
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            update_state(|state| state.press(side));
            None
        }
        WM_KEYUP | WM_SYSKEYUP => update_state(|state| state.release(side)),
        _ => None,
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let kb = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if let Some(target) = keyboard_transition(wparam.0 as u32, &kb) {
            let mode = COMPENSATION.get();
            if matches!(mode, Compensation::Both | Compensation::Post) {
                unsafe { post_shift_keyup(HWND(target as *mut _), wparam.0 as u32, &kb) };
            }
            if matches!(mode, Compensation::Both | Compensation::Inject)
                && !unsafe { inject_shift_keyup(kb.vkCode) }
            {
                // UIPI can reject injection. Let the real release through so
                // Windows cannot be left believing Shift is still held.
                return unsafe { CallNextHookEx(None, code, wparam, lparam) };
            }
            SUPPRESS_COUNT.set(SUPPRESS_COUNT.get().saturating_add(1));
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let mouse = unsafe { *(lparam.0 as *const MSLLHOOKSTRUCT) };
        if mouse.flags & LLMHF_INJECTED != 0 {
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }
        if wparam.0 as u32 == WM_LBUTTONDOWN {
            let hwnd = unsafe { GetForegroundWindow() };
            update_state(|state| state.click(hwnd.0 as isize));
        }
        if wparam.0 as u32 == WM_MOUSEWHEEL || wparam.0 as u32 == WM_MOUSEHWHEEL {
            update_state(ShiftState::cancel_selection);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

pub fn run(register_ctrlc: bool, compensation: Compensation) -> Result<()> {
    SHIFT_STATE.set(ShiftState::default());
    COMPENSATION.set(compensation);
    SUPPRESS_COUNT.set(0);
    COMPENSATION_ERRORS.set(0);
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

        println!("Hooks set successfully (compensation: {compensation:?})");

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

        println!(
            "Hooks unset successfully; protected releases: {}; compensation errors: {}",
            SUPPRESS_COUNT.get(),
            COMPENSATION_ERRORS.get()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_release_does_not_consume_physical_selection() {
        SHIFT_STATE.set(ShiftState::default());
        let mut kb = KBDLLHOOKSTRUCT {
            vkCode: VK_LSHIFT.0 as u32,
            ..Default::default()
        };
        assert_eq!(keyboard_transition(WM_KEYDOWN, &kb), None);
        update_state(|state| state.click(42));
        kb.flags = LLKHF_INJECTED;
        assert_eq!(keyboard_transition(WM_KEYUP, &kb), None);
        kb.flags = Default::default();
        assert_eq!(keyboard_transition(WM_KEYUP, &kb), Some(42));
        assert_eq!(keyboard_transition(WM_KEYUP, &kb), None);
    }

    #[test]
    fn injected_press_does_not_arm_selection() {
        SHIFT_STATE.set(ShiftState::default());
        let mut kb = KBDLLHOOKSTRUCT {
            vkCode: VK_RSHIFT.0 as u32,
            flags: LLKHF_INJECTED,
            ..Default::default()
        };
        keyboard_transition(WM_KEYDOWN, &kb);
        update_state(|state| state.click(42));
        kb.flags = Default::default();
        assert_eq!(keyboard_transition(WM_KEYUP, &kb), None);
    }

    #[test]
    fn system_shift_messages_are_tracked() {
        SHIFT_STATE.set(ShiftState::default());
        let kb = KBDLLHOOKSTRUCT {
            vkCode: VK_RSHIFT.0 as u32,
            flags: LLKHF_ALTDOWN,
            ..Default::default()
        };
        keyboard_transition(WM_SYSKEYDOWN, &kb);
        update_state(|state| state.click(42));
        assert_eq!(keyboard_transition(WM_SYSKEYUP, &kb), Some(42));
    }

    #[test]
    fn posted_release_preserves_scan_code_and_context() {
        let kb = KBDLLHOOKSTRUCT {
            scanCode: 0x36,
            flags: LLKHF_ALTDOWN | LLKHF_EXTENDED,
            ..Default::default()
        };
        assert_eq!(keyup_lparam(&kb).0 as u32, 0xE136_0001);
        assert_eq!(
            keyup_lparam(&KBDLLHOOKSTRUCT::default()).0 as u32,
            0xC000_0001
        );
    }
}
