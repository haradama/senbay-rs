//! QR/OpenCV video writer (requires the `video` feature).

use std::time::{SystemTime, UNIX_EPOCH};

use opencv::core::{Mat, MatTraitConst, Rect, Scalar, Size};
use opencv::prelude::*;
use opencv::{highgui, imgproc, videoio};
use qrcode::{Color, QrCode};

use crate::codec::Senbay;
use crate::error::Result;
use crate::record::{Encoding, Record};
use crate::{KEY_CODE_ESC, Radix};

/// Captures camera frames and overlays a QR code carrying a Senbay record.
///
/// ```no_run
/// # use senbay_rs::{Record, Writer};
/// // Stamp each frame with the current time.
/// Writer::new("out.avi")
///     .run(|| {
///         let mut record = Record::new();
///         record.set("TIME", now_millis());
///         record
///     })
///     .unwrap();
/// # fn now_millis() -> i64 { 0 }
/// ```
pub struct Writer {
    output: String,
    camera: i32,
    codec_fourcc: String,
    fps: f64,
    qr_size: i32,
    senbay: Senbay,
}

impl Writer {
    /// Creates a writer for the given output file with sensible defaults
    /// (camera 0, `MJPG` codec, 30 fps, 300px QR codes).
    pub fn new(output: impl Into<String>) -> Self {
        Writer {
            output: output.into(),
            camera: 0,
            codec_fourcc: "MJPG".to_owned(),
            fps: 30.0,
            qr_size: 300,
            senbay: Senbay::new(),
        }
    }

    /// Selects the camera device index.
    pub fn camera(mut self, index: u32) -> Self {
        self.camera = index as i32;
        self
    }

    /// Sets the FourCC codec string (e.g. `"MJPG"`, `"XVID"`).
    pub fn codec(mut self, fourcc: impl Into<String>) -> Self {
        self.codec_fourcc = fourcc.into();
        self
    }

    /// Sets the output frames-per-second.
    pub fn fps(mut self, fps: u32) -> Self {
        self.fps = fps as f64;
        self
    }

    /// Sets the rendered QR code size in pixels.
    pub fn qr_size(mut self, size: u32) -> Self {
        self.qr_size = size as i32;
        self
    }

    /// Uses a custom numeric radix for encoding.
    pub fn radix(mut self, radix: Radix) -> Self {
        self.senbay = Senbay::with_radix(radix.get()).expect("validated radix");
        self
    }

    /// Captures frames until <kbd>Esc</kbd>, embedding the record produced by
    /// `next_record` into each one.
    pub fn run(&self, mut next_record: impl FnMut() -> Record) -> Result<()> {
        const WINDOW: &str = "Senbay Writer";

        highgui::named_window(WINDOW, highgui::WINDOW_AUTOSIZE)?;

        let mut webcam = videoio::VideoCapture::new(self.camera, videoio::CAP_ANY)?;
        if !webcam.is_opened()? {
            return Err(opencv::Error::new(0, "cannot open camera").into());
        }

        let mut frame = Mat::default();
        if !webcam.read(&mut frame)? {
            return Err(opencv::Error::new(0, "cannot read from camera").into());
        }

        let fourcc = fourcc(&self.codec_fourcc)?;
        let mut video = videoio::VideoWriter::new(
            &self.output,
            fourcc,
            self.fps,
            Size::new(frame.cols(), frame.rows()),
            true,
        )?;

        loop {
            let text = self.senbay.encode(&next_record(), Encoding::Plain);
            let qr = render_qr(&text, self.qr_size, self.qr_size);

            if !webcam.read(&mut frame)? || frame.empty() {
                continue;
            }

            overlay_qr(&mut frame, &qr)?;
            video.write(&frame)?;

            highgui::imshow(WINDOW, &frame)?;
            if highgui::wait_key(1)? == KEY_CODE_ESC {
                break;
            }
        }

        Ok(())
    }

