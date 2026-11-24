use crate::glass::WINDOW_RADIUS;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::{
    Foundation::{HWND, RECT},
    Graphics::{
        Dwm::{
            DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DwmExtendFrameIntoClientArea,
            DwmSetWindowAttribute,
        },
        Gdi::{
            CreateRoundRectRgn, DeleteObject, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST,
            MONITORINFO, MonitorFromWindow, SetWindowRgn,
        },
    },
    UI::{
        Controls::MARGINS,
        WindowsAndMessaging::{
            GetWindowRect, IsIconic, IsWindowVisible, KillTimer, PostMessageW, SW_RESTORE,
            SetTimer, ShowWindow, WM_PAINT,
        },
    },
};

const HIDDEN_POLL: usize = 0x5061;

#[derive(Default)]
pub struct GlassWindow {
    handle: HWND,
    region_size: Option<(i32, i32, i32)>,
}

impl GlassWindow {
    pub fn handle(&self) -> HWND {
        self.handle
    }

    pub fn work_size(&self, scale: f32) -> (f32, f32) {
        let monitor = unsafe { MonitorFromWindow(self.handle, MONITOR_DEFAULTTONEAREST) };
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
            return (735.0, 730.0);
        }
        (
            ((info.rcWork.right - info.rcWork.left) as f32 / scale - 24.0).max(400.0),
            ((info.rcWork.bottom - info.rcWork.top) as f32 / scale - 24.0).max(240.0),
        )
    }

    pub fn show(&self) {
        if !self.handle.is_null() {
            unsafe { ShowWindow(self.handle, SW_RESTORE) };
        }
    }

    pub fn update_main(&mut self, window: &impl HasWindowHandle, scale: f32) {
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };
        let handle = handle.hwnd.get() as HWND;
        if self.handle != handle {
            if !self.handle.is_null() {
                unsafe { KillTimer(self.handle, HIDDEN_POLL) };
            }
            self.handle = handle;
            self.region_size = None;
            enable_acrylic(handle);
            // Windows may suppress ordinary Winit redraws on a hidden viewport.
            unsafe { SetTimer(handle, HIDDEN_POLL, 250, Some(poll_hidden)) };
        }
        let mut rect = RECT::default();
        if unsafe { IsIconic(handle) != 0 || GetWindowRect(handle, &mut rect) == 0 } {
            return;
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        // Keep the hardware clip just outside egui's antialiased outline.
        let diameter = ((f32::from(WINDOW_RADIUS) - 3.0) * 2.0 * scale).round() as i32;
        if width < diameter
            || height < diameter
            || self.region_size == Some((width, height, diameter))
        {
            return;
        }
        let region = unsafe { CreateRoundRectRgn(0, 0, width + 1, height + 1, diameter, diameter) };
        if region.is_null() {
            return;
        }
        if unsafe { SetWindowRgn(handle, region, 1) } != 0 {
            self.region_size = Some((width, height, diameter));
        } else {
            unsafe { DeleteObject(region) };
        }
    }
}

impl Drop for GlassWindow {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { KillTimer(self.handle, HIDDEN_POLL) };
        }
    }
}

unsafe extern "system" fn poll_hidden(hwnd: HWND, _: u32, _: usize, _: u32) {
    if unsafe { IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 } {
        unsafe { PostMessageW(hwnd, WM_PAINT, 0, 0) };
    }
}

fn enable_acrylic(hwnd: HWND) {
    let acrylic = DWMSBT_TRANSIENTWINDOW;
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    unsafe {
        if DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            (&raw const acrylic).cast(),
            size_of::<i32>() as u32,
        ) >= 0
        {
            DwmExtendFrameIntoClientArea(hwnd, &margins);
        }
    }
}
