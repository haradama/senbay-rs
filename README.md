# senbay-rs

A Rust implementation of the **Senbay** format — compact text that packs sensor
data for embedding as QR codes in video. The API is designed around Rust idioms
(typed values, a builder-style record, structured errors).

## Design

| Type         | Role                                                        |
| ------------ | ----------------------------------------------------------- |
| `Value`      | A typed field value: `Int`, `Float`, or `Text`.            |
| `Record`     | An ordered set of fields with a builder-style API.          |
| `Senbay`     | The codec — encodes/decodes records.                        |
| `Encoding`   | Selects the plain (`V:3`) or compressed (`V:4`) form.       |
| `Radix`      | A validated positional notation (the numeric base).         |
| `Error`      | A structured error type; `Result<T> = Result<T, Error>`.    |
| `Reader` / `Writer` | QR/OpenCV video I/O (behind the `video` feature).    |

Design highlights:

- Field values are a typed `Value` enum rather than stringly-typed map entries.
- `Record` keeps fields sorted, so encoding is **deterministic**.
- Encoding an in-memory record is **infallible**; only radix validation and
  video I/O return `Result`.
- The reader exposes a lazy iterator (and a `FnMut(Record)` callback).

## Usage

```rust
use senbay_rs::{Encoding, Record, Senbay};

let codec = Senbay::new();

let mut record = Record::new();
record
    .set("TIME", 1_700_000_000_000_i64)
    .set("LATI", 35.6895)
    .set("LONG", 139.6917)
    .set("MEMO", "hello");

// Compressed (V:4) or Plain (V:3).
let text = codec.encode(&record, Encoding::Compressed);

let decoded = codec.decode(&text);
assert_eq!(decoded.get("LATI").unwrap().as_f64(), Some(35.6895));
assert_eq!(decoded.get("MEMO").unwrap().as_str(), Some("hello"));

// JSON output (numbers and strings).
println!("{}", decoded.to_json());
```

Numbers are written in a custom positional notation (`Radix`, base `2..=122`;
121 is the canonical value) whose digits map onto a curated set of code points.

## The `video` feature

```sh
cargo build --features video
```

Pulls in [`opencv`](https://crates.io/crates/opencv) (camera/video I/O +
display), [`rqrr`](https://crates.io/crates/rqrr) (QR decode),
[`qrcode`](https://crates.io/crates/qrcode) (QR encode) and
[`image`](https://crates.io/crates/image) (grayscale bridge).

```rust
# #[cfg(feature = "video")]
# fn demo() -> senbay_rs::Result<()> {
use senbay_rs::{Reader, Writer};

// Read records from QR codes in a video file. `records()` is a lazy iterator,
// so you can stop early with `take`/`break`.
for record in Reader::from_file("input.mp4").records()?.take(10) {
    println!("{}", record?.to_json());
}

// Write a camera stream, stamping each frame with the current time.
Writer::new("out.avi").run_timestamps()?;
# Ok(())
# }
```

### Reader performance

The reader decodes QR codes with [`quircs`](https://crates.io/crates/quircs)
and tracks the code's location: once found, only a padded region around it is
re-scanned on later frames, falling back to the whole frame on a miss. The
`quircs` instance and scratch buffers are reused across frames.

On a ~2-minute 720p clip this brought QR detection down to ~3 ms/frame — at
which point **video decoding, not QR work, is the bottleneck** (libvpx decodes
this clip at ~140 fps ≈ 7 ms/frame). End to end, the full clip runs in ~24 s —
essentially the raw decode time.

Two knobs help headless use:

- `Reader::records()` is a **lazy iterator**, so `take(n)` / `break` stops
  immediately instead of scanning the whole file. Reading the first 20 records
  drops from ~76 s to under 1 s.
- `Reader::frame_step(n)` samples every *n*-th frame, skipping QR detection on
  the rest. Since the run is decode-bound, this mainly saves CPU rather than
  wall-clock (skipped frames must still be decoded); raise it only if detection
  is your bottleneck (e.g. higher-resolution frames).

```rust
# #[cfg(feature = "video")]
# fn demo() -> senbay_rs::Result<()> {
# use senbay_rs::Reader;
for record in Reader::from_file("input.mp4").frame_step(2).records()? {
    println!("{}", record?.to_json());
}
# Ok(())
# }
```

### Build prerequisites (video only)

Building the `opencv` crate needs system prerequisites the default build does
not:

- A system OpenCV install (e.g. `libopencv-dev` on Debian/Ubuntu).
- The `clang` toolchain (driver binary + builtin headers), e.g. `sudo apt install clang`.

If `clang-sys` cannot find `libclang.so` (the `clang` package ships only the
versioned `libclang-NN.so.NN`, while the bare `libclang.so` symlink lives in
`libclang-NN-dev`), either install the dev package or point `LIBCLANG_PATH` at a
directory containing a `libclang.so`:

```sh
# option A: install the dev package that provides the libclang.so symlink
sudo apt install libclang-18-dev

# option B: point LIBCLANG_PATH at a libclang.so symlink you create
mkdir -p ~/.local/libclang
ln -sf /usr/lib/llvm-18/lib/libclang-18.so.18 ~/.local/libclang/libclang.so
LIBCLANG_PATH=~/.local/libclang cargo build --features video
```

## Testing

```sh
cargo test                      # core codec tests (no native deps)
cargo test --features video     # also builds/tests the QR + OpenCV path
```