    /// Convenience for the common case: stamp each frame with the current
    /// wall-clock time in milliseconds under the `TIME` key.
    pub fn run_timestamps(&self) -> Result<()> {
        self.run(|| {
            let mut record = Record::new();
            record.set("TIME", now_millis());
            record
        })
    }
}

/// Current Unix time in milliseconds.
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Builds an OpenCV FourCC code from a codec string, padding to four chars.
fn fourcc(codec: &str) -> Result<i32> {
    let mut chars = codec.chars().chain(std::iter::repeat(' '));
    let mut next = || chars.next().unwrap();
    Ok(videoio::VideoWriter::fourcc(
        next(),
        next(),
        next(),
        next(),
    )?)
}

/// A rendered QR code: `dark[y * width + x]` is true for dark modules.
struct RenderedQr {
    width: usize,
    height: usize,
    dark: Vec<bool>,
}

/// Renders `text` as a QR code scaled (nearest-neighbour) to roughly
/// `width` x `height` pixels.
fn render_qr(text: &str, width: i32, height: i32) -> RenderedQr {
    let code = QrCode::new(text.as_bytes()).expect("QR encoding cannot fail for valid input");
    let modules = code.width();
    let colors = code.to_colors();

    let out_w = width.max(modules as i32) as usize;
    let out_h = height.max(modules as i32) as usize;
    let mut dark = vec![false; out_w * out_h];
    for (y, row) in dark.chunks_mut(out_w).enumerate() {
        let sy = y * modules / out_h;
        for (x, cell) in row.iter_mut().enumerate() {
            let sx = x * modules / out_w;
            *cell = colors[sy * modules + sx] == Color::Dark;
        }
    }

    RenderedQr {
        width: out_w,
        height: out_h,
        dark,
    }
}

/// Overlays the rendered QR code onto the top-left of `img`: dark modules
/// become black, light modules white.
fn overlay_qr(img: &mut Mat, qr: &RenderedQr) -> Result<()> {
    let rows = img.rows() as usize;
    let cols = img.cols() as usize;

    for y in 0..qr.height.min(rows) {
        for x in 0..qr.width.min(cols) {
            let shade = if qr.dark[y * qr.width + x] { 0.0 } else { 255.0 };
            imgproc::rectangle(
                img,
                Rect::new(x as i32, y as i32, 1, 1),
                Scalar::new(shade, shade, shade, 0.0),
                imgproc::FILLED,
                imgproc::LINE_8,
                0,
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises the full QR pipeline: encode a record (writer side) to a QR
    /// code, then read it back with quircs (reader side).
    #[test]
    fn qr_pipeline_round_trips() {
        let codec = Senbay::new();
        let mut record = Record::new();
        record.set("TIME", 1_700_000_000_000_i64).set("LATI", 35.6895);
        let text = codec.encode(&record, Encoding::Plain);

        let qr = render_qr(&text, 200, 200);

        // Render to a grayscale buffer with a quiet zone so quircs can detect it.
        let scale = 3usize;
        let border = 12usize;
        let img_w = qr.width * scale + border * 2;
        let img_h = qr.height * scale + border * 2;
        let mut luma = vec![255u8; img_w * img_h];
        for y in 0..qr.height {
            for x in 0..qr.width {
                if qr.dark[y * qr.width + x] {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let px = border + x * scale + dx;
                            let py = border + y * scale + dy;
                            luma[py * img_w + px] = 0;
                        }
                    }
                }
            }
        }

        let mut quirc = quircs::Quirc::default();
        let content = quirc
            .identify(img_w, img_h, &luma)
            .find_map(|code| code.ok()?.decode().ok())
            .map(|data| String::from_utf8_lossy(&data.payload).into_owned())
            .expect("QR decode failed");
        assert_eq!(content, text);

        let decoded = codec.decode(&content);
        assert_eq!(decoded.get("TIME").unwrap().as_f64(), Some(1_700_000_000_000.0));
        assert_eq!(decoded.get("LATI").unwrap().as_f64(), Some(35.6895));
    }
}
