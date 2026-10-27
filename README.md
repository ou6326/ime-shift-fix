# IME Shift Fix

[![Crates.io](https://img.shields.io/crates/v/ime_shift_fix.svg)](https://crates.io/crates/ime_shift_fix)
[![Docs.rs](https://docs.rs/ime_shift_fix/badge.svg)](https://docs.rs/ime_shift_fix)
[![CI](https://github.com/ou6326/ime-shift-fix/actions/workflows/ci.yml/badge.svg)](https://github.com/ou6326/ime-shift-fix/actions/workflows/ci.yml)
[![License](https://img.shields.io/crates/l/ime_shift_fix.svg)](https://github.com/ou6326/ime-shift-fix#license)

Windows utility that protects IME mode while selecting text with `Shift+Click`.

The foreground mode installs low-level keyboard and mouse hooks in the current user session. When a physical Shift key-up follows a Shift+left-click selection, the tool suppresses that key-up and posts/injects a replacement key-up. It does not read or set IME mode directly; the effect depends on how the application and input method handle synthetic input.

Left and right Shift are tracked separately. Injected keyboard and mouse events do not change the physical selection state. A wheel event cancels selection protection. If `SendInput` fails, the physical release is allowed through to avoid leaving Shift logically held. Posted releases preserve the original message type and scan code, rather than posting both normal and system key-up messages.

## Usage

Run in the current terminal:

```powershell
ime-shift-fix.exe
```

Install as a Windows service:

```powershell
ime-shift-fix.exe --install
```

Uninstall the service:

```powershell
ime-shift-fix.exe --uninstall
```

Show help:

```powershell
ime-shift-fix.exe --help
```

## Service Mode

The service does not install hooks inside Session 0. Instead, it starts the same executable in the active console user session with the internal `--user` option. That user-session process owns the hooks, so the behavior applies to the logged-in desktop.

The supervisor waits for a user token when started before login, retries failed launches once per second, replaces the agent when the console session changes, and relaunches an agent that exits. Service stop also terminates its agent. `RUNNING` means the supervisor is ready, not proof that hooks are attached. This mode follows one active console session; it does not enumerate independent RDP sessions.

`--install` and `--uninstall` first query whether the service already exists. They only request UAC elevation when a service state change is actually needed. The non-elevated parent waits for the elevated child process and then queries the service state again before printing the final result.

Internal options:

- `--service`: entry point used by the Windows Service Control Manager.
- `--user`: entry point used by the service to run hooks in the logged-in user session.

These are implementation details and are not needed for normal manual use.

## Build
### Prerequisites

For the `x86_64-pc-windows-msvc` target, build from an MSVC developer environment:

- Open "Developer PowerShell for VS 2022", or run `VsDevCmd.bat` or `vcvarsall.bat x64` before building.

### Building
```powershell
cargo build --release
```

The release binary is written to `target\release\ime-shift-fix.exe`.

## Verification

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo build --locked --release
cargo doc --locked --no-deps --document-private-items
cargo package --locked
```

CI uses Windows x64 MSVC runners, tests stable Rust and the declared minimum Rust 1.85, and collects a Windows LLVM coverage summary. Actions are pinned to commits and maintained by Dependabot. The `All checks passed` job requires every check to succeed. Workflows only check/build/package; they do not publish or upload artifacts or send coverage to Codecov.

Tests cover physical Shift event sequences, injected-event isolation, system key messages, posted message metadata, and the console-agent replacement policy. They do not prove IME compatibility or actual service recovery across login/logout.

### Manual compensation comparison

Run only one foreground instance at a time, with any installed service stopped. Compare each mode in the same app and IME, starting from the same input mode:

```powershell
ime-shift-fix.exe --compensation both
ime-shift-fix.exe --compensation post
ime-shift-fix.exe --compensation inject
```

`both` is the default and retains window-message plus system-input compensation. `post` and `inject` are diagnostic experiments, not recommended defaults. Post-only does not update Windows keyboard state; if Shift appears held after that experiment, press and release each Shift key physically, then exit the experiment. Console output is limited to startup/shutdown, with protected-release and compensation-error counts on shutdown.

For each mode, record the app/Windows/IME versions, initial and final IME mode, selected text, any duplicate key-up handling, and whether typing after release behaves normally. Check:

- Left Shift, right Shift, and both held together with either release order.
- Plain Shift without clicking, Shift+arrows, repeated clicks, and wheel cancellation.
- Alt+Shift, changing focus before release, and apps running at a different integrity level.

Separately verify service startup before login, logout/login, console-user switching, agent termination/restart, and service stop. These require a real Windows session; unit tests and CI cannot substitute for those observations.

API references: [low-level keyboard hook timing](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelkeyboardproc), [injected-event and context flags](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-kbdllhookstruct), [SendInput restrictions](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput), and [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov).

## License

This project is licensed under either of the following, at your option:

- Apache License, Version 2.0
- MIT License
