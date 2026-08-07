use super::*;
use std::fs;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "type-bridge-output-batch-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("test output root creates");
        Self(path)
    }

    fn output(&self) -> WorkspaceOutputDirectory {
        let root = WorkspaceRoot::new(fs::canonicalize(&self.0).expect("root canonicalizes"))
            .expect("canonical root validates");
        WorkspaceDirectoryAuthority::open(root)
            .expect("workspace authority opens")
            .output_root()
            .expect("output authority opens")
    }

    fn assert_no_sidecars(&self) {
        let mut directories = vec![self.0.clone()];
        while let Some(directory) = directories.pop() {
            for entry in fs::read_dir(directory).expect("test output directory reads") {
                let path = entry.expect("test output entry reads").path();
                if path.is_dir() {
                    directories.push(path);
                } else {
                    let name = path
                        .file_name()
                        .expect("entry has a file name")
                        .to_string_lossy();
                    assert!(
                        !name.contains("typebridge-tmp")
                            && !name.contains("typebridge-backup")
                            && !name.contains("typebridge-rollback"),
                        "generation sidecar leaked at {}",
                        path.display()
                    );
                }
            }
        }
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn existing_outputs_are_replaced_on_every_supported_platform() {
    let directory = TestDirectory::new();
    fs::create_dir(directory.0.join("nested")).expect("nested output directory creates");
    fs::write(directory.0.join("first.py"), b"first generation")
        .expect("first accepted output writes");
    fs::write(
        directory.0.join("nested/second.ts"),
        b"first nested generation",
    )
    .expect("nested accepted output writes");
    let output = directory.output();

    output
        .write_atomic_batch([
            (Path::new("first.py"), b"second generation".as_slice()),
            (
                Path::new("nested/second.ts"),
                b"second nested generation".as_slice(),
            ),
            (Path::new("new.rs"), b"new in second generation".as_slice()),
        ])
        .expect("a generation replaces existing outputs on this platform");

    assert_eq!(
        fs::read(directory.0.join("first.py")).expect("first output rereads"),
        b"second generation"
    );
    assert_eq!(
        fs::read(directory.0.join("nested/second.ts")).expect("nested output rereads"),
        b"second nested generation"
    );
    assert_eq!(
        fs::read(directory.0.join("new.rs")).expect("new output rereads"),
        b"new in second generation"
    );
    directory.assert_no_sidecars();
}

#[test]
fn induced_mid_commit_failure_restores_the_complete_prior_generation() {
    let directory = TestDirectory::new();
    fs::create_dir(directory.0.join("nested")).expect("nested output directory creates");
    fs::write(directory.0.join("first.py"), b"old first").expect("first accepted output writes");
    fs::write(directory.0.join("nested/second.ts"), b"old second")
        .expect("second accepted output writes");
    let output = directory.output();

    let error = output
        .write_atomic_batch_with(
            vec![
                (PathBuf::from("first.py"), b"new first".to_vec()),
                (PathBuf::from("nested/second.ts"), b"new second".to_vec()),
            ],
            |index| {
                if index == 1 {
                    Err("induced failure after the first final replacement".to_owned())
                } else {
                    Ok(())
                }
            },
        )
        .expect_err("the deterministic mid-commit fault must abort the batch");

    assert!(error.contains("induced failure after the first final replacement"));
    assert_eq!(
        fs::read(directory.0.join("first.py")).expect("first output rereads"),
        b"old first"
    );
    assert_eq!(
        fs::read(directory.0.join("nested/second.ts")).expect("second output rereads"),
        b"old second"
    );
    directory.assert_no_sidecars();
}

#[test]
fn oversized_existing_output_is_streamed_and_restored_without_a_memory_copy() {
    const OVERSIZED_LENGTH: u64 = 16 * 1024 * 1024 + 1;

    let directory = TestDirectory::new();
    let oversized_path = directory.0.join("oversized.py");
    let mut oversized = fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&oversized_path)
        .expect("oversized accepted output creates");
    oversized
        .set_len(OVERSIZED_LENGTH)
        .expect("sparse accepted output sizes");
    oversized
        .write_all(b"head")
        .expect("accepted output prefix writes");
    oversized
        .seek(io::SeekFrom::End(-4))
        .expect("accepted output suffix seeks");
    oversized
        .write_all(b"tail")
        .and_then(|()| oversized.sync_all())
        .expect("accepted output suffix writes");
    drop(oversized);
    fs::write(directory.0.join("later.ts"), b"old later").expect("later accepted output writes");
    let output = directory.output();

    output
        .write_atomic_batch_with(
            vec![
                (PathBuf::from("oversized.py"), b"new small output".to_vec()),
                (PathBuf::from("later.ts"), b"new later".to_vec()),
            ],
            |index| {
                if index == 1 {
                    Err("induced failure after oversized replacement".to_owned())
                } else {
                    Ok(())
                }
            },
        )
        .expect_err("the oversized rollback probe must abort mid-commit");

    assert_eq!(
        fs::metadata(&oversized_path)
            .expect("restored oversized output inspects")
            .len(),
        OVERSIZED_LENGTH
    );
    let mut restored = fs::File::open(&oversized_path).expect("restored oversized output opens");
    let mut marker = [0_u8; 4];
    restored
        .read_exact(&mut marker)
        .expect("restored prefix reads");
    assert_eq!(&marker, b"head");
    restored
        .seek(io::SeekFrom::End(-4))
        .expect("restored suffix seeks");
    restored
        .read_exact(&mut marker)
        .expect("restored suffix reads");
    assert_eq!(&marker, b"tail");
    assert_eq!(
        fs::read(directory.0.join("later.ts")).expect("later output rereads"),
        b"old later"
    );
    directory.assert_no_sidecars();
}

