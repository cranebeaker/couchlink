//! Displays on macOS, read with CoreGraphics on the SDL thread. SDL video cannot be used here: on macOS it must be
//! initialised on the main thread, and the daemon's main thread runs the async side.
//!
//! The protocol wants physical pixels. CoreGraphics works in points (global coordinates, 0,0 at the top left of
//! the main display). A display's size in pixels is its size in points times its own backing scale; its origin is
//! its point origin times the largest scale of any display, so displays with different scales never overlap in
//! pixel space (a 1x display next to a Retina one leaves a gap, which no region uses). [`to_points`] turns a region
//! back into points for window placement through the display it lies on. On one display this is plain geometry.

use crate::displays::RawDisplay;
use couchlink_core::session::Rect;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

type CGDirectDisplayID = u32;
type CGDisplayModeRef = *mut std::ffi::c_void;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGGetActiveDisplayList(max: u32, displays: *mut CGDirectDisplayID, count: *mut u32) -> i32;
    fn CGDisplayBounds(display: CGDirectDisplayID) -> CGRect;
    fn CGMainDisplayID() -> CGDirectDisplayID;
    fn CGDisplayIsBuiltin(display: CGDirectDisplayID) -> u32;
    fn CGDisplayVendorNumber(display: CGDirectDisplayID) -> u32;
    fn CGDisplayModelNumber(display: CGDirectDisplayID) -> u32;
    fn CGDisplaySerialNumber(display: CGDirectDisplayID) -> u32;
    fn CGDisplayCopyDisplayMode(display: CGDirectDisplayID) -> CGDisplayModeRef;
    fn CGDisplayModeGetPixelWidth(mode: CGDisplayModeRef) -> usize;
    fn CGDisplayModeGetRefreshRate(mode: CGDisplayModeRef) -> f64;
    fn CGDisplayModeRelease(mode: CGDisplayModeRef);
}

/// One display in points with its backing scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointDisplay {
    pub id: CGDirectDisplayID,
    pub bounds: CGRect,
    pub scale: f64,
}

/// Every active display in points.
pub fn point_displays() -> Vec<PointDisplay> {
    let mut ids = [0u32; 16];
    let mut n = 0u32;
    // SAFETY: the buffer holds 16 ids and CoreGraphics writes at most that many.
    if unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) } != 0 {
        return Vec::new();
    }
    ids[..n as usize]
        .iter()
        .map(|&id| {
            // SAFETY: plain queries on an active display id; the mode is released below.
            unsafe {
                let bounds = CGDisplayBounds(id);
                let mode = CGDisplayCopyDisplayMode(id);
                let mut scale = 1.0;
                if !mode.is_null() {
                    let px = CGDisplayModeGetPixelWidth(mode) as f64;
                    if bounds.size.width > 0.0 && px > 0.0 {
                        scale = px / bounds.size.width;
                    }
                    CGDisplayModeRelease(mode);
                }
                PointDisplay { id, bounds, scale }
            }
        })
        .collect()
}

/// The monitors for `displays.rs`: stable device keys from vendor, model and serial number.
pub fn read() -> Vec<RawDisplay> {
    // SAFETY: plain query.
    let main = unsafe { CGMainDisplayID() };
    let mut external = 0;
    let all = point_displays();
    let max_scale = largest_scale(&all);
    all.into_iter()
        .map(|d| {
            // SAFETY: plain queries on an active display id; the mode is released below.
            let (builtin, vendor, model, serial, hz) = unsafe {
                let mode = CGDisplayCopyDisplayMode(d.id);
                let hz = if mode.is_null() { 0.0 } else { CGDisplayModeGetRefreshRate(mode) };
                if !mode.is_null() {
                    CGDisplayModeRelease(mode);
                }
                (CGDisplayIsBuiltin(d.id) != 0, CGDisplayVendorNumber(d.id), CGDisplayModelNumber(d.id), CGDisplaySerialNumber(d.id), hz)
            };
            let name = if builtin {
                "Built-in Display".to_string()
            } else {
                external += 1;
                format!("Display {external}")
            };
            let px = to_pixels(&d, max_scale);
            RawDisplay {
                device: if serial != 0 { format!("{vendor:x}-{model:x}-{serial:x}") } else { format!("{vendor:x}-{model:x}-id{}", d.id) },
                name,
                x: px.x,
                y: px.y,
                w: px.w,
                h: px.h,
                scale: d.scale as f32,
                hz: hz as f32,
                primary: d.id == main,
            }
        })
        .collect()
}

