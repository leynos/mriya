# RFC 0003: Ubicloud backend

- Status: proposed.
- Date: 29 October 2026.
- Audience: Mriya implementers and operators.
- Baseline: `fd7d8193fae29d54867cc372dc30efdc6fab35bb`.
- Dependencies: the [shared backend contract](README.md).

## Summary

Add an Ubicloud backend that provisions a managed virtual machine (VM), runs
the existing SSH and rsync workflow, and deletes the VM after execution.
Ubicloud provide a public, beta REST API for this lifecycle.[^1] The initial
implementation targets CPU workloads on qualified x86-64 images and regions. It
treats GPU execution, machine-image management, and persistent caches as
separate capabilities with their own acceptance gates.

The backend uses managed Compute. GitHub Actions runners and their cache
service have different interfaces and prices; Mriya must not use their rates to
estimate an ordinary VM. Operating an Ubicloud control plane on rented hardware
also remains out of scope.[^2]

## Current implementation and provider contract

Mriya already separates provisioning through `Backend::create`,
`wait_for_ready`, and `destroy`. Its CLI still constructs `ScalewayConfig`,
while the request carries Scaleway-oriented project, zone, image, and volume
fields. The runner mounts `/dev/vdb` and detects cloud-init completion through
a fixed marker. These assumptions require the shared refactor before the new
backend becomes selectable.[^3]

Table 1 records the provider contracts used by this proposal. The reviewed
documentation establishes API behaviour; it does not establish availability for
a particular account or a completed Mriya integration test.

| Concern        | Verified contract                                                                                                                                          | Proposed use                                                              |
| -------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| Authentication | Project personal access token in a bearer authorization header.[^1]                                                                                        | Read the token from `MRIYA_UBICLOUD_TOKEN`; redact it from diagnostics.   |
| Creation       | `POST /project/{project}/location/{location}/vm/{name}` accepts an SSH public key, image, size, storage size, subnet, user, and initialization script.[^4] | Use a generated run name and an explicit, validated request.              |
| Identity       | Names are unique within project and location; the API documents name-based idempotency and static access by ID.[^1]                                        | Persist the name before creation, then record and use the returned VM ID. |
| Observation    | VM details include state, public addresses, image, size, storage capacity, user, subnet, and GPU configuration.[^5]                                        | Check the observed allocation and readiness against the request.          |
| Deletion       | The API accepts `DELETE`; the provider implementation schedules asynchronous destruction.[^6]                                                              | Poll until the VM disappears before declaring cleanup complete.           |
| Storage        | Managed Compute includes local block storage; creation accepts a capacity in GiB.[^2][^4]                                                                  | Treat the disk as disposable with the VM.                                 |
| Networking     | A VM can join an existing private subnet; firewalls apply to that subnet.[^7]                                                                              | Require an explicitly selected, preconfigured subnet initially.           |

The adapter uses `https://api.ubicloud.com` through Mriya's HTTP stack. The
provider's `ubi` CLI remains useful for operators but does not become a runtime
dependency. This keeps subprocess parsing and CLI login state out of the
backend's control path.

## Proposed configuration

All settings below are proposed. Current Mriya releases do not accept this
backend selector or the `[ubicloud]` section. The shared contract defines
selection precedence and preserves existing Scaleway configuration.

```toml
[mriya]
backend = "ubicloud"

[ubicloud]
project_id = "pjkkmx0f2vke4h36nk9cm8v8q0"
location = "eu-central-h1"
instance_type = "standard-4"
image = "ubuntu-noble"
architecture = "x86_64"
storage_size_gib = 80
public_key_file = "~/.ssh/id_ed25519.pub"
private_subnet_id = "ps-existing-subnet-id"
enable_ipv4 = true
unix_user = "ubi"
# init_script_file = "./bootstrap.sh"

[sync]
ssh_user = "ubi"
ssh_identity_file = "~/.ssh/id_ed25519"
remote_path = "/home/ubi/project"
route_build_caches = false
create_cache_directories = false
```

The project and subnet identifiers illustrate the shape; operators supply their
own. `ubuntu-noble` appears in the reviewed provider image catalogue; the
implementation must qualify that image before selecting it by default.[^8] The
token stays outside committed TOML. Other Ubicloud settings use the
`MRIYA_UBICLOUD_` environment prefix.

Map `location` to the provider location, `instance_type` to `size`, `image` to
`boot_image`, `storage_size_gib` to `storage_size`, and `enable_ipv4` to
`enable_ip4`. Expand the public-key path locally and send only its contents.
Validate that the private key used by SSH corresponds to the supplied public
key. Never upload the private key or the API token to the VM. Validate
`storage_size` against the selected size's `storage_size_options`; arbitrary
positive GiB values are not necessarily valid. Reject unsupported capacities
without rounding or substituting another allocation.[^8]

