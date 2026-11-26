//! Opt-in live check that the root volume matches the requested boot plan.
//!
//! The loopback suites prove what Mriya sends; only a live account can prove
//! what Scaleway actually creates. This suite therefore creates real servers,
//! so it runs only when `MRIYA_RUN_SCALEWAY_BOOT_TESTS` is truthy. Case parsing
//! lives in [`cases`] and the independent provider reads in [`api`].
//!
//! Every created server is destroyed even when an assertion fails. The
//! captured root volume is then checked for survival, because deleting a
//! server through the API may or may not remove its Block Storage volumes;
//! ownership-aware cleanup is tracked in issue #154.

use std::io::Write;

use mriya::{Backend, BootVolumeType, InstanceHandle, RootVolume, ScalewayBackend, ScalewayConfig};

#[path = "scaleway_boot_volume/api.rs"]
mod api;
#[path = "scaleway_boot_volume/cases.rs"]
mod cases;

use api::{
    BLOCK_API,
    HTTP_TIMEOUT,
    INSTANCE_API,
    build_request,
    get_json,
    read_block_volume,
    read_server_root,
};
use cases::{Case, cases_from_env, enabled};

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
