//! RadarEchoTrails processing logic
//!
//! Motion trail generation for radar image sequences

use std::fs;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};
use rayon::prelude::*;


use crate::queue::{self, FolderInfo};

#[derive(Clone)]
pub struct ProcessingSettings {
    pub history_length: usize,
    pub background_color: String,
    pub current_color: String,
    pub history_color: String,
    pub threads: usize,
    pub limit: Option<usize>,
}

#[derive(Debug)]
pub enum ProgressUpdate {
    FolderStarted { folder_index: usize, folder_name: String },
    FileProgress { 
        folder_index: usize, 
        files_done: usize, 
        files_total: usize,
        current_file: String,
        files_per_second: f64,
    },
    FolderCompleted { folder_index: usize },
    FolderError { folder_index: usize, error: String },
    AllComplete,
    Cancelled,
}

/// Parse a hex color string to RGB
fn parse_hex_color(hex: &str) -> Result<(u8, u8, u8)> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return Err(anyhow!("Invalid hex color: {}", hex));
    }
    
    let r = u8::from_str_radix(&hex[0..2], 16)?;
    let g = u8::from_str_radix(&hex[2..4], 16)?;
    let b = u8::from_str_radix(&hex[4..6], 16)?;
    
    Ok((r, g, b))
}

/// Process all folders in the queue
pub fn process_folders(
    folders: Vec<FolderInfo>,
    settings: ProcessingSettings,
    tx: Sender<ProgressUpdate>,
    stop_flag: Arc<AtomicBool>,
) {
    let threads = if settings.threads == 0 {
        num_cpus::get()
    } else {
        settings.threads
    };
    
    let pool = match rayon::ThreadPoolBuilder::new().num_threads(threads).build() {
        Ok(p) => p,
        Err(e) => {
            let _ = tx.send(ProgressUpdate::FolderError {
                folder_index: 0,
                error: format!("Failed to create thread pool: {}", e),
            });
            return;
        }
    };
    
    // Parse colors
    let background_rgb = parse_hex_color(&settings.background_color).unwrap_or((0, 0, 0));
    let current_rgb = parse_hex_color(&settings.current_color).unwrap_or((0, 255, 0));
    let history_rgb = parse_hex_color(&settings.history_color).unwrap_or((255, 127, 0));
    
    for (folder_idx, folder) in folders.iter().enumerate() {
        // Check stop flag
        if stop_flag.load(Ordering::Relaxed) {
            let _ = tx.send(ProgressUpdate::Cancelled);
            return;
        }
        
        let _ = tx.send(ProgressUpdate::FolderStarted {
            folder_index: folder_idx,
            folder_name: folder.name.clone(),
        });
        
        // Get image files
        let mut image_files = queue::get_image_files(&folder.path);
        
        // Apply limit if set
        if let Some(limit) = settings.limit {
            image_files.truncate(limit);
        }
        
        let files_total = image_files.len();
        
        if files_total == 0 {
            let _ = tx.send(ProgressUpdate::FolderError {
                folder_index: folder_idx,
                error: "No image files found".to_string(),
            });
            continue;
        }
        
        // Create output directory as sibling with _trail_N suffix
        let folder_name = folder.path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("output");
        let output_folder_name = format!("{}_trail_{}", folder_name, settings.history_length);
        let output_dir = folder.path.parent()
            .map(|p| p.join(&output_folder_name))
            .unwrap_or_else(|| folder.path.join("trails_output"));
        if let Err(e) = fs::create_dir_all(&output_dir) {
            let _ = tx.send(ProgressUpdate::FolderError {
                folder_index: folder_idx,
                error: format!("Failed to create output directory: {}", e),
            });
            continue;
        }
        
        // Pre-load images for history access
        // For efficiency, we process in order and maintain a sliding window
        let history_len = settings.history_length;
        let files_done = AtomicUsize::new(0);
        let start_time = Instant::now();
        let last_update = Mutex::new(Instant::now());
        let tx_clone = tx.clone();
        let stop_flag_clone = stop_flag.clone();
        
        // Process frames sequentially for history consistency, but parallelize compositing
        let results: Vec<Result<()>> = pool.install(|| {
            (0..files_total).into_par_iter().map(|frame_idx| -> Result<()> {
                // Check stop flag
                if stop_flag_clone.load(Ordering::Relaxed) {
                    return Ok(());
                }
                
                let current_path = &image_files[frame_idx];
                
                // Load current frame
                let current_img = image::open(current_path)
                    .with_context(|| format!("loading {}", current_path.display()))?;
                
                let (width, height) = current_img.dimensions();
                
                // Create output image with background
                let mut output = RgbaImage::from_pixel(
                    width, height,
                    Rgba([background_rgb.0, background_rgb.1, background_rgb.2, 255])
                );
                
                // Calculate history range
                let history_start = if frame_idx >= history_len {
                    frame_idx - history_len
                } else {
                    0
                };
                
                // Draw history frames (oldest to newest, with increasing opacity)
                let history_frames: Vec<_> = (history_start..frame_idx).collect();
                let history_count = history_frames.len();
                
                for (hist_idx, &frame_i) in history_frames.iter().enumerate() {
                    let hist_path = &image_files[frame_i];
                    if let Ok(hist_img) = image::open(hist_path) {
                        // Calculate fade: older = more transparent
                        let alpha = ((hist_idx + 1) as f32 / (history_count + 1) as f32 * 128.0) as u8;
                        overlay_tinted(&mut output, &hist_img, history_rgb, alpha);
                    }
                }
                
                // Draw current frame on top
                overlay_tinted(&mut output, &current_img, current_rgb, 255);
                
                // Save output
                let output_name = current_path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("frame.png");
                let output_path = output_dir.join(output_name);
                
                output.save(&output_path)
                    .with_context(|| format!("saving {}", output_path.display()))?;
                
                // Update progress
                let done = files_done.fetch_add(1, Ordering::Relaxed) + 1;
                
                // Only send updates every 100ms to avoid flooding
                let mut last = last_update.lock().unwrap();
                if last.elapsed().as_millis() >= 100 || done == files_total {
                    *last = Instant::now();
                    
                    let elapsed = start_time.elapsed().as_secs_f64();
                    let files_per_second = if elapsed > 0.0 { done as f64 / elapsed } else { 0.0 };
                    
                    let current_file = current_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_string();
                    
                    let _ = tx_clone.send(ProgressUpdate::FileProgress {
                        folder_index: folder_idx,
                        files_done: done,
                        files_total,
                        current_file,
                        files_per_second,
                    });
                }
                
                Ok(())
            }).collect()
        });
        
        // Check for errors
        let errors: Vec<_> = results.iter().filter_map(|r| r.as_ref().err()).collect();
        if !errors.is_empty() {
            let _ = tx.send(ProgressUpdate::FolderError {
                folder_index: folder_idx,
                error: format!("{} files failed to process", errors.len()),
            });
        } else {
            let _ = tx.send(ProgressUpdate::FolderCompleted { folder_index: folder_idx });
        }
    }
    
    let _ = tx.send(ProgressUpdate::AllComplete);
}

