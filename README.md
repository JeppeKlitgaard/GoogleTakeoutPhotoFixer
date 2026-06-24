# Yet Another Google Takeout Photo Metadata Fixer

A largely vibe-coded CLI tool to fix metadata in media exported from Google Photos via Google Takeout.

In short, it inserts metadata that Google Photos has _detached_ into `.json` files back into the original media.

The implementation is relatively fast and safe. I struggled to get existing tools to even work on the raw Takeout archives
and had little faith in their ability to handle edge-cases like metadata and media being split across archives.

This does not require any complicated setups or intricate pre-steps from the user.
Simply chuck your takeout archives in a folder and run the tool to get an output directory of fixed media.

Please report any issues in a reproducible way or submit PR's.

**No guarantees on the correctness of the tools output are made!**

## Installation

You can either grab the appropriate executable from the GitHub Releases or install it with `cargo binstall`:

```sh
cargo binstall takeout-fixer
```

## Usage

You must download your Google Photos data using [Google Takeout](https://takeout.google.com/) and store the archive(s) in a folder.

You may only get a single archive (preferred: `.zip` or `.tar.gz`), or multiple archives. Store all archives in a folder, say `MyTakeout`, then run:

```sh
takeout-fixer --photo-dir "Google Photos" --output fixed-photos fix MyTakeout
```

Where `"Google Photos"` is the name of the folder for your photos inside of the archives beneath the `Takeout` folder.
This needs to be specified since Google localises this to your account language. As an example, for Danish users an archive will folder structure:

```txt
takeout-XXXXYYZZTHHMMSSZ-P-123.zip/Takeout/Google Fotos/ALBUMS
```

Thus if your Google Photos is set up for a Danish account, you would use `--photo-dir "Google Fotos"`.

By default, the final summary lists any media files copied without matched metadata. Use
`--no-list-media-without-metadata` after `fix` to suppress that list.

By default, album `metadata.json` files are handled separately from media sidecars. The tool writes an
`album-metadata.md` summary at the output root and copies each album metadata JSON file into the matching
output album directory unchanged. Use `--no-album-metadata-summary` or `--no-copy-album-metadata-json`
after `fix` to disable either output.

## Edge cases handled

Google Takeout sidecar names vary. The matcher handles:

1. Bare JSON sidecars: `photo.jpg.json`.
2. Full supplemental sidecars: `photo.jpg.supplemental-metadata.json`.
3. Truncated supplemental suffixes: `photo.jpg.supplemental-metadat.json`, `photo.jpg.supple.json`, `photo.jpg.s.json`.
4. Duplicate media copy markers: `photo(1).jpg` can match `photo.jpg.supplemental-metadata(1).json`.
5. Very long filename truncation: metadata may drop a final stem character or retain only part of the original extension before `.json`.
6. Split archives: media and metadata can be spread across multiple `.zip` or `.tar.gz` files from the same Takeout export.

Exact matches are preferred before fuzzy long-name matching, and fuzzy matches are limited to the same album directory.

Image metadata is written into supported image files. For videos, matched sidecar metadata is embedded into supported
containers and the matched timestamp is also applied to the output file modified time. MP4/MOV files also get
QuickTime location metadata for compatibility with tools that read Apple's `location.ISO6709` key.

## Alternatives

- [Joshua Holmes' Google Photos Metadata Fix](https://github.com/joshua-holmes/google-photos-metadata-fix)
