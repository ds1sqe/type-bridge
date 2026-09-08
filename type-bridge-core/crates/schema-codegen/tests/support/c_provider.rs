use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const DATABASE_CREATED_MARKER: &str = "C test database created";
pub const SETUP: &str = include_str!("../../../../../tests/support/provider/main.rs");
pub const SETUP_LOCK: &[u8] = include_bytes!("../../../../../tests/support/provider/Cargo.lock");

fn manifest_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\")
}

pub struct IsolatedDatabase {
    pub cargo: OsString,
    pub manifest: PathBuf,
    pub target: PathBuf,
    pub environment: Vec<(String, String)>,
    pub active: bool,
    pub env_prefix: &'static str,
}

impl IsolatedDatabase {
    fn run(&self, mode: &str) -> Output {
        let mut command = Command::new(&self.cargo);
        command
            .args(["run", "--locked", "--quiet", "--manifest-path"])
            .arg(&self.manifest)
            .arg("--")
            .arg(mode)
            .env("CARGO_TARGET_DIR", &self.target)
            .env("TYPE_BRIDGE_TEST_ENV_PREFIX", self.env_prefix);
        for (name, value) in &self.environment {
            command.env(name, value);
        }
        command
            .output()
            .unwrap_or_else(|error| panic!("C live database {mode} did not launch: {error}"))
    }

    pub fn setup(&mut self) {
        let output = self.run("setup");
        let stdout = String::from_utf8_lossy(&output.stdout);
        self.active = stdout.lines().any(|line| line == DATABASE_CREATED_MARKER);
        assert!(
            output.status.success(),
            "C live database setup failed:\nstdout:\n{}\nstderr:\n{}",
            stdout,
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(self.active, "C live setup omitted its creation marker");
        assert!(stdout.contains("C test provider schema setup: passed"));
    }

    pub fn cleanup(&mut self) {
        let output = self.cleanup_with_retries();
        assert!(
            output.status.success(),
            "C live database cleanup failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        self.active = false;
    }

    fn cleanup_with_retries(&self) -> Output {
        let mut output = self.run("cleanup");
        for _ in 1..3 {
            if output.status.success() {
                break;
            }
            output = self.run("cleanup");
        }
        output
    }
}

impl Drop for IsolatedDatabase {
    fn drop(&mut self) {
        if self.active {
            let _ = self.cleanup_with_retries();
        }
    }
}

pub fn stage_setup(
    stage: &Path,
    environment: Vec<(String, String)>,
    env_prefix: &'static str,
    provider_schema: &str,
) -> IsolatedDatabase {
    let root = stage.join("setup");
    fs::create_dir_all(root.join("src")).expect("setup source directory creates");
    fs::write(root.join("src/main.rs"), SETUP).expect("setup source writes");
    fs::write(root.join("provider.tql"), provider_schema).expect("provider schema writes");
    let orm = Path::new(env!("CARGO_MANIFEST_DIR")).join("../orm");
    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        format!(
            "[package]\nname = \"type-bridge-test-provider\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n[[bin]]\nname = \"setup\"\npath = \"src/main.rs\"\n\n[dependencies]\ntype-bridge-orm = {{ path = \"{}\" }}\ntokio = {{ version = \"1\", features = [\"macros\", \"rt-multi-thread\"] }}\n\n[workspace]\n",
            manifest_path(&orm),
        ),
    )
    .expect("setup manifest writes");
    fs::write(root.join("Cargo.lock"), SETUP_LOCK).expect("setup lockfile is staged");
    IsolatedDatabase {
        cargo: env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")),
        manifest,
        target: env::var_os("ACCEPTANCE_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| stage.join("target")),
        environment,
        active: false,
        env_prefix,
    }
}