#[test]
fn hostile_later_target_rejects_before_earlier_output_mutation() {
    let directory = TestDirectory::new();
    fs::write(directory.0.join("first.py"), b"accepted first").expect("accepted output writes");
    fs::create_dir(directory.0.join("hostile.json")).expect("hostile final directory creates");
    let output = directory.output();

    let error = output
        .write_atomic_batch([
            (Path::new("first.py"), b"replaced first".as_slice()),
            (Path::new("hostile.json"), b"never published".as_slice()),
        ])
        .expect_err("a later special final entry must reject the whole batch");

    assert!(error.contains("regular file, not a link or special entry"));
    assert_eq!(
        fs::read(directory.0.join("first.py")).expect("accepted output rereads"),
        b"accepted first"
    );
    assert!(directory.0.join("hostile.json").is_dir());
    directory.assert_no_sidecars();
}

#[test]
fn portable_case_aliases_reject_before_any_output_is_published() {
    let directory = TestDirectory::new();
    let output = directory.output();

    let error = output
        .write_atomic_batch([
            (Path::new("Generated/Model.py"), b"first".as_slice()),
            (Path::new("generated/model.py"), b"second".as_slice()),
        ])
        .expect_err("portable case aliases must collide on every host");

    assert!(error.contains("collide in one atomic batch"));
    assert!(!directory.0.join("Generated").exists());
    assert!(!directory.0.join("generated").exists());
    directory.assert_no_sidecars();

    let error = output
        .write_atomic_batch([
            (Path::new("generated/caf\u{e9}.py"), b"first".as_slice()),
            (Path::new("generated/cafe\u{301}.py"), b"second".as_slice()),
        ])
        .expect_err("portable normalization aliases must collide on every host");

    assert!(error.contains("collide in one atomic batch"));
    assert!(!directory.0.join("generated").exists());
    directory.assert_no_sidecars();
}

#[test]
fn concurrent_reappearance_is_never_overwritten_during_publication() {
    let directory = TestDirectory::new();
    let destination = directory.0.join("model.py");
    fs::write(&destination, b"accepted generation").expect("accepted output writes");
    let output = directory.output();

    let error = output
        .write_atomic_batch_with(
            vec![(PathBuf::from("model.py"), b"new generation".to_vec())],
            |_| {
                fs::write(&destination, b"concurrent writer")
                    .map_err(|error| format!("concurrent write failed: {error}"))
            },
        )
        .expect_err("no-replace publication must detect a concurrent destination");

    assert!(error.contains("without replacing a concurrent destination"));
    assert_eq!(
        fs::read(&destination).expect("concurrent destination rereads"),
        b"concurrent writer"
    );
    let recovery_backups = fs::read_dir(&directory.0)
        .expect("output root reads")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains("typebridge-backup")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        recovery_backups.len(),
        1,
        "the displaced accepted generation must remain recoverable"
    );
    assert_eq!(
        fs::read(recovery_backups[0].path()).expect("recovery backup reads"),
        b"accepted generation"
    );
}