fn largest_scale(displays: &[PointDisplay]) -> f64 {
    displays.iter().map(|d| d.scale).fold(1.0, f64::max)
}

/// A display's rectangle in protocol pixels; `max_scale` is the largest scale of all displays.
pub fn to_pixels(d: &PointDisplay, max_scale: f64) -> Rect {
    Rect {
        x: (d.bounds.origin.x * max_scale).round() as i32,
        y: (d.bounds.origin.y * max_scale).round() as i32,
        w: (d.bounds.size.width * d.scale).round() as i32,
        h: (d.bounds.size.height * d.scale).round() as i32,
    }
}

/// A region in protocol pixels as a point rectangle on the display under the region's centre (the first display
/// when none is).
pub fn to_points(rect: Rect, displays: &[PointDisplay]) -> CGRect {
    let max = largest_scale(displays);
    let (cx, cy) = (rect.x + rect.w / 2, rect.y + rect.h / 2);
    let hit = displays.iter().find(|d| {
        let p = to_pixels(d, max);
        cx >= p.x && cx < p.x + p.w && cy >= p.y && cy < p.y + p.h
    });
    let Some(d) = hit.or(displays.first()) else {
        return CGRect { origin: CGPoint { x: rect.x as f64, y: rect.y as f64 }, size: CGSize { width: rect.w as f64, height: rect.h as f64 } };
    };
    let p = to_pixels(d, max);
    let s = d.scale;
    CGRect {
        origin: CGPoint { x: d.bounds.origin.x + (rect.x - p.x) as f64 / s, y: d.bounds.origin.y + (rect.y - p.y) as f64 / s },
        size: CGSize { width: rect.w as f64 / s, height: rect.h as f64 / s },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disp(x: f64, w: f64, h: f64, scale: f64) -> PointDisplay {
        PointDisplay { id: 1, bounds: CGRect { origin: CGPoint { x, y: 0.0 }, size: CGSize { width: w, height: h } }, scale }
    }

    #[test]
    fn regions_go_back_to_points_on_their_display() {
        let ds = [disp(0.0, 1512.0, 982.0, 2.0), disp(1512.0, 1920.0, 1080.0, 1.0)];
        assert_eq!(to_pixels(&ds[0], 2.0), Rect { x: 0, y: 0, w: 3024, h: 1964 });
        // The 1x display starts where the Retina one ends and keeps its real pixel size: no overlap.
        assert_eq!(to_pixels(&ds[1], 2.0), Rect { x: 3024, y: 0, w: 1920, h: 1080 });
        // Left half of the Retina display.
        let r = to_points(Rect { x: 0, y: 0, w: 1512, h: 1964 }, &ds);
        assert_eq!((r.origin.x, r.size.width, r.size.height), (0.0, 756.0, 982.0));
        // Right half of the 1x display.
        let r = to_points(Rect { x: 3024 + 960, y: 0, w: 960, h: 1080 }, &ds);
        assert_eq!((r.origin.x, r.origin.y, r.size.width, r.size.height), (1512.0 + 960.0, 0.0, 960.0, 1080.0));
        // One display: plain division by its scale.
        let one = [disp(0.0, 1512.0, 982.0, 2.0)];
        assert_eq!(to_points(Rect { x: 1512, y: 982, w: 1512, h: 982 }, &one).origin, CGPoint { x: 756.0, y: 491.0 });
    }

    #[test]
    fn this_mac_reports_its_displays() {
        // A Mac with no display attached (a headless CI runner) reports none; that is fine.
        for d in read() {
            assert!(d.w > 0 && d.h > 0 && d.scale >= 1.0, "{d:?}");
        }
    }
}
