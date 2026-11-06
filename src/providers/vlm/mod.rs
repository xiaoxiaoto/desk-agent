use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{ImageFormat, codecs::jpeg::JpegEncoder, imageops::FilterType};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const ENDPOINT: &str = "http://127.0.0.1/v1/chat/completions";
const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 100_000_000;
const MAX_JPEG_BYTES: usize = 600 * 1024;
const MAX_REQUEST_BYTES: usize = 900 * 1024;
// The service may include a base64 overlay alongside the structured result.
const MAX_RESPONSE_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Deserialize)]
struct Completion {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Deserialize)]
struct Message {
    pixel: Pixel,
}

#[derive(Deserialize)]
struct Pixel {
    image_size: [u32; 2],
    predictions: Vec<Prediction>,
}

#[derive(Deserialize)]
struct Prediction {
    label: String,
    #[serde(rename = "type")]
    kind: String,
    coords: [f64; 4],
}

#[derive(Serialize)]
struct LocatedObject<'a> {
    query: &'a str,
    predictions: Vec<LocatedPrediction>,
}

#[derive(Serialize)]
struct LocatedPrediction {
    label: String,
    coords: [f64; 4],
}

/// Locate objects in a freshly captured app window PNG. Coordinates are image pixels, not desktop coordinates.
/// The caller is responsible for authorizing capture and disclosure and supplying the capture path.
pub fn locate_app_window_objects(path: &Path, objects: &[String]) -> Result<Value> {
    ensure!(!objects.is_empty() && objects.len() <= 16, "invalid object count");
    ensure!(objects.iter().all(|object| !object.trim().is_empty() && object.len() <= 256), "invalid object query");

    let mut image = Vec::new();
    File::open(path)
        .context("failed to open app window capture")?
        .take(MAX_IMAGE_BYTES + 1)
        .read_to_end(&mut image)
        .context("failed to read app window capture")?;
    ensure!(image.len() as u64 <= MAX_IMAGE_BYTES, "app window capture exceeds size limit");
    ensure!(image.len() >= 24 && &image[..8] == b"\x89PNG\r\n\x1a\n" && &image[8..16] == b"\0\0\0\rIHDR", "app window capture is not a PNG");
    let image_size = [
        u32::from_be_bytes(image[16..20].try_into().unwrap()),
        u32::from_be_bytes(image[20..24].try_into().unwrap()),
    ];
    ensure!(image_size[0] > 0 && image_size[1] > 0
        && u64::from(image_size[0]) * u64::from(image_size[1]) <= MAX_IMAGE_PIXELS,
        "invalid app window capture dimensions");
    let decoded = image::load_from_memory_with_format(&image, ImageFormat::Png)
        .map_err(|_| anyhow::anyhow!("failed to decode app window capture"))?
        .to_rgb8();
    ensure!([decoded.width(), decoded.height()] == image_size, "app window capture dimensions do not match PNG header");
    drop(image);

    let longest = image_size[0].max(image_size[1]);
    let divisor = u64::from(longest.max(1280));
    let mut sent_size = [
        ((u64::from(image_size[0]) * 1280 / divisor) as u32).max(1),
        ((u64::from(image_size[1]) * 1280 / divisor) as u32).max(1),
    ];
    let jpeg = loop {
        let resized = image::imageops::resize(&decoded, sent_size[0], sent_size[1], FilterType::Triangle);
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 75)
            .encode_image(&resized)
            .context("failed to encode app window capture")?;
        if jpeg.len() <= MAX_JPEG_BYTES {
            break jpeg;
        }
        ensure!(sent_size != [1, 1], "app window capture cannot fit local vision request limit");
        sent_size = [(sent_size[0] * 3 / 4).max(1), (sent_size[1] * 3 / 4).max(1)];
    };
    let image_url = format!("data:image/jpeg;base64,{}", STANDARD.encode(&jpeg));

    // No proxy or redirect may cause the screenshot to leave the fixed local endpoint.
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(90))
        .build()
        .context("failed to initialize local vision client")?;
    let mut located = Vec::with_capacity(objects.len());
    for query in objects {
        let request = serde_json::to_vec(&json!({
            "model": "wevisone-vl-4b",
            "wevisone_task": "referring",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": query},
                    {"type": "image_url", "image_url": {"url": image_url}}
                ]
            }]
        })).context("failed to encode local vision request")?;
        ensure!(request.len() < MAX_REQUEST_BYTES, "local vision request exceeds size limit");
        let response = client.post(ENDPOINT)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request).send().context("local vision request failed")?;
        ensure!(response.status().is_success(), "local vision request failed with HTTP status {}", response.status());
        if let Some(length) = response.content_length() {
            ensure!(length <= MAX_RESPONSE_BYTES, "local vision response exceeds size limit");
        }
        let mut body = Vec::new();
        response.take(MAX_RESPONSE_BYTES + 1).read_to_end(&mut body)
            .context("failed to read local vision response")?;
        ensure!(body.len() as u64 <= MAX_RESPONSE_BYTES, "local vision response exceeds size limit");
        // Deserialize only the structured fields; unknown fields, including overlays, are discarded.
        let completion: Completion = serde_json::from_slice(&body)
            .map_err(|_| anyhow::anyhow!("malformed local vision response"))?;
        let mut choices = completion.choices.into_iter();
        let pixel = choices.next().context("local vision response has no choice")?.message.pixel;
        ensure!(pixel.image_size == sent_size, "local vision image dimensions do not match uploaded image");
        let mut predictions = Vec::with_capacity(pixel.predictions.len());
        for prediction in pixel.predictions {
            let [x1, y1, x2, y2] = prediction.coords;
            if prediction.kind != "box"
                || !prediction.coords.iter().all(|coordinate| coordinate.is_finite())
                || x1 < 0.0 || y1 < 0.0 || x1 >= x2 || y1 >= y2
                || x2 > f64::from(sent_size[0]) || y2 > f64::from(sent_size[1])
            {
                bail!("invalid local vision bounding box");
            }
            let x_scale = f64::from(image_size[0]) / f64::from(sent_size[0]);
            let y_scale = f64::from(image_size[1]) / f64::from(sent_size[1]);
            let coords = [
                (x1 * x_scale).min(f64::from(image_size[0])),
                (y1 * y_scale).min(f64::from(image_size[1])),
                (x2 * x_scale).min(f64::from(image_size[0])),
                (y2 * y_scale).min(f64::from(image_size[1])),
            ];
            predictions.push(LocatedPrediction { label: prediction.label, coords });
        }
        located.push(LocatedObject { query, predictions });
    }
    Ok(json!({"image_size": image_size, "objects": located}))
}
