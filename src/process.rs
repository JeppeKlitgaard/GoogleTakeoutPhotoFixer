use crate::archive::{ArchiveFile, Takeout};
use crate::metadata::{apply_google_metadata, MetadataError};
use flate2::read::GzDecoder;
use indicatif::{ProgressBar, ProgressStyle};
use little_exif::filetype::FileExtension;
use little_exif::metadata::Metadata;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use tar::Archive as TarArchive;
use zip::ZipArchive;

/// Error type for processing operations
#[derive(Debug)]
pub enum ProcessError {
    IoError(String),
    ArchiveError(String),
    MetadataError(MetadataError),
    ExifError(String),
}

impl std::fmt::Display for ProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcessError::IoError(msg) => write!(f, "IO error: {}", msg),
            ProcessError::ArchiveError(msg) => write!(f, "Archive error: {}", msg),
            ProcessError::MetadataError(e) => write!(f, "Metadata error: {}", e),
            ProcessError::ExifError(msg) => write!(f, "EXIF error: {}", msg),
        }
    }
}

impl std::error::Error for ProcessError {}

impl From<MetadataError> for ProcessError {
    fn from(e: MetadataError) -> Self {
        ProcessError::MetadataError(e)
    }
}

/// Statistics for the processing operation
#[derive(Debug, Default)]
pub struct ProcessStats {
    pub images_processed: usize,
    pub images_skipped: usize,
    pub metadata_applied: usize,
    pub unused_metadata_files: usize,
    pub media_copied_without_metadata: usize,
    pub images_processed_with_metadata: usize,
    pub images_processed_without_metadata: usize,
    pub videos_copied: usize,
    pub errors: usize,
}

struct ArchiveCache {
    zip_archives: HashMap<PathBuf, ZipArchive<BufReader<File>>>,
}

impl ArchiveCache {
    fn new() -> Self {
        Self {
            zip_archives: HashMap::new(),
        }
    }
}

/// Image file extensions we support
const IMAGE_EXTENSIONS: &[&str] = &[
    ".jpg", ".jpeg", ".png", ".gif", ".webp", ".heic", ".heif", ".tiff", ".tif", ".bmp",
];

/// Video file extensions (we copy but don't modify EXIF)
const VIDEO_EXTENSIONS: &[&str] = &[
    ".mp4", ".mov", ".avi", ".mkv", ".webm", ".m4v", ".3gp", ".wmv",
];

/// Check if a file is an image based on extension
fn is_image_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    IMAGE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

/// Check if a file is a video based on extension
fn is_video_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    VIDEO_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

/// Check if a file is a media file (image or video)
fn is_media_file(path: &str) -> bool {
    is_image_file(path) || is_video_file(path)
}

/// Extracts the album path from an archive path
/// e.g., "Takeout/Google Photos/Album Name/photo.jpg" -> "Album Name"
fn extract_album_path(archive_path: &str, photo_path_prefix: &str) -> String {
    let relative = archive_path
        .strip_prefix(photo_path_prefix)
        .unwrap_or(archive_path);

    // Get the directory part (everything before the last /)
    if let Some(last_slash) = relative.rfind('/') {
        relative[..last_slash].to_string()
    } else {
        String::new()
    }
}

fn read_zip_file_cached(cache: &mut ArchiveCache, file: &ArchiveFile) -> Result<Vec<u8>, ProcessError> {
    let archive_name = file
        .source_archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    if archive_name.ends_with(".zip") {
        if !cache.zip_archives.contains_key(&file.source_archive) {
            let archive_file = File::open(&file.source_archive)
                .map_err(|e| ProcessError::IoError(format!("Failed to open archive: {}", e)))?;
            let reader = BufReader::new(archive_file);
            let archive = ZipArchive::new(reader)
                .map_err(|e| ProcessError::ArchiveError(format!("Failed to read zip: {}", e)))?;
            cache.zip_archives.insert(file.source_archive.clone(), archive);
        }

        let archive = cache
            .zip_archives
            .get_mut(&file.source_archive)
            .ok_or_else(|| ProcessError::ArchiveError("Zip cache missing".to_string()))?;
        let mut entry = archive
            .by_index(file.index)
            .map_err(|e| ProcessError::ArchiveError(format!("Failed to read entry: {}", e)))?;
        let mut contents = Vec::new();
        entry
            .read_to_end(&mut contents)
            .map_err(|e| ProcessError::IoError(format!("Failed to read file contents: {}", e)))?;
        Ok(contents)
    } else {
        Err(ProcessError::ArchiveError(format!(
            "Unsupported archive format: {}",
            archive_name
        )))
    }
}

