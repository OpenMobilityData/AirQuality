//! Interactive 3D surface of modelled ultrafine-particle (UFP) concentrations
//! (Lloyd et al. 2023 combined model, Montréal 2020–2022).
//!
//! Rendered natively: the grid is software-rasterized in Rust into an RGBA
//! pixel buffer (flat-shaded quads, painter's order) and blitted with a single
//! `putImageData` per frame — the same buffer-not-per-shape-calls strategy as
//! the map heatmap, which keeps a full redraw fast enough to follow a drag.
//! Orthographic turntable camera: drag rotates, wheel/pinch zooms.

use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::{Clamped, JsCast};

use crate::data::loader::UfpSurface;
use crate::i18n::Lang;

// ── Scene proportions (matching the original Plotly figure) ────────────────

/// World-space half-extents: x spans ±AX/2, y scales by the real km aspect,
/// and the full z (value) range maps to AZ — the original's vertical
/// exaggeration, chosen so peaks read clearly without towering.
const AX: f32 = 1.6;
const AZ: f32 = 0.55;
/// Intro fly-in: the first frame is straight down with the grid box's
/// northwest corner at the top of the screen (NW–SE diagonal vertical) — it
/// reads as an ordinary 2D map, anchoring the geography — and holds there for
/// a beat before easing to the default camera.
const AZIM_TOP: f64 = -std::f64::consts::FRAC_PI_4;
const ELEV_TOP: f64 = std::f64::consts::FRAC_PI_2;
/// Default turntable camera, where the fly-in lands: tipped in from the
/// southeast down to ~60° — high enough that the tall downtown/highway ridges
/// don't occlude the corridors behind them, low enough to read as relief —
/// with a slight azimuth swing (decreasing azimuth spins the map
/// counterclockwise on screen) to break the symmetry of the straight tilt.
const AZIM0: f64 = AZIM_TOP - 15.0 * std::f64::consts::PI / 180.0;
const ELEV0: f64 = 1.05;
const INTRO_HOLD_MS: f64 = 1000.0;
const INTRO_MOVE_MS: f64 = 2400.0;
/// Orthographic fill: world radius ~1.1 maps to this fraction of the viewport.
const FIT: f32 = 0.42;
/// Ground-plane reference grid spacing (km).
const GRID_KM: f64 = 5.0;

/// Viridis, as used by the original figure's colour scale.
const VIRIDIS: &[(u8, u8, u8)] = &[
    (0x44, 0x01, 0x54),
    (0x48, 0x28, 0x78),
    (0x3e, 0x49, 0x89),
    (0x31, 0x68, 0x8e),
    (0x26, 0x82, 0x8e),
    (0x1f, 0x9e, 0x89),
    (0x35, 0xb7, 0x79),
    (0x6e, 0xce, 0x58),
    (0xb5, 0xde, 0x2b),
    (0xfd, 0xe7, 0x25),
];

/// CSS gradient matching `VIRIDIS`, for the colour-bar legend.
pub const VIRIDIS_CSS: &str = "linear-gradient(90deg,#440154,#482878,#3e4989,#31688e,#26828e,#1f9e89,#35b779,#6ece58,#b5de2b,#fde725)";

fn viridis_rgb(t: f32) -> (f32, f32, f32) {
    let t = t.clamp(0.0, 1.0) * (VIRIDIS.len() - 1) as f32;
    let k = (t as usize).min(VIRIDIS.len() - 2);
    let f = t - k as f32;
    let (a, b) = (VIRIDIS[k], VIRIDIS[k + 1]);
    let lerp = |x: u8, y: u8| x as f32 + (y as f32 - x as f32) * f;
    (lerp(a.0, b.0), lerp(a.1, b.1), lerp(a.2, b.2))
}

// ── Precomputed scene geometry (built once when the data arrives) ───────────

/// One renderable grid cell (all four corners inside the modelled area).
/// Colour and shading use a world-fixed light, so the final RGB is constant
/// across frames; only projection and depth ordering change with the camera.
struct Cell {
    /// Top-left vertex index (`j * nx + i`); the other corners are `v+1`,
    /// `v+nx`, `v+nx+1`.
    v: u32,
    /// Cell-centre world coordinates, for per-frame depth sorting.
    cx: f32,
    cy: f32,
    cz: f32,
    rgb: (u8, u8, u8),
}

