//! Opt-in live check that the root volume matches the requested boot plan.
//!
//! The loopback suites prove what Mriya sends; only a live account can prove
//! what Scaleway actually creates. This suite therefore creates real servers,
//! so it runs only when `MRIYA_RUN_SCALEWAY_BOOT_TESTS` is truthy.
//!
//! When enabled it never skips silently. A missing credential, an unreadable
//! configuration, a provider permission error, or an empty case list is a
//! failure: an operator who asked for the check must not be told it passed
//! when it never ran.
//!
//! `MRIYA_TEST_BOOT_CASES` lists the cases to execute as
//! `instance_type:volume_type:size_gb` triples separated by semicolons, for
//! example `DEV1-S:l_ssd:20;DEV1-S:sbs_volume:20`. Sizes are decimal
//! gigabytes (10^9 bytes), matching the configuration surface.
//!
//! Every created server is destroyed even when an assertion fails. The
//! captured root volume is then checked for survival, because deleting a
//! server through the API may or may not remove its Block Storage volumes;
//! ownership-aware cleanup is tracked in issue #154.

use std::{io::Write, time::Duration};

use mockable::{DefaultEnv, Env};
use mriya::{
    Backend,
    BootVolumeRequest,
    BootVolumeType,
    InstanceHandle,
    InstanceRequest,
    RootVolume,
    ScalewayBackend,
    ScalewayConfig,
};

/// Variable that opts the live suite in.
const ENABLE_VARIABLE: &str = "MRIYA_RUN_SCALEWAY_BOOT_TESTS";

/// Variable listing the cases to execute.
const CASES_VARIABLE: &str = "MRIYA_TEST_BOOT_CASES";

/// Production Instances API base.
const INSTANCE_API: &str = "https://api.scaleway.com/instance/v1";

/// Production Block Storage API base.
const BLOCK_API: &str = "https://api.scaleway.com/block/v1";

/// Per-request budget for the independent reads this suite makes.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// One live case to execute.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Case {
    /// Commercial type to provision.
    instance_type: String,
    /// Boot storage type to request.
    volume_type: BootVolumeType,
    /// Capacity to request, in decimal gigabytes.
    size_gb: u32,
}

/// Reports whether the suite was opted into.
fn enabled() -> bool {
    DefaultEnv.raw(ENABLE_VARIABLE).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    })
}

/// Reads the configured cases, refusing an empty list.
///
/// # Errors
///
/// Returns a description naming the variable when the list is absent, empty,
/// or contains an unparseable case.
fn cases_from_env() -> Result<Vec<Case>, String> {
    let raw = DefaultEnv
        .raw(CASES_VARIABLE)
        .map_err(|_| missing_cases())?;
    let cases = parse_cases(&raw)?;
    if cases.is_empty() {
        return Err(missing_cases());
    }
    Ok(cases)
}

/// The failure reported when no case was listed.
fn missing_cases() -> String {
    format!("{CASES_VARIABLE} must list at least one instance_type:volume_type:size_gb case")
}

/// Parses the semicolon-separated case list.
///
/// For example, `DEV1-S:l_ssd:20;DEV1-S:sbs_volume:20` yields two cases.
///
/// # Errors
///
/// Returns the offending entry when a field is missing or unparseable.
fn parse_cases(raw: &str) -> Result<Vec<Case>, String> {
    raw.split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(parse_case)
        .collect()
}

/// Parses one `instance_type:volume_type:size_gb` triple.
fn parse_case(entry: &str) -> Result<Case, String> {
    let mut fields = entry.split(':');
    let instance_type = fields
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("case '{entry}' is missing its instance type"))?;
    let volume_type = fields
        .next()
        .ok_or_else(|| format!("case '{entry}' is missing its volume type"))?
        .trim()
        .parse::<BootVolumeType>()
        .map_err(|error| format!("case '{entry}': {error}"))?;
    let size_gb = fields
        .next()
        .ok_or_else(|| format!("case '{entry}' is missing its size in GB"))?
        .trim()
        .parse::<u32>()
        .map_err(|error| format!("case '{entry}': invalid size: {error}"))?;
    if fields.next().is_some() {
        return Err(format!("case '{entry}' has more than three fields"));
    }
    Ok(Case {
        instance_type: instance_type.to_owned(),
        volume_type,
        size_gb,
    })
}

