# Frame Trails

Create motion trails by compositing sequential image frames. Each output frame is the current frame drawn over a fading stack of its recent predecessors, which makes motion paths visible without video editing software.

## What it does
- Reads a directory of image frames (PNG recommended; JPG/JPEG/BMP/TGA/GIF also supported).
- Verifies every frame has identical dimensions.
- For each frame `i`, draws the previous `n` frames with a tinted, linearly fading alpha, then overlays frame `i` in a solid tint.
- Writes composited frames to an output directory.

## Install
```bash
cargo install --path .
# or from the repo after cloning:
cargo install --locked --path .
```
Requires Rust 1.82+ (edition 2024).

## Usage
```bash
frame-trails \
  --input-dir input_frames \
  --output-dir output_frames \
  --history-length 8 \
  --background "#000000" \
  --current-color "#00ff00" \
  --history-color "#ff7f00" \
  --threads 8
```

### CLI options
- `-i, --input-dir <DIR>`: Directory containing the input frames. Must not be empty.
- `-o, --output-dir <DIR>`: Where composited frames are written. Default: `output_frames` (auto-created).
- `-n, --history-length <N>`: How many previous frames to include, fading to zero at age `N`. Minimum 1. Default: `5`.
- `--limit <N>`: Optional cap on how many frames to process (useful for quick tests).
- `-t, --threads <N>`: Worker threads. Default is all logical cores.
- `--background <#RRGGBB>`: Background color. Default `#000000`.
- `--current-color <#RRGGBB>`: Tint applied to the current frame (fully opaque where pixels are non-transparent). Default `#00ff00`.
- `--history-color <#RRGGBB>`: Tint applied to historical frames (alpha fades with age). Default `#ff7f00`.

### Input expectations
- All files in `--input-dir` with extensions `png`, `jpg`, `jpeg`, `bmp`, `tga`, `gif` (case-insensitive) are processed.
- Every frame **must** share identical width and height; mismatches abort the run.
- Source frames should have an alpha channel if you want transparent backgrounds in the composition; otherwise opaque pixels are treated as fully solid.

### Output
- For each input frame `frame_0001.png`, the program writes a composited image with the same filename into `--output-dir`.
- Progress is printed every 25 frames and on completion.

## Examples
Fade the last 10 frames with teal trails and a dark gray background:
```bash
frame-trails -i footage/png -o trails -n 10 \
  --background "#111111" \
  --current-color "#00d1b2" \
  --history-color "#30bced"
```

Process only the first 120 frames using 4 threads:
```bash
frame-trails -i footage/png --limit 120 --threads 4
```

## How the fading works
- For frame `i`, frames `i-1` … `i-n` are drawn oldest-to-newest.
- The alpha for a historical frame of age `a` (1 = previous frame) is scaled by `(history_length - a) / history_length`.
- Colors are first tinted, then alpha-blended over the background. The current frame is drawn last, fully opaque wherever its source pixel alpha is non-zero.

## Troubleshooting
- **"input directory is empty"**: Check the path or supported extensions.
- **"frame X has different dimensions"**: Ensure all frames were exported at the same resolution.
- **Performance**: Reduce `--history-length` or set `--threads` to a lower number to limit CPU usage.

## License
MIT
