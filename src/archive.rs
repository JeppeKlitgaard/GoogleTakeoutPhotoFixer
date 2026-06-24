use std::collections::HashMap;
use std::path::{Path, PathBuf};

const SUPPLEMENTAL_METADATA_SUFFIXES: &[&str] = &[
    // Google normally emits "photo.jpg.supplemental-metadata.json".
    // For long filenames it can truncate the supplemental marker at many
    // different points, so matching tries the longest, most specific suffixes.
    ".supplemental-metadata.",
    ".supplemental-metadat.",
    ".supplemental-metada.",
    ".supplemental-metad.",
    ".supplemental-meta.",
    ".supplemental-met.",
    ".supplemental-me.",
    ".supplemental-m.",
    ".supplemental-.",
    ".supplemental.",
    ".supplementa.",
    ".supplement.",
    ".supplemen.",
    ".suppleme.",
    ".supplem.",
    ".supple.",
    ".suppl.",
    ".supp.",
    ".sup.",
    ".su.",
    ".s.",
];

const BARE_METADATA_SUFFIXES: &[&str] = &[
    // Google also emits bare sidecars such as "photo.jpg.json". A few long
    // filename cases can show up as "photo.jpg..json"; keep these broad
    // fallbacks separate from the stricter supplemental suffix check.
    "..", ".",
];

fn metadata_sidecar_suffixes() -> impl Iterator<Item = &'static str> {
    SUPPLEMENTAL_METADATA_SUFFIXES
        .iter()
        .chain(BARE_METADATA_SUFFIXES)
        .copied()
}

fn is_google_metadata_sidecar_path(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".json")
}

fn is_album_metadata_path(path: &str) -> bool {
    archive_file_name(path).eq_ignore_ascii_case("metadata.json")
}

fn has_supplemental_metadata_suffix(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();

    has_standard_supplemental_metadata_suffix(&lower)
        || has_numbered_supplemental_metadata_suffix(&lower)
}

fn has_standard_supplemental_metadata_suffix(lower_path: &str) -> bool {
    SUPPLEMENTAL_METADATA_SUFFIXES
        .iter()
        .any(|suffix| lower_path.ends_with(&format!("{}json", suffix)))
}

fn has_numbered_supplemental_metadata_suffix(lower_path: &str) -> bool {
    // Duplicate media exports may use "photo(1).jpg" while the JSON keeps the
    // copy marker after the supplemental suffix:
    // "photo.jpg.supplemental-metadata(1).json".
    let Some(before_json) = lower_path.strip_suffix(".json") else {
        return false;
    };
    let Some(before_close) = before_json.strip_suffix(')') else {
        return false;
    };
    let Some(marker_open) = before_close.rfind('(') else {
        return false;
    };

    let marker_digits = &before_close[marker_open + 1..];
    if marker_digits.is_empty() || !marker_digits.chars().all(|ch| ch.is_ascii_digit()) {
        return false;
    }

    let before_marker = &before_close[..marker_open];
    SUPPLEMENTAL_METADATA_SUFFIXES.iter().any(|suffix| {
        suffix.strip_suffix('.').is_some_and(|suffix_stem| {
            !suffix_stem.is_empty() && before_marker.ends_with(suffix_stem)
        })
    })
}

fn archive_parent_path(path: &str) -> &str {
    path.rfind(|ch| ch == '/' || ch == '\\')
        .map(|index| &path[..index])
        .unwrap_or("")
}

fn archive_file_name(path: &str) -> &str {
    path.rfind(|ch| ch == '/' || ch == '\\')
        .map(|index| &path[index + 1..])
        .unwrap_or(path)
}

fn file_stem(path: &str) -> &str {
    let file_start = path
        .rfind(|ch| ch == '/' || ch == '\\')
        .map_or(0, |index| index + 1);
    let Some(dot) = path[file_start..].rfind('.') else {
        return path;
    };

    &path[..file_start + dot]
}

