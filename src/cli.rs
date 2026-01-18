use clap::{Parser, Subcommand};
use glob::glob;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "google-takeout-photo-fixer")]
#[command(about = "Fixes Google Takeout photo metadata issues", long_about = None)]
pub struct Cli {
    /// Turn on debugging
    #[arg(short, long, action = clap::ArgAction::SetTrue)]
    pub debug: bool,

    /// Dry run - show what would be done without making changes
    #[arg(short = 'n', long, action = clap::ArgAction::SetTrue)]
    pub dry_run: bool,

    /// Photo directory name inside the archive
    #[arg(short, long, default_value = "Google Photos")]
    pub photo_dir: String,

    /// Output directory for fixed files
    #[arg(short, long, default_value = "takeout-fixed")]
    pub output: PathBuf,

    /// Disable the progress bar
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub no_progress: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Fixes Google Takeout photo metadata issues
    Fix {
        /// Paths to .zip or .tar.gz files, directories containing them, or glob patterns like *.zip
        #[arg(required = true, num_args = 1.., value_parser = validate_path)]
        paths: Vec<PathBuf>,
    },
}

fn is_archive_file(path: &std::path::Path) -> bool {
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    file_name.ends_with(".zip") || file_name.ends_with(".tar.gz")
}

fn validate_path(s: &str) -> Result<PathBuf, String> {
    // Check if it looks like a glob pattern
    if s.contains('*') || s.contains('?') || s.contains('[') {
        // It's a glob pattern - but clap calls this per-argument,
        // so we need to handle this differently
        return Err(format!(
            "Glob pattern '{}' did not match any files. \
            Note: On Windows, you may need to quote the pattern or let the shell expand it.",
            s
        ));
    }

    let path = PathBuf::from(s);

    if !path.exists() {
        return Err(format!("Path does not exist: {}", s));
    }

    if path.is_dir() {
        // Directory is allowed - we'll scan it for archives
        return Ok(path);
    }

    if !path.is_file() {
        return Err(format!("Path must be a file or directory: {}", s));
    }

    if is_archive_file(&path) {
        Ok(path)
    } else {
        Err(format!("File must be a .zip or .tar.gz file: {}", s))
    }
}

/// Expands glob patterns and directories in a list of path arguments
pub fn expand_paths(paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();

    for path in paths {
        let path_str = path.to_string_lossy();

        // Check if it's a glob pattern
        if path_str.contains('*') || path_str.contains('?') || path_str.contains('[') {
            let matches: Vec<_> = glob(&path_str)
                .map_err(|e| format!("Invalid glob pattern '{}': {}", path_str, e))?
                .filter_map(|r| r.ok())
                .filter(|p| is_archive_file(p))
                .collect();

            if matches.is_empty() {
                return Err(format!(
                    "No .zip or .tar.gz files matched pattern: {}",
                    path_str
                ));
            }

            files.extend(matches);
        } else if path.is_dir() {
            // Scan directory for archive files
            let dir_files: Vec<_> = std::fs::read_dir(path)
                .map_err(|e| format!("Failed to read directory '{}': {}", path_str, e))?
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|p| p.is_file() && is_archive_file(p))
                .collect();

            if dir_files.is_empty() {
                return Err(format!(
                    "No .zip or .tar.gz files found in directory: {}",
                    path_str
                ));
            }

            files.extend(dir_files);
        } else {
            // Regular file, just use it directly
            files.push(path.clone());
        }
    }

    if files.is_empty() {
        return Err("No files to process".to_string());
    }

    // Sort for consistent ordering
    files.sort();

    Ok(files)
}