# IME Shift Fix

[![Crates.io](https://img.shields.io/crates/v/ime_shift_fix.svg)](https://crates.io/crates/ime_shift_fix)
[![Docs.rs](https://docs.rs/ime_shift_fix/badge.svg)](https://docs.rs/ime_shift_fix)
[![CI](https://github.com/ou6326/ime-shift-fix/actions/workflows/ci.yml/badge.svg)](https://github.com/ou6326/ime-shift-fix/actions/workflows/ci.yml)
[![Coverage](https://codecov.io/gh/ou6326/ime-shift-fix/branch/main/graph/badge.svg)](https://codecov.io/gh/ou6326/ime-shift-fix)
[![License](https://img.shields.io/crates/l/ime_shift_fix.svg)](https://github.com/ou6326/ime-shift-fix#license)

Windows utility that protects IME mode while selecting text with `Shift+Click`.

The foreground mode installs low-level keyboard and mouse hooks in the current user session. When a Shift key-up follows a Shift+left-click selection, the tool suppresses that key-up and posts/injects a replacement key-up so the target app does not toggle IME mode unexpectedly.

## Usage

Run in the current terminal:

```powershell
ime_shift_fix.exe
```

Install as a Windows service:

```powershell
ime_shift_fix.exe --install
```

Uninstall the service:

```powershell
ime_shift_fix.exe --uninstall
```

Show help:

```powershell
ime_shift_fix.exe --help
```

## Service Mode

The service does not install hooks inside Session 0. Instead, it starts the same executable in the active console user session with the internal `--user` option. That user-session process owns the hooks, so the behavior applies to the logged-in desktop.

`--install` and `--uninstall` first query whether the service already exists. They only request UAC elevation when a service state change is actually needed. The non-elevated parent waits for the elevated child process and then queries the service state again before printing the final result.

Internal options:

- `--service`: entry point used by the Windows Service Control Manager.
- `--user`: entry point used by the service to run hooks in the logged-in user session.

These are implementation details and are not needed for normal manual use.

## Build

```powershell
cargo build --release
```

The release binary is written to `target\release\ime_shift_fix.exe`.