struct Scene {
    nx: usize,
    /// Per-vertex world coordinates (z = 0 placeholder outside the data).
    wx: Vec<f32>,
    wy: Vec<f32>,
    wz: Vec<f32>,
    cells: Vec<Cell>,
    /// World y half-extent (x is AX/2) and ground-plane height.
    ay_half: f32,
    floor_z: f32,
    /// km extent and km → world conversion, for the reference grid (drawn at
    /// absolute multiples of `GRID_KM` in the source's km frame).
    x0_km: f64,
    x1_km: f64,
    y0_km: f64,
    y1_km: f64,
    km_to_world: f64,
}

fn build_scene(s: &UfpSurface) -> Scene {
    let (nx, ny) = (s.nx, s.ny);
    // Axis steps are signed: the exported grid stores rows north→south, so
    // `dy` is negative. World coordinates therefore come from the actual km
    // values (centred on the extent midpoint), which keeps world +y = north
    // regardless of row order — the N marker and grid depend on that.
    let x_span_km = (nx - 1) as f64 * s.dx;
    let y_span_km = (ny - 1) as f64 * s.dy;
    let (x_mid, y_mid) = (s.x0 + x_span_km / 2.0, s.y0 + y_span_km / 2.0);
    // Equal horizontal scale in both axes; z exaggeration as in the original.
    let km_to_world = AX as f64 / x_span_km.abs();
    let ay_half = (y_span_km.abs() * km_to_world / 2.0) as f32;
    let zspan = (s.zmax - s.zmin).max(1e-9) as f32;

    let wx: Vec<f32> =
        (0..nx).map(|i| ((s.x0 + i as f64 * s.dx - x_mid) * km_to_world) as f32).collect();
    let wy: Vec<f32> =
        (0..ny).map(|j| ((s.y0 + j as f64 * s.dy - y_mid) * km_to_world) as f32).collect();
    let wz: Vec<f32> = s
        .z
        .iter()
        .map(|v| match v {
            Some(v) => ((v - s.zmin as f32) / zspan - 0.5) * AZ,
            None => 0.0,
        })
        .collect();

    // World-fixed light from the upper south-west — static hill shading.
    let (lx, ly, lz) = (-0.32_f32, -0.40, 0.86);
    let cspan = (s.cmax - s.cmin).max(1e-9) as f32;
    let mut cells = Vec::new();
    for j in 0..ny - 1 {
        for i in 0..nx - 1 {
            let v = j * nx + i;
            let (z00, z10, z01, z11) =
                (s.z[v], s.z[v + 1], s.z[v + nx], s.z[v + nx + 1]);
            let (Some(z00), Some(z10), Some(z01), Some(z11)) = (z00, z10, z01, z11)
            else {
                continue;
            };
            let mean = (z00 + z10 + z01 + z11) / 4.0;
            let (r, g, b) = viridis_rgb((mean - s.cmin as f32) / cspan);

            // Normal from the two diagonals (world space, z exaggerated).
            let d1 = (
                wx[i + 1] - wx[i],
                wy[j + 1] - wy[j],
                wz[v + nx + 1] - wz[v],
            );
            let d2 = (
                wx[i] - wx[i + 1],
                wy[j + 1] - wy[j],
                wz[v + nx] - wz[v + 1],
            );
            let n = (
                d1.1 * d2.2 - d1.2 * d2.1,
                d1.2 * d2.0 - d1.0 * d2.2,
                d1.0 * d2.1 - d1.1 * d2.0,
            );
            let len = (n.0 * n.0 + n.1 * n.1 + n.2 * n.2).sqrt().max(1e-12);
            let flip = if n.2 < 0.0 { -1.0 } else { 1.0 };
            let dot = (n.0 * lx + n.1 * ly + n.2 * lz) * flip / len;
            let lum = 0.62 + 0.38 * dot.max(0.0);

            let shade = |c: f32| (c * lum).round().clamp(0.0, 255.0) as u8;
            cells.push(Cell {
                v: v as u32,
                cx: (wx[i] + wx[i + 1]) / 2.0,
                cy: (wy[j] + wy[j + 1]) / 2.0,
                cz: (wz[v] + wz[v + 1] + wz[v + nx] + wz[v + nx + 1]) / 4.0,
                rgb: (shade(r), shade(g), shade(b)),
            });
        }
    }

    Scene {
        nx,
        wx,
        wy,
        wz,
        cells,
        ay_half,
        floor_z: -AZ / 2.0,
        x0_km: s.x0,
        x1_km: s.x0 + x_span_km,
        y0_km: s.y0,
        y1_km: s.y0 + y_span_km,
        km_to_world,
    }
}

