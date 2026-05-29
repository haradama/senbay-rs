//! QR/OpenCV video reader (requires the `video` feature).

use opencv::core::{Mat, MatTraitConst, Rect};
use opencv::prelude::*;
use opencv::{highgui, imgproc, videoio};
use quircs::Quirc;

use crate::codec::Senbay;
use crate::error::Result;
use crate::record::Record;
use crate::{KEY_CODE_ESC, Radix};

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
    source: String,
    headless: bool,
    step: usize,
    codec: Senbay,
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

    /// Returns an iterator over the records decoded from the video.
    ///
    /// Each item is a [`Result`] so per-frame OpenCV errors surface instead of
    /// silently ending the stream. Consecutive frames often carry the same QR;
    /// the iterator yields one record per successful decode (deduplicate
    /// downstream if desired).
    pub fn records(&self) -> Result<RecordIter> {
        let capture = videoio::VideoCapture::from_file(&self.source, videoio::CAP_ANY)?;
        Ok(RecordIter {
            capture,
            frame: Mat::default(),
            scanner: Scanner::new(),
            codec: self.codec,
            step: self.step,
            started: false,
        })
    }

    /// Decodes the video, invoking `callback` with each record.
    ///
    /// Unless [`headless`](Reader::headless) was set, a preview window is shown
    /// and <kbd>Esc</kbd> stops playback. For headless, early-terminable use,
    /// prefer [`records`](Reader::records).
    pub fn for_each(&self, mut callback: impl FnMut(Record)) -> Result<()> {
        if self.headless {
            for record in self.records()? {
                callback(record?);
            }
            return Ok(());
        }

        const WINDOW: &str = "Senbay Reader";
        highgui::named_window(WINDOW, highgui::WINDOW_AUTOSIZE)?;

        let mut capture = videoio::VideoCapture::from_file(&self.source, videoio::CAP_ANY)?;
        let mut frame = Mat::default();
        let mut scanner = Scanner::new();
        let mut index = 0usize;
        loop {
            if !capture.read(&mut frame)? || frame.empty() {
                break;
            }
            if index.is_multiple_of(self.step)
                && let Some(text) = scanner.scan(&frame)?
            {
                callback(self.codec.decode(&text));
            }
            index += 1;

            highgui::imshow(WINDOW, &frame)?;
            if highgui::wait_key(1)? == KEY_CODE_ESC {
                break;
            }
        }
        Ok(())
    }
}

/// Iterator over the records decoded from a video, yielded one per QR decode.
///
/// Created by [`Reader::records`].
pub struct RecordIter {
    capture: videoio::VideoCapture,
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
                    if !self.capture.grab().unwrap_or(false) {
                        return None;
                    }
                }
            } else {
                self.started = true;
            }

            match self.capture.read(&mut self.frame) {
                Ok(true) if !self.frame.empty() => {}
                Ok(_) => return None,
                Err(err) => return Some(Err(err.into())),
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
        imgproc::cvt_color(frame, &mut self.gray, imgproc::COLOR_BGR2GRAY, 0)?;
        let (fw, fh) = (self.gray.cols(), self.gray.rows());
        if fw == 0 || fh == 0 {
            return Ok(None);
        }

        // Fast path: re-scan only the padded region around the last sighting.
        if let Some(rect) = self.roi {
            Mat::roi(&self.gray, rect)?.copy_to(&mut self.roi_buf)?;
            if let Some((text, _)) = decode(&mut self.quirc, &self.roi_buf) {
                return Ok(Some(text));
            }
        }

        // Slow path: scan the whole frame and remember where the code is.
        if let Some((text, corners)) = decode(&mut self.quirc, &self.gray) {
            self.roi = bounding_roi(&corners, fw, fh);
            return Ok(Some(text));
        }

        self.roi = None;
        Ok(None)
    }
}

/// Decodes the first QR code in a single-channel `Mat`, returning its text and
/// corner points (in that `Mat`'s coordinates).
fn decode(quirc: &mut Quirc, gray: &Mat) -> Option<(String, [quircs::Point; 4])> {
    let bytes = gray.data_bytes().ok()?;
    let (w, h) = (gray.cols() as usize, gray.rows() as usize);
    if bytes.len() < w * h {
        return None;
    }
    for code in quirc.identify(w, h, bytes) {
        let Ok(code) = code else { continue };
        if let Ok(data) = code.decode() {
            let text = String::from_utf8_lossy(&data.payload).into_owned();
            return Some((text, code.corners));
        }
    }
    None
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
