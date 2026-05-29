//! Reads a Senbay video and prints each decoded record as JSON to stdout.
//!
//! Runs headless (no preview window) and stops as soon as the requested number
//! of records has been printed.
//!
//! ```sh
//! cargo run --release --features video --example read_webm -- senbay.webm
//! # cap output to 20 records, sampling every 2nd frame:
//! cargo run --release --features video --example read_webm -- senbay.webm 20 2
//! ```
//!
//! Arguments: `<path> [max-records] [frame-step]`.

use std::process::ExitCode;

use senbay_rs::Reader;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "senbay.webm".to_owned());
    let limit: Option<usize> = args.next().and_then(|s| s.parse().ok());
    let step: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(2);

    eprintln!("reading {path} (headless, frame-step {step})...");

    let iter = match Reader::from_file(&path).frame_step(step).records() {
        Ok(iter) => iter,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    let mut count = 0usize;
    let mut last = String::new();

    for record in iter {
        let record = match record {
            Ok(record) => record,
            Err(err) => {
                eprintln!("error: {err}");
                return ExitCode::FAILURE;
            }
        };

        // The same QR persists across frames; collapse consecutive repeats.
        let json = record.to_json();
        if json == last {
            continue;
        }
        println!("{json}");
        last = json;
        count += 1;

        if limit == Some(count) {
            break;
        }
    }

    eprintln!("done: {count} record(s) printed");
    ExitCode::SUCCESS
}
