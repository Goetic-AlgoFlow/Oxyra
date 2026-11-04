//! Raw mouse motion read in batches, for a native mod's Win32 message filter
//! (`native_raw_input`, hooks 368). Windows posts a WM_INPUT for every report
//! of a 1000 to 8000 Hz mouse; read one by one through winit, the frame waits
//! behind them. [`read_batch`] takes the first WM_INPUT of a burst and every
//! report still queued behind it in one `GetRawInputBuffer` call.

use std::ffi::c_void;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Input::{
    GetRawInputBuffer, GetRawInputData, HRAWINPUT, MOUSE_MOVE_ABSOLUTE, RAWINPUT, RAWINPUTHEADER,
    RID_INPUT, RIM_TYPEMOUSE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DefWindowProcW, MSG, WM_INPUT};

/// `msg` is the `MSG` winit's message hook hands over. When it is a WM_INPUT of
/// plain relative mouse motion: the motion of it and of every raw report queued
/// behind it (x right, y down, in counts), and the caller keeps it from winit.
/// None for anything else, which winit handles as before. Buttons and the wheel
/// in that first report leave it to winit too; reports drained behind it give
/// only their motion (the game reads buttons, the wheel and keys from the
/// window's own messages, not from raw input). Absolute motion (a tablet, a
/// remote desktop) is no raw motion, as for winit.
pub fn read_batch(msg: *const c_void) -> Option<(i64, i64)> {
    if msg.is_null() {
        return None;
    }
    // SAFETY: winit's hook passes a valid MSG for the duration of the call.
    let msg = unsafe { &*(msg as *const MSG) };
    if msg.message != WM_INPUT {
        return None;
    }
    // SAFETY: zeroed RAWINPUT is a valid value; the size passed is its own.
    let mut raw: RAWINPUT = unsafe { std::mem::zeroed() };
    let mut size = size_of::<RAWINPUT>() as u32;
    let header = size_of::<RAWINPUTHEADER>() as u32;
    let got = unsafe {
        GetRawInputData(msg.lParam as HRAWINPUT, RID_INPUT, (&raw mut raw).cast(), &mut size, header)
    };
    if got == u32::MAX || got == 0 || raw.header.dwType != RIM_TYPEMOUSE {
        return None;
    }
    // SAFETY: the record is a mouse one (dwType).
    let mouse = unsafe { raw.data.mouse };
    let buttons = unsafe { mouse.Anonymous.Anonymous.usButtonFlags };
    if buttons != 0 || mouse.usFlags & MOUSE_MOVE_ABSOLUTE != 0 {
        return None;
    }
    let mut sum = (i64::from(mouse.lLastX), i64::from(mouse.lLastY));
    // Windows frees the report's handle here, as winit's window procedure lets it.
    unsafe { DefWindowProcW(msg.hwnd as HWND, WM_INPUT, msg.wParam, msg.lParam) };
    drain(&mut sum);
    Some(sum)
}

/// Adds the motion of every raw report still queued, in as few calls as it takes.
fn drain(sum: &mut (i64, i64)) {
    // 8-byte aligned, as the records in it are on 64-bit Windows.
    let mut buf = [0u64; 1024];
    let header = size_of::<RAWINPUTHEADER>() as u32;
    loop {
        let mut size = size_of_val(&buf) as u32;
        let n = unsafe { GetRawInputBuffer(buf.as_mut_ptr().cast(), &mut size, header) };
        if n == 0 || n == u32::MAX {
            return;
        }
        let base = buf.as_ptr() as usize;
        let end = base + size_of_val(&buf);
        let mut at = base;
        for _ in 0..n {
            if at + size_of::<RAWINPUTHEADER>() > end {
                return;
            }
            // SAFETY: `at` lies in `buf`, at a record GetRawInputBuffer wrote.
            let h = unsafe { *(at as *const RAWINPUTHEADER) };
            if h.dwType == RIM_TYPEMOUSE && at + size_of::<RAWINPUT>() <= end {
                let m = unsafe { (*(at as *const RAWINPUT)).data.mouse };
                if m.usFlags & MOUSE_MOVE_ABSOLUTE == 0 {
                    sum.0 += i64::from(m.lLastX);
                    sum.1 += i64::from(m.lLastY);
                }
            }
            // NEXTRAWINPUTBLOCK: the next record starts 8-byte aligned.
            at = (at + h.dwSize as usize + 7) & !7;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_message_that_is_not_raw_input_is_left_to_winit() {
        let msg: windows_sys::Win32::UI::WindowsAndMessaging::MSG = unsafe { std::mem::zeroed() };
        assert_eq!(super::read_batch((&raw const msg).cast()), None);
        assert_eq!(super::read_batch(std::ptr::null()), None);
    }
}
