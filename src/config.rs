use crate::navigation;
use std::env;
use std::path::{Path, PathBuf};
use url::Url;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const DEFAULT_URL: &str = "about:blank";

/// A command line failure together with the process exit code it should use.
///
/// `--help` and `--version` are successful requests and carry code 0; a
/// malformed invocation carries code 2.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliError {
    pub message: String,
    pub code: i32,
}

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 2,
        }
    }

    fn success(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 0,
        }
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for CliError {}

#[derive(Clone, Debug)]
pub struct Config {
    pub profile_dir: PathBuf,
    pub database_path: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    /// Downloads land here, inside the profile, never where a page asks.
    pub download_dir: PathBuf,
    pub start_url: String,
    /// A bookmarks file to import, as a maintenance command rather than a
    /// window: it is the one job that has to work while the browser is closed.
    pub import_bookmarks: Option<PathBuf>,
    /// Whether `start_url` was asked for rather than defaulted to.
    ///
    /// The difference decides what happens when a session was saved *and* a URL
    /// was given: a link a person clicked has to open, so the URL is not allowed
    /// to be swallowed by the session that happened to be on disk.
    pub start_url_given: bool,
    pub search_endpoint: Option<String>,
    pub restore_session: bool,
    pub smoke: bool,
    pub storage_check: bool,
}

impl Config {
    pub fn from_args() -> Result<Self, CliError> {
        Self::parse(env::args().skip(1))
    }

    pub fn parse<I>(arguments: I) -> Result<Self, CliError>
    where
        I: IntoIterator<Item = String>,
    {
        let data_home = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
            .ok_or_else(|| CliError::usage("HOME or XDG_DATA_HOME is required"))?;
        if !data_home.is_absolute() {
            return Err(CliError::usage("XDG_DATA_HOME must be an absolute path"));
        }

        // The whole path, name and all: this returns the profile directory
        // itself, so joining the name on here as well put the profile inside
        // itself - `brwsl/brwsl` - and the browser quietly carried an empty one
        // while the real one sat beside it.
        let default_profile = adopt_previous_profile(&data_home);
        let mut profile_dir = None;
        let mut database_path = None;
        let mut data_dir = None;
        let mut cache_dir = None;
        let mut download_dir = None;
        let mut start_url = DEFAULT_URL.to_string();
        let mut start_url_given = false;
        let mut positional: Option<String> = None;
        let mut search_endpoint = None;
        let mut restore_session = true;
        let mut import_bookmarks: Option<PathBuf> = None;
        let mut smoke = false;
        let mut storage_check = false;

        let mut args = arguments.into_iter();
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--profile-dir" => {
                    profile_dir =
                        Some(PathBuf::from(args.next().ok_or_else(|| {
                            CliError::usage("--profile-dir needs a path")
                        })?));
                }
                "--database-path" => {
                    database_path =
                        Some(PathBuf::from(args.next().ok_or_else(|| {
                            CliError::usage("--database-path needs a path")
                        })?));
                }
                "--data-dir" => {
                    data_dir = Some(PathBuf::from(
                        args.next()
                            .ok_or_else(|| CliError::usage("--data-dir needs a path"))?,
                    ));
                }
                "--cache-dir" => {
                    cache_dir =
                        Some(PathBuf::from(args.next().ok_or_else(|| {
                            CliError::usage("--cache-dir needs a path")
                        })?));
                }
                "--download-dir" => {
                    download_dir =
                        Some(PathBuf::from(args.next().ok_or_else(|| {
                            CliError::usage("--download-dir needs a path")
                        })?));
                }
                "--start-url" => {
                    start_url = args
                        .next()
                        .ok_or_else(|| CliError::usage("--start-url needs a URL"))?;
                    start_url_given = true;
                }
                "--search-endpoint" => {
                    let endpoint = args
                        .next()
                        .ok_or_else(|| CliError::usage("--search-endpoint needs a URL"))?;
                    validate_search_endpoint(&endpoint)?;
                    search_endpoint = Some(endpoint);
                }
                "--no-restore" => restore_session = false,
                "--import-bookmarks" => {
                    import_bookmarks =
                        Some(PathBuf::from(args.next().ok_or_else(|| {
                            CliError::usage("--import-bookmarks needs a file path")
                        })?));
                }
                "--smoke" => smoke = true,
                // Head-less exercise of bookmarks, history and the schema.
                "--storage-check" => storage_check = true,
                "-h" | "--help" => return Err(CliError::success(usage())),
                "-V" | "--version" => {
                    return Err(CliError::success(format!(
                        "brwsl {}",
                        env!("CARGO_PKG_VERSION")
                    )));
                }
                other if other.starts_with('-') => {
                    return Err(CliError::usage(format!(
                        "unknown argument: {other}\n\n{}",
                        usage()
                    )));
                }
                // A bare URL, because that is what the desktop entry promises to
                // hand over and what every other browser accepts: the entry is
                // `Exec=brwsl %U`, so a link opened from a chat client or a
                // file manager arrives as a plain positional argument. Treating it
                // as an unknown flag made the installed app refuse every link
                // pointed at it.
                url => {
                    if positional.is_some() {
                        return Err(CliError::usage(format!(
                            "only one URL can be opened at a time, got a second: {url}\n\n{}",
                            usage()
                        )));
                    }
                    positional = Some(url.to_string());
                }
            }
        }

        // A positional URL and --start-url mean the same thing, so the flag wins
        // if both are given: it is the more explicit of the two.
        if let Some(url) = positional.filter(|_| start_url == DEFAULT_URL) {
            start_url = url;
            start_url_given = true;
        }
        let profile_dir = profile_dir.unwrap_or(default_profile);
        validate_path("--profile-dir", &profile_dir)?;
        let database_path = database_path.unwrap_or_else(|| profile_dir.join("session.sqlite"));
        let data_dir = data_dir.unwrap_or_else(|| profile_dir.join("data"));
        let cache_dir = cache_dir.unwrap_or_else(|| profile_dir.join("cache"));
        let download_dir = download_dir.unwrap_or_else(|| profile_dir.join("downloads"));
        validate_path("--database-path", &database_path)?;
        validate_path("--data-dir", &data_dir)?;
        validate_path("--cache-dir", &cache_dir)?;
        validate_path("--download-dir", &download_dir)?;

        Ok(Self {
            profile_dir,
            database_path,
            data_dir,
            cache_dir,
            download_dir,
            start_url,
            start_url_given,
            import_bookmarks,
            search_endpoint,
            restore_session,
            smoke,
            storage_check,
        })
    }

    pub fn database_path(&self) -> &Path {
        self.database_path.as_path()
    }

    pub fn data_dir(&self) -> &Path {
        self.data_dir.as_path()
    }

    pub fn cache_dir(&self) -> &Path {
        self.cache_dir.as_path()
    }

    pub fn download_dir(&self) -> &Path {
        self.download_dir.as_path()
    }

    pub fn ensure_profile_dirs(&self) -> Result<(), String> {
        ensure_private_directory(&self.profile_dir)?;
        ensure_private_directory(&self.data_dir)?;
        ensure_private_directory(&self.cache_dir)?;
        ensure_private_directory(&self.download_dir)?;
        if let Some(parent) = self.database_path.parent()
            && parent.starts_with(&self.profile_dir)
        {
            ensure_private_directory(parent)?;
        }
        Ok(())
    }
}

