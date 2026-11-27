# Migrating a legacy cache volume

Scaleway stopped creating new Block Storage volumes through the Instances API
on 30 April 2025. Volumes created before that date with the legacy `b_ssd`
class still exist, but Mriya refuses to use them: it will not format, attach,
or reinterpret one, because the only documented conversion path changes the
volume ID and none of the alternatives preserve the data safely.

This guide moves the data from a legacy `b_ssd` volume to a current Block
Storage (`sbs_5k` or `sbs_15k`) volume using snapshot export and import. The
old volume is left untouched, and Mriya never reformats it.

## Before you start

- The legacy volume must not be attached to any instance. Unmount the
  filesystem from inside the guest over SSH and verify that the mount point is
  gone before stopping the instance, then detach the volume. Waiting for the
  guest-side unmount avoids snapshotting a filesystem with unflushed writes. Do
  not delete the instance while the volume is still attached: a default
  `scw instance server delete` deletes attached volumes too, which would
  destroy the source before it is snapshotted.
- You need an Object Storage bucket in the same region as the volume. The
  bucket key will hold a temporary QCOW2 image.
- Record the legacy volume ID from `mriya.toml` (or `SCW_DEFAULT_VOLUME_ID`)
  and its zone.

The commands below use `scw`. Set `SCW_PROFILE`, `SCW_DEFAULT_ZONE`, and
`SCW_DEFAULT_PROJECT_ID` as usual, and replace the placeholder UUIDs.

## 1. Snapshot the legacy volume

Snapshot management moved to the Block Storage API:
`scw instance snapshot create` is deprecated and no longer functional.
Scaleway's Block Storage FAQ documents creating a Block Storage snapshot of a
legacy `b_ssd` volume as the supported starting point for this conversion.

```bash
scw block snapshot create 11111111-1111-1111-1111-111111111111 \
  name=mriya-legacy-cache \
  zone=fr-par-1
```

Record the snapshot ID that the command prints, and wait until
`scw block snapshot get <snapshot-id> zone=fr-par-1` reports
`status: available` before exporting it.

## 2. Export the snapshot to Object Storage

```bash
scw block snapshot export-to-object-storage \
  22222222-2222-2222-2222-222222222222 \
  bucket=mriya-migration \
  key=mriya-legacy-cache.qcow2 \
  zone=fr-par-1
```

The export writes a QCOW2 image. The snapshot reports `exporting` until the
image is written; wait for that state to clear before continuing, using
`scw block snapshot get` to check it.

## 3. Import the image as a Block Storage snapshot

```bash
scw block snapshot import-from-object-storage \
  bucket=mriya-migration \
  key=mriya-legacy-cache.qcow2 \
  name=mriya-cache-snapshot \
  project-id=33333333-3333-3333-3333-333333333333 \
  zone=fr-par-1
```

The import produces a Block Storage snapshot in the same region. Record its
snapshot ID, then wait until
`scw block snapshot get <snapshot-id> zone=fr-par-1` reports
`status: available`. An imported snapshot stays in `creating` while the image
is being ingested, and creating a volume from it too early fails.

## 4. Create the replacement Block Storage volume

Create the new volume from the snapshot. Use the same tier you want for future
runs; `sbs_5k` matches the legacy 5,000 IOPS class.

```bash
scw block volume create \
  name=mriya-cache \
  perf-iops=5000 \
  from-snapshot.snapshot-id=44444444-4444-4444-4444-444444444444 \
  project-id=33333333-3333-3333-3333-333333333333 \
  zone=fr-par-1
```

Wait until `scw block volume get <new-id> zone=fr-par-1` reports
`status: available`.

The volume ID changes. Conversion never preserves the original ID, so update
every place that names the old one:

```bash
export SCW_DEFAULT_VOLUME_ID=55555555-5555-5555-5555-555555555555
```

or in `mriya.toml`:

```toml
[scaleway]
default_volume_id = "55555555-5555-5555-5555-555555555555"
```

## 5. Verify the data

Run a command that reads the migrated files without writing to them:

```bash
mriya run -- du -sh /mriya
```

Mriya resolves the guest disk by its `volume-<uuid>` serial, verifies that the
mount source is that disk, and refuses the run if the mount cannot be verified.
If the old volume was formatted ext4, no further formatting is needed; if it
carried another filesystem, Mriya refuses to mount it because the cache path
requires ext4.

## 6. Remove the leftover resources

Once the new volume is verified, delete the temporary snapshot, the exported
object, and then the legacy volume. Deleting the legacy volume is destructive
and is not reversible, so keep it until the new volume has been used
successfully at least once.

```bash
scw block snapshot delete 44444444-4444-4444-4444-444444444444 zone=fr-par-1
scw block snapshot delete 22222222-2222-2222-2222-222222222222 zone=fr-par-1
scw block volume delete 11111111-1111-1111-1111-111111111111 zone=fr-par-1
```

The Block Storage API manages legacy `b_ssd` volumes as well, so the legacy
volume is deleted through `scw block volume delete`, not the deprecated
Instances volume commands.

The exported QCOW2 object is not covered by those commands. Remove it from the
bucket once the new volume is verified, for example with an S3 client:

```bash
aws --endpoint-url https://s3.fr-par.scw.cloud s3 rm \
  s3://mriya-migration/mriya-legacy-cache.qcow2
```

## What Mriya does with a legacy ID

`mriya init` and `mriya run` look the configured volume up in its zone before
creating any server. A volume whose type is `b_ssd`, or whose storage class is
`bssd`, is refused with:

```text
volume <id> uses the legacy b_ssd storage class, which Mriya does not format,
attach or reinterpret; migrate it with the snapshot export/import steps in
docs/cache-volume-migration.md
```

Nothing is created, formatted, or deleted by that refusal.

## Compatibility and price are different questions

API compatibility is about which contract works today. Scaleway creates and
manages Block Storage volumes through the Block Storage API and refuses new
`b_ssd` creation through the Instances API, so Mriya uses the Block Storage API
and refuses legacy IDs.

Price is a separate question. The list prices published on
<https://www.scaleway.com/en/pricing/storage/> on 26 November 2026, before tax,
were:

| Tier | List price            |
| ---- | --------------------- |
| 5K   | €0.000130 per GB/hour |
| 15K  | €0.000177 per GB/hour |

Billing uses the provisioned size, not the bytes actually stored. At 730 hours
per month that arithmetic gives approximately €0.0949 per GB-month for 5K and
€0.1292 per GB-month for 15K; those monthly figures are derived, not quoted by
Scaleway. The same pricing page publishes no legacy `b_ssd` rate, so a
migration cannot be priced from a current `b_ssd` list price.