// ── Rasterization ───────────────────────────────────────────────────────────

/// Flat-fill one screen-space triangle into the RGBA buffer (no blending; the
/// painter's order supplies occlusion). Pixel centres inside all three edges;
/// the edge functions are evaluated once per row and stepped incrementally
/// across it (the rasterizer runs every animation/drag frame, so the inner
/// loop is three adds and a compare per pixel).
#[allow(clippy::too_many_arguments)]
fn fill_tri(
    buf: &mut [u8],
    pw: i32,
    ph: i32,
    (ax, ay): (f32, f32),
    (bx, by): (f32, f32),
    (cx, cy): (f32, f32),
    rgb: (u8, u8, u8),
) {
    let minx = (ax.min(bx).min(cx)).floor().max(0.0) as i32;
    let maxx = (ax.max(bx).max(cx)).ceil().min((pw - 1) as f32) as i32;
    let miny = (ay.min(by).min(cy)).floor().max(0.0) as i32;
    let maxy = (ay.max(by).max(cy)).ceil().min((ph - 1) as f32) as i32;
    if minx > maxx || miny > maxy {
        return;
    }
    let area = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
    if area.abs() < 1e-9 {
        return;
    }
    let sign = if area > 0.0 { 1.0 } else { -1.0 };
    // d(w)/dx for each edge function (constant across the triangle).
    let dw0 = -(by - ay) * sign;
    let dw1 = -(cy - by) * sign;
    let dw2 = -(ay - cy) * sign;
    let x0 = minx as f32 + 0.5;
    for py in miny..=maxy {
        let y = py as f32 + 0.5;
        let mut w0 = ((bx - ax) * (y - ay) - (by - ay) * (x0 - ax)) * sign;
        let mut w1 = ((cx - bx) * (y - by) - (cy - by) * (x0 - bx)) * sign;
        let mut w2 = ((ax - cx) * (y - cy) - (ay - cy) * (x0 - cx)) * sign;
        let row = (py * pw) as usize * 4;
        for px in minx..=maxx {
            if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                let idx = row + px as usize * 4;
                buf[idx] = rgb.0;
                buf[idx + 1] = rgb.1;
                buf[idx + 2] = rgb.2;
                buf[idx + 3] = 255;
            }
            w0 += dw0;
            w1 += dw1;
            w2 += dw2;
        }
    }
}

/// Orthographic turntable basis for (azimuth, elevation): screen-space `right`
/// and `up` unit vectors plus the toward-camera direction used for depth.
struct Basis {
    right: (f32, f32, f32),
    up: (f32, f32, f32),
    dir: (f32, f32, f32),
}

fn basis(azim: f64, elev: f64) -> Basis {
    let (st, ct) = (azim.sin() as f32, azim.cos() as f32);
    let (sp, cp) = (elev.sin() as f32, elev.cos() as f32);
    Basis {
        right: (-st, ct, 0.0),
        up: (-sp * ct, -sp * st, cp),
        dir: (cp * ct, cp * st, sp),
    }
}

/// Per-frame scratch reused across renders: the RGBA pixel buffer, projected
/// vertex coordinates, the depth-sort keys, and the offscreen compositing
/// canvas. Re-allocating these every frame (they run to tens of MB on retina)
/// is what stutters an animated camera.
#[derive(Default)]
struct Scratch {
    buf: Vec<u8>,
    sx: Vec<f32>,
    sy: Vec<f32>,
    order: Vec<(f32, u32)>,
    off: Option<web_sys::HtmlCanvasElement>,
}

