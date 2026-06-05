//! QR/OpenCV video reader (requires the `video` feature).
//!
//! All decoding logic here is testable in isolation: frame I/O and the preview
//! window are abstracted behind the [`FrameReader`] and [`Preview`] traits, and
//! the real OpenCV-backed implementations live in `video_backend.rs` (which is
//! excluded from coverage because it only exercises a camera/display).

use opencv::core::{Mat, MatTraitConst, Rect};
use opencv::imgproc;
use opencv::prelude::*;
use quircs::Quirc;

use crate::codec::Senbay;
use crate::error::Result;
use crate::record::Record;
use crate::{KEY_CODE_ESC, Radix};

/// A source of video frames. Implemented by the real OpenCV capture in
/// `video_backend.rs` and by fakes in tests.
pub(crate) trait FrameReader {
    /// Reads the next frame into `frame`, returning `false` at end of stream.
    fn read(&mut self, frame: &mut Mat) -> Result<bool>;
    /// Advances past one frame without decoding it.
    fn grab(&mut self) -> Result<bool>;
}

/// A preview surface for the decoded frames. Implemented by the real highgui
/// window in `video_backend.rs` and by fakes in tests.
pub(crate) trait Preview {
    /// Displays `frame`.
    fn show(&mut self, frame: &Mat) -> Result<()>;
    /// Waits briefly for a key press, returning its code.
    fn wait_key(&mut self) -> Result<i32>;
}

/// Reads Senbay records from QR codes embedded in a video file.
///
/// For headless processing prefer [`records`](Reader::records), which yields an
/// iterator so you can stop early (`take`, `break`, …). [`for_each`](Reader::for_each)
/// additionally shows a preview window unless [`headless`](Reader::headless) is set.
///
/// ```no_run
/// # use senbay_rs::Reader;
/// # fn main() -> senbay_rs::Result<()> {
/// for record in Reader::from_file("input.mp4").records()?.take(10) {
///     println!("{}", record?.to_json());
/// }
/// # Ok(())
/// # }
/// ```
pub struct Reader {
    pub(crate) source: String,
    pub(crate) headless: bool,
    pub(crate) step: usize,
    pub(crate) codec: Senbay,
}

impl Reader {
    /// Creates a reader for the given video file.
    pub fn from_file(source: impl Into<String>) -> Self {
        Reader {
            source: source.into(),
            headless: false,
            step: 1,
            codec: Senbay::new(),
        }
    }

    /// Runs without opening a preview window when `headless` is `true`.
    pub fn headless(mut self, headless: bool) -> Self {
        self.headless = headless;
        self
    }

    /// Decodes only every `step`-th frame (default `1` = every frame).
    ///
    /// A QR code typically persists across several consecutive frames, so a
    /// small step (2–3) can roughly halve the work without missing records.
    /// Skipped frames are advanced cheaply without being decoded.
    pub fn frame_step(mut self, step: usize) -> Self {
        self.step = step.max(1);
        self
    }

    /// Uses a custom numeric radix for decoding.
    pub fn radix(mut self, radix: Radix) -> Self {
        self.codec = Senbay::with_radix(radix.get()).expect("validated radix");
        self
    }

    /// Builds the record iterator over an arbitrary frame source.
    pub(crate) fn record_iter(&self, source: Box<dyn FrameReader>) -> RecordIter {
        RecordIter {
            source,
            frame: Mat::default(),
            scanner: Scanner::new(),
            codec: self.codec,
            step: self.step,
            started: false,
        }
    }

    /// Decodes every frame from `source`, invoking `callback` with each record.
    ///
    /// When `preview` is provided, each frame is shown and <kbd>Esc</kbd> stops
    /// playback; otherwise it runs headless.
    pub(crate) fn run_loop(
        &self,
        mut source: Box<dyn FrameReader>,
        mut preview: Option<Box<dyn Preview>>,
        callback: &mut dyn FnMut(Record),
    ) -> Result<()> {
        let mut frame = Mat::default();
        let mut scanner = Scanner::new();
        let mut index = 0usize;
        loop {
            if !source.read(&mut frame)? || frame.empty() {
                break;
            }
            if index.is_multiple_of(self.step)
                && let Some(text) = scanner.scan(&frame)?
            {
                callback(self.codec.decode(&text));
            }
            index += 1;

            if let Some(preview) = preview.as_mut() {
                preview.show(&frame)?;
                if preview.wait_key()? == KEY_CODE_ESC {
                    break;
                }
            }
        }
        Ok(())
    }
}