fn split_numbered_copy_path(path: &str) -> Option<(String, &str)> {
    // Turn "photo(1).jpg" into ("photo.jpg", "(1)") so callers can probe the
    // matching Google metadata spelling.
    let file_start = path
        .rfind(|ch| ch == '/' || ch == '\\')
        .map_or(0, |index| index + 1);
    let dot = path[file_start..].rfind('.')? + file_start;
    let stem = &path[..dot];
    let extension = &path[dot..];
    let marker_open = stem.rfind('(')?;
    let marker = &stem[marker_open..];

    if marker.len() <= 2 || !marker.ends_with(')') {
        return None;
    }

    let marker_digits = &marker[1..marker.len() - 1];
    if marker_digits.is_empty() || !marker_digits.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    Some((format!("{}{}", &stem[..marker_open], extension), marker))
}

fn truncated_metadata_score(photo_name: &str, metadata_name: &str) -> Option<usize> {
    // Very long names can be shortened by Takeout before ".json", sometimes
    // losing a final media-stem character or retaining only a fragment of the
    // original file extension. Restrict this heuristic to long stems and
    // same-directory candidates to avoid stealing ordinary short-name matches.
    if !metadata_name.to_ascii_lowercase().ends_with(".json") {
        return None;
    }

    let metadata_stem = &metadata_name[..metadata_name.len() - ".json".len()];
    let photo_stem = file_stem(photo_name);

    if metadata_stem == photo_name {
        return Some(metadata_stem.chars().count() + 1000);
    }

    if photo_stem.starts_with(metadata_stem) && metadata_stem.len() >= 32 {
        return Some(metadata_stem.chars().count());
    }

    if photo_stem.len() >= 32 {
        if let Some(rest) = metadata_stem.strip_prefix(photo_stem) {
            if rest.starts_with('.') && rest.len() <= 5 {
                return Some(photo_stem.chars().count());
            }
        }
    }

    None
}

/// Represents a file within an archive, abstracting over the archive format.
#[derive(Debug, Clone)]
pub struct ArchiveFile {
    /// The path of the file within the archive (e.g., "Takeout/Google Photos/Album/photo.jpg")
    pub archive_path: String,
    /// The path to the archive file on disk that contains this file
    pub source_archive: PathBuf,
    /// The index of this file within the archive (for zip files)
    pub index: usize,
    /// File size in bytes
    pub size: u64,
}

impl ArchiveFile {
    /// Creates a new ArchiveFile
    pub fn new(archive_path: String, source_archive: PathBuf, index: usize, size: u64) -> Self {
        Self {
            archive_path,
            source_archive,
            index,
            size,
        }
    }

