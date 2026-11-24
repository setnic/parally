use crate::i18n::text;
use anyhow::{Result, ensure};
use eframe::egui;
use std::{
    ffi::c_void,
    ptr::{null, null_mut},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
};
use windows_sys::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::*,
    },
    w,
};

const WIDTH: f32 = 379.0;
const HEIGHT: f32 = 48.0;
const TRACK_END: f32 = 104.0;
#[cfg(test)]
const BACK_ALPHA: u8 = 188;

struct Callback {
    ctx: egui::Context,
    opened: Arc<AtomicBool>,
    button_left: i32,
    padding: i32,
}

pub struct Panel {
    hwnd: HWND,
    callback: Box<Callback>,
    anchor: Option<(POINT, f32)>,
    previous: Option<(Option<u8>, String, u8, bool, &'static str)>,
}

impl Panel {
    pub fn new(ctx: &egui::Context, opened: Arc<AtomicBool>) -> Result<Self> {
        let instance = unsafe { GetModuleHandleW(null()) };
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: w!("ParallyTrayPanel"),
            hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
            ..Default::default()
        };
        ensure!(
            unsafe { RegisterClassW(&class) } != 0,
            "Could not create the Parally status panel."
        );
        let mut callback = Box::new(Callback {
            ctx: ctx.clone(),
            opened,
            button_left: i32::MAX,
            padding: 8,
        });
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                class.lpszClassName,
                w!("Parally status"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                null_mut(),
                null_mut(),
                instance,
                null(),
            )
        };
        ensure!(!hwnd.is_null(), "Could not open the Parally status panel.");
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&raw mut *callback) as isize) };
        Ok(Self {
            hwnd,
            callback,
            anchor: None,
            previous: None,
        })
    }

    pub fn toggle(&mut self, point: POINT, scale: f32) {
        if self.anchor.is_some() {
            self.hide();
        } else {
            self.anchor = Some((point, scale.clamp(0.8, 4.0)));
            self.previous = None;
        }
    }

    pub fn hide(&mut self) {
        self.anchor = None;
        unsafe { ShowWindow(self.hwnd, SW_HIDE) };
    }

    pub fn refresh(
        &mut self,
        percent: Option<u8>,
        label: String,
        transparency: u8,
        light: bool,
    ) -> Result<()> {
        let Some((point, scale)) = self.anchor else {
            return Ok(());
        };
        let key = (percent, label, transparency, light, text("Open Window"));
        if self.previous.as_ref() == Some(&key) {
            return Ok(());
        }
        let px = |logical: f32| (logical * scale).round() as i32;
        let width = px(WIDTH);
        let height = px(HEIGHT);
        let geometry = Geometry {
            width,
            height,
            button: width - px(119.0),
            scale,
            background_alpha: 255 - (u16::from(transparency) * 255 / 100) as u8,
            light,
        };
        self.callback.button_left = geometry.button;
        self.callback.padding = px(8.0);
        let position = position(point, width, height, px(8.0));
        self.render(&geometry, position, key.0, &key.1)?;
        self.previous = Some(key);
        unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE) };
        Ok(())
    }

    fn render(
        &self,
        geometry: &Geometry,
        destination: POINT,
        percent: Option<u8>,
        label: &str,
    ) -> Result<()> {
        let Geometry {
            width,
            height,
            button,
            scale,
            ..
        } = *geometry;
        let pixel_count = (width * height) as usize;
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = unsafe { CreateCompatibleDC(null_mut()) };
        ensure!(!dc.is_null(), "Could not draw the status panel.");
        let mut bits: *mut c_void = null_mut();
        let bitmap =
            unsafe { CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0) };
        if bitmap.is_null() || bits.is_null() {
            unsafe { DeleteDC(dc) };
            anyhow::bail!("Could not allocate the status panel.");
        }
        let old = unsafe { SelectObject(dc, bitmap) };
        let font = unsafe {
            CreateFontW(
                -(14.0 * scale).round() as i32,
                0,
                0,
                0,
                500,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                0,
                0,
                ANTIALIASED_QUALITY as u32,
                0,
                w!("Segoe UI"),
            )
        };
        let old_font = if font.is_null() {
            null_mut()
        } else {
            unsafe { SelectObject(dc, font) }
        };
        unsafe { std::slice::from_raw_parts_mut(bits.cast::<u32>(), pixel_count) }.fill(0);
        let px = |logical: f32| (logical * scale).round() as i32;
        let right = width - px(8.0);
        let mut button_rect = RECT {
            left: button,
            top: 0,
            right,
            bottom: height,
        };
        let mut label_rect = RECT {
            left: px(TRACK_END + 10.0),
            top: 0,
            right: button - px(10.0),
            bottom: height,
        };
        unsafe {
            SetBkMode(dc, TRANSPARENT as i32);
            SetTextColor(dc, 0xffffff);
            draw_text(dc, label, &mut label_rect, DT_LEFT | DT_END_ELLIPSIS);
            draw_text(dc, text("Open Window"), &mut button_rect, DT_CENTER);
            GdiFlush();
        }
        let pixels = unsafe { std::slice::from_raw_parts_mut(bits.cast::<u32>(), pixel_count) };
        for (index, pixel) in pixels.iter_mut().enumerate() {
            let x = index as i32 % width;
            let y = index as i32 / width;
            let (shade, alpha, text) = geometry.paint(x, y, percent);
            let coverage = (*pixel & 255) as u8;
            *pixel = blend(shade, alpha, text, coverage);
        }
        let size = SIZE {
            cx: width,
            cy: height,
        };
        let source = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let updated = unsafe {
            let updated = UpdateLayeredWindow(
                self.hwnd,
                null_mut(),
                &destination,
                &size,
                dc,
                &source,
                0,
                &blend,
                ULW_ALPHA,
            );
            if !old_font.is_null() {
                SelectObject(dc, old_font);
            }
            if !font.is_null() {
                DeleteObject(font);
            }
            SelectObject(dc, old);
            DeleteObject(bitmap);
            DeleteDC(dc);
            updated
        };
        ensure!(updated != 0, "Windows could not display the status panel.");
        Ok(())
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.hwnd) };
    }
}