/// Render the whole scene onto `canvas` (device-pixel sized `pw × ph`).
fn render(
    canvas: &web_sys::HtmlCanvasElement,
    scene: &Scene,
    azim: f64,
    elev: f64,
    zoom: f64,
    scratch: &mut Scratch,
) {
    let (pw, ph) = (canvas.width() as i32, canvas.height() as i32);
    if pw < 2 || ph < 2 {
        return;
    }
    let Some(ctx) = canvas
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|c| c.dyn_into::<web_sys::CanvasRenderingContext2d>().ok())
    else {
        return;
    };

    let b = basis(azim, elev);
    let scale = (pw.min(ph) as f32) * FIT * zoom as f32;
    let (cx0, cy0) = (pw as f32 / 2.0, ph as f32 / 2.0);
    let project = |x: f32, y: f32, z: f32| -> (f32, f32) {
        (
            cx0 + scale * (x * b.right.0 + y * b.right.1 + z * b.right.2),
            cy0 - scale * (x * b.up.0 + y * b.up.1 + z * b.up.2),
        )
    };

    // Background + ground-plane reference grid (under the surface).
    ctx.set_fill_style_str("#0d1b2a");
    ctx.fill_rect(0.0, 0.0, pw as f64, ph as f64);
    let fz = scene.floor_z;
    let (axh, ayh) = (AX / 2.0, scene.ay_half);
    let line = |ctx: &web_sys::CanvasRenderingContext2d, p: (f32, f32), q: (f32, f32)| {
        ctx.begin_path();
        ctx.move_to(p.0 as f64, p.1 as f64);
        ctx.line_to(q.0 as f64, q.1 as f64);
        ctx.stroke();
    };
    ctx.set_stroke_style_str("#22304a");
    ctx.set_line_width(1.0);
    // Grid lines at whole multiples of GRID_KM (in the source's km frame)
    // within the modelled extent. Axis endpoints are ordered (the y axis runs
    // north→south in the data), so iterate over the min..max km range.
    let x_mid = (scene.x0_km + scene.x1_km) / 2.0;
    let y_mid = (scene.y0_km + scene.y1_km) / 2.0;
    let (x_lo, x_hi) = (scene.x0_km.min(scene.x1_km), scene.x0_km.max(scene.x1_km));
    let (y_lo, y_hi) = (scene.y0_km.min(scene.y1_km), scene.y0_km.max(scene.y1_km));
    let mut k = (x_lo / GRID_KM).ceil();
    while k * GRID_KM <= x_hi {
        let wxl = ((k * GRID_KM - x_mid) * scene.km_to_world) as f32;
        line(&ctx, project(wxl, -ayh, fz), project(wxl, ayh, fz));
        k += 1.0;
    }
    let mut k = (y_lo / GRID_KM).ceil();
    while k * GRID_KM <= y_hi {
        let wyl = ((k * GRID_KM - y_mid) * scene.km_to_world) as f32;
        line(&ctx, project(-axh, wyl, fz), project(axh, wyl, fz));
        k += 1.0;
    }
    ctx.set_stroke_style_str("#2a3a5c");
    let corners =
        [(-axh, -ayh), (axh, -ayh), (axh, ayh), (-axh, ayh), (-axh, -ayh)];
    for w in corners.windows(2) {
        line(&ctx, project(w[0].0, w[0].1, fz), project(w[1].0, w[1].1, fz));
    }

    // Project every grid vertex once per frame.
    let n = scene.wz.len();
    let nxv = scene.nx;
    let nyv = n / nxv;
    scratch.sx.clear();
    scratch.sx.resize(n, 0.0);
    scratch.sy.clear();
    scratch.sy.resize(n, 0.0);
    let (sx, sy) = (&mut scratch.sx, &mut scratch.sy);
    for j in 0..nyv {
        for i in 0..nxv {
            let v = j * nxv + i;
            let (px, py) = project(scene.wx[i], scene.wy[j], scene.wz[v]);
            sx[v] = px;
            sy[v] = py;
        }
    }

    // Painter's order: draw far cells first (depth = distance toward camera).
    let order = &mut scratch.order;
    order.clear();
    order.extend(scene.cells.iter().enumerate().map(|(k, c)| {
        (c.cx * b.dir.0 + c.cy * b.dir.1 + c.cz * b.dir.2, k as u32)
    }));
    order.sort_unstable_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let buf = &mut scratch.buf;
    buf.clear();
    buf.resize((pw * ph * 4) as usize, 0);
    for &(_, k) in order.iter() {
        let c = &scene.cells[k as usize];
        let v = c.v as usize;
        let p00 = (sx[v], sy[v]);
        let p10 = (sx[v + 1], sy[v + 1]);
        let p01 = (sx[v + nxv], sy[v + nxv]);
        let p11 = (sx[v + nxv + 1], sy[v + nxv + 1]);
        fill_tri(buf, pw, ph, p00, p10, p11, c.rgb);
        fill_tri(buf, pw, ph, p00, p11, p01, c.rgb);
    }

    // Blit through a transparent offscreen canvas (kept across frames) so the
    // surface composites over the grid (putImageData would replace the whole
    // rectangle).
    if scratch.off.is_none() {
        scratch.off = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.create_element("canvas").ok())
            .and_then(|c| c.dyn_into::<web_sys::HtmlCanvasElement>().ok());
    }
    let Some(off) = scratch.off.as_ref() else { return };
    if off.width() != pw as u32 || off.height() != ph as u32 {
        off.set_width(pw as u32);
        off.set_height(ph as u32);
    }
    let Some(octx) = off
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|c| c.dyn_into::<web_sys::CanvasRenderingContext2d>().ok())
    else {
        return;
    };
    if let Ok(img) = web_sys::ImageData::new_with_u8_clamped_array_and_sh(
        Clamped(&buf[..]),
        pw as u32,
        ph as u32,
    ) {
        let _ = octx.put_image_data(&img, 0.0, 0.0);
        let _ = ctx.draw_image_with_html_canvas_element(off, 0.0, 0.0);
    }

    // North marker just beyond the +y edge of the ground plane.
    let (nx_px, ny_px) = project(0.0, ayh * 1.07, fz);
    let dpr_font = (pw.min(ph) as f64 / 55.0).clamp(11.0, 26.0);
    ctx.set_fill_style_str("#8892a4");
    ctx.set_font(&format!("600 {dpr_font:.0}px Inter, system-ui, sans-serif"));
    ctx.set_text_align("center");
    ctx.set_text_baseline("middle");
    let _ = ctx.fill_text("N", nx_px as f64, ny_px as f64);
}