/// Everything one live run needs, plus what it has learned.
struct LiveRun<'a> {
    /// Backend under test.
    backend: &'a ScalewayBackend,
    /// Client used for the independent reads.
    client: &'a reqwest::Client,
    /// Configuration supplying credentials and defaults.
    config: &'a ScalewayConfig,
    /// Failures where the provider disagreed with the plan.
    assertion_failures: Vec<String>,
    /// Failures while destroying a server or reaping a root volume.
    cleanup_errors: Vec<String>,
}

/// The live suite.
///
/// It is a single test because the cases share one account, one backend, and
/// one summary; splitting them would multiply the credential checks without
/// adding evidence.
#[tokio::test]
async fn live_boot_volume_matches_the_request() {
    if !enabled() {
        return;
    }

    let cases = cases_from_env().unwrap_or_else(|error| panic!("{error}"));
    let config = ScalewayConfig::load_from_sources()
        .unwrap_or_else(|error| panic!("live boot check cannot read configuration: {error}"));
    let backend = ScalewayBackend::new(config.clone())
        .unwrap_or_else(|error| panic!("live boot check cannot build the backend: {error}"));
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .unwrap_or_else(|error| panic!("live boot check cannot build an HTTP client: {error}"));

    let mut run = LiveRun {
        backend: &backend,
        client: &client,
        config: &config,
        assertion_failures: Vec::new(),
        cleanup_errors: Vec::new(),
    };
    for case in &cases {
        run.execute_case(case).await;
    }
    run.report(cases.len());
}

