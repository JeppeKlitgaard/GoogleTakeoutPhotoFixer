# Yet Another Google Takeout Photo Metadata Fixer

A largely vibe-coded CLI tool to fix metadata in photos exported from Google Photos via Google Takeout.

In short, it inserts metadata that Google Photos has _detached_ into `.json` files back into the original media.

The implementation is relatively fast and safe. I struggled to get existing tools to even work on the raw Takeout archives
and had little faith in their ability to handle edge-cases like metadata and media being split across archives.

This does not require any complicated setups or intricate pre-steps from the user.
Simply chuck your takeout archives in a folder and run the tool to get an output directory of fixed photos.

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
takeout-fixer --photo-dir "Google Photos" --output-dir fixed-photos fix MyTakeout
```

Where `"Google Photos"` is the name of the folder for your photos inside of the archives beneath the `Takeout` folder.
This needs to be specified since Google localises this to your account language. As an example, for Danish users an archive will folder structure:

```txt
takeout-XXXXYYZZTHHMMSSZ-P-123.zip/Takeout/Google Fotos/ALBUMS
```

Thus if your Google Photos is set up for a Danish account, you would use `--photo-dir "Google Fotos"`.

## Alternatives

- [Joshua Holmes' Google Photos Metadata Fix](https://github.com/joshua-holmes/google-photos-metadata-fix)