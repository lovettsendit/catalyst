//! The one rule for every path a user names.
//!
//! `docs/interface.md` §0: a path a user names must be relative, contain no
//! `..` component, contain no symbolic link in any component that already
//! exists, and resolve inside the current directory. An absolute path is
//! accepted only when it lies inside the current directory, and is then the
//! same place a relative one would have named.
//!
//! # Why the symlink check is over components and not over the result
//!
//! Because the result does not exist yet. `--out export/run` is refused when
//! `export` is a link, and it has to be refused *before* the directory is
//! created, or the refusal arrives after the write it was meant to prevent.
//! So the walk goes component by component from the current directory
//! outwards, asks `symlink_metadata` about each one that exists, and stops at
//! the first link. Nothing is created until the whole path has passed.
//!
//! # Why a refused path writes nothing at all
//!
//! A partially applied refusal is worse than none: it leaves a directory
//! somewhere the user did not ask for and did not get told about. Validation
//! is therefore complete before the first `create_dir`, and the acceptance
//! oracle checks the parent directory's listing to prove it.

use crate::cli::Refused;
use std::path::{Component, Path, PathBuf};

/// Where a validated path points, and the text the user wrote for it.
#[derive(Clone, Debug)]
pub struct Resolved {
    /// The absolute location, below the current directory.
    pub full: PathBuf,
    /// Exactly what the user typed, for echoing back in the answer.
    pub named: String,
}

fn refused(named: &str, why: &str) -> Refused {
    Refused::new(
        "catalyst.path_refused",
        format!("the path `{named}` is refused: {why}"),
        "name a relative path inside the current directory, with no `..` component \
         and no symbolic link, for example `export/run`",
    )
}

/// Apply the rule. On success the path is inside the current directory and
/// every component that exists is a real file or directory.
pub fn resolve(named: &str) -> Result<Resolved, Refused> {
    if named.is_empty() {
        return Err(refused(named, "it is empty"));
    }
    let cwd = std::env::current_dir()
        .map_err(|_| refused(named, "the current directory cannot be read"))?;
    let raw = Path::new(named);

    // An absolute path is accepted only where it names the same place a
    // relative one would have: inside the current directory, strictly below it.
    let relative: PathBuf = if raw.is_absolute() {
        match raw.strip_prefix(&cwd) {
            Ok(rest) if rest.as_os_str().is_empty() => {
                return Err(refused(named, "it names the current directory itself"))
            }
            Ok(rest) => rest.to_path_buf(),
            Err(_) => {
                return Err(refused(
                    named,
                    "an absolute path is accepted only inside the current directory",
                ))
            }
        }
    } else {
        raw.to_path_buf()
    };

    let mut parts: Vec<String> = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => {
                let Some(text) = part.to_str() else {
                    return Err(refused(named, "it is not valid UTF-8"));
                };
                parts.push(text.to_owned());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(refused(named, "it contains a `..` component"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(refused(named, "it leaves the current directory"));
            }
        }
    }
    if parts.is_empty() {
        return Err(refused(named, "it names the current directory itself"));
    }

    let mut full = cwd;
    for part in &parts {
        full.push(part);
        match std::fs::symlink_metadata(&full) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(refused(
                    named,
                    "one of its components is a symbolic link, which could lead \
                     outside the current directory",
                ));
            }
            _ => {}
        }
    }

    Ok(Resolved {
        full,
        named: named.to_owned(),
    })
}

/// Resolve an output directory: created when absent, refused when it holds
/// anything, so an export never overwrites a previous one.
pub fn prepare_out_dir(named: &str) -> Result<Resolved, Refused> {
    let resolved = resolve(named)?;
    match std::fs::read_dir(&resolved.full) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err(Refused::new(
                    "catalyst.path_not_empty",
                    format!("the output directory `{named}` already has something in it"),
                    "name a directory that does not exist yet, or empty this one first",
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&resolved.full).map_err(|error| {
                Refused::new(
                    "catalyst.path_refused",
                    format!(
                        "the output directory `{named}` could not be created: {}",
                        kind(&error)
                    ),
                    "name a directory the current user may create, inside the current directory",
                )
            })?;
        }
        Err(error) => {
            return Err(Refused::new(
                "catalyst.path_refused",
                format!(
                    "the output directory `{named}` could not be read: {}",
                    kind(&error)
                ),
                "name a directory inside the current directory that this user may write",
            ));
        }
    }
    Ok(resolved)
}

/// An input file, read as text. The path rule applies to it too: a problem
/// file outside the current directory is refused before it is opened.
pub fn read_input(named: &str) -> Result<String, Refused> {
    let resolved = resolve(named)?;
    std::fs::read(&resolved.full)
        .map_err(|error| {
            Refused::new(
                "catalyst.path_refused",
                format!("the file `{named}` could not be read: {}", kind(&error)),
                "name a readable file inside the current directory",
            )
        })
        .and_then(|bytes| {
            String::from_utf8(bytes).map_err(|_| {
                Refused::new(
                    "catalyst.syntax",
                    format!("the file `{named}` is not valid UTF-8 text"),
                    "write the document as UTF-8 JSON",
                )
            })
        })
}

/// Write one file below an already validated directory.
pub fn write_file(dir: &Path, name: &str, contents: &str) -> Result<(), Refused> {
    let path = dir.join(name);
    std::fs::write(&path, contents).map_err(|error| {
        Refused::new(
            "catalyst.path_refused",
            format!("`{name}` could not be written: {}", kind(&error)),
            "name an output directory this user may write",
        )
    })
}

/// The kind of an I/O error, without the path the operating system attached
/// to it: an absolute path must never reach the output.
fn kind(error: &std::io::Error) -> String {
    format!("{:?}", error.kind()).to_lowercase()
}
