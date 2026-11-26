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

use mriya::{
    Backend,
    BootVolumeAllocation,
    BootVolumePlan,
    BootVolumeType,
    InstanceHandle,
    InstanceRequest,
    RootVolume,
    ScalewayBackend,
    ScalewayConfig,
};

#[path = "scaleway_boot_volume/api.rs"]
mod api;
#[path = "scaleway_boot_volume/cases.rs"]
mod cases;

use api::{
    BLOCK_API,
    HTTP_TIMEOUT,
    INSTANCE_API,
    ReportedEntry,
    build_request,
    get_json,
    read_block_volume,
    read_server_root,
};
use cases::{Case, cases_from_env, enabled};

/// Confirms the handle's recorded root is the plan's.
fn ensure_record_matches_plan(
    handle: &InstanceHandle,
    record: &BootVolumeAllocation,
    plan: &BootVolumePlan,
) -> Result<(), String> {
    if record.root.volume_type == plan.volume_type && record.root.size_bytes == plan.size_bytes {
        return Ok(());
    }
    Err(format!(
        "instance {} recorded {} / {} bytes, but the plan asked for {} / {} bytes",
        handle.id,
        record.root.volume_type,
        record.root.size_bytes,
        plan.volume_type,
        plan.size_bytes
    ))
}

/// Confirms the server's boot entry is the root the handle recorded.
fn ensure_entry_matches_record(
    handle: &InstanceHandle,
    entry: &ReportedEntry,
    record: &BootVolumeAllocation,
) -> Result<(), String> {
    if entry.id != record.root.id {
        return Err(format!(
            "instance {} reports root {} but the handle recorded {}",
            handle.id, entry.id, record.root.id
        ));
    }
    if entry.volume_type != record.root.volume_type.provider_id() {
        return Err(format!(
            "instance {} reports root type {}, expected {}",
            handle.id,
            entry.volume_type,
            record.root.volume_type.provider_id()
        ));
    }
    Ok(())
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
        let Some((request, plan)) = self.resolve_case(case).await else {
            return;
        };
        let handle = match self.backend.create(&request).await {
            Ok(handle) => handle,
            Err(error) => {
                self.assertion_failures
                    .push(format!("create for {} failed: {error}", case.instance_type));
                return;
            }
        };
        self.verify_and_finish(case, handle, &plan).await;
    }

    /// Builds and preflights the request for one case.
    ///
    /// Returns `None` after recording the reason, so the caller simply moves
    /// on to the next case.
    async fn resolve_case(&mut self, case: &Case) -> Option<(InstanceRequest, BootVolumePlan)> {
        let request = match build_request(self.config, case) {
            Ok(request) => request,
            Err(error) => {
                self.assertion_failures.push(error);
                return None;
            }
        };
        match self.backend.preflight(&request).await {
            Ok(plan) => Some((request, plan)),
            Err(error) => {
                self.assertion_failures.push(format!(
                    "preflight for {} failed: {error}",
                    case.instance_type
                ));
                None
            }
        }
    }

    /// Verifies the live root, destroys the server, and reaps anything left.
    ///
    /// The destroy runs whatever the verification found, so one failing
    /// assertion cannot leave a billing server behind.
    async fn verify_and_finish(
        &mut self,
        case: &Case,
        handle: InstanceHandle,
        plan: &BootVolumePlan,
    ) {
        let zone = handle.zone.clone();
        let captured = handle
            .boot_volume
            .as_ref()
            .map(|allocation| allocation.root.clone());

        if let Err(error) = self.verify_live_root(&handle, plan).await {
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
    ///
    /// The steps are separate functions so each failure names one comparison:
    /// the handle against the plan, the server entry against the handle, and
    /// the reported size against the plan. A single function doing all three
    /// reads as one long conditional and hides which comparison failed.
    async fn verify_live_root(
        &self,
        handle: &InstanceHandle,
        plan: &BootVolumePlan,
    ) -> Result<(), String> {
        let record = handle
            .boot_volume
            .as_ref()
            .ok_or_else(|| format!("instance {} recorded no boot volume", handle.id))?;

        ensure_record_matches_plan(handle, record, plan)?;

        let entry = read_server_root(self.client, self.config, handle).await?;
        ensure_entry_matches_record(handle, &entry, record)?;

        self.ensure_reported_size(handle, &entry, plan).await
    }

    /// Compares the size the provider reports with the plan.
    ///
    /// A Block Storage root's size comes from the Block Storage API, because
    /// the Instances API does not report it reliably; a local root's size is
    /// already in the server entry.
    async fn ensure_reported_size(
        &self,
        handle: &InstanceHandle,
        entry: &ReportedEntry,
        plan: &BootVolumePlan,
    ) -> Result<(), String> {
        if plan.volume_type == BootVolumeType::Sbs {
            return self
                .ensure_block_volume(handle, &entry.id, plan.size_bytes)
                .await;
        }
        match entry.size {
            Some(size) if size == plan.size_bytes => Ok(()),
            other => Err(format!(
                "instance {} reports local root size {other:?}, expected {}",
                handle.id, plan.size_bytes
            )),
        }
    }

    /// Reads the Block Storage volume and compares it with the plan.
    async fn ensure_block_volume(
        &self,
        handle: &InstanceHandle,
        root_id: &str,
        planned_size: u64,
    ) -> Result<(), String> {
        let (block_size, block_id) =
            read_block_volume(self.client, self.config, &handle.zone, root_id).await?;
        if block_id != root_id {
            return Err(format!(
                "Block Storage returned volume {block_id} for {root_id}"
            ));
        }
        if block_size != planned_size {
            return Err(format!(
                "Block Storage volume {root_id} is {block_size} bytes, expected {planned_size}"
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