struct Geometry {
    width: i32,
    height: i32,
    button: i32,
    scale: f32,
    background_alpha: u8,
    light: bool,
}

impl Geometry {
    fn paint(&self, x: i32, y: i32, percent: Option<u8>) -> (u8, u8, u8) {
        let (x, y) = ((x as f32 + 0.5) / self.scale, (y as f32 + 0.5) / self.scale);
        let w = self.width as f32 / self.scale;
        let h = self.height as f32 / self.scale;
        let button = self.button as f32 / self.scale;
        let coverage = |left: f32, top: f32, right: f32, bottom: f32, radius: f32| {
            let dx = ((x - (left + right) * 0.5).abs() - ((right - left) * 0.5 - radius)).max(0.0);
            let dy = ((y - (top + bottom) * 0.5).abs() - ((bottom - top) * 0.5 - radius)).max(0.0);
            ((radius - dx.hypot(dy)) * self.scale + 0.5).clamp(0.0, 1.0)
        };
        let outer = coverage(0.0, 0.0, w, h, 18.0);
        if outer == 0.0 {
            return (0, 0, 0);
        }
        let inner = coverage(1.0, 1.0, w - 1.0, h - 1.0, 17.0);
        let border = (outer - inner).max(0.0) * 192.0;
        let fill = inner * f32::from(self.background_alpha);
        let mut alpha = border + fill;
        let surface = if self.light {
            236.0
        } else {
            34.0 + 10.0 * (1.0 - y / h)
        };
        let mut premultiplied = border * 157.0 + fill * surface;
        let button_coverage = coverage(button, 8.0, w - 8.0, h - 8.0, 10.0);
        let top = (h - 7.0) * 0.5;
        let right = TRACK_END;
        let track = coverage(14.0, top, right, top + 7.0, 3.5);
        let end = 14.0 + (right - 14.0) * f32::from(percent.unwrap_or(0)) / 100.0;
        let track_shade = if x <= end { 255.0 } else { 104.0 };
        let button_shade = if self.light { 49.0 } else { 230.0 };
        let over = button_coverage + track;
        alpha = alpha * (1.0 - over) + 255.0 * over;
        premultiplied = premultiplied * (1.0 - over)
            + 255.0 * (button_coverage * button_shade + track * track_shade);
        let foreground = if self.light == (button_coverage == 1.0) {
            255
        } else {
            18
        };
        (
            (premultiplied / alpha.max(1.0)).round() as u8,
            alpha.round() as u8,
            foreground,
        )
    }
}

