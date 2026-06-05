//! Reads a Senbay video and prints each decoded record as JSON to stdout.
//!
//! By default it runs headless (no preview window) and stops as soon as the
//! requested number of records has been printed. Pass `--preview` to open an
//! OpenCV window showing the video while it decodes; press <kbd>Esc</kbd> to stop.
//!
//! ```sh
//! cargo run --release --features video --example read_webm -- senbay.webm
//! # cap output to 20 records, sampling every 2nd frame:
//! cargo run --release --features video --example read_webm -- senbay.webm 20 2
//! # show a preview window (Esc to quit):
//! cargo run --release --features video --example read_webm -- --preview senbay.webm
//! ```
//!
//! Arguments: `[--preview] <path> [max-records] [frame-step]`.
//! In `--preview` mode playback is driven by the window, so `max-records` is not
//! enforced — stop with <kbd>Esc</kbd>.

use std::process::ExitCode;

use senbay_rs::{Reader, Record};

/// Tracks dedup/printing state shared by both the headless and preview drivers.
#[derive(Default)]
struct Printer {
    count: usize,
    last: String,
}

impl Printer {
    /// Prints the record as JSON unless it repeats the previous one (the same QR
    /// persists across frames). Returns `true` when a new record was printed.
    fn emit(&mut self, record: &Record) -> bool {
        let json = record.to_json();
        if json == self.last {
            return false;
        }
        println!("{json}");
        self.last = json;
        self.count += 1;
        true
    }
}

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let preview = raw.iter().any(|a| a == "--preview" || a == "-p");

    // Positional args are everything that is not a flag.
    let mut positional = raw.iter().filter(|a| !a.starts_with('-'));
    let path = positional.next().cloned().unwrap_or_else(|| "senbay.webm".to_owned());
    let limit: Option<usize> = positional.next().and_then(|s| s.parse().ok());
    let step: usize = positional.next().and_then(|s| s.parse().ok()).unwrap_or(2);

    let reader = Reader::from_file(&path).frame_step(step);
    let mut printer = Printer::default();

    let result = if preview {
        eprintln!("reading {path} (preview window, frame-step {step}); press Esc to stop...");
        reader.headless(false).for_each(|record| {
            printer.emit(&record);
        })
    } else {
        eprintln!("reading {path} (headless, frame-step {step})...");
        run_headless(&reader, &mut printer, limit)
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        return ExitCode::FAILURE;
    }

    eprintln!("done: {} record(s) printed", printer.count);
    ExitCode::SUCCESS
}

/// Headless driver: iterates records so it can stop early once `limit` is hit.
fn run_headless(reader: &Reader, printer: &mut Printer, limit: Option<usize>) -> senbay_rs::Result<()> {
    for record in reader.records()? {
        printer.emit(&record?);
        if limit == Some(printer.count) {
            break;
        }
    }
    Ok(())
}
