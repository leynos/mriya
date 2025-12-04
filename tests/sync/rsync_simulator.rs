//! Test-only rsync simulator used by sync BDD scenarios.
//!
//! Provides a minimal gitignore-aware file copier that preserves ignored cache
//! directories and prunes destination entries not present in the source.

use std::collections::HashSet;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::fs_utf8::{DirEntry, FileType};
use cap_std::{ambient_authority, fs_utf8::Dir};
use mriya::sync::SyncError;

#[derive(Clone, Debug)]
struct IgnoreRules {
    dirs: HashSet<String>,
    files: HashSet<String>,
}

struct SimulationContext<'a> {
    fs: &'a Dir,
    rules: &'a IgnoreRules,
    source_root: &'a Utf8Path,
    destination_root: &'a Utf8Path,
    kept: &'a mut HashSet<Utf8PathBuf>,
    ancestors: &'a mut HashSet<Utf8PathBuf>,
}

pub fn simulate_rsync(source: &Utf8Path, destination: &Utf8Path) -> Result<(), SyncError> {
    let fs = Dir::open_ambient_dir("/", ambient_authority())
        .map_err(|err| map_io_error(err.to_string()))?;
    let rules = load_ignores(source)?;
    let mut kept: HashSet<Utf8PathBuf> = HashSet::new();
    let mut ancestors: HashSet<Utf8PathBuf> = HashSet::new();
    let mut context = SimulationContext {
        fs: &fs,
        rules: &rules,
        source_root: source,
        destination_root: destination,
        kept: &mut kept,
        ancestors: &mut ancestors,
    };
    copy_tree(&mut context, source, destination)?;
    compute_ancestors(context.kept, context.ancestors);
    prune_destination(&mut context, destination)?;
    Ok(())
}

fn compute_ancestors(kept: &HashSet<Utf8PathBuf>, ancestors: &mut HashSet<Utf8PathBuf>) {
    for path in kept {
        let mut current = path.clone();
        while let Some(parent) = current.parent() {
            let parent_buf = parent.to_path_buf();
            if !ancestors.insert(parent_buf.clone()) {
                break;
            }
            current = parent_buf;
        }
    }
}

fn load_ignores(source: &Utf8Path) -> Result<IgnoreRules, SyncError> {
    let ignore_path = source.join(".gitignore");
    if !ignore_path.exists() {
        return Ok(IgnoreRules {
            dirs: HashSet::new(),
            files: HashSet::new(),
        });
    }

    // This parser intentionally supports only top-level directory entries
    // (for example `target/`) and file-name patterns (for example `.DS_Store`)
    // to keep the simulator minimal. Nested `.gitignore` files, globbing, and
    // negation rules are not implemented.
    let content = Dir::open_ambient_dir("/", ambient_authority())
        .map_err(|err| map_io_error(err.to_string()))?
        .read_to_string(relative_to_root(&ignore_path))
        .map_err(|err| map_io_error(err.to_string()))?;

    let mut dirs = HashSet::new();
    let mut files = HashSet::new();
    for line in content.lines() {
        let pattern = line.trim();
        if pattern.is_empty() || pattern.starts_with('#') {
            continue;
        }
        if pattern.ends_with('/') {
            dirs.insert(pattern.trim_end_matches('/').to_owned());
        } else {
            files.insert(pattern.to_owned());
        }
    }

    Ok(IgnoreRules { dirs, files })
}

fn should_ignore(relative: &Utf8Path, rules: &IgnoreRules) -> bool {
    for component in relative.components() {
        if rules.dirs.contains(component.as_str()) {
            return true;
        }
    }

    relative
        .file_name()
        .is_some_and(|name| rules.files.contains(name))
}

fn relative_to_root(path: &Utf8Path) -> &Utf8Path {
    path.strip_prefix("/").unwrap_or(path)
}