fn blend(shade: u8, alpha: u8, text: u8, coverage: u8) -> u32 {
    let coverage = u32::from(coverage);
    let backing = (u32::from(alpha) * (255 - coverage) + 127) / 255;
    let rgb = (u32::from(shade) * backing + u32::from(text) * coverage + 127) / 255;
    ((backing + coverage) << 24) | (rgb * 0x010101)
}

unsafe fn draw_text(dc: HDC, text: &str, rect: &mut RECT, alignment: u32) {
    let text: Vec<_> = text.encode_utf16().collect();
    unsafe {
        DrawTextW(
            dc,
            text.as_ptr(),
            text.len() as i32,
            rect,
            alignment | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
        )
    };
}

fn position(point: POINT, width: i32, height: i32, padding: i32) -> POINT {
    let handle = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
    let mut monitor = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(handle, &mut monitor) } == 0 {
        return POINT {
            x: point.x - width,
            y: point.y - height,
        };
    }
    let area = monitor.rcWork;
    let max_x = (area.right - width - padding).max(area.left);
    let max_y = (area.bottom - height - padding).max(area.top);
    POINT {
        x: (point.x - width / 2).clamp(area.left, max_x),
        y: (point.y - height - padding).clamp(area.top, max_y),
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_LBUTTONUP {
        let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Callback;
        if let Some(callback) = unsafe { pointer.as_ref() } {
            let x = lparam as i16 as i32;
            let y = (lparam >> 16) as i16 as i32;
            let inset = callback.padding;
            let mut rect = RECT::default();
            if unsafe { GetClientRect(hwnd, &mut rect) } != 0
                && x >= callback.button_left
                && x <= rect.right - inset
                && y >= inset
                && y <= rect.bottom - inset
            {
                callback.opened.store(true, Relaxed);
                callback.ctx.request_repaint();
            }
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn per_pixel_tray_opacity() {
        let shape = Geometry {
            width: WIDTH as i32,
            height: HEIGHT as i32,
            button: WIDTH as i32 - 119,
            scale: 1.0,
            background_alpha: BACK_ALPHA,
            light: false,
        };
        for (x, y, expect) in [
            (0, 0, 0),
            (20, 13, BACK_ALPHA),
            (300, 28, 255),
            (40, 24, 255),
            (80, 24, 255),
        ] {
            let (shade, alpha, color) = shape.paint(x, y, Some(50));
            assert_eq!(blend(shade, alpha, color, 0) >> 24, u32::from(expect));
        }
        assert_eq!(blend(20, BACK_ALPHA, 255, 255), 0xffff_ffff);
        assert_eq!(shape.paint(40, 24, Some(50)).0, 255);
        assert_eq!(shape.paint(80, 24, Some(50)).0, 104);
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            let sample = Geometry {
                scale,
                width: (WIDTH * scale).round() as i32,
                height: (HEIGHT * scale).round() as i32,
                button: ((WIDTH - 119.0) * scale).round() as i32,
                ..shape
            };
            let edge = (18.0 * scale).ceil() as i32;
            assert!((0..edge).any(|y| (0..edge).any(|x| {
                let alpha = sample.paint(x, y, None).1;
                alpha > 0 && alpha < BACK_ALPHA
            })));
        }
        let context = egui::Context::default();
        let panel = Panel::new(&context, Arc::new(AtomicBool::new(false))).unwrap();
        panel
            .render(&shape, POINT { x: 0, y: 0 }, Some(50), "50%  ·  1.2 MiB/s")
            .unwrap();
        assert_eq!(unsafe { IsWindowVisible(panel.hwnd) }, 0);
        let mut bounds = RECT::default();
        assert_ne!(unsafe { GetWindowRect(panel.hwnd, &mut bounds) }, 0);
        assert_eq!(
            (bounds.right - bounds.left, bounds.bottom - bounds.top),
            (shape.width, shape.height)
        );
    }
}