/// Composite the rendered canvas plus a caption strip (title + source) into a
/// PNG `Blob` for the copy/download widgets.
async fn build_png_blob(
    canvas: web_sys::HtmlCanvasElement,
    title: String,
) -> Result<web_sys::Blob, JsValue> {
    let document = web_sys::window()
        .ok_or_else(|| JsValue::from_str("no window"))?
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let (pw, ph) = (canvas.width(), canvas.height());
    if pw == 0 || ph == 0 {
        return Err(JsValue::from_str("empty canvas"));
    }
    // Caption sized relative to the bitmap so device-pixel ratio cancels out.
    let cap_h = (ph as f64 * 0.055).clamp(26.0, 64.0);
    let font = cap_h * 0.42;
    let out = document.create_element("canvas")?.dyn_into::<web_sys::HtmlCanvasElement>()?;
    out.set_width(pw);
    out.set_height(ph + cap_h as u32);
    let ctx = out
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("no 2d context"))?
        .dyn_into::<web_sys::CanvasRenderingContext2d>()?;
    ctx.draw_image_with_html_canvas_element(&canvas, 0.0, 0.0)?;
    ctx.set_fill_style_str("#0d1b2a");
    ctx.fill_rect(0.0, ph as f64, pw as f64, cap_h);
    ctx.set_fill_style_str("#eaeaea");
    ctx.set_font(&format!("{font:.0}px Inter, system-ui, sans-serif"));
    ctx.set_text_baseline("middle");
    let _ = ctx.fill_text(&title, cap_h * 0.4, ph as f64 + cap_h / 2.0);
    ctx.set_fill_style_str("#8892a4");
    ctx.set_text_align("right");
    let _ = ctx.fill_text(
        "Lloyd et al. 2023 · Environment International",
        pw as f64 - cap_h * 0.4,
        ph as f64 + cap_h / 2.0,
    );
    crate::components::export::canvas_to_png_blob(&out).await
}

// ── Component ───────────────────────────────────────────────────────────────

/// Active drag/pinch pointers: `(pointer id, last x, last y)`.
type Pointers = Vec<(i32, f64, f64)>;

