use crate::archive::{ArchiveFile, Takeout};
use crate::metadata::{
    GeoData, GoogleVideoMetadata, MetadataError, apply_google_metadata, google_video_metadata,
};
use exiftool_rs::writer::matroska_writer;
use exiftool_rs::writer::mp4_writer;
use exiftool_rs::writer::xmp_writer::{self, XmpProperty, XmpPropertyType};
use filetime::{FileTime, set_file_mtime};
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
    pub media_processed: usize,
    pub images_skipped: usize,
    pub metadata_applied: usize,
    pub unused_metadata_files: usize,
    pub album_metadata_files: usize,
    pub media_copied_without_metadata: usize,
    pub images_processed_with_metadata: usize,
    pub images_processed_without_metadata: usize,
    pub media_without_metadata: Vec<String>,
    pub videos_processed_with_metadata: usize,
    pub videos_processed_without_metadata: usize,
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

/// Video file extensions. Sidecar timestamps are applied to output file mtimes.
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

fn read_zip_file_cached(
    cache: &mut ArchiveCache,
    file: &ArchiveFile,
) -> Result<Vec<u8>, ProcessError> {
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
            cache
                .zip_archives
                .insert(file.source_archive.clone(), archive);
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
        FileExtension::PNG {
            as_zTXt_chunk: false,
        }
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

fn write_file_data(data: Vec<u8>, output_path: &Path) -> Result<(), ProcessError> {
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

fn looks_like_quicktime_container(data: &[u8]) -> bool {
    if data.len() >= 12 && &data[4..8] == b"ftyp" {
        return true;
    }

    data.len() >= 8 && matches!(&data[4..8], b"moov" | b"mdat" | b"free" | b"skip" | b"wide")
}

fn xmp_gps_coordinate(value: f64, positive_ref: char, negative_ref: char) -> String {
    let absolute = value.abs();
    let degrees = absolute.trunc();
    let minutes = (absolute - degrees) * 60.0;
    let reference = if value >= 0.0 {
        positive_ref
    } else {
        negative_ref
    };

    format!("{degrees:.0},{minutes:.6}{reference}")
}

fn quicktime_iso6709_coordinate(geo: &GeoData) -> String {
    let mut coordinate = format!("{:+010.6}{:+011.6}", geo.latitude, geo.longitude);

    if geo.altitude != 0.0 {
        coordinate.push_str(&format!("{:+.3}", geo.altitude));
    }

    coordinate.push('/');
    coordinate
}

fn video_xmp_properties(metadata: &GoogleVideoMetadata) -> Vec<XmpProperty> {
    let mut properties = Vec::new();

    if !metadata.title.is_empty() {
        properties.push(XmpProperty {
            namespace: "dc".to_string(),
            property: "title".to_string(),
            values: vec![metadata.title.clone()],
            prop_type: XmpPropertyType::LangAlt,
        });
    }

    if let Some(description) = &metadata.description {
        properties.push(XmpProperty {
            namespace: "dc".to_string(),
            property: "description".to_string(),
            values: vec![description.clone()],
            prop_type: XmpPropertyType::LangAlt,
        });
    }

    if let Some(datetime) = &metadata.xmp_datetime {
        properties.push(XmpProperty {
            namespace: "xmp".to_string(),
            property: "CreateDate".to_string(),
            values: vec![datetime.clone()],
            prop_type: XmpPropertyType::Simple,
        });
        properties.push(XmpProperty {
            namespace: "exif".to_string(),
            property: "DateTimeOriginal".to_string(),
            values: vec![datetime.clone()],
            prop_type: XmpPropertyType::Simple,
        });
    }

    if let Some(geo) = &metadata.geo_data {
        properties.extend(video_gps_xmp_properties(geo));
    }

    properties
}

fn video_gps_xmp_properties(geo: &GeoData) -> Vec<XmpProperty> {
    let mut properties = vec![
        XmpProperty {
            namespace: "exif".to_string(),
            property: "GPSLatitude".to_string(),
            values: vec![xmp_gps_coordinate(geo.latitude, 'N', 'S')],
            prop_type: XmpPropertyType::Simple,
        },
        XmpProperty {
            namespace: "exif".to_string(),
            property: "GPSLongitude".to_string(),
            values: vec![xmp_gps_coordinate(geo.longitude, 'E', 'W')],
            prop_type: XmpPropertyType::Simple,
        },
    ];

    if geo.altitude != 0.0 {
        properties.push(XmpProperty {
            namespace: "exif".to_string(),
            property: "GPSAltitude".to_string(),
            values: vec![geo.altitude.abs().to_string()],
            prop_type: XmpPropertyType::Simple,
        });
        properties.push(XmpProperty {
            namespace: "exif".to_string(),
            property: "GPSAltitudeRef".to_string(),
            values: vec![(if geo.altitude >= 0.0 { "0" } else { "1" }).to_string()],
            prop_type: XmpPropertyType::Simple,
        });
    }

    properties
}

fn atom_size(data: &[u8], pos: usize) -> Option<(usize, usize)> {
    if pos + 8 > data.len() {
        return None;
    }

    let size = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
    match size {
        0 => Some((data.len() - pos, 8)),
        1 => {
            if pos + 16 > data.len() {
                return None;
            }
            let extended = u64::from_be_bytes([
                data[pos + 8],
                data[pos + 9],
                data[pos + 10],
                data[pos + 11],
                data[pos + 12],
                data[pos + 13],
                data[pos + 14],
                data[pos + 15],
            ]);
            usize::try_from(extended).ok().map(|size| (size, 16))
        }
        size => Some((size as usize, 8)),
    }
}

fn build_quicktime_atom(atom_type: &[u8; 4], content: &[u8]) -> Result<Vec<u8>, ProcessError> {
    let size = content
        .len()
        .checked_add(8)
        .and_then(|size| u32::try_from(size).ok())
        .ok_or_else(|| ProcessError::ExifError("QuickTime metadata atom is too large".into()))?;
    let mut atom = Vec::with_capacity(size as usize);
    atom.extend_from_slice(&size.to_be_bytes());
    atom.extend_from_slice(atom_type);
    atom.extend_from_slice(content);
    Ok(atom)
}

fn build_quicktime_location_meta_atom(location: &str) -> Result<Vec<u8>, ProcessError> {
    const LOCATION_KEY: &[u8] = b"com.apple.quicktime.location.ISO6709";

    let mut hdlr_content = Vec::new();
    hdlr_content.extend_from_slice(&[0, 0, 0, 0]);
    hdlr_content.extend_from_slice(&[0, 0, 0, 0]);
    hdlr_content.extend_from_slice(b"mdta");
    hdlr_content.extend_from_slice(&[0; 12]);
    hdlr_content.extend_from_slice(b"mdta\0");
    let hdlr = build_quicktime_atom(b"hdlr", &hdlr_content)?;

    let mut key_entry = Vec::new();
    let key_entry_size = LOCATION_KEY
        .len()
        .checked_add(8)
        .and_then(|size| u32::try_from(size).ok())
        .ok_or_else(|| ProcessError::ExifError("QuickTime metadata key is too large".into()))?;
    key_entry.extend_from_slice(&key_entry_size.to_be_bytes());
    key_entry.extend_from_slice(b"mdta");
    key_entry.extend_from_slice(LOCATION_KEY);

    let mut keys_content = Vec::new();
    keys_content.extend_from_slice(&[0, 0, 0, 0]);
    keys_content.extend_from_slice(&1u32.to_be_bytes());
    keys_content.extend_from_slice(&key_entry);
    let keys = build_quicktime_atom(b"keys", &keys_content)?;

    let mut data_content = Vec::new();
    data_content.extend_from_slice(&[0, 0, 0, 1]);
    data_content.extend_from_slice(&[0, 0, 0, 0]);
    data_content.extend_from_slice(location.as_bytes());
    let data_atom = build_quicktime_atom(b"data", &data_content)?;

    let mut item_content = Vec::new();
    item_content.extend_from_slice(&data_atom);
    let item = build_quicktime_atom(&1u32.to_be_bytes(), &item_content)?;

    let mut ilst_content = Vec::new();
    ilst_content.extend_from_slice(&item);
    let ilst = build_quicktime_atom(b"ilst", &ilst_content)?;

    let mut meta_content = Vec::new();
    meta_content.extend_from_slice(&[0, 0, 0, 0]);
    meta_content.extend_from_slice(&hdlr);
    meta_content.extend_from_slice(&keys);
    meta_content.extend_from_slice(&ilst);

    build_quicktime_atom(b"meta", &meta_content)
}

fn contains_quicktime_location_key(atom: &[u8]) -> bool {
    atom.windows(b"com.apple.quicktime.location.ISO6709".len())
        .any(|window| window == b"com.apple.quicktime.location.ISO6709")
}

fn rewrite_moov_with_quicktime_location(
    moov_content: &[u8],
    location: &str,
) -> Result<Vec<u8>, ProcessError> {
    let mut output = Vec::with_capacity(moov_content.len());
    let mut pos = 0;

    while pos + 8 <= moov_content.len() {
        let Some((size, _header_size)) = atom_size(moov_content, pos) else {
            break;
        };
        if size < 8 || pos + size > moov_content.len() {
            output.extend_from_slice(&moov_content[pos..]);
            break;
        }

        let atom_type = &moov_content[pos + 4..pos + 8];
        let atom = &moov_content[pos..pos + size];
        if !(atom_type == b"meta" && contains_quicktime_location_key(atom)) {
            output.extend_from_slice(atom);
        }

        pos += size;
    }

    if pos < moov_content.len() {
        output.extend_from_slice(&moov_content[pos..]);
    }

    output.extend_from_slice(&build_quicktime_location_meta_atom(location)?);
    Ok(output)
}

fn add_quicktime_location_metadata(data: &[u8], location: &str) -> Result<Vec<u8>, ProcessError> {
    let mut output = Vec::with_capacity(data.len() + 256);
    let mut pos = 0;
    let mut wrote_moov = false;

    while pos + 8 <= data.len() {
        let Some((size, header_size)) = atom_size(data, pos) else {
            break;
        };
        if size < header_size || pos + size > data.len() {
            output.extend_from_slice(&data[pos..]);
            break;
        }

        let atom_type = &data[pos + 4..pos + 8];
        if atom_type == b"moov" {
            let content = &data[pos + header_size..pos + size];
            let rewritten = rewrite_moov_with_quicktime_location(content, location)?;
            output.extend_from_slice(&build_quicktime_atom(b"moov", &rewritten)?);
            wrote_moov = true;
        } else {
            output.extend_from_slice(&data[pos..pos + size]);
        }

        pos += size;
    }

    if pos < data.len() {
        output.extend_from_slice(&data[pos..]);
    }

    if !wrote_moov {
        let meta = build_quicktime_location_meta_atom(location)?;
        output.extend_from_slice(&build_quicktime_atom(b"moov", &meta)?);
    }

    Ok(output)
}

fn apply_quicktime_video_metadata(
    data: &[u8],
    metadata: &GoogleVideoMetadata,
) -> Result<Vec<u8>, ProcessError> {
    let mut ilst_tags = Vec::new();

    if !metadata.title.is_empty() {
        if let Some(key) = mp4_writer::tag_to_ilst_key("Title") {
            ilst_tags.push((key, metadata.title.clone()));
        }
    }
    if let Some(description) = &metadata.description {
        if let Some(key) = mp4_writer::tag_to_ilst_key("Description") {
            ilst_tags.push((key, description.clone()));
        }
        if let Some(key) = mp4_writer::tag_to_ilst_key("Comment") {
            ilst_tags.push((key, description.clone()));
        }
    }
    if let Some(datetime) = &metadata.xmp_datetime {
        if let Some(key) = mp4_writer::tag_to_ilst_key("Date") {
            ilst_tags.push((key, datetime.clone()));
        }
    }

    let tag_refs: Vec<(&[u8; 4], &str)> = ilst_tags
        .iter()
        .map(|(key, value)| (key, value.as_str()))
        .collect();
    let xmp_properties = video_xmp_properties(metadata);
    let xmp = (!xmp_properties.is_empty()).then(|| xmp_writer::build_xmp(&xmp_properties));

    let updated = mp4_writer::write_mp4(data, &tag_refs, xmp.as_deref().map(str::as_bytes))
        .map_err(|e| ProcessError::ExifError(format!("Failed to write video metadata: {}", e)))?;

    if let Some(geo) = &metadata.geo_data {
        add_quicktime_location_metadata(&updated, &quicktime_iso6709_coordinate(geo))
    } else {
        Ok(updated)
    }
}

fn apply_matroska_video_metadata(
    data: &[u8],
    metadata: &GoogleVideoMetadata,
) -> Result<Vec<u8>, ProcessError> {
    let mut changes = Vec::new();

    if !metadata.title.is_empty() {
        changes.push(("TITLE".to_string(), metadata.title.clone()));
    }
    if let Some(description) = &metadata.description {
        changes.push(("DESCRIPTION".to_string(), description.clone()));
    }
    if let Some(datetime) = &metadata.xmp_datetime {
        changes.push(("DATE_RECORDED".to_string(), datetime.clone()));
    }
    if let Some(geo) = &metadata.geo_data {
        changes.push(("GPS_LATITUDE".to_string(), geo.latitude.to_string()));
        changes.push(("GPS_LONGITUDE".to_string(), geo.longitude.to_string()));
        if geo.altitude != 0.0 {
            changes.push(("GPS_ALTITUDE".to_string(), geo.altitude.to_string()));
        }
    }

    let change_refs: Vec<(&str, &str)> = changes
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();

    matroska_writer::write_matroska(data, &change_refs)
        .map_err(|e| ProcessError::ExifError(format!("Failed to write video metadata: {}", e)))
}

fn apply_video_embedded_metadata(
    output_path: &Path,
    metadata: &GoogleVideoMetadata,
    debug: bool,
) -> Result<bool, ProcessError> {
    let data = fs::read(output_path)
        .map_err(|e| ProcessError::IoError(format!("Failed to read video for metadata: {}", e)))?;

    let updated = if looks_like_quicktime_container(&data) {
        apply_quicktime_video_metadata(&data, metadata)?
    } else if data.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        apply_matroska_video_metadata(&data, metadata)?
    } else {
        if debug {
            println!("    Video container does not support embedded metadata writing yet");
        }
        return Ok(false);
    };

    fs::write(output_path, updated).map_err(|e| {
        ProcessError::IoError(format!("Failed to write video with metadata: {}", e))
    })?;

    Ok(true)
}

/// Process a video file: copy it, embed supported metadata, and apply Google's media timestamp to the output file mtime.
fn process_video_data(
    video_data: Vec<u8>,
    metadata_json: Option<&str>,
    output_path: &Path,
    debug: bool,
) -> Result<bool, ProcessError> {
    write_file_data(video_data, output_path)?;

    let Some(json_str) = metadata_json else {
        return Ok(false);
    };

    let video_metadata = google_video_metadata(json_str)?;
    let embedded = match apply_video_embedded_metadata(output_path, &video_metadata, debug) {
        Ok(applied) => applied,
        Err(ProcessError::ExifError(e)) => {
            if debug {
                println!(
                    "    Warning: Could not write embedded video metadata: {}",
                    e
                );
            }
            false
        }
        Err(e) => return Err(e),
    };

    let Some(timestamp) = video_metadata.timestamp else {
        if debug {
            println!("    Metadata JSON has no timestamp for video");
        }
        return Ok(embedded);
    };

    if debug {
        println!("    Applying video timestamp from JSON to file modified time");
    }

    set_file_mtime(output_path, FileTime::from_unix_time(timestamp, 0))
        .map_err(|e| ProcessError::IoError(format!("Failed to set video timestamp: {}", e)))?;

    Ok(true)
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

    for meta in takeout.metadata_sidecar_candidates() {
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

        let entries = archive.entries().map_err(|e| {
            ProcessError::ArchiveError(format!("Failed to read tar entries: {}", e))
        })?;

        for entry in entries {
            let mut entry = entry
                .map_err(|e| ProcessError::ArchiveError(format!("Failed to read entry: {}", e)))?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let entry_path = entry
                .path()
                .map_err(|e| ProcessError::ArchiveError(format!("Failed to get path: {}", e)))?;
            let mut entry_path_str = entry_path.to_string_lossy().to_string();
            if let Some(stripped) = entry_path_str.strip_prefix("./") {
                entry_path_str = stripped.to_string();
            }

            if wanted_paths.contains(&entry_path_str) {
                let mut contents = Vec::new();
                entry.read_to_end(&mut contents).map_err(|e| {
                    ProcessError::IoError(format!("Failed to read contents: {}", e))
                })?;
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

#[derive(Debug)]
struct AlbumMetadataFile {
    archive_path: String,
    album_path: String,
    file_name: String,
    data: Vec<u8>,
}

fn read_tar_entries(
    archive_path: &Path,
    wanted_paths: &HashSet<String>,
) -> Result<HashMap<String, Vec<u8>>, ProcessError> {
    let file = File::open(archive_path)
        .map_err(|e| ProcessError::IoError(format!("Failed to open archive: {}", e)))?;
    let reader = BufReader::new(file);
    let decoder = GzDecoder::new(reader);
    let mut archive = TarArchive::new(decoder);

    let entries = archive
        .entries()
        .map_err(|e| ProcessError::ArchiveError(format!("Failed to read tar entries: {}", e)))?;

    let mut found = HashMap::new();
    for entry in entries {
        let mut entry = entry
            .map_err(|e| ProcessError::ArchiveError(format!("Failed to read entry: {}", e)))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }

        let entry_path = entry
            .path()
            .map_err(|e| ProcessError::ArchiveError(format!("Failed to get path: {}", e)))?;
        let mut entry_path_str = entry_path.to_string_lossy().to_string();
        if let Some(stripped) = entry_path_str.strip_prefix("./") {
            entry_path_str = stripped.to_string();
        }

        if wanted_paths.contains(&entry_path_str) {
            let mut contents = Vec::new();
            entry
                .read_to_end(&mut contents)
                .map_err(|e| ProcessError::IoError(format!("Failed to read contents: {}", e)))?;
            found.insert(entry_path_str, contents);
        } else {
            std::io::copy(&mut entry, &mut std::io::sink())
                .map_err(|e| ProcessError::IoError(format!("Failed to skip entry: {}", e)))?;
        }
    }

    Ok(found)
}

fn archive_path_file_name(path: &str) -> String {
    path.rfind(|ch| ch == '/' || ch == '\\')
        .map(|index| &path[index + 1..])
        .unwrap_or(path)
        .to_string()
}

fn read_album_metadata_files(
    takeout: &Takeout,
    archive_cache: &mut ArchiveCache,
    photo_path_prefix: &str,
) -> Result<Vec<AlbumMetadataFile>, ProcessError> {
    let mut files = Vec::new();
    let mut tar_files_by_archive: HashMap<PathBuf, HashSet<String>> = HashMap::new();

    for file in takeout.album_metadata_files() {
        if is_tar_gz_archive(&file.source_archive) {
            tar_files_by_archive
                .entry(file.source_archive.clone())
                .or_default()
                .insert(file.archive_path.clone());
        } else if is_zip_archive(&file.source_archive) {
            files.push(AlbumMetadataFile {
                archive_path: file.archive_path.clone(),
                album_path: extract_album_path(&file.archive_path, photo_path_prefix),
                file_name: file.file_name().to_string(),
                data: read_zip_file_cached(archive_cache, file)?,
            });
        }
    }

    for (archive_path, wanted_paths) in tar_files_by_archive {
        let found = read_tar_entries(&archive_path, &wanted_paths)?;
        for file_path in wanted_paths {
            if let Some(data) = found.get(&file_path) {
                files.push(AlbumMetadataFile {
                    archive_path: file_path.clone(),
                    album_path: extract_album_path(&file_path, photo_path_prefix),
                    file_name: archive_path_file_name(&file_path),
                    data: data.clone(),
                });
            }
        }
    }

    files.sort_by(|a, b| a.archive_path.cmp(&b.archive_path));
    Ok(files)
}

fn markdown_heading_text(text: &str) -> String {
    if text.is_empty() {
        "Root".to_string()
    } else {
        text.to_string()
    }
}

fn markdown_table_value(value: &serde_json::Value) -> String {
    let rendered = match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Array(values) => format!("{} item(s)", values.len()),
        serde_json::Value::Object(values) => format!("{} field(s)", values.len()),
    };

    rendered
        .replace('\r', " ")
        .replace('\n', " ")
        .replace('|', "\\|")
}

fn album_summary_title(album_path: &str, json: &serde_json::Value) -> String {
    if let Some(title) = json
        .get("title")
        .and_then(|value| value.as_str())
        .filter(|title| !title.trim().is_empty())
    {
        title.to_string()
    } else {
        markdown_heading_text(album_path)
    }
}

fn build_album_metadata_markdown(files: &[AlbumMetadataFile]) -> String {
    let mut markdown = String::from("# Album Metadata Summary\n\n");
    markdown.push_str("Generated from Google Takeout album metadata files.\n");

    if files.is_empty() {
        markdown.push_str("\nNo album metadata files were found.\n");
        return markdown;
    }

    for file in files {
        let json_text = String::from_utf8_lossy(&file.data);
        match serde_json::from_str::<serde_json::Value>(&json_text) {
            Ok(json) => {
                let title = album_summary_title(&file.album_path, &json);
                markdown.push_str(&format!("\n## {}\n\n", title));
                markdown.push_str(&format!("Album path: `{}`\n\n", file.album_path));
                markdown.push_str(&format!("Source: `{}`\n\n", file.archive_path));

                if let serde_json::Value::Object(fields) = json {
                    if !fields.is_empty() {
                        markdown.push_str("| Field | Value |\n| --- | --- |\n");
                        for (key, value) in fields {
                            markdown.push_str(&format!(
                                "| {} | {} |\n",
                                key.replace('|', "\\|"),
                                markdown_table_value(&value)
                            ));
                        }
                    }
                } else {
                    markdown.push_str(&format!("Value: `{}`\n", markdown_table_value(&json)));
                }
            }
            Err(error) => {
                markdown.push_str(&format!(
                    "\n## {}\n\n",
                    markdown_heading_text(&file.album_path)
                ));
                markdown.push_str(&format!("Album path: `{}`\n\n", file.album_path));
                markdown.push_str(&format!("Source: `{}`\n\n", file.archive_path));
                markdown.push_str(&format!(
                    "Could not parse album metadata JSON for summary: `{}`\n",
                    error
                ));
            }
        }
    }

    markdown
}

fn copy_album_metadata_files(
    files: &[AlbumMetadataFile],
    output_dir: &Path,
) -> Result<(), ProcessError> {
    for file in files {
        let output_path = output_dir.join(&file.album_path).join(&file.file_name);
        write_file_data(file.data.clone(), &output_path)?;
    }

    Ok(())
}

fn write_album_metadata_summary(
    files: &[AlbumMetadataFile],
    output_dir: &Path,
) -> Result<(), ProcessError> {
    let markdown = build_album_metadata_markdown(files);
    write_file_data(markdown.into_bytes(), &output_dir.join("album-metadata.md"))
}

/// Process all files in the takeout and output to the specified directory
pub fn process_takeout(
    takeout: &Takeout,
    output_dir: &Path,
    photo_path_prefix: &str,
    dry_run: bool,
    debug: bool,
    show_progress: bool,
    write_album_summary: bool,
    copy_album_json: bool,
) -> Result<ProcessStats, ProcessError> {
    let mut stats = ProcessStats::default();
    let mut used_metadata = HashSet::new();
    let mut archive_cache = ArchiveCache::new();

    let metadata_cache = build_metadata_cache(takeout, &mut archive_cache)?;
    let album_metadata_files =
        read_album_metadata_files(takeout, &mut archive_cache, photo_path_prefix)?;
    stats.album_metadata_files = album_metadata_files.len();

    if !album_metadata_files.is_empty() && (write_album_summary || copy_album_json) {
        if dry_run {
            if write_album_summary {
                println!(
                    "\n[DRY RUN] Would write album metadata summary for {} file(s) to {}",
                    album_metadata_files.len(),
                    output_dir.join("album-metadata.md").display()
                );
            }
            if copy_album_json {
                println!(
                    "[DRY RUN] Would copy {} album metadata file(s) into output albums",
                    album_metadata_files.len()
                );
            }
        } else {
            if write_album_summary {
                write_album_metadata_summary(&album_metadata_files, output_dir)?;
            }
            if copy_album_json {
                copy_album_metadata_files(&album_metadata_files, output_dir)?;
            }
        }
    }

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
                } else if is_video_file(&file.archive_path) {
                    stats.videos_processed_with_metadata += 1;
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
                } else if is_video_file(&file.archive_path) {
                    stats.videos_processed_without_metadata += 1;
                    stats.videos_copied += 1;
                }
                stats.media_without_metadata.push(file.archive_path.clone());
            }
            stats.media_processed += 1;
            if let Some(pb) = progress.as_ref() {
                pb.inc(1);
            }
            continue;
        }

        // Process based on file type
        let result = if is_image_file(&file.archive_path) {
            let image_data = read_zip_file_cached(&mut archive_cache, file)?;
            process_image_data(
                &file.archive_path,
                image_data,
                metadata_json,
                &output_path,
                debug,
            )
        } else {
            let data = read_zip_file_cached(&mut archive_cache, file)?;
            process_video_data(data, metadata_json, &output_path, debug)
        };

        match result {
            Ok(had_metadata) => {
                stats.media_processed += 1;
                if had_metadata {
                    stats.metadata_applied += 1;
                    if is_image_file(&file.archive_path) {
                        stats.images_processed_with_metadata += 1;
                    } else if is_video_file(&file.archive_path) {
                        stats.videos_processed_with_metadata += 1;
                    }
                } else {
                    stats.media_copied_without_metadata += 1;
                    if is_image_file(&file.archive_path) {
                        stats.images_processed_without_metadata += 1;
                    } else if is_video_file(&file.archive_path) {
                        stats.videos_processed_without_metadata += 1;
                        stats.videos_copied += 1;
                    }
                    stats.media_without_metadata.push(file.archive_path.clone());
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

        let entries = archive.entries().map_err(|e| {
            ProcessError::ArchiveError(format!("Failed to read tar entries: {}", e))
        })?;

        for entry in entries {
            let mut entry = entry
                .map_err(|e| ProcessError::ArchiveError(format!("Failed to read entry: {}", e)))?;
            if !entry.header().entry_type().is_file() {
                continue;
            }

            let entry_path = entry
                .path()
                .map_err(|e| ProcessError::ArchiveError(format!("Failed to get path: {}", e)))?;
            let mut entry_path_str = entry_path.to_string_lossy().to_string();
            if let Some(stripped) = entry_path_str.strip_prefix("./") {
                entry_path_str = stripped.to_string();
            }

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
                    } else if is_video_file(&entry_path_str) {
                        stats.videos_processed_with_metadata += 1;
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
                    } else if is_video_file(&entry_path_str) {
                        stats.videos_processed_without_metadata += 1;
                        stats.videos_copied += 1;
                    }
                    stats.media_without_metadata.push(entry_path_str.clone());
                }
                stats.media_processed += 1;
                std::io::copy(&mut entry, &mut std::io::sink())
                    .map_err(|e| ProcessError::IoError(format!("Failed to skip entry: {}", e)))?;
                if let Some(pb) = progress.as_ref() {
                    pb.inc(1);
                }
                continue;
            }

            let result = if is_image_file(&entry_path_str) {
                let mut image_data = Vec::new();
                entry.read_to_end(&mut image_data).map_err(|e| {
                    ProcessError::IoError(format!("Failed to read contents: {}", e))
                })?;
                process_image_data(
                    &entry_path_str,
                    image_data,
                    metadata_json,
                    &output_path,
                    debug,
                )
            } else {
                let mut data = Vec::new();
                entry.read_to_end(&mut data).map_err(|e| {
                    ProcessError::IoError(format!("Failed to read contents: {}", e))
                })?;
                process_video_data(data, metadata_json, &output_path, debug)
            };

            match result {
                Ok(had_metadata) => {
                    stats.media_processed += 1;
                    if had_metadata {
                        stats.metadata_applied += 1;
                        if is_image_file(&entry_path_str) {
                            stats.images_processed_with_metadata += 1;
                        } else if is_video_file(&entry_path_str) {
                            stats.videos_processed_with_metadata += 1;
                        }
                    } else {
                        stats.media_copied_without_metadata += 1;
                        if is_image_file(&entry_path_str) {
                            stats.images_processed_without_metadata += 1;
                        } else if is_video_file(&entry_path_str) {
                            stats.videos_processed_without_metadata += 1;
                            stats.videos_copied += 1;
                        }
                        stats.media_without_metadata.push(entry_path_str.clone());
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
        .metadata_sidecar_candidates()
        .filter(|f| !used_metadata.contains(&f.archive_path))
        .collect();

    stats.unused_metadata_files = unused_metadata.len();

    if stats.unused_metadata_files > 0 {
        println!(
            "\nWarning: {} metadata sidecar files were not matched to any media file.",
            stats.unused_metadata_files
        );
        for file in &unused_metadata {
            println!("  Unused metadata sidecar: {}", file.archive_path);
        }
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file_path(file_name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        path.push(format!(
            "takeout-fixer-process-test-{}-{}-{}",
            std::process::id(),
            nanos,
            file_name
        ));
        path
    }

    fn minimal_mp4() -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&24u32.to_be_bytes());
        data.extend_from_slice(b"ftyp");
        data.extend_from_slice(b"isom");
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(b"isom");
        data.extend_from_slice(b"mp42");
        data.extend_from_slice(&8u32.to_be_bytes());
        data.extend_from_slice(b"moov");
        data
    }

    #[test]
    fn process_video_data_embeds_metadata_for_mp4() {
        let output_path = temp_file_path("clip.mp4");
        let json = r#"{
            "title": "clip.mp4",
            "description": "Summer clip",
            "creationTime": {
                "timestamp": "1587036746",
                "formatted": "16. apr. 2020, 11.32.26 UTC"
            },
            "photoTakenTime": {
                "timestamp": "1563032119",
                "formatted": "13. jul. 2019, 15.35.19 UTC"
            },
            "geoData": {
                "latitude": 0.0,
                "longitude": 0.0,
                "altitude": 0.0,
                "latitudeSpan": 0.0,
                "longitudeSpan": 0.0
            },
            "geoDataExif": {
                "latitude": 55.6761,
                "longitude": 12.5683,
                "altitude": 7.0,
                "latitudeSpan": 0.0,
                "longitudeSpan": 0.0
            }
        }"#;

        let applied = process_video_data(minimal_mp4(), Some(json), &output_path, false)
            .expect("video processing should succeed");
        assert!(applied);

        let output = fs::read(&output_path).expect("output video should exist");
        let output_text = String::from_utf8_lossy(&output);
        assert!(output_text.contains("Summer clip"));
        assert!(output_text.contains("2019-07-13T15:35:19Z"));
        assert!(output_text.contains("DateTimeOriginal"));
        assert!(output_text.contains("GPSLatitude"));
        assert!(output_text.contains("com.apple.quicktime.location.ISO6709"));

        let tags = exiftool_rs::ExifTool::new()
            .extract_info(&output_path)
            .expect("output video metadata should be readable");
        let gps_coordinates = tags
            .iter()
            .find(|tag| tag.group.family0 == "QuickTime" && tag.name == "GPSCoordinates")
            .expect("QuickTime GPSCoordinates should be written");
        assert_eq!(gps_coordinates.print_value, "+55.676100+012.568300+7.000/");

        let modified = fs::metadata(&output_path)
            .expect("metadata should be readable")
            .modified()
            .expect("modified timestamp should be readable")
            .duration_since(UNIX_EPOCH)
            .expect("modified timestamp should be after Unix epoch")
            .as_secs();
        assert_eq!(modified, 1563032119);

        let _ = fs::remove_file(output_path);
    }
}