impl LiveRun<'_> {
    /// Runs one case, recording failures instead of aborting the suite.
    ///
    /// Recording rather than panicking is what guarantees the remaining cases
    /// still run and, more importantly, that a failing assertion cannot skip
    /// the destroy step for its own server.
    async fn execute_case(&mut self, case: &Case) {
        let request = match build_request(self.config, case) {
            Ok(request) => request,
            Err(error) => {
                self.assertion_failures.push(error);
                return;
            }
        };

        let plan = match self.backend.preflight(&request).await {
            Ok(plan) => plan,
            Err(error) => {
                self.assertion_failures.push(format!(
                    "preflight for {} failed: {error}",
                    case.instance_type
                ));
                return;
            }
        };

        let handle = match self.backend.create(&request).await {
            Ok(handle) => handle,
            Err(error) => {
                self.assertion_failures
                    .push(format!("create for {} failed: {error}", case.instance_type));
                return;
            }
        };

        let zone = handle.zone.clone();
        let captured = handle
            .boot_volume
            .as_ref()
            .map(|allocation| allocation.root.clone());

        if let Err(error) = self
            .verify_live_root(&handle, plan.volume_type, plan.size_bytes)
            .await
        {
            self.assertion_failures.push(error);
        }

        if let Err(error) = self.backend.destroy(handle).await {
            self.cleanup_errors.push(format!(
                "destroy for {} failed: {error}",
                case.instance_type
            ));
        }

        if let Some(root) = captured {
            self.reap_orphaned_root(&zone, &root).await;
        }
    }

    /// Compares the handle's record with the plan, then re-reads the provider.
    async fn verify_live_root(
        &self,
        handle: &InstanceHandle,
        planned_type: BootVolumeType,
        planned_size: u64,
    ) -> Result<(), String> {
        let allocation = handle
            .boot_volume
            .as_ref()
            .ok_or_else(|| format!("instance {} recorded no boot volume", handle.id))?;

        if allocation.root.volume_type != planned_type || allocation.root.size_bytes != planned_size
        {
            return Err(format!(
                "instance {} recorded {} / {} bytes, but the plan asked for {} / {} bytes",
                handle.id,
                allocation.root.volume_type,
                allocation.root.size_bytes,
                planned_type,
                planned_size
            ));
        }

        let (reported_id, reported_type, reported_size) =
            read_server_root(self.client, self.config, handle).await?;

        if reported_id != allocation.root.id {
            return Err(format!(
                "instance {} reports root {} but the handle recorded {}",
                handle.id, reported_id, allocation.root.id
            ));
        }
        if reported_type != planned_type.provider_id() {
            return Err(format!(
                "instance {} reports root type {reported_type}, expected {planned_type}",
                handle.id
            ));
        }

        if planned_type == BootVolumeType::Sbs {
            let (block_size, block_id) =
                read_block_volume(self.client, self.config, &handle.zone, &reported_id).await?;
            if block_id != reported_id {
                return Err(format!(
                    "Block Storage returned volume {block_id} for {reported_id}"
                ));
            }
            if block_size != planned_size {
                return Err(format!(
                    "Block Storage volume {reported_id} is {block_size} bytes, \
                     expected {planned_size}"
                ));
            }
        } else if reported_size != Some(planned_size) {
            return Err(format!(
                "instance {} reports local root size {reported_size:?}, \
                 expected {planned_size}",
                handle.id
            ));
        }

        Ok(())
    }

    /// Deletes the captured root if the server destroy left it behind.
    ///
    /// Deleting a server through the Instances API is documented to delete its
    /// attached volumes, but the Block Storage FAQ and the Instances API notes
    /// disagree about Block Storage in particular, so the check is empirical.
    /// Ownership-aware cleanup of a boot volume is issue #154's scope; this
    /// only stops the check itself from leaking a disk.
    async fn reap_orphaned_root(&mut self, zone: &str, root: &RootVolume) {
        let (base, label) = match root.volume_type {
            BootVolumeType::Sbs => (BLOCK_API, "Block Storage"),
            BootVolumeType::Local => (INSTANCE_API, "Instances"),
        };
        let url = format!("{base}/zones/{zone}/volumes/{}", root.id);

        match get_json(self.client, &url, &self.config.secret_key).await {
            Ok(None) => return,
            Ok(Some(_)) => {}
            Err(error) => {
                self.cleanup_errors.push(format!(
                    "cannot check whether root volume {} remains: {error}",
                    root.id
                ));
                return;
            }
        }

        let deleted = self
            .client
            .delete(&url)
            .header("X-Auth-Token", &self.config.secret_key)
            .send()
            .await;
        match deleted {
            Ok(response) if response.status().is_success() => {
                let mut stdout = std::io::stdout();
                writeln!(
                    stdout,
                    "note: deleted {label} root volume {} after the server destroy; \
                     ownership-aware cleanup is tracked in #154",
                    root.id
                )
                .ok();
            }
            Ok(response) => self.cleanup_errors.push(format!(
                "could not delete root volume {}: status {}",
                root.id,
                response.status()
            )),
            Err(error) => self
                .cleanup_errors
                .push(format!("could not delete root volume {}: {error}", root.id)),
        }
    }

    /// Writes the executed-case summary, failing when anything went wrong.
    ///
    /// Cleanup errors are reported separately from assertion errors because
    /// they mean different things: an assertion failure says the provider did
    /// not do what was asked, while a cleanup failure says the check may have
    /// leaked a resource.
    fn report(&self, executed: usize) {
        let mut stdout = std::io::stdout();
        writeln!(
            stdout,
            "boot-volume live check: {executed} case(s) executed, \
             {} assertion failure(s), {} cleanup error(s)",
            self.assertion_failures.len(),
            self.cleanup_errors.len()
        )
        .ok();
        for failure in &self.assertion_failures {
            writeln!(stdout, "assertion failure: {failure}").ok();
        }
        for error in &self.cleanup_errors {
            writeln!(stdout, "cleanup error: {error}").ok();
        }
        if !self.cleanup_errors.is_empty() {
            writeln!(
                stdout,
                "note: a root volume may still exist; ownership-aware cleanup \
                 is tracked in #154"
            )
            .ok();
        }

        assert!(
            self.assertion_failures.is_empty(),
            "live boot check found {} assertion failure(s)",
            self.assertion_failures.len()
        );
        assert!(
            self.cleanup_errors.is_empty(),
            "live boot check found {} cleanup error(s)",
            self.cleanup_errors.len()
        );
    }
}

