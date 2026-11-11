//! The borrowed answer API is usable with strict C17 and C++17 consumers.

#[cfg(not(windows))]
#[path = "support/temp.rs"]
mod temp;

#[cfg(not(windows))]
#[test]
fn answer_cursors_compile_under_strict_c17_and_cpp17() {
    use std::path::Path;
    use std::process::Command;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let stage = temp::TempDirectory::new("answer-cursors");
    for (compiler, standard, language) in [("cc", "c17", "c"), ("c++", "c++17", "c++")] {
        let output = Command::new(compiler)
            .args([
                "-x",
                language,
                &format!("-std={standard}"),
                "-Wall",
                "-Wextra",
                "-Werror",
                "-fsyntax-only",
            ])
            .arg("-I")
            .arg(root.join("include"))
            .arg(root.join("tests/support/plan_answers.c"))
            .current_dir(stage.path())
            .output()
            .expect("C compiler available");
        assert!(
            output.status.success(),
            "{compiler}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
