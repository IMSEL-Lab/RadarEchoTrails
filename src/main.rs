use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use image::{ImageBuffer, Rgba, RgbaImage};
use rayon::prelude::*;

/// Simple color holder
#[derive(Clone, Copy, Debug)]
struct Color {
    r: u8,
    g: u8,
    b: u8,
}

impl Color {
    fn from_hex(s: &str) -> Result<Self> {
        let trimmed = s.trim_start_matches('#');
        if trimmed.len() != 6 {
            return Err(anyhow!("color must be 6 hex digits (e.g. #RRGGBB)"));
        }
        let r = u8::from_str_radix(&trimmed[0..2], 16)?;
        let g = u8::from_str_radix(&trimmed[2..4], 16)?;
        let b = u8::from_str_radix(&trimmed[4..6], 16)?;
        Ok(Color { r, g, b })
    }

    fn to_rgba(self, a: u8) -> Rgba<u8> {
        Rgba([self.r, self.g, self.b, a])
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about = "Overlay frames with fading history", long_about = None)]
struct Cli {
    /// Directory containing input frames (PNG recommended)
    #[arg(short, long, value_name = "DIR")]
    input_dir: PathBuf,

    /// Directory to write the composited frames
    #[arg(short, long, value_name = "DIR", default_value = "output_frames")]
    output_dir: PathBuf,

    /// Number of previous frames to keep visible (will fade to 0 by this age)
    #[arg(short = 'n', long, default_value_t = 5)]
    history_length: usize,

    /// Optional cap on number of frames to process (useful for quick tests)
    #[arg(long)]
    limit: Option<usize>,

    /// Number of worker threads (default: all logical cores)
    #[arg(short = 't', long)]
    threads: Option<usize>,

    /// Background color hex (#RRGGBB)
    #[arg(long, default_value = "#000000")]
    background: String,

    /// Current frame color hex (#RRGGBB)
    #[arg(long, default_value = "#00ff00")]
    current_color: String,

    /// History frame color hex (#RRGGBB)
    #[arg(long, default_value = "#ff7f00")]
    history_color: String,
}

fn main() -> Result<()> {
    let args = Cli::parse();

    if let Some(t) = args.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(t)
            .build_global()
            .context("failed to configure thread pool")?;
    }

    if args.history_length == 0 {
        return Err(anyhow!("history_length must be at least 1"));
    }

    let bg = Color::from_hex(&args.background)?;
    let current = Color::from_hex(&args.current_color)?;
    let history = Color::from_hex(&args.history_color)?;

    let mut entries: Vec<PathBuf> = fs::read_dir(&args.input_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| matches_ignore_case(ext, &["png", "jpg", "jpeg", "bmp", "tga", "gif"]))
                .unwrap_or(false)
        })
        .collect();
    entries.sort();

    if let Some(limit) = args.limit {
        entries.truncate(limit);
    }

    if entries.is_empty() {
        return Err(anyhow!("input directory is empty"));
    }

    fs::create_dir_all(&args.output_dir)?;

    // Load all frames once; assumes consistent dimensions.
    let frames: Vec<RgbaImage> = entries
        .iter()
        .map(|path| {
            image::open(path)
                .with_context(|| format!("failed to open {}", path.display()))?
                .to_rgba8()
                .pipe(Ok)
        })
        .collect::<Result<Vec<_>>>()?;

    let (width, height) = frames[0].dimensions();
    for (idx, frame) in frames.iter().enumerate() {
        if frame.dimensions() != (width, height) {
            return Err(anyhow!(
                "frame {} has different dimensions; all frames must match",
                entries[idx].display()
            ));
        }
    }

    let counter = AtomicUsize::new(0);

    frames
        .par_iter()
        .enumerate()
        .try_for_each(|(i, frame)| -> Result<()> {
            let mut canvas: RgbaImage = ImageBuffer::from_pixel(width, height, bg.to_rgba(255));

            let max_age = args.history_length.min(i);
            // Oldest first so newer history is on top.
            for age in (1..=max_age).rev() {
                let src = &frames[i - age];
                let fade = (args.history_length as f32 - age as f32) / args.history_length as f32;
                overlay_tinted(&mut canvas, src, history, fade.max(0.0));
            }

            // Current frame last, fully opaque where non-empty
            overlay_current(&mut canvas, frame, current);

            let out_name = entries[i]
                .file_name()
                .map(|n| n.to_owned())
                .ok_or_else(|| anyhow!("bad filename"))?;
            let mut out_path = args.output_dir.clone();
            out_path.push(out_name);
            image::save_buffer(&out_path, &canvas, width, height, image::ColorType::Rgba8)
                .with_context(|| format!("failed to save {}", out_path.display()))?;

            let done = counter.fetch_add(1, Ordering::Relaxed) + 1;
            if done % 25 == 0 || done == frames.len() {
                println!("processed {} / {}", done, frames.len());
            }
            Ok(())
        })?;

    println!(
        "done. wrote {} frames to {}",
        frames.len(),
        args.output_dir.display()
    );
    Ok(())
}

/// Overlay `src` onto `dst`, tinting to `color` and scaling alpha by `fade` (0.0-1.0).
fn overlay_tinted(dst: &mut RgbaImage, src: &RgbaImage, color: Color, fade: f32) {
    let (w, h) = dst.dimensions();
    for y in 0..h {
        for x in 0..w {
            let sp = src.get_pixel(x, y);
            let sa = sp[3] as f32 / 255.0;
            if sa == 0.0 {
                continue;
            }
            let alpha = (sa * fade).clamp(0.0, 1.0);
            if alpha <= 0.0 {
                continue;
            }
            let tinted = Rgba([color.r, color.g, color.b, (alpha * 255.0).round() as u8]);
            blend_pixel(dst.get_pixel_mut(x, y), tinted);
        }
    }
}

/// Overlay current frame: any non-transparent pixel becomes the current color at full opacity.
fn overlay_current(dst: &mut RgbaImage, src: &RgbaImage, color: Color) {
    let (w, h) = dst.dimensions();
    for y in 0..h {
        for x in 0..w {
            let sp = src.get_pixel(x, y);
            if sp[3] == 0 {
                continue;
            }
            let tinted = Rgba([color.r, color.g, color.b, 255]);
            blend_pixel(dst.get_pixel_mut(x, y), tinted);
        }
    }
}

/// Alpha blend `src` over `dst` (premultiplied-style math).
fn blend_pixel(dst: &mut Rgba<u8>, src: Rgba<u8>) {
    let da = dst[3] as f32 / 255.0;
    let sa = src[3] as f32 / 255.0;
    let out_a = sa + da * (1.0 - sa);

    let blend = |dc: u8, sc: u8| -> u8 {
        let dc = dc as f32 / 255.0;
        let sc = sc as f32 / 255.0;
        if out_a == 0.0 {
            0
        } else {
            (((sc * sa) + dc * da * (1.0 - sa)) / out_a * 255.0).round() as u8
        }
    };

    dst[0] = blend(dst[0], src[0]);
    dst[1] = blend(dst[1], src[1]);
    dst[2] = blend(dst[2], src[2]);
    dst[3] = (out_a * 255.0).round() as u8;
}

// Small helper to allow ? after map/pipe
trait Pipe: Sized {
    fn pipe<F, T>(self, f: F) -> T
    where
        F: FnOnce(Self) -> T,
    {
        f(self)
    }
}

impl<T> Pipe for T {}

fn matches_ignore_case(ext: &str, list: &[&str]) -> bool {
    list.iter().any(|e| e.eq_ignore_ascii_case(ext))
}