/// Iterator over the records decoded from a video, yielded one per QR decode.
///
/// Created by [`Reader::records`].
pub struct RecordIter {
    source: Box<dyn FrameReader>,
    frame: Mat,
    scanner: Scanner,
    codec: Senbay,
    step: usize,
    started: bool,
}

impl Iterator for RecordIter {
    type Item = Result<Record>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Sample frames 0, step, 2*step, … — skip cheaply with grab()
            // (no decode), but never skip ahead of the very first frame.
            if self.started {
                for _ in 1..self.step {
                    if !self.source.grab().unwrap_or(false) {
                        return None;
                    }
                }
            } else {
                self.started = true;
            }

            match self.source.read(&mut self.frame) {
                Ok(true) if !self.frame.empty() => {}
                Ok(_) => return None,
                Err(err) => return Some(Err(err)),
            }

            match self.scanner.scan(&self.frame) {
                Ok(Some(text)) => return Some(Ok(self.codec.decode(&text))),
                Ok(None) => continue,
                Err(err) => return Some(Err(err)),
            }
        }
    }
}

/// QR scanner over BGR frames, backed by [`quircs`].
///
/// Detection cost scales with the scanned area, so once a code is found the
/// scanner remembers its location and, on subsequent frames, only re-scans a
/// padded region around it — falling back to the whole frame whenever that
/// region comes up empty. The `quircs` instance and scratch buffers are reused
/// across frames to avoid per-frame allocation.
struct Scanner {
    quirc: Quirc,
    gray: Mat,
    roi_buf: Mat,
    roi: Option<Rect>,
}

impl Scanner {
    fn new() -> Self {
        Scanner {
            quirc: Quirc::default(),
            gray: Mat::default(),
            roi_buf: Mat::default(),
            roi: None,
        }
    }

    /// Scans `frame` for a QR code, returning its decoded text if found.
    fn scan(&mut self, frame: &Mat) -> Result<Option<String>> {
        let (fw, fh) = (frame.cols(), frame.rows());
        if fw == 0 || fh == 0 {
            return Ok(None);
        }
        // `_def` keeps the call portable: OpenCV 4.11+ added a trailing
        // `AlgorithmHint` arg to `cvt_color`; the def variant uses the defaults.
        imgproc::cvt_color_def(frame, &mut self.gray, imgproc::COLOR_BGR2GRAY)?;

        // Fast path: re-scan only the padded region around the last sighting.
        if let Some(rect) = self.roi {
            Mat::roi(&self.gray, rect)?.copy_to(&mut self.roi_buf)?;
            if let Some((text, _)) = scan_mat(&mut self.quirc, &self.roi_buf) {
                return Ok(Some(text));
            }
        }

        // Slow path: scan the whole frame and remember where the code is.
        if let Some((text, corners)) = scan_mat(&mut self.quirc, &self.gray) {
            self.roi = bounding_roi(&corners, fw, fh);
            return Ok(Some(text));
        }

        self.roi = None;
        Ok(None)
    }
}

/// Extracts the single-channel pixel buffer from `gray` and decodes the first
/// QR code in it. Returns `None` if the `Mat` is not a contiguous byte buffer.
fn scan_mat(quirc: &mut Quirc, gray: &Mat) -> Option<(String, [quircs::Point; 4])> {
    let bytes = gray.data_bytes().ok()?;
    decode_luma(quirc, bytes, gray.cols() as usize, gray.rows() as usize)
}

