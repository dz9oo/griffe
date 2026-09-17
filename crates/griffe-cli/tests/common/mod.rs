//! Aides partagées par les tests d'intégration de la CLI (`cli_integration.rs`,
//! `closing_scenario.rs`) : coffre temporaire propre à chaque test, invocation in-process
//! via [`griffe_cli::run_capturing`], lecture des sorties `--json`.
//!
//! [`freeflow`] reste un vrai sous-processus : snapshots `--help`. [`capturing`] est
//! in-process sauf env hors `FREEFLOW_DB`, `current_dir`, ou retrait de `GRIFFE_TEST_KDF`
//! (Argon2 de prod). [`unlocked`] et [`provision`] restent dans le process du test.

#![allow(dead_code)]

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Output};
use std::sync::atomic::{AtomicU32, Ordering};

use assert_cmd::Command;
use assert_cmd::assert::OutputAssertExt;
use griffe_core::store::{Passphrase, Store};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Un répertoire propre à ce test, pas directement `<tmp>/vault.db` : entre autres,
/// `db_path.with_file_name("backups")` (utilisé aussi bien par la sauvegarde automatique que par
/// `passphrase change`) doit rester propre à un seul test, jamais un `<tmp>/backups` partagé par
/// tous les tests tournant en parallèle dans le même process.
pub fn temp_db(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let uniq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir()
        .join(format!(
            "griffe-cli-test-{label}-{}-{n}-{uniq}",
            std::process::id()
        ))
        .join("vault.db")
}

pub fn freeflow() -> Command {
    Command::cargo_bin("griffe").expect("le binaire griffe doit être compilé pour les tests")
}

/// Invocation in-process, sauf env hors `FREEFLOW_DB`, `current_dir` ou retrait de
/// `GRIFFE_TEST_KDF` — ces cas passent par le binaire (isolation process).
pub fn capturing() -> Capture {
    Capture::new()
}

/// Invocation in-process (`run_capturing`) d'un coffre déjà [`provision`]né.
/// Ne pas l'utiliser pour prouver qu'un coffre verrouillé refuse l'accès.
pub fn unlocked(db: &Path) -> Capture {
    let pass = db.with_extension("passphrase");
    let mut cmd = Capture::new();
    cmd.env("FREEFLOW_DB", db)
        .arg("--passphrase-file")
        .arg(&pass);
    cmd
}

/// Builder compatible avec le sous-ensemble `assert_cmd::Command` utilisé par les tests.
pub struct Capture {
    args: Vec<OsString>,
    db: Option<PathBuf>,
    extra_env: Vec<(OsString, OsString)>,
    current_dir: Option<PathBuf>,
    remove_test_kdf: bool,
}

impl Capture {
    fn new() -> Self {
        Self {
            args: Vec::new(),
            db: None,
            extra_env: Vec::new(),
            current_dir: None,
            remove_test_kdf: false,
        }
    }

    pub fn env(&mut self, key: impl AsRef<OsStr>, val: impl AsRef<OsStr>) -> &mut Self {
        if key.as_ref() == OsStr::new("FREEFLOW_DB") {
            self.db = Some(PathBuf::from(val.as_ref()));
        } else {
            self.extra_env
                .push((key.as_ref().to_os_string(), val.as_ref().to_os_string()));
        }
        self
    }

    pub fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        if key.as_ref() == OsStr::new("GRIFFE_TEST_KDF") {
            self.remove_test_kdf = true;
        }
        self
    }

    pub fn current_dir(&mut self, dir: impl AsRef<Path>) -> &mut Self {
        self.current_dir = Some(dir.as_ref().to_path_buf());
        self
    }

    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for arg in args {
            self.arg(arg);
        }
        self
    }

    fn needs_subprocess(&self) -> bool {
        self.remove_test_kdf || !self.extra_env.is_empty() || self.current_dir.is_some()
    }

    pub fn output(&mut self) -> std::io::Result<Output> {
        if self.needs_subprocess() {
            return self.output_via_bin();
        }
        let mut argv = vec![OsString::from("griffe")];
        if let Some(db) = &self.db {
            argv.push(OsString::from("--db"));
            argv.push(db.as_os_str().to_os_string());
        }
        argv.extend(self.args.iter().cloned());
        let (text, code) = griffe_cli::run_capturing(argv);
        Ok(output_from_capture(text, code))
    }

    fn output_via_bin(&self) -> std::io::Result<Output> {
        let mut cmd = freeflow();
        if let Some(db) = &self.db {
            cmd.env("FREEFLOW_DB", db);
        }
        for (key, val) in &self.extra_env {
            cmd.env(key, val);
        }
        if let Some(dir) = &self.current_dir {
            cmd.current_dir(dir);
        }
        if self.remove_test_kdf {
            cmd.env_remove("GRIFFE_TEST_KDF");
        }
        cmd.args(&self.args);
        cmd.output()
    }

    pub fn assert(&mut self) -> assert_cmd::assert::Assert {
        self.output()
            .expect("run_capturing n'échoue pas en IO")
            .assert()
    }
}

fn output_from_capture(text: String, code: i32) -> Output {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        let bytes = text.into_bytes();
        let (stdout, stderr) = if code == 0 {
            (bytes, Vec::new())
        } else {
            (Vec::new(), bytes)
        };
        Output {
            status: ExitStatus::from_raw(code << 8),
            stdout,
            stderr,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (text, code);
        unimplemented!("tests CLI in-process : Unix seulement")
    }
}

/// Écrit une passphrase dans un fichier temporaire en 0600 (Unix) et renvoie son chemin.
pub fn passphrase_file(db: &Path, passphrase: &str) -> PathBuf {
    let path = db.with_extension("passphrase");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, passphrase).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    path
}

/// Crée le coffre. Pas de `--remember` : les tests ne s'appuient pas sur le trousseau OS
/// (absent en CI, et en local il accumule des entrées). Les commandes suivantes du test
/// doivent utiliser [`unlocked`].
pub fn provision(db: &Path) {
    let _pass_file = passphrase_file(db, "s3cret");
    drop(Store::create(db, &Passphrase::from("s3cret")).unwrap());
}

pub fn json_result(output: &[u8]) -> serde_json::Value {
    serde_json::from_slice(output).expect("une sortie --json doit être du JSON valide")
}

pub fn create_client(db: &Path, name: &str) -> String {
    let output = unlocked(db)
        .args(["--json", "client", "create", "--name", name])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    json_result(&output)["result"].as_str().unwrap().to_string()
}
