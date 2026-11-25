//! The pins the build standard follows: the linker release in `tools/mold/VERSION`, the toolchain
//! channel in `rust-toolchain.toml`, and the component list the installer reads from the same file.
//! Everything here is read as text and a malformed file is an error rather than a fallback, so
//! cases that need the pin derive it from the file the gates use instead of copying the literal.

pub const TOOLCHAIN: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/rust-toolchain.toml"));

/// The file the developer guide says to bump when adopting a new linker release.
const LINKER_PIN: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tools/mold/VERSION"));

/// Returns the linker release the repository pins, as the version file spells it.
///
/// Cases that need the pin read it here, so moving `tools/mold/VERSION` moves
/// every one of them at once.
///
/// ```text
/// pinned_linker() == "2.41.0"   // while tools/mold/VERSION holds it
/// ```
pub fn pinned_linker() -> &'static str { LINKER_PIN.trim() }

/// Returns the channel `rust-toolchain.toml` names, for cases that need its
/// text rather than its kind.
///
/// ```text
/// pinned_channel("channel = \"stable\"") == Ok("stable")
/// ```
///
/// # Errors
///
/// Returns the reason when the channel is missing, repeated or malformed.
pub fn pinned_channel(toolchain: &str) -> Result<&str, String> {
    let channels = toolchain_channels(toolchain)?;
    match channels.as_slice() {
        [] => Err("rust-toolchain.toml names no channel".to_owned()),
        [channel] => Ok(*channel),
        _ => Err(format!(
            "rust-toolchain.toml names more than one channel: {channels:?}"
        )),
    }
}

/// Returns the components `rust-toolchain.toml` lists under `[toolchain]`, in file order.
///
/// Cases assert that the installer requested these components, so adding one to
/// the file extends what the contract checks without editing the cases.
///
/// ```text
/// pinned_components("components = [\"rustfmt\", \"clippy\"]") == Ok(vec!["rustfmt", "clippy"])
/// ```
///
/// # Errors
///
/// Returns the reason when no list is declared, one is declared twice, or an entry is not a quoted
/// name.
pub fn pinned_components(toolchain: &str) -> Result<Vec<&str>, String> {
    let mut declarations = values_of(toolchain, "components");
    let Some(declaration) = declarations.next() else {
        return Err("rust-toolchain.toml lists no components".to_owned());
    };
    if declarations.next().is_some() {
        return Err("rust-toolchain.toml declares the components more than once".to_owned());
    }
    let components = component_list(declaration)
        .map_err(|reason| format!("`components = {declaration}`: {reason}"))?;
    if components.is_empty() {
        return Err("rust-toolchain.toml lists no components".to_owned());
    }
    Ok(components)
}

/// Returns the values of every `wanted` declaration under `[toolchain]`, in file order.
fn values_of<'a>(toolchain: &'a str, wanted: &'a str) -> impl Iterator<Item = &'a str> {
    toolchain_declarations(toolchain)
        .into_iter()
        .filter(move |(key, _)| *key == wanted)
        .map(|(_, value)| value)
}

/// Parses one bracketed list of quoted component names.
fn component_list(value: &str) -> Result<Vec<&str>, String> {
    let Some(inner) = value
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return Err("the components are not a bracketed list".to_owned());
    };
    inner
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            entry
                .strip_prefix('"')
                .and_then(|name| name.strip_suffix('"'))
                .ok_or_else(|| format!("`{entry}` is not a quoted component name"))
        })
        .collect()
}

/// Returns whether text is a `YYYY-MM-DD` date, which is how a dated nightly names itself. A bare
/// `nightly` floats, so it is no pin and the reader does not know it.
///
/// ```text
/// is_date("2026-05-28") == true
/// is_date("preview")    == false
/// is_date("2026-5-28")  == false
/// ```
pub(super) fn is_date(text: &str) -> bool {
    let widths: Vec<usize> = text.split('-').map(str::len).collect();
    widths == [4, 2, 2] && text.chars().all(|c| c.is_ascii_digit() || c == '-')
}

/// Returns the quoted value of a `channel = "..."` value, if it is one.
///
/// A value that is not a closed quoted string, or that carries anything but a
/// comment after the closing quote, is a malformed declaration and an error:
/// skipping it would let `channel = nightly` beside a valid
/// `channel = "stable"` read as one stable pin.
///
/// ```text
/// "\"stable\""            -> Ok("stable")
/// "\"stable\" # pinned"   -> Ok("stable")
/// "nightly"               -> Err(..)
/// "\"stable"              -> Err(..)
/// "\"stable\" junk"       -> Err(..)
/// ```
fn channel_value(value: &str) -> Result<&str, String> {
    let Some(rest) = value.strip_prefix('"') else {
        return Err(format!("`channel = {value}` is not a quoted string"));
    };
    let Some((name, after)) = rest.split_once('"') else {
        return Err(format!("`channel = {value}` has no closing quote"));
    };
    let trailing = after.trim();
    if trailing.is_empty() || trailing.starts_with('#') {
        Ok(name)
    } else {
        Err(format!("`channel = {value}` has content after the channel"))
    }
}

/// Returns every `key = value` declaration under `[toolchain]`, in file order, with commented
/// lines, keys in another table and trailing comments on a value all skipped.
///
/// Both readers below start from this, so a lookalike key, a commented line or a key in another
/// table is not counted by either.
fn toolchain_declarations(toolchain: &str) -> Vec<(&str, &str)> {
    let mut table = "";
    let mut found = Vec::new();
    for line in toolchain.lines().map(str::trim) {
        if let Some(name) = table_name(line) {
            table = name;
        } else if table == "toolchain" {
            found.extend(declaration(line));
        }
    }
    found
}

/// Returns the table a `[name]` line opens, or `None` for any other line.
fn table_name(line: &str) -> Option<&str> {
    let name = line.strip_prefix('[')?.strip_suffix(']')?;
    Some(name.trim())
}

/// Returns the `(key, value)` one line declares, or `None` for a comment, a blank line or anything
/// else that declares nothing. A trailing comment is stripped from the value.
fn declaration(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once('=')?;
    let name = key.trim();
    if name.starts_with('#') {
        return None;
    }
    let without_comment = value.split('#').next().unwrap_or_default().trim();
    Some((name, without_comment))
}

/// Returns every `channel` declaration under `[toolchain]`, so a malformed or repeated one is an
/// error rather than a skipped line.
fn toolchain_channels(toolchain: &str) -> Result<Vec<&str>, String> {
    values_of(toolchain, "channel").map(channel_value).collect()
}