Use `mriya run --backend ubicloud -- cargo test` after implementation. Existing
`--instance-type` and `--image` overrides apply to the selected backend.
Validation examines only the selected provider's credentials. Reject an
unsupported architecture, GPU request, detachable volume, or raw cloud-init
payload before creating any resource. A configured `unix_user` and an explicit
`[sync].ssh_user` must agree; otherwise report the conflict locally.

## Provisioning and execution

The following sequence applies the shared durable run ledger to Ubicloud. Each
transition records enough identity to resume cleanup after interruption.

1. Validate credentials, image qualification, capacity, SSH identity,
   bootstrap size, and the selected subnet. Generate one `mriya-<run-id>` name
   within the provider's naming limits and persist it with the request digest.
   Record the supplied subnet as borrowed.
2. Create the VM using that name. Record its immutable ID, project, location,
   and owned status immediately after a successful response. Do not regenerate
   the name when retrying an ambiguous request.
3. On a timeout or lost response, look up the original name. Compare the
   returned identity and observable configuration with the ledger. A matching
   name alone never authorizes adoption or deletion. Report a collision or
   unresolved ownership instead of creating a second VM.
4. Poll VM details with a deadline and bounded backoff. Require a qualified
   running state and a usable address. Treat waiting for capacity as a pending
   state until the deadline; terminal failures trigger cleanup.
5. Establish authenticated SSH using the selected user, port, identity, and
   host-key policy. Confirm successful bootstrap and the availability of
   `rsync` before transferring the workspace.
6. Reuse Mriya's file filters, working-directory handling, streamed output,
   and remote exit-status propagation. Record workload and transfer durations
   separately from provisioning and teardown.
7. Delete the owned VM and poll its static identity until absent. Preserve
   command failure and cleanup failure as separate results. Cancellation,
   bootstrap failure, and sync failure enter the same cleanup path.

Provider documentation calls creation idempotent, but the adapter must still
handle duplicate-name errors and compare the observed resource.[^1] Once it
knows the ID, it uses static resource access so a later rename cannot redirect
cleanup to another VM: `GET` and `DELETE` use
`/project/{project}/location/{location}/vm/id/{id}`.[^1] Authentication
failures and inaccessible projects are not evidence that a resource disappeared.

VM details expose no request digest, SSH public key, or initialization script.
If both the response and immutable ID were lost, visible configuration alone
cannot establish ownership. Keep the allocation unresolved unless independent
provider evidence ties it to the recorded creation request; document manual
reconciliation for the initial implementation.

## Bootstrap and SSH readiness

Ubicloud's `init_script` is a shell command in the provider-generated cloud-init
`runcmd` sequence. It does not accept an arbitrary cloud-config document. The
reviewed implementation limits its length to 2,000 characters.[^9] Forwarding
Mriya's current `cloud_init_user_data` unchanged would therefore misrepresent
the supported contract.

For the first implementation, `[ubicloud].init_script_file` accepts a POSIX
shell script. Mriya composes a bounded script that installs required transport
tools for the qualified image, invokes the supplied script in a separate POSIX
shell process, checks its exit status, and writes a run-specific success marker
only after success. An explicit `exit` in the supplied script must not bypass
the wrapper. Validate the complete payload, including Mriya's wrapper, against
the 2,000-character limit before creation. Reject larger payloads with an
actionable error; do not truncate them or silently fetch an external script.
Reject the existing raw `--cloud-init` and `--cloud-init-file` options for this
backend.

Readiness requires successful cloud-init status and the run-specific marker,
not only the existence of `boot-finished`. A machine image can contain stale
completion files, and a completed boot can still include failed provisioning.
The initial qualification suite covers stock `ubuntu-noble`; adding another
image requires a working bootstrap recipe and the same failure tests.

Use the existing SSH transport after the shared endpoint changes. Ubicloud's
managed-service guide uses the `ubi` account and supports public IPv4 or
IPv6.[^10] An IPv6-only setting requires verified client reachability; failure
must not silently allocate a chargeable IPv4 address. Propagate the configured
host-key policy to readiness probes, rsync, and command execution consistently.
If no trusted fingerprint exists, expose the policy choice explicitly; an
unauthenticated key scan does not authenticate the host.

## Caches, images, and resource ownership

The first backend has no persistent attached-volume cache. The reviewed public
API documents a local disk capacity but no independent attachable volume
resource.[^4] Reject `mriya init --backend ubicloud` as unsupported before an
API mutation. Capability gating disables the `/dev/vdb` mount path and mounted
cache routing. A requested persistent cache produces an error, not a quiet
fallback to disposable storage.