fn validate_path(option: &str, path: &Path) -> Result<(), CliError> {
    if path.as_os_str().is_empty() {
        return Err(CliError::usage(format!("{option} needs a non-empty path")));
    }
    if path.to_string_lossy().contains('\0') {
        return Err(CliError::usage(format!("{option} contains a NUL byte")));
    }
    Ok(())
}

/// Accept only endpoints the navigation layer would also accept, so the CLI
/// cannot accept a URL that later fails at search time. Plain `http` is
/// refused unless the host is loopback, because queries would otherwise leave
/// the machine in cleartext.
fn validate_search_endpoint(endpoint: &str) -> Result<(), CliError> {
    let normalized = navigation::validate_explicit_url(endpoint).map_err(CliError::usage)?;
    let url = Url::parse(&normalized)
        .map_err(|error| CliError::usage(format!("invalid search endpoint: {error}")))?;
    let is_loopback = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    ) || matches!(url.host(), Some(url::Host::Ipv4(address)) if address.is_loopback())
        || matches!(url.host(), Some(url::Host::Ipv6(address)) if address.is_loopback());
    if url.scheme() != "https" && !is_loopback {
        return Err(CliError::usage(
            "search endpoint must use https (http is allowed only for a loopback host)",
        ));
    }
    Ok(())
}

/// The names this project's profile directory has been under, oldest first.
///
/// The project has been renamed twice, and the default profile directory was
/// named after it each time. Every name is kept, because a person's bookmarks,
/// history, saved session and cookie store are in whichever directory they were
/// written to, and an app that quietly started on an empty profile would look
/// like it had lost everything.
const PREVIOUS_PROFILE_NAMES: [&str; 2] = ["browsrl", "brwsrl"];

/// Move a profile directory from one of this project's previous names to its
/// current one.
///
/// The rename happens only when a previous directory is there and the current one
/// is not. Two profiles is not a thing to resolve by picking one, so when the
/// current name already exists nothing moves and the older directory is left
/// exactly where it is. Returns the profile directory to use either way, named.
fn adopt_previous_profile(data_home: &Path) -> PathBuf {
    let current = data_home.join("brwsl");
    if current.exists() {
        return current;
    }
    for name in PREVIOUS_PROFILE_NAMES {
        let previous = data_home.join(name);
        if !previous.is_dir() {
            continue;
        }
        return match std::fs::rename(&previous, &current) {
            Ok(()) => {
                eprintln!(
                    "profile renamed: {} -> {}",
                    previous.display(),
                    current.display()
                );
                current
            }
            Err(error) => {
                eprintln!(
                    "could not rename {} to {} ({error}); using an empty profile",
                    previous.display(),
                    current.display()
                );
                current
            }
        };
    }
    current
}

#[cfg(unix)]
fn ensure_private_directory(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path)
        .map_err(|error| format!("create private directory {}: {error}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("protect directory {}: {error}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_directory(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path)
        .map_err(|error| format!("create directory {}: {error}", path.display()))
}

pub fn usage() -> String {
    concat!(
        "Usage: brwsl [URL] [--profile-dir PATH] [--database-path PATH] ",
        "[--data-dir PATH] [--cache-dir PATH] [--download-dir PATH] ",
        "[--start-url URL] [--search-endpoint URL] [--no-restore] ",
        "[--import-bookmarks FILE] [--smoke] [--storage-check]"
    )
    .to_string()
}
