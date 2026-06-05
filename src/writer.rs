//! QR/OpenCV video writer (requires the `video` feature).
//!
//! As with the reader, all logic is testable in isolation: the camera, the
//! video sink, and the preview window are abstracted behind [`Camera`],
//! [`VideoSink`], and [`Preview`]. The real OpenCV implementations and the
//! public entry points that wire them live in `video_backend.rs`, which is
//! excluded from coverage because it only drives a camera/display.

use std::time::{SystemTime, UNIX_EPOCH};

use opencv::core::{Mat, MatTraitConst, Rect, Scalar, Size};
use opencv::{imgproc, videoio};
use qrcode::{Color, QrCode};

use crate::codec::Senbay;
use crate::error::Result;
use crate::reader::Preview;
use crate::record::{Encoding, Record};
use crate::{KEY_CODE_ESC, Radix};

/// A camera (frame source) for the writer. Implemented by the real OpenCV
/// capture in `video_backend.rs` and by fakes in tests.
pub(crate) trait Camera {
    /// Returns whether the camera opened successfully.
    fn is_opened(&self) -> Result<bool>;
    /// Reads the next frame into `frame`, returning `false` on failure.
    fn read(&mut self, frame: &mut Mat) -> Result<bool>;
}

/// A sink that records encoded frames (a video file). Implemented by the real
/// OpenCV writer in `video_backend.rs` and by fakes in tests.
pub(crate) trait VideoSink {
    /// Prepares the sink for frames of the given size (known after the first
    /// camera read).
    fn open(&mut self, size: Size) -> Result<()>;
    /// Writes one frame to the sink.
    fn write(&mut self, frame: &Mat) -> Result<()>;
}

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
    pub(crate) output: String,
    pub(crate) camera: i32,
    pub(crate) codec_fourcc: String,
    pub(crate) fps: f64,
    pub(crate) qr_size: i32,
    pub(crate) senbay: Senbay,
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

    /// Captures frames from `camera`, embeds the record produced by
    /// `next_record` into each, writes them to `sink` (opened at the first
    /// frame's size), and previews them until <kbd>Esc</kbd>.
    pub(crate) fn run_core(
        &self,
        mut camera: Box<dyn Camera>,
        mut sink: Box<dyn VideoSink>,
        mut preview: Box<dyn Preview>,
        mut next_record: impl FnMut() -> Record,
    ) -> Result<()> {
        if !camera.is_opened()? {
            return Err(opencv::Error::new(0, "cannot open camera").into());
        }

        let mut frame = Mat::default();
        if !camera.read(&mut frame)? {
            return Err(opencv::Error::new(0, "cannot read from camera").into());
        }

        sink.open(Size::new(frame.cols(), frame.rows()))?;

        loop {
            let text = self.senbay.encode(&next_record(), Encoding::Plain);
            let qr = render_qr(&text, self.qr_size, self.qr_size);

            if !camera.read(&mut frame)? || frame.empty() {
                continue;
            }

            overlay_qr(&mut frame, &qr)?;
            sink.write(&frame)?;

            preview.show(&frame)?;
            if preview.wait_key()? == KEY_CODE_ESC {
                break;
            }
        }

        Ok(())
    }
}