/// Get the FileExtension for a file based on its path
fn get_file_extension(path: &str) -> FileExtension {
    let lower = path.to_lowercase();
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        FileExtension::JPEG
    } else if lower.ends_with(".png") {
        FileExtension::PNG { as_zTXt_chunk: false }
    } else if lower.ends_with(".webp") {
        FileExtension::WEBP
    } else if lower.ends_with(".jxl") {
        FileExtension::JXL
    } else if lower.ends_with(".tiff") || lower.ends_with(".tif") {
        FileExtension::TIFF
    } else if lower.ends_with(".heic") || lower.ends_with(".heif") {
        FileExtension::HEIF
    } else {
        // Default to JPEG for unknown types
        FileExtension::JPEG
    }
}

/// Process a single image file: read it, apply metadata, write to output
fn process_image_data(
    image_path: &str,
    image_data: Vec<u8>,
    metadata_json: Option<&str>,
    output_path: &Path,
    debug: bool,
) -> Result<bool, ProcessError> {

    // Determine file extension for little_exif
    let file_ext = get_file_extension(image_path);

    // Try to read existing EXIF metadata from the image
    let metadata = match Metadata::new_from_vec(&image_data, file_ext.clone()) {
        Ok(m) => m,
        Err(_) => {
            // No existing metadata, create empty
            Metadata::new()
        }
    };

    // Apply Google metadata if available
    let final_metadata = if let Some(json_str) = metadata_json {
        if debug {
            println!("    Applying metadata from JSON");
        }

        apply_google_metadata(json_str, metadata)?
    } else {
        metadata
    };

    // Create parent directories
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| ProcessError::IoError(format!("Failed to create directory: {}", e)))?;
    }

    // Write the image with updated metadata
    // First write the original image data
    let mut output_file = File::create(output_path)
        .map_err(|e| ProcessError::IoError(format!("Failed to create output file: {}", e)))?;
    output_file
        .write_all(&image_data)
        .map_err(|e| ProcessError::IoError(format!("Failed to write image data: {}", e)))?;
    drop(output_file);

    // Then write the metadata to the file
    if let Err(e) = final_metadata.write_to_file(output_path) {
        if debug {
            println!("    Warning: Could not write EXIF metadata: {}", e);
        }
        // Don't fail the whole process, just note the warning
    }

    Ok(metadata_json.is_some())
}

/// Copy a file without modification (for videos, etc.)
fn copy_file_data(data: Vec<u8>, output_path: &Path, _debug: bool) -> Result<(), ProcessError> {

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| ProcessError::IoError(format!("Failed to create directory: {}", e)))?;
    }

    let mut output_file = File::create(output_path)
        .map_err(|e| ProcessError::IoError(format!("Failed to create output file: {}", e)))?;
    output_file
        .write_all(&data)
        .map_err(|e| ProcessError::IoError(format!("Failed to write file data: {}", e)))?;

    Ok(())
}

fn is_zip_archive(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.ends_with(".zip"))
        .unwrap_or(false)
}

fn is_tar_gz_archive(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.ends_with(".tar.gz"))
        .unwrap_or(false)
}

