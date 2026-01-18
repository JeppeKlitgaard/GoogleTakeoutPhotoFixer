use std::collections::HashMap;
use std::path::{Path, PathBuf};

const SUPPLEMENTAL_SUFFIXES: &[&str] = &[
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

fn is_supplemental_metadata_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    SUPPLEMENTAL_SUFFIXES
        .iter()
        .any(|suffix| lower.ends_with(&format!("{}json", suffix)))
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

    /// Checks if this is a JSON metadata file
    pub fn is_metadata(&self) -> bool {
        self.archive_path.ends_with(".json")
    }

    /// Checks if this is a supplemental metadata file
    pub fn is_supplemental_metadata(&self) -> bool {
        is_supplemental_metadata_path(&self.archive_path)
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

    /// Returns an iterator over supplemental metadata files in the takeout
    pub fn supplemental_metadata_files(&self) -> impl Iterator<Item = &ArchiveFile> {
        self.files.values().filter(|f| f.is_supplemental_metadata())
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
    /// "photo.jpg" -> "photo.jpg.supplemental-metadata.json"
    pub fn find_metadata_for(&self, photo_path: &str) -> Option<&ArchiveFile> {
        for suffix in SUPPLEMENTAL_SUFFIXES {
            let candidate = format!("{}{}json", photo_path, suffix);
            if let Some(file) = self.files.get(&candidate) {
                return Some(file);
            }
        }
        None
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
        assert!(takeout.get("Takeout/Google Photos/Album/photo.jpg").is_some());
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
}
