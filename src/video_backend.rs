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

/// Preview window backed by OpenCV highgui.
struct Window(&'static str);

impl Window {
    fn new(name: &'static str) -> Result<Self> {
        highgui::named_window(name, highgui::WINDOW_AUTOSIZE)?;
        Ok(Window(name))
    }
}

impl Preview for Window {
    fn show(&mut self, frame: &Mat) -> Result<()> {
        highgui::imshow(self.0, frame)?;
        Ok(())
    }

    fn wait_key(&mut self) -> Result<i32> {
        Ok(highgui::wait_key(1)?)
    }
}

impl Reader {
    /// Returns an iterator over the records decoded from the video.
    ///
    /// Each item is a [`Result`] so per-frame OpenCV errors surface instead of
    /// silently ending the stream. Consecutive frames often carry the same QR;
    /// the iterator yields one record per successful decode (deduplicate
    /// downstream if desired).
    pub fn records(&self) -> Result<RecordIter> {
        let capture = videoio::VideoCapture::from_file(&self.source, videoio::CAP_ANY)?;
        Ok(self.record_iter(Box::new(CaptureReader(capture))))
    }

    /// Decodes the video, invoking `callback` with each record.
    ///
    /// Unless [`headless`](Reader::headless) was set, a preview window is shown
    /// and <kbd>Esc</kbd> stops playback. For headless, early-terminable use,
    /// prefer [`records`](Reader::records).
    pub fn for_each(&self, mut callback: impl FnMut(Record)) -> Result<()> {
        let capture = videoio::VideoCapture::from_file(&self.source, videoio::CAP_ANY)?;
        let source: Box<dyn FrameReader> = Box::new(CaptureReader(capture));
        let preview: Option<Box<dyn Preview>> = if self.headless {
            None
        } else {
            Some(Box::new(Window::new("Senbay Reader")?))
        };
        self.run_loop(source, preview, &mut callback)
    }
}

impl Writer {
    /// Captures frames until <kbd>Esc</kbd>, embedding the record produced by
    /// `next_record` into each one.
    pub fn run(&self, next_record: impl FnMut() -> Record) -> Result<()> {
        let window = Window::new("Senbay Writer")?;
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