    /// Returns the filename (last component) of the archive path
    pub fn file_name(&self) -> &str {
        Path::new(&self.archive_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
    }

    /// Returns the parent directory path within the archive
    pub fn parent_path(&self) -> &str {
        Path::new(&self.archive_path)
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or("")
    }

    /// Checks if this is a JSON metadata sidecar candidate.
    ///
    /// Under the Google Photos export tree, JSON files are treated as possible
    /// media sidecars. The filename matcher decides later whether a candidate
    /// actually belongs to a specific media file.
    pub fn is_google_metadata_sidecar_candidate(&self) -> bool {
        is_google_metadata_sidecar_path(&self.archive_path) && !self.is_album_metadata()
    }

    /// Checks if this is an album metadata file.
    pub fn is_album_metadata(&self) -> bool {
        is_album_metadata_path(&self.archive_path)
    }

    /// Checks if this is a JSON metadata file.
    pub fn is_metadata(&self) -> bool {
        self.is_google_metadata_sidecar_candidate()
    }

    /// Checks if this has one of Google's supplemental metadata suffixes.
    pub fn is_supplemental_metadata(&self) -> bool {
        has_supplemental_metadata_suffix(&self.archive_path)
    }
}

/// Error type for Takeout operations
#[derive(Debug)]
pub enum TakeoutError {
    /// A file with the same archive path already exists
    DuplicateFile {
        path: String,
        existing_archive: PathBuf,
        new_archive: PathBuf,
    },
    /// Generic error with a message
    Other(String),
}

impl std::fmt::Display for TakeoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TakeoutError::DuplicateFile {
                path,
                existing_archive,
                new_archive,
            } => {
                write!(
                    f,
                    "Duplicate file '{}' found in archives: '{}' and '{}'",
                    path,
                    existing_archive.display(),
                    new_archive.display()
                )
            }
            TakeoutError::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for TakeoutError {}

/// Represents a complete Google Takeout, potentially spanning multiple archive files.
/// Files are indexed by their archive path for fast lookup.
#[derive(Debug)]
pub struct Takeout {
    /// All files in the takeout, keyed by their archive path
    files: HashMap<String, ArchiveFile>,
    /// List of source archive paths that make up this takeout
    source_archives: Vec<PathBuf>,
}

impl Takeout {
    /// Creates a new empty Takeout
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
            source_archives: Vec::new(),
        }
    }

    /// Adds a source archive to the list of archives in this takeout
    pub fn add_source_archive(&mut self, path: PathBuf) {
        if !self.source_archives.contains(&path) {
            self.source_archives.push(path);
        }
    }

    /// Inserts an ArchiveFile into the Takeout.
    /// Returns an error if a file with the same archive path already exists.
    pub fn insert(&mut self, file: ArchiveFile) -> Result<(), TakeoutError> {
        if let Some(existing) = self.files.get(&file.archive_path) {
            return Err(TakeoutError::DuplicateFile {
                path: file.archive_path.clone(),
                existing_archive: existing.source_archive.clone(),
                new_archive: file.source_archive,
            });
        }

        self.files.insert(file.archive_path.clone(), file);
        Ok(())
    }

    /// Gets an ArchiveFile by its archive path
    pub fn get(&self, archive_path: &str) -> Option<&ArchiveFile> {
        self.files.get(archive_path)
    }

    /// Returns the number of files in the takeout
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Returns true if the takeout contains no files
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Returns an iterator over all files in the takeout
    pub fn files(&self) -> impl Iterator<Item = &ArchiveFile> {
        self.files.values()
    }

    /// Returns an iterator over JSON files that may be Google metadata sidecars.
    pub fn metadata_sidecar_candidates(&self) -> impl Iterator<Item = &ArchiveFile> {
        self.files
            .values()
            .filter(|f| f.is_google_metadata_sidecar_candidate())
    }

    /// Returns an iterator over album metadata files.
    pub fn album_metadata_files(&self) -> impl Iterator<Item = &ArchiveFile> {
        self.files.values().filter(|f| f.is_album_metadata())
    }

    /// Returns the list of source archives
    pub fn source_archives(&self) -> &[PathBuf] {
        &self.source_archives
    }

    /// Finds all files in a specific directory path within the archive
    pub fn files_in_directory(&self, dir_path: &str) -> Vec<&ArchiveFile> {
        let dir_prefix = if dir_path.ends_with('/') {
            dir_path.to_string()
        } else {
            format!("{}/", dir_path)
        };

        self.files
            .values()
            .filter(|f| f.archive_path.starts_with(&dir_prefix))
            .collect()
    }

    /// Finds a potential metadata file for a given photo file.
    /// Google Takeout uses the pattern: "photo.jpg" -> "photo.jpg.json" or
    /// "photo.jpg" -> "photo.jpg.supplemental-metadata.json". It can also
    /// emit duplicate-numbered metadata and long filenames truncated before
    /// ".json".
    pub fn find_metadata_for(&self, photo_path: &str) -> Option<&ArchiveFile> {
        // Prefer exact path candidates first; these cover normal files and the
        // simple truncated supplemental suffixes without scanning the archive.
        for suffix in metadata_sidecar_suffixes() {
            let candidate = format!("{}{}json", photo_path, suffix);
            if let Some(file) = self.files.get(&candidate) {
                return Some(file);
            }
        }

        // Then handle duplicate-numbered media, where the "(1)" moves from
        // the media filename stem to the metadata filename suffix.
        if let Some((base_photo_path, copy_marker)) = split_numbered_copy_path(photo_path) {
            for suffix in SUPPLEMENTAL_METADATA_SUFFIXES {
                let suffix_stem = suffix.strip_suffix('.').unwrap_or(suffix);
                let candidate = format!("{}{}{}.json", base_photo_path, suffix_stem, copy_marker);
                if let Some(file) = self.files.get(&candidate) {
                    return Some(file);
                }
            }
        }

        // Finally fall back to the fuzzy long-name rules. This is intentionally
        // last because ".json" metadata is otherwise too broad.
        self.find_truncated_metadata_for(photo_path)
    }

    fn find_truncated_metadata_for(&self, photo_path: &str) -> Option<&ArchiveFile> {
        let photo_parent = archive_parent_path(photo_path);
        let photo_name = archive_file_name(photo_path);
        let mut best_match: Option<(&ArchiveFile, usize)> = None;

        for file in self.files.values() {
            if !file.is_google_metadata_sidecar_candidate()
                || archive_parent_path(&file.archive_path) != photo_parent
            {
                continue;
            }

            let Some(score) = truncated_metadata_score(photo_name, file.file_name()) else {
                continue;
            };

            let should_replace = match best_match {
                None => true,
                Some((best_file, best_score)) => {
                    score > best_score
                        || (score == best_score && file.archive_path < best_file.archive_path)
                }
            };

            if should_replace {
                best_match = Some((file, score));
            }
        }

        best_match.map(|(file, _)| file)
    }
}