#[component]
pub fn UfpView(surface: ReadSignal<Option<UfpSurface>>) -> impl IntoView {
    let lang = use_context::<ReadSignal<Lang>>().expect("Lang context not provided");

    // Camera opens on the top-down view; the intro animation below brings it
    // to the default oblique (AZIM0/ELEV0) once the first frame can draw.
    let (azim, set_azim) = signal(AZIM_TOP);
    let (elev, set_elev) = signal(ELEV_TOP);
    let (zoom, set_zoom) = signal(1.0_f64);

    // Intro-animation state. `intro_active` is the cancel flag (any user
    // interaction clears it); the rAF closure and request id live here so the
    // loop can reschedule itself and unmount can cancel a pending frame.
    let intro_started = StoredValue::new_local(false);
    let intro_active = StoredValue::new_local(false);
    let raf_closure = StoredValue::new_local(None::<Closure<dyn FnMut(f64)>>);
    let raf_id = StoredValue::new_local(0i32);
    let request_next = move || {
        let Some(win) = web_sys::window() else { return };
        raf_closure.with_value(|c| {
            if let Some(c) = c {
                if let Ok(id) = win.request_animation_frame(c.as_ref().unchecked_ref()) {
                    raf_id.set_value(id);
                }
            }
        });
    };
    on_cleanup(move || {
        if let Some(id) = raf_id.try_get_value() {
            if id != 0 {
                if let Some(win) = web_sys::window() {
                    let _ = win.cancel_animation_frame(id);
                }
            }
        }
    });

    let container_ref = NodeRef::<leptos::html::Div>::new();
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();

    // Container size, observed via ResizeObserver (see RegionMap for why a
    // window-resize listener alone is not enough).
    let (size, set_size) = signal((0.0_f64, 0.0_f64));
    let (resize_tick, set_resize_tick) = signal(0u32);
    let _ = leptos::prelude::window_event_listener(leptos::ev::resize, move |_| {
        set_resize_tick.update(|n| *n = n.wrapping_add(1));
    });
    Effect::new(move |_| {
        resize_tick.track();
        if let Some(el) = container_ref.get() {
            let rect = el.get_bounding_client_rect();
            set_size.set((rect.width(), rect.height()));
        }
    });
    Effect::new(move |attached: Option<bool>| {
        if attached == Some(true) {
            return true;
        }
        let Some(el) = container_ref.get() else { return false };
        let target: web_sys::Element = el.unchecked_into();
        let cb = Closure::<dyn FnMut()>::new(move || {
            set_resize_tick.update(|n| *n = n.wrapping_add(1));
        });
        match web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()) {
            Ok(observer) => {
                observer.observe(&target);
                let held = StoredValue::new_local(Some((observer, cb)));
                on_cleanup(move || {
                    held.try_update_value(|slot| {
                        if let Some((obs, _cb)) = slot.take() {
                            obs.disconnect();
                        }
                    });
                });
                true
            }
            Err(_) => false,
        }
    });

    // Scene geometry, built once when the surface data arrives; the per-frame
    // scratch (pixel buffer, projection arrays, offscreen canvas) is reused
    // across frames so an animated camera doesn't churn allocations.
    let scene_store = StoredValue::new_local(None::<std::rc::Rc<Scene>>);
    let scratch_store = StoredValue::new_local(Scratch::default());

    // Redraw on any input change: data arrival, container size, camera.
    Effect::new(move |_| {
        let (w, h) = size.get();
        let (az, el, zm) = (azim.get(), elev.get(), zoom.get());
        let has_data = surface.with(|s| s.is_some());
        let Some(canvas) = canvas_ref.get() else { return };
        if w < 2.0 || h < 2.0 || !has_data {
            return;
        }
        if scene_store.with_value(|s| s.is_none()) {
            surface.with_untracked(|s| {
                if let Some(s) = s {
                    scene_store.set_value(Some(std::rc::Rc::new(build_scene(s))));
                }
            });
        }
        let Some(scene) = scene_store.with_value(|s| s.clone()) else { return };
        let dpr = web_sys::window().map(|w| w.device_pixel_ratio()).unwrap_or(1.0).clamp(1.0, 2.0);
        canvas.set_width((w * dpr) as u32);
        canvas.set_height((h * dpr) as u32);
        scratch_store.update_value(|s| render(&canvas, &scene, az, el, zm, s));
    });

    // ── Intro fly-in: top-down → oblique, once the first frame can draw ──
    Effect::new(move |_| {
        let (w, _) = size.get();
        if w < 2.0 || surface.with(|s| s.is_none()) || intro_started.get_value() {
            return;
        }
        intro_started.set_value(true);
        intro_active.set_value(true);
        let t0 = web_sys::window()
            .and_then(|w| w.performance())
            .map(|p| p.now())
            .unwrap_or(0.0);
        raf_closure.set_value(Some(Closure::new(move |now: f64| {
            // Stop silently once cancelled (user interaction) or unmounted —
            // `try_*` so a stale frame after teardown touches nothing.
            if intro_active.try_get_value() != Some(true) {
                return;
            }
            // Hold the top-down view for a beat, then ease to the oblique.
            let t = (((now - t0) - INTRO_HOLD_MS) / INTRO_MOVE_MS).clamp(0.0, 1.0);
            if t > 0.0 {
                // Quintic smootherstep: zero velocity *and* acceleration at
                // both ends, so leaving the hold doesn't visibly kick.
                let e = t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
                let _ = set_azim.try_set(AZIM_TOP + (AZIM0 - AZIM_TOP) * e);
                let _ = set_elev.try_set(ELEV_TOP + (ELEV0 - ELEV_TOP) * e);
            }
            if t < 1.0 {
                request_next();
            } else {
                intro_active.set_value(false);
                raf_id.set_value(0);
            }
        })));
        request_next();
    });

    // ── Turntable interaction (pointer events cover mouse + touch) ──
    let pointers = StoredValue::new_local(Pointers::new());
    let on_pointer_down = move |e: web_sys::PointerEvent| {
        e.prevent_default();
        intro_active.set_value(false); // grabbing the scene ends the fly-in
        if let Some(canvas) = canvas_ref.get() {
            let _ = canvas.set_pointer_capture(e.pointer_id());
        }
        pointers.update_value(|ps| ps.push((e.pointer_id(), e.client_x() as f64, e.client_y() as f64)));
    };
    let on_pointer_move = move |e: web_sys::PointerEvent| {
        let (id, x, y) = (e.pointer_id(), e.client_x() as f64, e.client_y() as f64);
        pointers.update_value(|ps| {
            let Some(k) = ps.iter().position(|p| p.0 == id) else { return };
            match ps.len() {
                // One pointer: rotate. Drag right spins the scene rightward;
                // drag down tilts toward a top-down view (Plotly-like).
                1 => {
                    let (dx, dy) = (x - ps[k].1, y - ps[k].2);
                    set_azim.update(|a| *a -= dx * 0.008);
                    set_elev.update(|p| *p = (*p + dy * 0.008).clamp(0.05, 1.55));
                }
                // Two pointers: pinch zoom on the distance ratio.
                2 => {
                    let other = ps[1 - k];
                    let d0 = ((ps[k].1 - other.1).powi(2) + (ps[k].2 - other.2).powi(2)).sqrt();
                    let d1 = ((x - other.1).powi(2) + (y - other.2).powi(2)).sqrt();
                    if d0 > 1.0 {
                        set_zoom.update(|z| *z = (*z * d1 / d0).clamp(0.4, 6.0));
                    }
                }
                _ => {}
            }
            ps[k] = (id, x, y);
        });
    };
    let release = move |e: web_sys::PointerEvent| {
        pointers.update_value(|ps| ps.retain(|p| p.0 != e.pointer_id()));
    };
    let on_wheel = move |e: web_sys::WheelEvent| {
        e.prevent_default();
        intro_active.set_value(false);
        set_zoom.update(|z| *z = (*z * (-e.delta_y() * 0.0015).exp()).clamp(0.4, 6.0));
    };
    let on_dblclick = move |_| {
        intro_active.set_value(false);
        set_azim.set(AZIM0);
        set_elev.set(ELEV0);
        set_zoom.set(1.0);
    };

    // ── PNG export (copy / download), mirroring the chart/map widgets ──
    let export_title = move || lang.get().t().ufp_title.to_string();
    let on_download = move |_| {
        let Some(canvas) = canvas_ref.get() else { return };
        let title = export_title();
        let filename =
            format!("airquality-ufp-{}.png", chrono::Local::now().format("%Y-%m-%d_%H%M%S"));
        crate::components::export::run_download(filename, move || {
            Box::pin(build_png_blob(canvas, title))
        });
    };
    let (copy_flash, set_copy_flash) = signal(false);
    let on_copy_success = Callback::new(move |_: ()| {
        set_copy_flash.set(true);
        let cb = wasm_bindgen::closure::Closure::once_into_js(move || set_copy_flash.set(false));
        let _ = web_sys::window()
            .unwrap()
            .set_timeout_with_callback_and_timeout_and_arguments_0(cb.as_ref().unchecked_ref(), 1500);
    });
    let on_copy = move |_| {
        let Some(canvas) = canvas_ref.get() else { return };
        let title = export_title();
        crate::components::export::run_copy(
            move || Box::pin(build_png_blob(canvas.clone(), title.clone())),
            on_copy_success,
        );
    };

    let paper_url = "https://www.sciencedirect.com/science/article/pii/S0160412023003793";

    view! {
        <div class="ufp-page">
            <div class="ufp-container" node_ref=container_ref>
                <canvas class="ufp-canvas" node_ref=canvas_ref
                        on:pointerdown=on_pointer_down
                        on:pointermove=on_pointer_move
                        on:pointerup=release
                        on:pointercancel=release
                        on:wheel=on_wheel
                        on:dblclick=on_dblclick></canvas>

                <div class="chart-export-group">
                    <button class="chart-export"
                            class:flash=move || copy_flash.get()
                            title=move || if copy_flash.get() {
                                lang.get().t().copied_to_clipboard
                            } else {
                                lang.get().t().copy_chart_to_clipboard
                            }
                            on:click=on_copy>
                        {move || if copy_flash.get() {
                            view! {
                                <svg viewBox="0 0 24 24" aria-hidden="true">
                                    <path d="M5 12 l4 4 l10 -10" fill="none" stroke="currentColor"
                                          stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                                </svg>
                            }.into_any()
                        } else {
                            view! {
                                <svg viewBox="0 0 24 24" aria-hidden="true">
                                    <rect x="6" y="5" width="12" height="16" rx="1.5"
                                          fill="none" stroke="currentColor" stroke-width="1.6"/>
                                    <rect x="9" y="3" width="6" height="3" rx="0.8"
                                          fill="none" stroke="currentColor" stroke-width="1.6"/>
                                </svg>
                            }.into_any()
                        }}
                    </button>
                    <button class="chart-export"
                            title=move || lang.get().t().download_chart_png
                            on:click=on_download>
                        <svg viewBox="0 0 24 24" aria-hidden="true">
                            <path d="M12 4 V15 M7 11 l5 5 l5 -5 M5 20 h14"
                                  fill="none" stroke="currentColor" stroke-width="1.8"
                                  stroke-linecap="round" stroke-linejoin="round"/>
                        </svg>
                    </button>
                </div>

                <div class="map-hint ufp-title-line">{move || lang.get().t().ufp_title}</div>
                <div class="map-hint ufp-hint-line">
                    {move || if surface.with(|s| s.is_some()) {
                        lang.get().t().ufp_hint
                    } else {
                        lang.get().t().ufp_loading
                    }}
                </div>

                {move || surface.with(|s| s.as_ref().map(|s| {
                    let l = lang.get();
                    let lo = crate::components::chart::fmt_val(s.cmin);
                    let hi = format!("{}+", crate::components::chart::fmt_val(s.cmax));
                    let mid = crate::components::chart::fmt_val((s.cmin + s.cmax) / 2.0);
                    view! {
                        <div class="colorbar">
                            <div class="colorbar-title">{l.t().ufp_legend_title}</div>
                            <div class="colorbar-gradient" style=format!("background:{VIRIDIS_CSS};")></div>
                            <div class="colorbar-scale">
                                <span>{lo}</span>
                                <span>{mid}</span>
                                <span>{hi}</span>
                            </div>
                            <div class="colorbar-note">{l.t().ufp_grid_note}</div>
                        </div>
                    }
                }))}
            </div>

            // Attribution: the source paper, the data provenance, and the
            // modelled-not-measured caveat (both languages via i18n).
            <p class="ufp-attribution">
                {move || lang.get().t().ufp_source_label}
                " "
                <a href=paper_url target="_blank" rel="noopener noreferrer">
                    "Predicting spatial variations in annual average outdoor ultrafine particle concentrations in Montreal and Toronto, Canada: Integrating land use regression and deep learning models"
                </a>
                " "
                {move || lang.get().t().ufp_data_credit}
                " "
                {move || lang.get().t().ufp_modelled_note}
            </p>
        </div>
    }
}
