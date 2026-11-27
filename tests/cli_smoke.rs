//! Behavioural smoke test for the CLI entrypoint.

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::{prelude::*, str::contains};
use tempfile::TempDir;

const PROJECT_ID: &str = "11111111-2222-3333-4444-555555555555";
const FILE_VOLUME_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const ENV_VOLUME_ID: &str = "ffffffff-1111-2222-3333-444444444444";

#[test]
fn cli_displays_help() {
    let mut cmd = cargo_bin_cmd!("mriya");
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(contains("run").and(contains("Provision")));
}

/// Writes `contents` to the discovered configuration file.
///
/// The helpers return `Result` rather than panicking so `expect_used` stays
/// strict outside a `#[test]` body, as the repository's lint policy requires.
fn write_config(dir: &TempDir, contents: &str) -> Result<(), String> {
    let dir_path = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
        .map_err(|path| format!("temp dir is not UTF-8: {}", path.display()))?;
    cap_std::fs_utf8::Dir::open_ambient_dir(&dir_path, cap_std::ambient_authority())
        .map_err(|error| format!("open temp dir: {error}"))?
        .write("mriya.toml", contents)
        .map_err(|error| format!("write config: {error}"))
}

/// Reads the discovered configuration file.
fn read_config(dir: &TempDir) -> Result<String, String> {
    let dir_path = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
        .map_err(|path| format!("temp dir is not UTF-8: {}", path.display()))?;
    cap_std::fs_utf8::Dir::open_ambient_dir(&dir_path, cap_std::ambient_authority())
        .map_err(|error| format!("open temp dir: {error}"))?
        .read_to_string("mriya.toml")
        .map_err(|error| format!("read config: {error}"))
}

/// Builds an `init` command that cannot reach the provider before the guard.
fn init_command(config_path: &std::path::Path) -> assert_cmd::Command {
    let mut cmd = cargo_bin_cmd!("mriya");
    cmd.env("MRIYA_CONFIG_PATH", config_path)
        .env("SCW_SECRET_KEY", "dummy-secret")
        .env("SCW_DEFAULT_PROJECT_ID", PROJECT_ID)
        .env_remove("SCW_DEFAULT_VOLUME_ID")
        .arg("init");
    cmd
}

/// Scenario: an environment volume ID is already effective.
///
/// Invariant: `mriya init` refuses before creating anything, because a new
/// volume recorded in the file would be ignored while `mriya run` keeps using
/// the environment ID.
#[test]
fn init_refuses_an_environment_volume_id_without_force() -> Result<(), String> {
    let dir = TempDir::new().map_err(|error| format!("tempdir: {error}"))?;
    let config = dir.path().join("mriya.toml");
    write_config(&dir, "")?;
    let mut cmd = init_command(&config);
    cmd.env("SCW_DEFAULT_VOLUME_ID", ENV_VOLUME_ID)
        .assert()
        .failure()
        .stderr(contains(format!("SCW_DEFAULT_VOLUME_ID={ENV_VOLUME_ID}")));
    if !read_config(&dir)?.is_empty() {
        return Err(String::from("the guard must not write the config"));
    }
    Ok(())
}

/// Scenario: the file configures a volume.
///
/// Invariant: the refusal names the file value, which must be replaced first,
/// and does not touch the provider.
#[test]
fn init_refuses_a_file_volume_id_without_force() -> Result<(), String> {
    let dir = TempDir::new().map_err(|error| format!("tempdir: {error}"))?;
    let config = dir.path().join("mriya.toml");
    write_config(
        &dir,
        &format!("[scaleway]\ndefault_volume_id = \"{FILE_VOLUME_ID}\"\n"),
    )?;

    init_command(&config)
        .assert()
        .failure()
        .stderr(contains(format!(
            "default volume ID already configured as {FILE_VOLUME_ID}"
        )));
    Ok(())
}

/// Scenario: the file and the environment both configure a volume.
///
/// Invariant: the refusal names the environment override first, because
/// `--force` would not make a new volume the one a run uses.
#[test]
fn init_reports_the_environment_override_before_the_file() -> Result<(), String> {
    let dir = TempDir::new().map_err(|error| format!("tempdir: {error}"))?;
    let config = dir.path().join("mriya.toml");
    write_config(
        &dir,
        &format!("[scaleway]\ndefault_volume_id = \"{FILE_VOLUME_ID}\"\n"),
    )?;

    let mut cmd = init_command(&config);
    cmd.env("SCW_DEFAULT_VOLUME_ID", ENV_VOLUME_ID)
        .assert()
        .failure()
        .stderr(contains(format!(
            "SCW_DEFAULT_VOLUME_ID={ENV_VOLUME_ID} is already in effect"
        )));
    Ok(())
}