/// Builds the request for one case, clearing the cache volume explicitly.
///
/// Clearing `volume_id` is deliberate: this check is about the boot disk, and
/// a configured cache volume would add an attachment this suite does not need
/// and would then have to clean up.
fn build_request(config: &ScalewayConfig, case: &Case) -> Result<InstanceRequest, String> {
    let mut request = config
        .as_request()
        .map_err(|error| format!("cannot build a request: {error}"))?;
    request.instance_type.clone_from(&case.instance_type);
    request.volume_id = None;
    request.boot_volume = BootVolumeRequest::new(case.volume_type, case.size_gb);
    Ok(request)
}

/// Reads the server's boot entry straight from the Instances API.
async fn read_server_root(
    client: &reqwest::Client,
    config: &ScalewayConfig,
    handle: &InstanceHandle,
) -> Result<(String, String, Option<u64>), String> {
    let url = format!(
        "{}/zones/{}/servers/{}",
        INSTANCE_API, handle.zone, handle.id
    );
    let body = get_json(client, &url, &config.secret_key)
        .await?
        .ok_or_else(|| format!("instance {} disappeared before it could be read", handle.id))?;
    let volumes = body
        .pointer("/server/volumes")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| format!("instance {} reports no volumes object", handle.id))?;

    let entry = volumes
        .iter()
        .find(|(_, volume)| volume.get("boot") == Some(&serde_json::Value::Bool(true)))
        .or_else(|| volumes.get_key_value("0"))
        .map(|(_, volume)| volume)
        .ok_or_else(|| format!("instance {} reports no boot volume", handle.id))?;

    let id = entry
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("instance {} root volume has no id", handle.id))?;
    let volume_type = entry
        .get("volume_type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("instance {} root volume has no type", handle.id))?;
    let size = entry.get("size").and_then(serde_json::Value::as_u64);
    Ok((id.to_owned(), volume_type.to_owned(), size))
}

/// Reads an SBS volume from the Block Storage API.
async fn read_block_volume(
    client: &reqwest::Client,
    config: &ScalewayConfig,
    zone: &str,
    volume_id: &str,
) -> Result<(u64, String), String> {
    let url = format!("{}/zones/{zone}/volumes/{volume_id}", BLOCK_API);
    let body = get_json(client, &url, &config.secret_key)
        .await?
        .ok_or_else(|| format!("Block Storage has no volume {volume_id}"))?;
    let size = body
        .get("size")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("Block Storage volume {volume_id} reports no size"))?;
    let id = body
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("Block Storage volume {volume_id} reports no id"))?;
    Ok((size, id.to_owned()))
}

/// Fetches `url`, returning `None` for a 404 and an error otherwise.
async fn get_json(
    client: &reqwest::Client,
    url: &str,
    token: &str,
) -> Result<Option<serde_json::Value>, String> {
    let response = client
        .get(url)
        .header("X-Auth-Token", token)
        .send()
        .await
        .map_err(|error| format!("request to {url} failed: {error}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("request to {url} returned {status}: {body}"));
    }
    response
        .json::<serde_json::Value>()
        .await
        .map(Some)
        .map_err(|error| format!("response from {url} was not JSON: {error}"))
}
