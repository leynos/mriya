use std::collections::HashSet;
use std::fs::{copy, create_dir_all, read_dir, read_to_string, remove_dir_all, remove_file, DirEntry, FileType};

use camino::{Utf8Path, Utf8PathBuf};
use mriya::sync::SyncError;

#[derive(Clone, Debug)]
struct IgnoreRules {
    dirs: HashSet<String>,
    files: HashSet<String>,
}

pub fn simulate_rsync(source: &Utf8Path, destination: &Utf8Path) -> Result<(), SyncError> {
    let rules = load_ignores(source)?;
    let mut kept: HashSet<Utf8PathBuf> = HashSet::new();
    copy_tree(source, destination, &rules, &mut kept)?;
    prune_destination(destination, &kept, &rules)?;
    Ok(())
}

fn load_ignores(source: &Utf8Path) -> Result<IgnoreRules, SyncError> {
    let ignore_path = source.join(".gitignore");
    if !ignore_path.exists() {
        return Ok(IgnoreRules {
            dirs: HashSet::new(),
            files: HashSet::new(),
        });
    }

    let content = read_to_string(&ignore_path).map_err(map_io_error)?;

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

fn copy_tree(
    source_root: &Utf8Path,
    destination_root: &Utf8Path,
    rules: &IgnoreRules,
    kept: &mut HashSet<Utf8PathBuf>,
) -> Result<(), SyncError> {
    for source_entry in read_dir(source_root).map_err(map_io_error)? {
        let entry = source_entry.map_err(map_io_error)?;
        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(map_io_error)?;
        let relative = path
            .strip_prefix(source_root)
            .map_err(map_io_error)?;

        if should_ignore(relative, rules) {
            continue;
        }

        let destination_path = destination_root.join(relative);
        let metadata = entry.file_type().map_err(map_io_error)?;

        if metadata.is_dir() {
            create_dir_all(&destination_path).map_err(map_io_error)?;
            kept.insert(relative.to_path_buf());
            copy_tree(&path, &destination_path, rules, kept)?;
        } else {
            if let Some(parent) = destination_path.parent() {
                create_dir_all(parent).map_err(map_io_error)?;
            }
            copy(&path, &destination_path).map_err(map_io_error)?;
            kept.insert(relative.to_path_buf());
        }
    }

    Ok(())
}

fn prune_destination(
    destination_root: &Utf8Path,
    kept: &HashSet<Utf8PathBuf>,
    rules: &IgnoreRules,
) -> Result<(), SyncError> {
    if !destination_root.exists() {
        return Ok(());
    }

    for destination_entry in read_dir(destination_root).map_err(map_io_error)? {
        let entry = destination_entry.map_err(map_io_error)?;
        process_destination_entry(entry, destination_root, kept, rules)?;
    }

    Ok(())
}

fn process_destination_entry(
    entry: DirEntry,
    destination_root: &Utf8Path,
    kept: &HashSet<Utf8PathBuf>,
    rules: &IgnoreRules,
) -> Result<(), SyncError> {
    let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(map_io_error)?;
    let relative = path
        .strip_prefix(destination_root)
        .map_err(map_io_error)?;

    if should_ignore(relative, rules) {
        return Ok(());
    }

    let file_type = entry.file_type().map_err(map_io_error)?;

    if should_keep_entry(relative, file_type, kept) {
        if file_type.is_dir() {
            prune_destination(&path, kept, rules)?;
        }
        return Ok(());
    }

    remove_entry(&path, file_type.is_dir())
}

fn should_keep_entry(relative: &Utf8Path, file_type: FileType, kept: &HashSet<Utf8PathBuf>) -> bool {
    let has_children = kept.iter().any(|kept_path| kept_path.starts_with(relative));
    kept.contains(relative) || (file_type.is_dir() && has_children)
}

fn remove_entry(path: &Utf8Path, is_dir: bool) -> Result<(), SyncError> {
    if is_dir {
        remove_dir_all(path).map_err(map_io_error)
    } else {
        remove_file(path).map_err(map_io_error)
    }
}

fn map_io_error(err: impl ToString) -> SyncError {
    SyncError::Spawn {
        program: String::from("rsync"),
        message: err.to_string(),
    }
}