Caches on the VM's local disk survive within one run and disappear when that VM
is destroyed. The RFC does not introduce a retained VM pool or claim cache
reuse between independent runs. An object-store cache can follow under a
separate transfer and consistency design.

Ubicloud Machine Images now support reusable image versions. Their current
contract requires a stopped source VM with one disk no larger than 40 GiB; the
image and new VM must share a location.[^11] These constraints make them
unsuitable as an automatic snapshot of the example 80 GiB worker. A later
image-baking capability can prepare a smaller source image, generalize it, and
pin an immutable version. It must account for image storage and scrub
credentials and project data before capture.[^12]

The first backend borrows a preconfigured subnet and creates only the VM. It
must not delete that subnet, its firewalls, registered SSH keys, or referenced
images. The provider handles VM-associated storage and addresses; qualification
must establish that VM disappearance ends their allocation charges. The ledger
retains unresolved cleanup records for operator reconciliation.

Requiring a subnet avoids relying on implicit network creation and broad
default firewall rules. Validate a policy that permits SSH from the intended
client network. Do not rewrite shared firewall rules during a run. Ubicloud
document subnet-wide ingress filtering and no outbound-rule support, so the
adapter must not promise provider-enforced egress isolation.[^13]

## Billing and failure handling

Use managed Compute rates and provisioned resource lifetime. Ubicloud document
per-minute consumption accounting; their current Germany table lists
`standard-4` at 4 vCPUs, 16 GB RAM, 80 GB storage, and $0.00208 per minute.[^2]
Treat that value as a dated example, excluding any additional IPv4, storage,
egress, tax, or future price changes. Ubicloud describe standard VMs as
dedicated CPU. That allocation still does not establish equal throughput across
providers; do not substitute the Actions runner price.[^2]

The cost record includes the selected rate's source date and currency,
provisioning-to-deletion duration, allocation settings, cache policy, and any
unresolved resources. Report an estimate when the billing boundaries or rate
are unknown. Use provider-confirmed metering rules for final calculations;
per-minute granularity alone does not prove a separate minimum or a monthly
cap. Benchmark cost per completed gate with cold and warm in-run caches.

Stopping does not end charges. The provider's current implementation warns that
stopped VMs continue billing and directs users to delete them.[^6] Mriya
therefore uses deletion as its terminal operation and does not implement a
stop-based cost-saving mode.

Table 2 defines failure behaviour independently of the remote command result.

| Failure                                    | Required behaviour                                                                            |
| ------------------------------------------ | --------------------------------------------------------------------------------------------- |
| Invalid or expired token                   | Fail authentication; do not retry creation under another identity.                            |
| Rate limit or transient server error       | Apply bounded backoff and any documented retry guidance; reconcile before repeating a create. |
| Capacity unavailable                       | Respect the deadline, delete any owned pending VM, and retain cleanup evidence.               |
| SSH or bootstrap failure                   | Preserve useful redacted diagnostics and delete the owned VM.                                 |
| Delete accepted but VM remains             | Continue polling within the cleanup deadline; then report incomplete cleanup and the VM ID.   |
| Process interrupted during creation        | Reconcile the persisted run name before deleting or retrying.                                 |
| Existing resource differs from the request | Refuse adoption and deletion; report the conflicting identity.                                |

## GPU boundary

The public VM API includes a `gpu` field with `count:type` syntax.[^4] Current
provider source gates GPU requests on project enablement and a matching rate
for the location; it rejects GPUs on burstable VMs.[^14] GPU support therefore
exists as a provider capability, but the API field alone does not establish
availability, price, driver readiness, or reproducible execution for Mriya.

The initial backend rejects GPU requests. A later qualification must pin the
GPU type and count, allowed region and project, compatible image and drivers,
memory requirements, actual billing contract, and cleanup behaviour. It must
run a representative GPU workload and verify the returned device before
advertising support. Actions GPU runner availability does not satisfy this gate
for managed VMs.

## Delivery and acceptance

1. Implement selected-backend configuration, typed requests, capability
   checks, endpoint propagation, and the durable ownership ledger. Existing
   Scaleway users must retain their configuration and command behaviour.
2. Add an HTTP adapter with fixture-based creation, observation, reconciliation,
   and deletion tests. Cover duplicate names, malformed responses, unknown
   states, deadlines, authentication failures, and interrupted requests.