/// Current Unix time in milliseconds.
pub(crate) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Builds an OpenCV FourCC code from a codec string, padding to four chars.
pub(crate) fn fourcc(codec: &str) -> Result<i32> {
    let mut chars = codec.chars().chain(std::iter::repeat(' '));
    let mut next = || chars.next().unwrap();
    Ok(videoio::VideoWriter::fourcc(next(), next(), next(), next())?)
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
            let pixel = Rect::new(x as i32, y as i32, 1, 1);
            let color = Scalar::new(shade, shade, shade, 0.0);
            imgproc::rectangle(img, pixel, color, imgproc::FILLED, imgproc::LINE_8, 0)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use opencv::core::CV_8UC3;

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

    #[test]
    fn now_millis_is_positive() {
        assert!(now_millis() > 0);
    }

    #[test]
    fn fourcc_pads_short_codes() {
        // Both a full and a short code produce a valid (non-zero) FourCC.
        assert!(fourcc("MJPG").is_ok());
        assert!(fourcc("X").is_ok());
    }

    #[test]
    fn render_qr_floors_to_module_count() {
        // A requested size below the module count is clamped up to it.
        let qr = render_qr("hello", 1, 1);
        assert!(qr.width >= 21 && qr.height >= 21);
        assert!(qr.dark.iter().any(|&d| d)); // has dark modules
        assert!(qr.dark.iter().any(|&d| !d)); // and light ones
    }

    #[test]
    fn overlay_qr_paints_within_bounds() {
        let qr = render_qr("hello", 8, 8);
        let mut frame =
            Mat::new_rows_cols_with_default(4, 4, CV_8UC3, Scalar::all(50.0)).unwrap();
        // Frame is smaller than the QR; overlay must clamp to the frame.
        overlay_qr(&mut frame, &qr).unwrap();
    }

    fn valid_frame() -> Mat {
        Mat::new_rows_cols_with_default(4, 4, CV_8UC3, Scalar::all(255.0)).unwrap()
    }

    struct FakeCamera {
        opened: bool,
        frames: VecDeque<Option<Mat>>, // Some = real frame, None = empty frame
    }

    impl Camera for FakeCamera {
        fn is_opened(&self) -> Result<bool> {
            Ok(self.opened)
        }

        fn read(&mut self, frame: &mut Mat) -> Result<bool> {
            match self.frames.pop_front() {
                Some(Some(m)) => {
                    *frame = m;
                    Ok(true)
                }
                Some(None) => {
                    *frame = Mat::default();
                    Ok(true)
                }
                None => Ok(false),
            }
        }
    }

    struct FakeSink {
        writes: Rc<RefCell<usize>>,
        opened: Rc<RefCell<bool>>,
    }

    impl VideoSink for FakeSink {
        fn open(&mut self, _size: Size) -> Result<()> {
            *self.opened.borrow_mut() = true;
            Ok(())
        }

        fn write(&mut self, _frame: &Mat) -> Result<()> {
            *self.writes.borrow_mut() += 1;
            Ok(())
        }
    }

    struct FakePreview {
        keys: VecDeque<i32>,
    }

    impl Preview for FakePreview {
        fn show(&mut self, _frame: &Mat) -> Result<()> {
            Ok(())
        }

        fn wait_key(&mut self) -> Result<i32> {
            Ok(self.keys.pop_front().unwrap_or(KEY_CODE_ESC))
        }
    }

    fn fake_sink() -> FakeSink {
        FakeSink {
            writes: Rc::new(RefCell::new(0)),
            opened: Rc::new(RefCell::new(false)),
        }
    }

    #[test]
    fn run_core_overlays_writes_and_stops_on_esc() {
        let writer = Writer::new("out.avi").qr_size(40);
        let camera = FakeCamera {
            opened: true,
            frames: vec![
                Some(valid_frame()), // first read: used for sizing
                None,                // empty frame -> continue
                Some(valid_frame()), // processed
                Some(valid_frame()), // processed, then Esc
            ]
            .into(),
        };
        let preview = FakePreview {
            keys: vec![1, KEY_CODE_ESC].into(), // first non-Esc, then Esc
        };
        let sink = fake_sink();
        let (writes, opened) = (sink.writes.clone(), sink.opened.clone());

        writer
            .run_core(Box::new(camera), Box::new(sink), Box::new(preview), Record::new)
            .unwrap();

        assert!(*opened.borrow());
        assert_eq!(*writes.borrow(), 2);
    }

    #[test]
    fn run_core_errors_when_camera_not_opened() {
        let writer = Writer::new("out.avi");
        let camera = FakeCamera {
            opened: false,
            frames: VecDeque::new(),
        };
        let err = writer.run_core(
            Box::new(camera),
            Box::new(fake_sink()),
            Box::new(FakePreview { keys: VecDeque::new() }),
            Record::new,
        );
        assert!(err.is_err());
    }

    #[test]
    fn run_core_errors_when_first_read_fails() {
        let writer = Writer::new("out.avi");
        let camera = FakeCamera {
            opened: true,
            frames: VecDeque::new(), // first read returns false
        };
        let err = writer.run_core(
            Box::new(camera),
            Box::new(fake_sink()),
            Box::new(FakePreview { keys: VecDeque::new() }),
            Record::new,
        );
        assert!(err.is_err());
    }

    #[test]
    fn builders_set_all_fields() {
        let writer = Writer::new("out.avi")
            .camera(2)
            .codec("XVID")
            .fps(60)
            .qr_size(150)
            .radix(crate::Radix::new(64).unwrap());
        assert_eq!(writer.camera, 2);
        assert_eq!(writer.codec_fourcc, "XVID");
        assert_eq!(writer.fps, 60.0);
        assert_eq!(writer.qr_size, 150);
        assert_eq!(writer.senbay.radix().get(), 64);
    }
}
