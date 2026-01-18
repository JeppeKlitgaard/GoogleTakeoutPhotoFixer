pub mod archive;
pub mod cli;
pub mod metadata;
pub mod process;

use archive::{ArchiveFile, Takeout, TakeoutError};
use flate2::read::GzDecoder;
use process::process_takeout;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use tar::Archive as TarArchive;
use zip::ZipArchive;

pub fn run(args: cli::Cli) {
    if args.debug {
        println!("Debug mode enabled");
    }
    if args.dry_run {
        println!("Dry run mode - no changes will be made");
    }

    match args.command {
        Some(cli::Commands::Fix { paths }) => {
            // Check if output directory already exists
            if args.output.exists() && !args.dry_run {
                eprintln!(
                    "Error: Output directory '{}' already exists. Please remove it or specify a different output directory with --output.",
                    args.output.display()
                );
                std::process::exit(1);
            }

            println!("Output directory: {}", args.output.display());

            // Expand any glob patterns and directories
            let expanded_files = match cli::expand_paths(&paths) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            };

            let photo_path_prefix = format!("Takeout/{}/", args.photo_dir);

            println!("Processing {} archive(s)...", expanded_files.len());
            println!("Looking for files in: {}", photo_path_prefix);

            // Build the Takeout structure from all archives
            let mut takeout = Takeout::new();
            for file in &expanded_files {
                println!("\nReading archive: {}", file.display());
                if let Err(e) = load_archive_into_takeout(&mut takeout, file, &photo_path_prefix, args.debug) {
                    eprintln!("  Error: {}", e);
                    std::process::exit(1);
                }
            }

            println!("\n=== Takeout Summary ===");
            println!("Total files: {}", takeout.len());
            println!("Source archives: {}", takeout.source_archives().len());

            if args.debug {
                println!("\n{:#?}", takeout);
            }

            // Process the takeout (fix metadata and output files)
            let show_progress = !args.no_progress;
            match process_takeout(
                &takeout,
                &args.output,
                &photo_path_prefix,
                args.dry_run,
                args.debug,
                show_progress,
            ) {
                Ok(stats) => {
                    println!("\n=== Processing Complete ===");
                    println!("Total media processed: {}", stats.images_processed);
                    println!("Images with metadata applied: {}", stats.images_processed_with_metadata);
                    println!("Images without metadata: {}", stats.images_processed_without_metadata);
                    println!("Videos copied: {}", stats.videos_copied);
                    println!("Metadata applied: {}", stats.metadata_applied);
                    println!("Copied without metadata: {}", stats.media_copied_without_metadata);
                    if stats.unused_metadata_files > 0 {
                        println!("Unused metadata files: {}", stats.unused_metadata_files);
                    }
                    if stats.errors > 0 {
                        println!("Errors: {}", stats.errors);
                    }
                    if !args.dry_run {
                        println!("\nOutput written to: {}", args.output.display());
                    }
                }
                Err(e) => {
                    eprintln!("Error during processing: {}", e);
                    std::process::exit(1);
                }
            }
        }
        None => {
            println!("No command specified. Use --help for usage.");
        }
    }
}

fn load_archive_into_takeout(
    takeout: &mut Takeout,
    path: &Path,
    photo_path_prefix: &str,
    debug: bool,
) -> Result<(), TakeoutError> {
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    takeout.add_source_archive(path.to_path_buf());

    if file_name.ends_with(".zip") {
        load_zip_into_takeout(takeout, path, photo_path_prefix, debug)
    } else if file_name.ends_with(".tar.gz") {
        load_tar_gz_into_takeout(takeout, path, photo_path_prefix, debug)
    } else {
        Err(TakeoutError::Other(format!(
            "Unsupported archive format: {}",
            file_name
        )))
    }
}

fn load_zip_into_takeout(
    takeout: &mut Takeout,
    path: &Path,
    photo_path_prefix: &str,
    debug: bool,
) -> Result<(), TakeoutError> {
    let file =
        File::open(path).map_err(|e| TakeoutError::Other(format!("Failed to open file: {}", e)))?;
    let reader = BufReader::new(file);
    let mut archive = ZipArchive::new(reader)
        .map_err(|e| TakeoutError::Other(format!("Failed to read zip archive: {}", e)))?;

    let mut count = 0;
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| TakeoutError::Other(format!("Failed to read entry: {}", e)))?;
        let entry_path = entry.name().to_string();

        if entry_path.starts_with(photo_path_prefix) && !entry.is_dir() {
            let archive_file = ArchiveFile::new(
                entry_path.clone(),
                path.to_path_buf(),
                i,
                entry.size(),
            );

            if debug {
                println!("  Found: {}", entry_path);
            }

            takeout.insert(archive_file)?;
            count += 1;
        }
    }

    println!("  Loaded {} files from {}", count, path.display());
    Ok(())
}

fn load_tar_gz_into_takeout(
    takeout: &mut Takeout,
    path: &Path,
    photo_path_prefix: &str,
    debug: bool,
) -> Result<(), TakeoutError> {
    let file =
        File::open(path).map_err(|e| TakeoutError::Other(format!("Failed to open file: {}", e)))?;
    let reader = BufReader::new(file);
    let decoder = GzDecoder::new(reader);
    let mut archive = TarArchive::new(decoder);

    let entries = archive
        .entries()
        .map_err(|e| TakeoutError::Other(format!("Failed to read tar entries: {}", e)))?;

    let mut count = 0;
    let mut index = 0;
    for entry in entries {
        let entry =
            entry.map_err(|e| TakeoutError::Other(format!("Failed to read entry: {}", e)))?;
        let entry_path = entry
            .path()
            .map_err(|e| TakeoutError::Other(format!("Failed to get path: {}", e)))?;
        let entry_path_str = entry_path.to_string_lossy().to_string();

        if entry_path_str.starts_with(photo_path_prefix) && entry.header().entry_type().is_file() {
            let archive_file = ArchiveFile::new(
                entry_path_str.clone(),
                path.to_path_buf(),
                index,
                entry.size(),
            );

            if debug {
                println!("  Found: {}", entry_path_str);
            }

            takeout.insert(archive_file)?;
            count += 1;
        }
        index += 1;
    }

    println!("  Loaded {} files from {}", count, path.display());
    Ok(())
}
