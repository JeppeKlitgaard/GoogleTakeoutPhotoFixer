use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct TempDir {
    base: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let mut base = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        base.push(format!(
            "takeout-fixer-test-{}-{}-{}",
            prefix,
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&base).expect("Failed to create temp base dir");
        Self { base }
    }

    fn output_path(&self) -> PathBuf {
        self.base.join("output")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn run_fix(input: &Path, output: &Path) {
    let exe = env!("CARGO_BIN_EXE_takeout-fixer");
    let status = Command::new(exe)
        .arg("--no-progress")
        .arg("--output")
        .arg(output)
        .arg("fix")
        .arg(input)
        .status()
        .expect("Failed to run takeout-fixer");

    assert!(status.success(), "takeout-fixer exited with failure");
}

fn collect_files(root: &Path) -> BTreeSet<PathBuf> {
    let mut files = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).expect("Failed to read directory");
        for entry in entries {
            let entry = entry.expect("Failed to read directory entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                let rel = path
                    .strip_prefix(root)
                    .expect("Failed to compute relative path")
                    .to_path_buf();
                files.insert(rel);
            }
        }
    }

    files
}

fn compare_directories(expected: &Path, actual: &Path) {
    let expected_files = collect_files(expected);
    let actual_files = collect_files(actual);

    assert_eq!(
        expected_files, actual_files,
        "File sets differ between expected and actual output"
    );

    for rel in expected_files {
        let expected_path = expected.join(&rel);
        let actual_path = actual.join(&rel);
        let expected_bytes = fs::read(&expected_path)
            .unwrap_or_else(|_| panic!("Failed to read expected file: {}", expected_path.display()));
        let actual_bytes = fs::read(&actual_path)
            .unwrap_or_else(|_| panic!("Failed to read actual file: {}", actual_path.display()));
        assert_eq!(
            expected_bytes, actual_bytes,
            "File contents differ for {}",
            rel.display()
        );
    }
}

#[test]
fn integration_input_zipped_matches_expected_output() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("test_data").join("input_zipped");
    let expected = root.join("test_data").join("output");
    let temp = TempDir::new("input-zipped");
    let output = temp.output_path();

    run_fix(&input, &output);
    compare_directories(&expected, &output);
}

#[test]
fn integration_input_gzipped_matches_expected_output() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = root.join("test_data").join("input_gzipped");
    let expected = root.join("test_data").join("output");
    let temp = TempDir::new("input-gzipped");
    let output = temp.output_path();

    run_fix(&input, &output);
    compare_directories(&expected, &output);
}
