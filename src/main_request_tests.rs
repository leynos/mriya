//! Unit tests for `mriya run` request construction.
//!
//! These tests replace what the CLI behaviour suite used to observe through a
//! feature-gated request dump. They parse a real command line with the real
//! Clap parser and build the request through the same seam the binary uses,
//! supplying both the configuration and the environment so no test reads or
//! mutates the process environment.

use super::*;

/// The Scaleway secret key a request build needs; the value is never sent.
const DUMMY_SECRET_KEY: &str = "dummy-secret";

/// The Scaleway project ID a request build needs; the value is never sent.
const DUMMY_PROJECT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// Builds the configuration a run with no overrides resolves.
///
/// The values are the defaults `ScalewayConfig` declares, so a request built
/// from this fixture is the request an empty configuration produces. The
/// fixture is explicit rather than loaded so the test reads no process
/// environment variable.
fn default_config() -> ScalewayConfig {
    ScalewayConfig {
        access_key: None,
        secret_key: String::from(DUMMY_SECRET_KEY),
        default_organization_id: None,
        default_project_id: String::from(DUMMY_PROJECT_ID),
        default_zone: String::from("fr-par-1"),
        default_instance_type: String::from("DEV1-S"),
        default_image: String::from("Ubuntu 24.04 Noble Numbat"),
        default_architecture: String::from("x86_64"),
        default_volume_id: None,
        cloud_init_user_data: None,
        cloud_init_user_data_file: None,
    }
}

/// Parses `args` as a `mriya run` invocation and builds its request.
///
/// Parsing goes through the real Clap definitions and the request through the
/// same builder the binary uses, so a test observes the values a run with
/// `config` and an unset environment would provision.
fn request_from_args(config: &ScalewayConfig, args: &[&str]) -> InstanceRequest {
    let cli = Cli::try_parse_from(args.iter().copied()).expect("the run arguments should parse");
    let Cli::Run(command) = cli else {
        panic!("the arguments should select the run subcommand");
    };
    let (_backend, request) =
        build_backend_and_request_with(config, &command, &unset_environment())
            .expect("the request should build");
    request
}

/// An environment that answers the variables a request build may read and
/// fails the test on any other key.
///
/// `HOME` answers `home` and `MRIYA_TEST_RUN_ID` is unset, which are the only
/// reads the build performs. Failing on anything else is what keeps "the test
/// reads no process environment" true rather than merely intended: a new
/// process-environment read introduced on this path breaks these tests.
fn request_environment(home: Option<&str>) -> mockable::MockEnv {
    let home_value = home.map(str::to_owned);
    let for_raw = home_value.clone();
    let for_string = home_value;
    let mut env = mockable::MockEnv::new();
    env.expect_raw().returning(move |key| match key {
        "HOME" => for_raw.clone().ok_or(std::env::VarError::NotPresent),
        mriya::TEST_RUN_ID_ENV => Err(std::env::VarError::NotPresent),
        other => panic!("unexpected environment read: {other}"),
    });
    env.expect_string().returning(move |key| match key {
        "HOME" => for_string.clone(),
        mriya::TEST_RUN_ID_ENV => None,
        other => panic!("unexpected environment read: {other}"),
    });
    env
}

/// An environment with no `HOME` and no test run identifier.
fn unset_environment() -> mockable::MockEnv { request_environment(None) }

#[test]
fn run_request_uses_the_configured_defaults() {
    let request = request_from_args(&default_config(), &["mriya", "run", "--", "echo", "ok"]);

    assert_eq!(request.instance_type, "DEV1-S");
    assert_eq!(request.image_label, "Ubuntu 24.04 Noble Numbat");
    assert_eq!(
        request.cloud_init_user_data, None,
        "a run without a cloud-init flag must request no user-data"
    );
}

#[test]
fn run_request_carries_inline_cloud_init_user_data() {
    let request = request_from_args(
        &default_config(),
        &["mriya", "run", "--cloud-init", "userdata-123", "--", "echo"],
    );

    assert_eq!(
        request.cloud_init_user_data.as_deref(),
        Some("userdata-123")
    );
}

#[test]
fn run_request_expands_a_tilde_cloud_init_file_through_the_injected_home() {
    let tmp = tempfile::TempDir::new().expect("a temporary home should be created");
    let home_path = camino::Utf8PathBuf::from_path_buf(tmp.path().to_path_buf())
        .expect("the temporary home path should be UTF-8");
    // The capability handle roots the fixture inside the temporary home, which
    // is also the only directory the expansion may read.
    cap_std::fs_utf8::Dir::open_ambient_dir(&home_path, cap_std::ambient_authority())
        .expect("the temporary home should open")
        .write("user-data.txt", "tilde-user-data")
        .expect("the tilde fixture should be written");
    let home = home_path.as_str().to_owned();
    let args = [
        "mriya",
        "run",
        "--cloud-init-file",
        "~/user-data.txt",
        "--",
        "echo",
    ];

    let cli = Cli::try_parse_from(args).expect("the run arguments should parse");
    let Cli::Run(command) = cli else {
        panic!("the arguments should select the run subcommand");
    };
    let (_backend, request) = build_backend_and_request_with(
        &default_config(),
        &command,
        &request_environment(Some(&home)),
    )
    .expect("the request should build");

    assert_eq!(
        request.cloud_init_user_data.as_deref(),
        Some("tilde-user-data"),
        "the tilde path must expand through the injected HOME, not the process environment"
    );
}

#[test]
fn run_request_reads_cloud_init_user_data_from_a_file() {
    let mut file = tempfile::Builder::new()
        .suffix(".txt")
        .tempfile()
        .expect("a cloud-init fixture file should be created");
    // The fully qualified call keeps the fixture write independent of any
    // `Write` binding in scope in this module.
    std::io::Write::write_all(&mut file, b"file-user-data")
        .expect("the cloud-init fixture should be written");
    let path = file
        .path()
        .to_str()
        .expect("the fixture path should be UTF-8")
        .to_owned();

    // An absolute path reads no environment variable: tilde expansion consults
    // HOME only for a path that starts with `~/`.
    let request = request_from_args(
        &default_config(),
        &[
            "mriya",
            "run",
            "--cloud-init-file",
            path.as_str(),
            "--",
            "echo",
        ],
    );

    assert_eq!(
        request.cloud_init_user_data.as_deref(),
        Some("file-user-data")
    );
}

#[test]
fn run_request_applies_instance_type_and_image_overrides() {
    let request = request_from_args(
        &default_config(),
        &[
            "mriya",
            "run",
            "--instance-type",
            "DEV1-M",
            "--image",
            "Debian 12 Bookworm",
            "--",
            "echo",
        ],
    );

    assert_eq!(request.instance_type, "DEV1-M");
    assert_eq!(request.image_label, "Debian 12 Bookworm");
}

#[test]
fn run_request_override_equal_to_the_default_beats_a_configured_value() {
    let mut config = default_config();
    config.default_instance_type = String::from("DEV1-XL");

    // The supplied value is the documented default, not the configured one.
    // An override that merely restates a default must still replace what the
    // configuration resolved, which is the v0.10.0 rule at the binary seam.
    let request = request_from_args(
        &config,
        &["mriya", "run", "--instance-type", "DEV1-S", "--", "echo"],
    );

    assert_eq!(
        request.instance_type, "DEV1-S",
        "an explicit override equal to the default must beat the configured value"
    );
}
