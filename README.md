# Yet Another Google Takeout Photo Metadata Fixer

A largely vibe-coded CLI tool to fix metadata in photos exported from Google Photos via Google Takeout.

In short, it inserts metadata that Google Photos has _detached_ into `.json` files back into the original media.

The implementation is relatively fast and safe. I struggled to get existing tools to even work on the raw Takeout archives
and had little faith in their ability to handle edge-cases like metadata and media being split across archives.

Please report any issues in a reproducible way or submit PR's.

## Installation

You can either grab the appropriate executable from the GitHub Releases or install it with `cargo binstall`:
```sh
cargo binstall TODO
```

## Usage

You must download your Google Photos data using [Google Takeout](https://takeout.google.com/) and store the archive(s) in a folder.

You may only get a single archive (preferred: `.zip` or `.tar.gz`), or multiple archives. Store all archives in a folder, say `MyTakeout`, then run:

```sh
takeout-fixer --photo-dir "Google Photos" --output-dir fix MyTakeout


## Alternatives

- [Joshua Holmes' Google Photos Metadata Fix](https://github.com/joshua-holmes/google-photos-metadata-fix)