/// Overlay a tinted version of src onto dst
fn overlay_tinted(dst: &mut RgbaImage, src: &DynamicImage, tint: (u8, u8, u8), alpha: u8) {
    let src_rgba = src.to_rgba8();
    let (width, height) = src_rgba.dimensions();
    
    for y in 0..height.min(dst.height()) {
        for x in 0..width.min(dst.width()) {
            let src_pixel = src_rgba.get_pixel(x, y);
            
            // Skip fully transparent pixels
            if src_pixel[3] == 0 {
                continue;
            }
            
            // Convert to grayscale for intensity
            let intensity = (0.299 * src_pixel[0] as f32 
                          + 0.587 * src_pixel[1] as f32 
                          + 0.114 * src_pixel[2] as f32) / 255.0;
            
            // Apply tint based on intensity
            let r = (tint.0 as f32 * intensity) as u8;
            let g = (tint.1 as f32 * intensity) as u8;
            let b = (tint.2 as f32 * intensity) as u8;
            
            // Blend with alpha
            let src_alpha = ((src_pixel[3] as u32 * alpha as u32) / 255) as u8;
            
            if src_alpha > 0 {
                let dst_pixel = dst.get_pixel(x, y);
                let blend_alpha = src_alpha as f32 / 255.0;
                let inv_alpha = 1.0 - blend_alpha;
                
                let new_r = (r as f32 * blend_alpha + dst_pixel[0] as f32 * inv_alpha) as u8;
                let new_g = (g as f32 * blend_alpha + dst_pixel[1] as f32 * inv_alpha) as u8;
                let new_b = (b as f32 * blend_alpha + dst_pixel[2] as f32 * inv_alpha) as u8;
                
                dst.put_pixel(x, y, Rgba([new_r, new_g, new_b, 255]));
            }
        }
    }
}