3. Add qualified bootstrap and SSH transport tests. Prove that scripts cannot
   exceed the provider limit, failed provisioning prevents workspace sync, and
   unsupported cloud-init, GPU, and cache settings allocate nothing. Cover
   successful completion, explicit `exit 0`, and non-zero script exits.
4. Run an explicitly enabled managed-Compute integration suite: create, sync,
   successful and failing commands, cancellation, and deletion. Compare the
   final inventory and billable-resource evidence with the initial inventory.
   Keep paid tests outside the default test suite.
5. Document the qualified image, region, API contract, measured timings, and
   a cost-per-gate example before enabling the backend for ordinary use.

The RFC proposes implementation work; it records no paid test or performance
measurement. Acceptance requires evidence for cleanup and bootstrap as well as
successful command execution.

## Alternatives and unresolved questions

Wrapping `ubi` would reuse its authentication and discovery, but would add a
versioned executable and output contract to every run. Direct API integration
fits the existing backend boundary. Using GitHub Actions would require a
workflow-dispatch and artefact protocol rather than Mriya's interactive SSH
execution model. Neither alternative supplies a detachable cache volume.

Before enabling the backend, resolve these questions:

- Which API contract or source revision will the adapter qualify against
  while the public API remains beta?
- Which authenticated catalogue or maintained compatibility data establishes
  valid image, size, capacity, and region combinations without creation?
- Which API evidence confirms allocation and billing termination after a
  deletion, including initially pending VMs?
- Which host-key trust mechanism will each supported deployment use?
- Does the qualified hosted service match the reviewed initialization-script
  limit and shell semantics?

## References

Sources checked on 29 October 2026. Provider-source observations use commit
`39353394d420818c66e8355f12973c014cc09f52`; they inform qualification and do
not replace the hosted service's documented contract.

[^1]: Ubicloud,
      [API reference overview](https://www.ubicloud.com/docs/api-reference/overview).
[^2]: Ubicloud,
      [managed-service pricing](https://www.ubicloud.com/docs/about/pricing).
[^3]: Mriya baseline:
      [backend](https://github.com/leynos/mriya/blob/fd7d8193fae29d54867cc372dc30efdc6fab35bb/src/backend.rs), [CLI wiring](https://github.com/leynos/mriya/blob/fd7d8193fae29d54867cc372dc30efdc6fab35bb/src/main.rs),
      and
      [runner](https://github.com/leynos/mriya/blob/fd7d8193fae29d54867cc372dc30efdc6fab35bb/src/run.rs).
[^4]: Ubicloud,
      [create a new VM](https://www.ubicloud.com/docs/api-reference/virtual-machine/create-a-new-vm).
[^5]: Ubicloud,
      [get VM details](https://www.ubicloud.com/docs/api-reference/virtual-machine/get-vm-details).
[^6]: Ubicloud,
      [delete a VM](https://www.ubicloud.com/docs/api-reference/virtual-machine/delete-a-vm),
      and
      [VM lifecycle routes](https://github.com/ubicloud/ubicloud/blob/39353394d420818c66e8355f12973c014cc09f52/routes/project/location/vm.rb).
[^7]: Ubicloud,
      [private subnets](https://www.ubicloud.com/docs/networking/private-subnet).
[^8]: Ubicloud,
      [provider options](https://github.com/ubicloud/ubicloud/blob/39353394d420818c66e8355f12973c014cc09f52/lib/option.rb), [capacity validation](https://github.com/ubicloud/ubicloud/blob/39353394d420818c66e8355f12973c014cc09f52/lib/validation.rb),
      and
      [VM request handling](https://github.com/ubicloud/ubicloud/blob/39353394d420818c66e8355f12973c014cc09f52/helpers/vm.rb).
[^9]: Ubicloud,
      [guest initialization](https://github.com/ubicloud/ubicloud/blob/39353394d420818c66e8355f12973c014cc09f52/rhizome/host/lib/vm_setup.rb),
      and
      [script validation](https://github.com/ubicloud/ubicloud/blob/39353394d420818c66e8355f12973c014cc09f52/model/vm_init_script.rb).
[^10]: Ubicloud,
       [managed-services quickstart](https://www.ubicloud.com/docs/quick-start/managed-services).
[^11]: Ubicloud,
       [machine images overview](https://www.ubicloud.com/docs/machine-images/overview).
[^12]: Ubicloud,
       [machine images announcement](https://www.ubicloud.com/blog/announcing-ubicloud-machine-images).
[^13]: Ubicloud,
       [networking firewall](https://www.ubicloud.com/docs/networking/firewall).
[^14]: Ubicloud,
       [GPU request validation](https://github.com/ubicloud/ubicloud/blob/39353394d420818c66e8355f12973c014cc09f52/lib/validation.rb).