fn copy_tree(
    context: &mut SimulationContext,
    source_dir: &Utf8Path,
    destination_dir: &Utf8Path,
) -> Result<(), SyncError> {
    for source_entry in context
        .fs
        .read_dir(relative_to_root(source_dir))
        .map_err(|err| map_io_error(err.to_string()))?
    {
        let entry = source_entry.map_err(|err| map_io_error(err.to_string()))?;
        let name = entry
            .file_name()
            .map_err(|err| map_io_error(err.to_string()))?;
        let name_path = Utf8PathBuf::from(name);
        let path = source_dir.join(&name_path);
        let relative = path
            .strip_prefix(context.source_root)
            .map_err(|err| map_io_error(err.to_string()))?;

        if should_ignore(relative, context.rules) {
            continue;
        }

        let destination_path = destination_dir.join(&name_path);
        let metadata = entry
            .file_type()
            .map_err(|err| map_io_error(err.to_string()))?;

        if metadata.is_dir() {
            context
                .fs
                .create_dir_all(relative_to_root(&destination_path))
                .map_err(|err| map_io_error(err.to_string()))?;
            context.kept.insert(relative.to_path_buf());
            copy_tree(context, &path, &destination_path)?;
        } else {
            if let Some(parent) = destination_path.parent() {
                context
                    .fs
                    .create_dir_all(relative_to_root(parent))
                    .map_err(|err| map_io_error(err.to_string()))?;
            }
            context
                .fs
                .copy(
                    relative_to_root(&path),
                    context.fs,
                    relative_to_root(&destination_path),
                )
                .map_err(|err| map_io_error(err.to_string()))?;
            context.kept.insert(relative.to_path_buf());
        }
    }

    Ok(())
}

fn map_io_error(message: impl Into<String>) -> SyncError {
    SyncError::Spawn {
        program: String::from("rsync"),
        message: message.into(),
    }
}

fn should_keep_entry(
    relative: &Utf8Path,
    file_type: FileType,
    kept: &HashSet<Utf8PathBuf>,
    ancestors: &HashSet<Utf8PathBuf>,
) -> bool {
    kept.contains(relative) || (file_type.is_dir() && ancestors.contains(relative))
}

fn remove_entry(
    context: &SimulationContext,
    path: &Utf8Path,
    is_dir: bool,
) -> Result<(), SyncError> {
    if is_dir {
        context
            .fs
            .remove_dir_all(relative_to_root(path))
            .map_err(|err| map_io_error(err.to_string()))
    } else {
        context
            .fs
            .remove_file(relative_to_root(path))
            .map_err(|err| map_io_error(err.to_string()))
    }
}

fn process_destination_entry(
    context: &mut SimulationContext,
    entry: &DirEntry,
    destination_dir: &Utf8Path,
) -> Result<(), SyncError> {
    let name = entry
        .file_name()
        .map_err(|err| map_io_error(err.to_string()))?;
    let name_path = Utf8PathBuf::from(name);
    let path = destination_dir.join(&name_path);
    let relative = path
        .strip_prefix(context.destination_root)
        .map_err(|err| map_io_error(err.to_string()))?;

    if should_ignore(relative, context.rules) {
        return Ok(());
    }

    let file_type = entry
        .file_type()
        .map_err(|err| map_io_error(err.to_string()))?;
    let is_dir = file_type.is_dir();

    if should_keep_entry(relative, file_type, context.kept, context.ancestors) {
        if is_dir {
            prune_destination(context, &path)?;
        }
        return Ok(());
    }

    remove_entry(context, &path, is_dir)
}

fn prune_destination(
    context: &mut SimulationContext,
    destination_dir: &Utf8Path,
) -> Result<(), SyncError> {
    if !destination_dir.exists() {
        return Ok(());
    }

    for destination_entry in context
        .fs
        .read_dir(relative_to_root(destination_dir))
        .map_err(|err| map_io_error(err.to_string()))?
    {
        let entry = destination_entry.map_err(|err| map_io_error(err.to_string()))?;
        process_destination_entry(context, &entry, destination_dir)?;
    }

    Ok(())
}