impl Default for Takeout {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_get() {
        let mut takeout = Takeout::new();
        let file = ArchiveFile::new(
            "Takeout/Google Photos/Album/photo.jpg".to_string(),
            PathBuf::from("archive1.zip"),
            0,
            1024,
        );

        assert!(takeout.insert(file).is_ok());
        assert_eq!(takeout.len(), 1);
        assert!(
            takeout
                .get("Takeout/Google Photos/Album/photo.jpg")
                .is_some()
        );
    }

    #[test]
    fn test_duplicate_detection() {
        let mut takeout = Takeout::new();

        let file1 = ArchiveFile::new(
            "Takeout/Google Photos/photo.jpg".to_string(),
            PathBuf::from("archive1.zip"),
            0,
            1024,
        );

        let file2 = ArchiveFile::new(
            "Takeout/Google Photos/photo.jpg".to_string(),
            PathBuf::from("archive2.zip"),
            0,
            1024,
        );

        assert!(takeout.insert(file1).is_ok());
        assert!(takeout.insert(file2).is_err());
    }

    #[test]
    fn test_find_metadata() {
        let mut takeout = Takeout::new();

        let photo = ArchiveFile::new(
            "Takeout/Google Photos/photo.jpg".to_string(),
            PathBuf::from("archive1.zip"),
            0,
            1024,
        );

        let metadata = ArchiveFile::new(
            "Takeout/Google Photos/photo.jpg.supplemental-metadata.json".to_string(),
            PathBuf::from("archive2.zip"),
            1,
            256,
        );

        takeout.insert(photo).unwrap();
        takeout.insert(metadata).unwrap();

        let found = takeout.find_metadata_for("Takeout/Google Photos/photo.jpg");
        assert!(found.is_some());
        assert!(found.unwrap().is_supplemental_metadata());
    }

    #[test]
    fn test_find_truncated_metadata() {
        let mut takeout = Takeout::new();

        let photo = ArchiveFile::new(
            "Takeout/Google Photos/photo.jpg".to_string(),
            PathBuf::from("archive1.zip"),
            0,
            1024,
        );

        let metadata = ArchiveFile::new(
            "Takeout/Google Photos/photo.jpg.supplemental-metadat.json".to_string(),
            PathBuf::from("archive2.zip"),
            1,
            256,
        );

        takeout.insert(photo).unwrap();
        takeout.insert(metadata).unwrap();

        let found = takeout.find_metadata_for("Takeout/Google Photos/photo.jpg");
        assert!(found.is_some());
        assert!(found.unwrap().is_supplemental_metadata());
    }

    #[test]
    fn album_metadata_files_are_not_media_sidecar_candidates() {
        let metadata = ArchiveFile::new(
            "Takeout/Google Photos/Album/metadata.json".to_string(),
            PathBuf::from("archive1.zip"),
            0,
            128,
        );

        assert!(metadata.is_album_metadata());
        assert!(!metadata.is_google_metadata_sidecar_candidate());
        assert!(!metadata.is_metadata());
    }

    #[test]
    fn album_metadata_files_are_listed_separately() {
        let mut takeout = Takeout::new();
        takeout
            .insert(ArchiveFile::new(
                "Takeout/Google Photos/Album/metadata.json".to_string(),
                PathBuf::from("archive1.zip"),
                0,
                128,
            ))
            .unwrap();
        takeout
            .insert(ArchiveFile::new(
                "Takeout/Google Photos/Album/photo.jpg.json".to_string(),
                PathBuf::from("archive1.zip"),
                1,
                128,
            ))
            .unwrap();

        assert_eq!(takeout.album_metadata_files().count(), 1);
        assert_eq!(takeout.metadata_sidecar_candidates().count(), 1);
    }
}
