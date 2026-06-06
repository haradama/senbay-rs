//! Real OpenCV-backed implementations of the injectable video I/O traits, plus
//! the public [`Reader`]/[`Writer`] entry points that wire them up.
//!
//! Everything here is a thin shim over a live camera, a video file, or a
//! highgui window: it cannot run without hardware/display, so it is excluded
//! from coverage (`--ignore-filename-regex 'video_backend\.rs'`). All decoding
//! and encoding *logic* lives in `reader.rs`/`writer.rs` and is fully tested.

use opencv::core::{Mat, Size};
use opencv::prelude::*;
use opencv::{highgui, videoio};

use crate::error::Result;
use crate::reader::{FrameReader, Preview, Reader, RecordIter};
use crate::record::Record;
use crate::writer::{Camera, VideoSink, Writer, fourcc, now_millis};

/// Frame source backed by an OpenCV [`VideoCapture`](videoio::VideoCapture).
struct CaptureReader(videoio::VideoCapture);

impl FrameReader for CaptureReader {
    fn read(&mut self, frame: &mut Mat) -> Result<bool> {
        Ok(self.0.read(frame)?)
    }

    fn grab(&mut self) -> Result<bool> {
        Ok(self.0.grab()?)
    }
}

/// Camera backed by an OpenCV [`VideoCapture`](videoio::VideoCapture).
struct CaptureCamera(videoio::VideoCapture);

impl Camera for CaptureCamera {
    fn is_opened(&self) -> Result<bool> {
        Ok(self.0.is_opened()?)
    }

    fn read(&mut self, frame: &mut Mat) -> Result<bool> {
        Ok(self.0.read(frame)?)
    }
}

/// Video sink backed by an OpenCV [`VideoWriter`](videoio::VideoWriter), opened
/// lazily once the frame size is known.
struct CaptureSink {
    output: String,
    fourcc: i32,
    fps: f64,
    writer: Option<videoio::VideoWriter>,
}

impl VideoSink for CaptureSink {
    fn open(&mut self, size: Size) -> Result<()> {
        self.writer = Some(videoio::VideoWriter::new(
            &self.output,
            self.fourcc,
            self.fps,
            size,
            true,
        )?);
        Ok(())
    }

    fn write(&mut self, frame: &Mat) -> Result<()> {
        if let Some(writer) = self.writer.as_mut() {
            writer.write(frame)?;
        }
        Ok(())
    }
}

/// Preview window backed by OpenCV highgui. `delay_ms` is how long each frame
/// is shown (the `wait_key` timeout), which paces playback to the source's
/// frame rate instead of running as fast as frames can be decoded.
struct Window {
    name: &'static str,
    delay_ms: i32,
}

impl Window {
    fn new(name: &'static str, delay_ms: i32) -> Result<Self> {
        highgui::named_window(name, highgui::WINDOW_AUTOSIZE)?;
        // wait_key(0) blocks until a key is pressed, so never let the pace hit 0.
        Ok(Window { name, delay_ms: delay_ms.max(1) })
    }
}

impl Preview for Window {
    fn show(&mut self, frame: &Mat) -> Result<()> {
        highgui::imshow(self.name, frame)?;
        Ok(())
    }

    fn wait_key(&mut self) -> Result<i32> {
        Ok(highgui::wait_key(self.delay_ms)?)
    }
}

/// Opens a video file, returning a clear error if it could not be opened
/// (a missing path or unsupported codec otherwise yields a silent empty stream).
fn open_file(source: &str) -> Result<videoio::VideoCapture> {
    let capture = videoio::VideoCapture::from_file(source, videoio::CAP_ANY)?;
    if !capture.is_opened()? {
        return Err(opencv::Error::new(0, format!("cannot open video file: {source}")).into());
    }
    Ok(capture)
}

/// Per-frame preview delay in milliseconds: the capture's frame interval (from
/// `CAP_PROP_FPS`, falling back to ~30 fps) divided by the playback `speed`.
fn frame_delay_ms(capture: &videoio::VideoCapture, speed: f64) -> i32 {
    let fps = capture.get(videoio::CAP_PROP_FPS).unwrap_or(0.0);
    let interval = if fps.is_finite() && fps > 1.0 { 1000.0 / fps } else { 33.0 };
    let speed = if speed > 0.0 { speed } else { 1.0 };
    (interval / speed).round().max(1.0) as i32
}

impl Reader {
    /// Returns an iterator over the records decoded from the video.
    ///
    /// Each item is a [`Result`] so per-frame OpenCV errors surface instead of
    /// silently ending the stream. Consecutive frames often carry the same QR;
    /// the iterator yields one record per successful decode (deduplicate
    /// downstream if desired).
    pub fn records(&self) -> Result<RecordIter> {
        let capture = open_file(&self.source)?;
        Ok(self.record_iter(Box::new(CaptureReader(capture))))
    }

    /// Decodes the video, invoking `callback` with each record.
    ///
    /// Unless [`headless`](Reader::headless) was set, a preview window is shown
    /// and <kbd>Esc</kbd> stops playback. For headless, early-terminable use,
    /// prefer [`records`](Reader::records).
    pub fn for_each(&self, mut callback: impl FnMut(Record)) -> Result<()> {
        let capture = open_file(&self.source)?;
        // Pace the window to the video's frame rate (read before moving the
        // capture into the reader) so playback runs in real time.
        let preview: Option<Box<dyn Preview>> = if self.headless {
            None
        } else {
            let delay = frame_delay_ms(&capture, self.playback_speed);
            Some(Box::new(Window::new("Senbay Reader", delay)?))
        };
        let source: Box<dyn FrameReader> = Box::new(CaptureReader(capture));
        self.run_loop(source, preview, &mut callback)
    }
}

impl Writer {
    /// Captures frames until <kbd>Esc</kbd>, embedding the record produced by
    /// `next_record` into each one.
    pub fn run(&self, next_record: impl FnMut() -> Record) -> Result<()> {
        // The camera supplies frames at its own rate, so don't add extra delay.
        let window = Window::new("Senbay Writer", 1)?;
        let camera = videoio::VideoCapture::new(self.camera, videoio::CAP_ANY)?;
        let sink = CaptureSink {
            output: self.output.clone(),
            fourcc: fourcc(&self.codec_fourcc)?,
            fps: self.fps,
            writer: None,
        };

        self.run_core(
            Box::new(CaptureCamera(camera)),
            Box::new(sink),
            Box::new(window),
            next_record,
        )
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
