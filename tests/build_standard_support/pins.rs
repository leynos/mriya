//! The pins the build standard follows: the linker release in `tools/mold/VERSION`, the toolchain
//! channel in `rust-toolchain.toml`, and the component list the installer reads from the same file.
//! Everything here is read as text and a malformed file is an error rather than a fallback, so cases
//! that need the pin derive it from the file the gates use instead of copying the literal.

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
pub fn pinned_linker() -> &'static str {
    LINKER_PIN.trim()
}

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
    let declarations = toolchain_channels(toolchain);
    if let Some(reason) = declarations
        .iter()
        .find_map(|declaration| declaration.as_ref().err())
    {
        return Err(reason.clone());
    }
    let channels: Vec<&str> = declarations
        .iter()
        .filter_map(|declaration| declaration.as_ref().ok().copied())
        .collect();
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
    let mut table = "";
    let mut found: Option<Vec<&str>> = None;
    for line in toolchain
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
    {
        if line.starts_with('[') {
            table = line.trim_matches(|c| c == '[' || c == ']').trim();
            continue;
        }
        if table != "toolchain" {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "components" {
            continue;
        }
        if found.is_some() {
            return Err(format!(
                "`{line}`: the components are declared more than once"
            ));
        }
        let list = value.split('#').next().unwrap_or_default().trim();
        found = Some(component_list(list).map_err(|reason| format!("`{line}`: {reason}"))?);
    }
    match found {
        Some(components) if !components.is_empty() => Ok(components),
        _ => Err("rust-toolchain.toml lists no components".to_owned()),
    }
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

/// Returns the quoted value of a `channel = "..."` line, if the line is one.
///
/// A `channel` key whose value is not a closed quoted string, or that carries
/// anything but a comment after the closing quote, is a malformed declaration
/// and an error: skipping it would let `channel = nightly` beside a valid
/// `channel = "stable"` read as one stable pin.
///
/// ```text
/// channel = "stable"            -> Some(Ok("stable"))
/// channel = "stable" # pinned   -> Some(Ok("stable"))
/// channel = nightly             -> Some(Err(..))
/// channel = "stable             -> Some(Err(..))
/// channel = "stable" junk       -> Some(Err(..))
/// channel_x = "stable"          -> None
/// ```
fn channel_declaration(line: &str) -> Option<Result<&str, String>> {
    let (key, value) = line.split_once('=')?;
    if key.trim() != "channel" {
        return None;
    }
    let Some(rest) = value.trim().strip_prefix('"') else {
        return Some(Err(format!("`{line}`: the channel is not a quoted string")));
    };
    let Some((name, after)) = rest.split_once('"') else {
        return Some(Err(format!("`{line}`: the channel has no closing quote")));
    };
    let trailing = after.trim();
    if trailing.is_empty() || trailing.starts_with('#') {
        Some(Ok(name))
    } else {
        Some(Err(format!(
            "`{line}`: unexpected content after the channel"
        )))
    }
}

/// Returns every `channel` declaration under `[toolchain]`, skipping comments, so a
/// lookalike key, a commented line or a key in another table is not counted.
fn toolchain_channels(toolchain: &str) -> Vec<Result<&str, String>> {
    let mut table = "";
    let mut found = Vec::new();
    for line in toolchain
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
    {
        if line.starts_with('[') {
            table = line.trim_matches(|c| c == '[' || c == ']').trim();
        } else if table == "toolchain" {
            found.extend(channel_declaration(line));
        }
    }
    found
}