fn build_metadata_cache(
    takeout: &Takeout,
    archive_cache: &mut ArchiveCache,
) -> Result<HashMap<String, String>, ProcessError> {
    let mut metadata_map = HashMap::new();
    let mut tar_metadata_by_archive: HashMap<PathBuf, HashSet<String>> = HashMap::new();

    for meta in takeout.supplemental_metadata_files() {
        if is_tar_gz_archive(&meta.source_archive) {
            tar_metadata_by_archive
                .entry(meta.source_archive.clone())
                .or_default()
                .insert(meta.archive_path.clone());
        } else if is_zip_archive(&meta.source_archive) {
            let json_data = read_zip_file_cached(archive_cache, meta)?;
            let json_str = String::from_utf8(json_data)
                .map_err(|e| ProcessError::IoError(format!("Invalid UTF-8 in metadata: {}", e)))?;
            metadata_map.insert(meta.archive_path.clone(), json_str);
        }
    }

    for (archive_path, wanted_paths) in tar_metadata_by_archive {
        let file = File::open(&archive_path)
            .map_err(|e| ProcessError::IoError(format!("Failed to open archive: {}", e)))?;
        let reader = BufReader::new(file);
        let decoder = GzDecoder::new(reader);
        let mut archive = TarArchive::new(decoder);

        let entries = archive
            .entries()
            .map_err(|e| ProcessError::ArchiveError(format!("Failed to read tar entries: {}", e)))?;

        for entry in entries {
            let mut entry = entry
                .map_err(|e| ProcessError::ArchiveError(format!("Failed to read entry: {}", e)))?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let entry_path = entry
                .path()
                .map_err(|e| ProcessError::ArchiveError(format!("Failed to get path: {}", e)))?;
            let entry_path_str = entry_path.to_string_lossy().to_string();

            if wanted_paths.contains(&entry_path_str) {
                let mut contents = Vec::new();
                entry
                    .read_to_end(&mut contents)
                    .map_err(|e| ProcessError::IoError(format!("Failed to read contents: {}", e)))?;
                let json_str = String::from_utf8(contents).map_err(|e| {
                    ProcessError::IoError(format!("Invalid UTF-8 in metadata: {}", e))
                })?;
                metadata_map.insert(entry_path_str, json_str);
            } else {
                std::io::copy(&mut entry, &mut std::io::sink())
                    .map_err(|e| ProcessError::IoError(format!("Failed to skip entry: {}", e)))?;
            }
        }
    }

    Ok(metadata_map)
}
/// Process all files in the takeout and output to the specified directory
pub fn process_takeout(
    takeout: &Takeout,
    output_dir: &Path,
    photo_path_prefix: &str,
    dry_run: bool,
    debug: bool,
    show_progress: bool,
) -> Result<ProcessStats, ProcessError> {
    let mut stats = ProcessStats::default();
    let mut used_metadata = HashSet::new();
    let mut archive_cache = ArchiveCache::new();

    let metadata_cache = build_metadata_cache(takeout, &mut archive_cache)?;

    // Collect all media files (non-metadata files)
    let media_files: Vec<_> = takeout
        .files()
        .filter(|f| is_media_file(&f.archive_path))
        .collect();

    println!("\nProcessing {} media files...", media_files.len());

    let progress = if show_progress {
        let pb = ProgressBar::new(media_files.len() as u64);
        let style = ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}<{eta_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}",
        )
        .unwrap_or_else(|_| ProgressStyle::default_bar());
        pb.set_style(style);
        Some(pb)
    } else {
        None
    };

    let zip_media_files: Vec<_> = media_files
        .iter()
        .filter(|f| is_zip_archive(&f.source_archive))
        .collect();

    for file in zip_media_files {
        let file_name = file.file_name();
        let album = extract_album_path(&file.archive_path, photo_path_prefix);
        let output_path = output_dir.join(&album).join(file_name);

        if debug {
            if let Some(pb) = progress.as_ref() {
                pb.println(format!("  Processing: {}/{}", album, file_name));
            } else {
                println!("  Processing: {}/{}", album, file_name);
            }
        }

        // Find associated metadata
        let metadata_file = takeout.find_metadata_for(&file.archive_path);
        if let Some(meta_file) = metadata_file {
            used_metadata.insert(meta_file.archive_path.clone());
        }

        let metadata_json = metadata_file
            .and_then(|meta| metadata_cache.get(&meta.archive_path))
            .map(|s| s.as_str());

        if dry_run {
            if metadata_file.is_some() {
                if let Some(pb) = progress.as_ref() {
                    pb.println(format!(
                        "  [DRY RUN] Would process: {} -> {}",
                        file.archive_path,
                        output_path.display()
                    ));
                } else {
                    println!(
                        "  [DRY RUN] Would process: {} -> {}",
                        file.archive_path,
                        output_path.display()
                    );
                }
                stats.metadata_applied += 1;
                if is_image_file(&file.archive_path) {
                    stats.images_processed_with_metadata += 1;
                }
            } else {
                if let Some(pb) = progress.as_ref() {
                    pb.println(format!(
                        "  [DRY RUN] Would copy (no metadata): {} -> {}",
                        file.archive_path,
                        output_path.display()
                    ));
                } else {
                    println!(
                        "  [DRY RUN] Would copy (no metadata): {} -> {}",
                        file.archive_path,
                        output_path.display()
                    );
                }
                stats.media_copied_without_metadata += 1;
                if is_image_file(&file.archive_path) {
                    stats.images_processed_without_metadata += 1;
                } else {
                    stats.videos_copied += 1;
                }
            }
            stats.images_processed += 1;
            if let Some(pb) = progress.as_ref() {
                pb.inc(1);
            }
            continue;
        }

        // Process based on file type
        let result = if is_image_file(&file.archive_path) {
            let image_data = read_zip_file_cached(&mut archive_cache, file)?;
            process_image_data(&file.archive_path, image_data, metadata_json, &output_path, debug)
        } else {
            // Video or other file - just copy
            let data = read_zip_file_cached(&mut archive_cache, file)?;
            copy_file_data(data, &output_path, debug).map(|_| false)
        };

        match result {
            Ok(had_metadata) => {
                stats.images_processed += 1;
                if had_metadata {
                    stats.metadata_applied += 1;
                    stats.images_processed_with_metadata += 1;
                } else {
                    stats.media_copied_without_metadata += 1;
                    stats.images_processed_without_metadata += 1;
                }
            }
            Err(e) => {
                if let Some(pb) = progress.as_ref() {
                    pb.println(format!("  Error processing {}: {}", file.archive_path, e));
                } else {
                    eprintln!("  Error processing {}: {}", file.archive_path, e);
                }
                stats.errors += 1;
            }
        }

        if let Some(pb) = progress.as_ref() {
            pb.inc(1);
        }
    }

    let tar_archives: Vec<_> = takeout
        .source_archives()
        .iter()
        .filter(|p| is_tar_gz_archive(p))
        .cloned()
        .collect();

    for archive_path in tar_archives {
        let file = File::open(&archive_path)
            .map_err(|e| ProcessError::IoError(format!("Failed to open archive: {}", e)))?;
        let reader = BufReader::new(file);
        let decoder = GzDecoder::new(reader);
        let mut archive = TarArchive::new(decoder);

        let entries = archive
            .entries()
            .map_err(|e| ProcessError::ArchiveError(format!("Failed to read tar entries: {}", e)))?;

        for entry in entries {
            let mut entry = entry
                .map_err(|e| ProcessError::ArchiveError(format!("Failed to read entry: {}", e)))?;
            if !entry.header().entry_type().is_file() {
                continue;
            }

            let entry_path = entry
                .path()
                .map_err(|e| ProcessError::ArchiveError(format!("Failed to get path: {}", e)))?;
            let entry_path_str = entry_path.to_string_lossy().to_string();

            if !entry_path_str.starts_with(photo_path_prefix) || !is_media_file(&entry_path_str) {
                std::io::copy(&mut entry, &mut std::io::sink())
                    .map_err(|e| ProcessError::IoError(format!("Failed to skip entry: {}", e)))?;
                continue;
            }

            let file_name = entry_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            let album = extract_album_path(&entry_path_str, photo_path_prefix);
            let output_path = output_dir.join(&album).join(file_name);

            if debug {
                if let Some(pb) = progress.as_ref() {
                    pb.println(format!("  Processing: {}/{}", album, file_name));
                } else {
                    println!("  Processing: {}/{}", album, file_name);
                }
            }

            let metadata_file = takeout.find_metadata_for(&entry_path_str);
            if let Some(meta_file) = metadata_file {
                used_metadata.insert(meta_file.archive_path.clone());
            }

            let metadata_json = metadata_file
                .and_then(|meta| metadata_cache.get(&meta.archive_path))
                .map(|s| s.as_str());

            if dry_run {
                if metadata_file.is_some() {
                    if let Some(pb) = progress.as_ref() {
                        pb.println(format!(
                            "  [DRY RUN] Would process: {} -> {}",
                            entry_path_str,
                            output_path.display()
                        ));
                    } else {
                        println!(
                            "  [DRY RUN] Would process: {} -> {}",
                            entry_path_str,
                            output_path.display()
                        );
                    }
                    stats.metadata_applied += 1;
                    if is_image_file(&entry_path_str) {
                        stats.images_processed_with_metadata += 1;
                    }
                } else {
                    if let Some(pb) = progress.as_ref() {
                        pb.println(format!(
                            "  [DRY RUN] Would copy (no metadata): {} -> {}",
                            entry_path_str,
                            output_path.display()
                        ));
                    } else {
                        println!(
                            "  [DRY RUN] Would copy (no metadata): {} -> {}",
                            entry_path_str,
                            output_path.display()
                        );
                    }
                    stats.media_copied_without_metadata += 1;
                    if is_image_file(&entry_path_str) {
                        stats.images_processed_without_metadata += 1;
                    } else {
                        stats.videos_copied += 1;
                    }
                }
                stats.images_processed += 1;
                std::io::copy(&mut entry, &mut std::io::sink())
                    .map_err(|e| ProcessError::IoError(format!("Failed to skip entry: {}", e)))?;
                if let Some(pb) = progress.as_ref() {
                    pb.inc(1);
                }
                continue;
            }

            let result = if is_image_file(&entry_path_str) {
                let mut image_data = Vec::new();
                entry
                    .read_to_end(&mut image_data)
                    .map_err(|e| ProcessError::IoError(format!("Failed to read contents: {}", e)))?;
                process_image_data(&entry_path_str, image_data, metadata_json, &output_path, debug)
            } else {
                let mut data = Vec::new();
                entry
                    .read_to_end(&mut data)
                    .map_err(|e| ProcessError::IoError(format!("Failed to read contents: {}", e)))?;
                copy_file_data(data, &output_path, debug).map(|_| false)
            };

            match result {
                Ok(had_metadata) => {
                    stats.images_processed += 1;
                    if had_metadata {
                        stats.metadata_applied += 1;
                        if is_image_file(&entry_path_str) {
                            stats.images_processed_with_metadata += 1;
                        }
                    } else {
                        stats.media_copied_without_metadata += 1;
                        if is_image_file(&entry_path_str) {
                            stats.images_processed_without_metadata += 1;
                        } else {
                            stats.videos_copied += 1;
                        }
                    }
                }
                Err(e) => {
                    if let Some(pb) = progress.as_ref() {
                        pb.println(format!("  Error processing {}: {}", entry_path_str, e));
                    } else {
                        eprintln!("  Error processing {}: {}", entry_path_str, e);
                    }
                    stats.errors += 1;
                }
            }

            if let Some(pb) = progress.as_ref() {
                pb.inc(1);
            }
        }
    }

    if let Some(pb) = progress.as_ref() {
        pb.finish_and_clear();
    }

    let unused_metadata: Vec<_> = takeout
        .supplemental_metadata_files()
        .filter(|f| !used_metadata.contains(&f.archive_path))
        .collect();

    stats.unused_metadata_files = unused_metadata.len();

    if stats.unused_metadata_files > 0 {
        println!(
            "\nWarning: {} supplemental metadata files were not matched to any media file.",
            stats.unused_metadata_files
        );
        for file in &unused_metadata {
            println!("  Unused metadata: {}", file.archive_path);
        }
    }

    Ok(stats)
}
