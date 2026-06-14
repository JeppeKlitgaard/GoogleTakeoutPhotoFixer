use std::collections::HashSet;
use std::path::PathBuf;

use takeout_fixer::archive::{ArchiveFile, Takeout};

const FIXTURE: &str = include_str!("../test_data/metadata_matches.txt");
const PHOTO_PREFIX: &str = "Takeout/Google Photos/Test Album/";

fn fixture_names() -> Vec<&'static str> {
    FIXTURE
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix('\'')
                .and_then(|line| line.strip_suffix('\''))
                .unwrap_or(trimmed)
        })
        .filter(|line| !line.is_empty())
        .collect()
}

fn archive_path(name: &str) -> String {
    format!("{PHOTO_PREFIX}{name}")
}

fn fixture_takeout(names: &[&str]) -> Takeout {
    let mut takeout = Takeout::new();

    for (index, name) in names.iter().enumerate() {
        takeout
            .insert(ArchiveFile::new(
                archive_path(name),
                PathBuf::from("fixture.zip"),
                index,
                0,
            ))
            .unwrap();
    }

    takeout
}

fn is_json(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".json")
}

#[test]
fn metadata_fixture_json_files_are_detected_as_sidecar_candidates() {
    let names = fixture_names();
    let metadata_names = names
        .iter()
        .filter(|name| is_json(name))
        .collect::<Vec<_>>();

    assert!(
        !metadata_names.is_empty(),
        "fixture should contain metadata files"
    );

    for (index, name) in metadata_names.iter().enumerate() {
        let file = ArchiveFile::new(archive_path(name), PathBuf::from("fixture.zip"), index, 0);

        assert!(
            file.is_google_metadata_sidecar_candidate(),
            "fixture metadata file was not detected: {name}"
        );
    }
}

#[test]
fn bare_json_sidecars_do_not_need_supplemental_suffixes() {
    let bare = ArchiveFile::new(
        archive_path("photo.jpg.json"),
        PathBuf::from("fixture.zip"),
        0,
        0,
    );
    let supplemental = ArchiveFile::new(
        archive_path("photo.jpg.supplemental-metadata.json"),
        PathBuf::from("fixture.zip"),
        1,
        0,
    );

    assert!(bare.is_google_metadata_sidecar_candidate());
    assert!(!bare.is_supplemental_metadata());
    assert!(supplemental.is_google_metadata_sidecar_candidate());
    assert!(supplemental.is_supplemental_metadata());
}

#[test]
fn metadata_fixture_media_files_match_unique_sidecars() {
    let names = fixture_names();
    let takeout = fixture_takeout(&names);
    let metadata_paths = names
        .iter()
        .filter(|name| is_json(name))
        .map(|name| archive_path(name))
        .collect::<HashSet<_>>();
    let media_names = names
        .iter()
        .copied()
        .filter(|name| !is_json(name))
        .collect::<Vec<_>>();
    let mut matched_metadata_paths = HashSet::new();

    assert!(
        !media_names.is_empty(),
        "fixture should contain media files"
    );

    for media_name in media_names {
        let media_path = archive_path(media_name);
        let found = takeout
            .find_metadata_for(&media_path)
            .unwrap_or_else(|| panic!("no metadata matched for fixture media file: {media_name}"));

        assert_eq!(
            archive_path(found.file_name()),
            found.archive_path,
            "fixture uses a flat test album, so matches should stay in that album"
        );
        assert!(
            metadata_paths.contains(&found.archive_path),
            "matched metadata was not present in fixture: {}",
            found.archive_path
        );
        assert!(
            matched_metadata_paths.insert(found.archive_path.clone()),
            "metadata sidecar matched more than one fixture media file: {}",
            found.archive_path
        );
    }
}

#[test]
fn representative_edge_cases_match_expected_sidecars() {
    let names = fixture_names();
    let takeout = fixture_takeout(&names);
    let cases = [
        (
            "wbretra_inyragva_fbaar-fnaxg_unaf_ang-1847.jpg",
            "wbretra_inyragva_fbaar-fnaxg_unaf_ang-1847.jpg.json",
        ),
        ("depbqr(1).png", "depbqr.png.supplemental-metadata(1).json"),
        (
            "gung_bar_gvzr_jura_v_jnf_snapl_jvgu_wnxbo.jpg",
            "gung_bar_gvzr_jura_v_jnf_snapl_jvgu_wnxbo.jpg..json",
        ),
        (
            "gung_bar_gvzr_jura_v_jnf_snapl_jvgu_orawnzva.jpg",
            "gung_bar_gvzr_jura_v_jnf_snapl_jvgu_orawnzva.w.json",
        ),
        (
            "gung_bar_gvzr_jura_v_jnf_snapl_jvgu_wnxbo_senhx.jpg",
            "gung_bar_gvzr_jura_v_jnf_snapl_jvgu_wnxbo_senh.json",
        ),
    ];

    for (media_name, metadata_name) in cases {
        let media_path = archive_path(media_name);
        let metadata_path = archive_path(metadata_name);
        let found = takeout
            .find_metadata_for(&media_path)
            .unwrap_or_else(|| panic!("no metadata matched for fixture media file: {media_name}"));

        assert_eq!(
            found.archive_path, metadata_path,
            "wrong metadata matched for fixture media file: {media_name}"
        );
    }
}
