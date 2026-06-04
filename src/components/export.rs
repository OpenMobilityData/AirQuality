//! Shared PNG export helpers (download + clipboard) used by both the chart and
//! the map. Each view supplies its own async "build a PNG `Blob`" closure; this
//! module handles the download anchor and the Safari-friendly clipboard write.

use std::future::Future;
use std::pin::Pin;

use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};

/// A boxed future producing a PNG blob (or a JS error).
pub type BlobFut = Pin<Box<dyn Future<Output = Result<web_sys::Blob, JsValue>>>>;

/// Trigger a file download of `blob` via a synthetic anchor click.
///
/// The object URL is intentionally not revoked synchronously: `anchor.click()`
/// only schedules the download, and revoking immediately can race the fetch and
/// produce a zero-byte file (Safari / some Chromium). It is released on unload.
pub fn download_blob(blob: &web_sys::Blob, filename: &str) -> Result<(), JsValue> {
    let document = web_sys::window()
        .ok_or_else(|| JsValue::from_str("no window"))?
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let url = web_sys::Url::create_object_url_with_blob(blob)?;
    let anchor = document.create_element("a")?.dyn_into::<web_sys::HtmlAnchorElement>()?;
    anchor.set_href(&url);
    anchor.set_download(filename);
    anchor.click();
    Ok(())
}

/// Build a PNG (via `make`) and download it as `filename`.
pub fn run_download(filename: String, make: impl FnOnce() -> BlobFut + 'static) {
    spawn_local(async move {
        let result: Result<(), JsValue> = async {
            let blob = make().await?;
            download_blob(&blob, &filename)
        }
        .await;
        if let Err(e) = result {
            web_sys::console::error_1(&format!("PNG download failed: {e:?}").into());
        }
    });
}

/// Build a PNG (via `make`) and write it to the clipboard, calling `on_success`
/// when the write resolves. `make` is invoked synchronously inside the
/// `ClipboardItem`'s `Promise<Blob>` so Safari keeps the user-activation alive.
pub fn run_copy(make: impl Fn() -> BlobFut + 'static, on_success: Callback<()>) {
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        let fut = make();
        spawn_local(async move {
            match fut.await {
                Ok(b) => {
                    let _ = resolve.call1(&JsValue::NULL, &b);
                }
                Err(e) => {
                    let _ = reject.call1(&JsValue::NULL, &e);
                }
            }
        });
    });
    let map = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&map, &JsValue::from_str("image/png"), &promise);
    let item = match web_sys::ClipboardItem::new_with_record_from_str_to_blob_promise(&map) {
        Ok(it) => it,
        Err(e) => {
            web_sys::console::error_1(&format!("ClipboardItem failed: {e:?}").into());
            return;
        }
    };
    let arr = js_sys::Array::of1(&item);
    let clipboard = web_sys::window().unwrap().navigator().clipboard();
    spawn_local(async move {
        match JsFuture::from(clipboard.write(&arr)).await {
            Ok(_) => on_success.run(()),
            Err(e) => web_sys::console::error_1(&format!("Clipboard write failed: {e:?}").into()),
        }
    });
}

/// Build a composite SVG document into a PNG blob, rasterized at 2× for retina.
/// `svg` must be a complete `<svg …>…</svg>` string with explicit width/height.
pub async fn svg_to_png_blob(svg: String, w: f64, h: f64) -> Result<web_sys::Blob, JsValue> {
    let document = web_sys::window()
        .ok_or_else(|| JsValue::from_str("no window"))?
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;

    let parts = js_sys::Array::of1(&JsValue::from_str(&svg));
    let bag = web_sys::BlobPropertyBag::new();
    bag.set_type("image/svg+xml;charset=utf-8");
    let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &bag)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)?;

    let image = web_sys::HtmlImageElement::new()?;
    image.set_src(&url);
    JsFuture::from(image.decode()).await?;

    let scale = 2u32;
    let canvas = document.create_element("canvas")?.dyn_into::<web_sys::HtmlCanvasElement>()?;
    canvas.set_width(w as u32 * scale);
    canvas.set_height(h as u32 * scale);
    let ctx = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("no 2d context"))?
        .dyn_into::<web_sys::CanvasRenderingContext2d>()?;
    ctx.scale(scale as f64, scale as f64)?;
    ctx.draw_image_with_html_image_element_and_dw_and_dh(&image, 0.0, 0.0, w, h)?;
    web_sys::Url::revoke_object_url(&url)?;

    canvas_to_png_blob(&canvas).await
}

/// `canvas.toBlob()` wrapped as an awaitable returning a real `image/png` Blob.
pub async fn canvas_to_png_blob(canvas: &web_sys::HtmlCanvasElement) -> Result<web_sys::Blob, JsValue> {
    let canvas = canvas.clone();
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let cb_resolve = resolve.clone();
        let closure: Closure<dyn FnMut(JsValue)> =
            Closure::once(Box::new(move |blob_val: JsValue| {
                let _ = cb_resolve.call1(&JsValue::NULL, &blob_val);
            }));
        let _ = canvas.to_blob_with_type(closure.as_ref().unchecked_ref(), "image/png");
        closure.forget();
    });
    let blob_val = JsFuture::from(promise).await?;
    blob_val
        .dyn_into::<web_sys::Blob>()
        .map_err(|_| JsValue::from_str("toBlob returned non-Blob"))
}