/// Decodes the first QR code in a `w` x `h` grayscale buffer, returning its text
/// and corner points (in buffer coordinates).
fn decode_luma(quirc: &mut Quirc, bytes: &[u8], w: usize, h: usize) -> Option<(String, [quircs::Point; 4])> {
    if bytes.len() < w * h {
        return None;
    }
    quirc.identify(w, h, bytes).find_map(|code| {
        let code = code.ok()?;
        let corners = code.corners;
        let payload = code.decode().ok()?.payload;
        Some((String::from_utf8_lossy(&payload).into_owned(), corners))
    })
}

/// Computes a padded bounding rectangle around the QR corners, clamped to the
/// frame. Returns `None` if the rectangle would be degenerate.
fn bounding_roi(corners: &[quircs::Point; 4], fw: i32, fh: i32) -> Option<Rect> {
    let x0 = corners.iter().map(|p| p.x).min()?.clamp(0, fw);
    let x1 = corners.iter().map(|p| p.x).max()?.clamp(0, fw);
    let y0 = corners.iter().map(|p| p.y).min()?.clamp(0, fh);
    let y1 = corners.iter().map(|p| p.y).max()?.clamp(0, fh);

    // Pad by half the code's size to tolerate slight movement between frames.
    let margin = (x1 - x0).max(y1 - y0).max(1) / 2;
    let rx = (x0 - margin).clamp(0, fw);
    let ry = (y0 - margin).clamp(0, fh);
    let rw = (x1 + margin).min(fw) - rx;
    let rh = (y1 + margin).min(fh) - ry;
    if rw <= 0 || rh <= 0 {
        return None;
    }
    Some(Rect::new(rx, ry, rw, rh))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    use opencv::core::{CV_8UC1, CV_8UC3, Scalar};
    use qrcode::{Color, QrCode};

    /// Builds a BGR `Mat` containing a detectable QR code for `text`.
    fn qr_frame(text: &str) -> Mat {
        let code = QrCode::new(text.as_bytes()).unwrap();
        let modules = code.width();
        let colors = code.to_colors();
        let (scale, border) = (4usize, 16usize);
        let w = modules * scale + border * 2;
        let h = w;

        let mut luma = vec![255u8; w * h];
        for my in 0..modules {
            for mx in 0..modules {
                if colors[my * modules + mx] == Color::Dark {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let px = border + mx * scale + dx;
                            let py = border + my * scale + dy;
                            luma[py * w + px] = 0;
                        }
                    }
                }
            }
        }
        bgr_from_luma(&luma, w, h)
    }

    /// A blank (all-white) BGR frame with no QR code.
    fn blank_frame(w: usize, h: usize) -> Mat {
        bgr_from_luma(&vec![255u8; w * h], w, h)
    }

    fn bgr_from_luma(luma: &[u8], w: usize, h: usize) -> Mat {
        let mut m =
            Mat::new_rows_cols_with_default(h as i32, w as i32, CV_8UC3, Scalar::all(0.0)).unwrap();
        let buf = m.data_bytes_mut().unwrap();
        for (i, &v) in luma.iter().enumerate() {
            buf[i * 3] = v;
            buf[i * 3 + 1] = v;
            buf[i * 3 + 2] = v;
        }
        m
    }

    /// A single-channel `Mat`; feeding it to `cvt_color(BGR2GRAY)` errors,
    /// which exercises the error path of the scanner.
    fn gray_frame(w: usize, h: usize) -> Mat {
        Mat::new_rows_cols_with_default(h as i32, w as i32, CV_8UC1, Scalar::all(128.0)).unwrap()
    }

    /// Scripted frame source for the iterator and loop tests.
    enum Step {
        Frame(Mat),
        Empty,
        ReadErr,
    }

    struct FakeReader {
        steps: VecDeque<Step>,
        grabs: VecDeque<bool>,
    }

    impl FakeReader {
        fn new(steps: Vec<Step>) -> Self {
            FakeReader {
                steps: steps.into(),
                grabs: VecDeque::new(),
            }
        }

        fn with_grabs(mut self, grabs: Vec<bool>) -> Self {
            self.grabs = grabs.into();
            self
        }
    }

    impl FrameReader for FakeReader {
        fn read(&mut self, frame: &mut Mat) -> Result<bool> {
            match self.steps.pop_front() {
                Some(Step::Frame(m)) => {
                    *frame = m;
                    Ok(true)
                }
                Some(Step::Empty) => {
                    *frame = Mat::default();
                    Ok(true)
                }
                Some(Step::ReadErr) => Err(opencv::Error::new(0, "read failed").into()),
                None => Ok(false),
            }
        }

        fn grab(&mut self) -> Result<bool> {
            Ok(self.grabs.pop_front().unwrap_or(false))
        }
    }

    struct FakePreview {
        keys: VecDeque<i32>,
        shown: usize,
    }

    impl Preview for FakePreview {
        fn show(&mut self, _frame: &Mat) -> Result<()> {
            self.shown += 1;
            Ok(())
        }

        fn wait_key(&mut self) -> Result<i32> {
            Ok(self.keys.pop_front().unwrap_or(KEY_CODE_ESC))
        }
    }

    const SAMPLE: &str = "V:3,TIME:12345,LATI:35.6895";

    #[test]
    fn builders_set_all_fields() {
        let reader = Reader::from_file("input.mp4")
            .headless(true)
            .frame_step(3)
            .radix(crate::Radix::new(64).unwrap());
        assert_eq!(reader.source, "input.mp4");
        assert!(reader.headless);
        assert_eq!(reader.step, 3);
        assert_eq!(reader.codec.radix().get(), 64);
    }

    #[test]
    fn frame_step_is_clamped_to_one() {
        assert_eq!(Reader::from_file("x").frame_step(0).step, 1);
    }

    #[test]
    fn scanner_finds_then_reuses_roi_then_resets() {
        let mut scanner = Scanner::new();

        // Slow path: locates the code and remembers its ROI.
        let text = scanner.scan(&qr_frame(SAMPLE)).unwrap();
        assert_eq!(text.as_deref(), Some(SAMPLE));
        assert!(scanner.roi.is_some());

        // Fast path: the remembered ROI still contains the code.
        let again = scanner.scan(&qr_frame(SAMPLE)).unwrap();
        assert_eq!(again.as_deref(), Some(SAMPLE));

        // ROI now empty and no code anywhere -> falls back, then clears the ROI.
        let none = scanner.scan(&blank_frame(160, 160)).unwrap();
        assert_eq!(none, None);
        assert!(scanner.roi.is_none());
    }

    #[test]
    fn scanner_handles_empty_and_invalid_frames() {
        let mut scanner = Scanner::new();
        // Zero-sized frame returns early.
        assert_eq!(scanner.scan(&Mat::default()).unwrap(), None);
        // Single-channel frame makes BGR2GRAY fail.
        assert!(scanner.scan(&gray_frame(32, 32)).is_err());
    }

    #[test]
    fn scan_mat_decodes_contiguous_qr() {
        let mut quirc = Quirc::default();
        let frame = qr_frame(SAMPLE);
        let mut gray = Mat::default();
        opencv::imgproc::cvt_color_def(&frame, &mut gray, opencv::imgproc::COLOR_BGR2GRAY).unwrap();
        let (text, _) = scan_mat(&mut quirc, &gray).expect("should decode");
        assert_eq!(text, SAMPLE);
    }

    #[test]
    fn decode_luma_covers_all_outcomes() {
        let mut quirc = Quirc::default();
        // Too-short buffer is rejected before scanning.
        assert!(decode_luma(&mut quirc, &[0u8; 4], 10, 10).is_none());
        // Blank buffer holds no QR code.
        assert!(decode_luma(&mut quirc, &vec![255u8; 64 * 64], 64, 64).is_none());

        // A real QR buffer decodes.
        let code = QrCode::new(SAMPLE.as_bytes()).unwrap();
        let modules = code.width();
        let colors = code.to_colors();
        let (scale, border) = (4usize, 16usize);
        let w = modules * scale + border * 2;
        let mut luma = vec![255u8; w * w];
        for my in 0..modules {
            for mx in 0..modules {
                if colors[my * modules + mx] == Color::Dark {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            luma[(border + my * scale + dy) * w + border + mx * scale + dx] = 0;
                        }
                    }
                }
            }
        }
        let (text, _corners) = decode_luma(&mut quirc, &luma, w, w).expect("should decode");
        assert_eq!(text, SAMPLE);
    }

    #[test]
    fn bounding_roi_normal_and_degenerate() {
        let pt = |x, y| quircs::Point { x, y };
        let corners = [pt(10, 10), pt(40, 10), pt(40, 40), pt(10, 40)];
        assert!(bounding_roi(&corners, 100, 100).is_some());

        // All corners collapsed at the far edge -> empty rectangle.
        let degenerate = [pt(100, 100); 4];
        assert!(bounding_roi(&degenerate, 100, 100).is_none());
    }

    #[test]
    fn record_iter_yields_hits_skips_misses_and_ends() {
        let reader = Reader::from_file("x");
        let source = FakeReader::new(vec![
            Step::Frame(qr_frame(SAMPLE)),
            Step::Frame(blank_frame(160, 160)),
            Step::Frame(qr_frame(SAMPLE)),
        ]);
        let mut iter = reader.record_iter(Box::new(source));

        // First hit.
        assert!(iter.next().unwrap().is_ok());
        // Skips the blank frame, then hits the next QR.
        assert!(iter.next().unwrap().is_ok());
        // End of stream.
        assert!(iter.next().is_none());
    }

    #[test]
    fn record_iter_propagates_read_and_scan_errors() {
        let reader = Reader::from_file("x");

        let mut read_err = reader.record_iter(Box::new(FakeReader::new(vec![Step::ReadErr])));
        assert!(read_err.next().unwrap().is_err());

        let mut scan_err =
            reader.record_iter(Box::new(FakeReader::new(vec![Step::Frame(gray_frame(32, 32))])));
        assert!(scan_err.next().unwrap().is_err());
    }

    #[test]
    fn record_iter_steps_with_grab() {
        let reader = Reader::from_file("x").frame_step(2);
        let source = FakeReader::new(vec![
            Step::Frame(qr_frame(SAMPLE)),
            Step::Frame(qr_frame(SAMPLE)),
        ])
        .with_grabs(vec![true]);
        let mut iter = reader.record_iter(Box::new(source));

        assert!(iter.next().unwrap().is_ok()); // first frame, no grab
        assert!(iter.next().unwrap().is_ok()); // grab() == true, then read
        assert!(iter.next().is_none()); // grab() == false -> stop
    }

    #[test]
    fn run_loop_headless_invokes_callback_and_respects_step() {
        let reader = Reader::from_file("x").frame_step(2);
        let source = FakeReader::new(vec![
            Step::Frame(qr_frame(SAMPLE)), // index 0: scanned -> hit
            Step::Frame(qr_frame(SAMPLE)), // index 1: skipped by step
            Step::Empty,                   // ends the loop
        ]);

        let mut hits = 0;
        reader
            .run_loop(Box::new(source), None, &mut |_record| hits += 1)
            .unwrap();
        assert_eq!(hits, 1);
    }

    #[test]
    fn run_loop_with_preview_shows_frames_and_breaks_on_esc() {
        let reader = Reader::from_file("x");
        let source = FakeReader::new(vec![
            Step::Frame(qr_frame(SAMPLE)),
            Step::Frame(blank_frame(160, 160)),
            Step::Frame(qr_frame(SAMPLE)), // never reached: Esc breaks first
        ]);
        let preview = FakePreview {
            keys: vec![1, KEY_CODE_ESC].into(),
            shown: 0,
        };

        let mut hits = 0;
        reader
            .run_loop(Box::new(source), Some(Box::new(preview)), &mut |_record| {
                hits += 1
            })
            .unwrap();
        // Two frames shown before Esc; one QR hit among them.
        assert_eq!(hits, 1);
    }
}
