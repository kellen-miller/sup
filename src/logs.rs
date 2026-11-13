use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

/// Creates a private, unique run directory beneath the user's cache.
///
/// # Errors
///
/// Returns an error when the cache path contains unsafe entries or cannot be
/// created or secured.
pub fn create_run_dir(home: &Path) -> io::Result<PathBuf> {
    let cache = home.join(".cache");
    let sup = cache.join("sup");
    let runs = sup.join("runs");
    for directory in [&cache, &sup, &runs] {
        match fs::symlink_metadata(directory) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unsafe cache directory {}", directory.display()),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                builder.mode(0o700);
                match builder.create(directory) {
                    Ok(()) => {}
                    Err(create_error) if create_error.kind() == io::ErrorKind::AlreadyExists => {
                        match fs::symlink_metadata(directory) {
                            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                            Ok(_) => {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidInput,
                                    format!("unsafe cache directory {}", directory.display()),
                                ));
                            }
                            Err(recheck) => return Err(recheck),
                        }
                    }
                    Err(create_error) => return Err(create_error),
                }
            }
            Err(error) => return Err(error),
        }
    }
    #[cfg(unix)]
    fs::set_permissions(&runs, fs::Permissions::from_mode(0o700))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for sequence in 0..1000_u16 {
        let path = runs.join(format!("{now}-{}-{sequence}", std::process::id()));
        let mut builder = fs::DirBuilder::new();
        builder.recursive(false);
        #[cfg(unix)]
        builder.mode(0o700);
        match builder.create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique run directory",
    ))
}

/// Creates a private log file with a single-component filename.
///
/// # Errors
///
/// Returns an error when `name` is unsafe, already exists, or cannot be opened.
pub fn create_job_log(run_dir: &Path, name: &str) -> io::Result<(PathBuf, fs::File)> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "unsafe log filename"));
    }
    let path = run_dir.join(name);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(&path)?;
    Ok((path, file))
}

/// Removes run directories last modified before local midnight `retention_days` calendar days ago, so `0` keeps
/// only today's runs. Symlinks are never followed.
///
/// # Errors
///
/// Returns an error when local midnight cannot be computed, the run directory cannot be read, or an expired entry
/// cannot be inspected or removed.
pub fn cleanup_old_runs(home: &Path, current: &Path, retention_days: i64) -> io::Result<()> {
    if retention_days < 0 {
        return Ok(());
    }
    let runs = home.join(".cache/sup/runs");
    let entries = match fs::read_dir(&runs) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let cutoff = local_midnight(retention_days)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path == current {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() || metadata.modified()? >= cutoff {
            continue;
        }
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

/// Returns the start of the local calendar day `days_ago` days before today.
fn local_midnight(days_ago: i64) -> io::Result<SystemTime> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let now = libc::time_t::try_from(now).map_err(|_| io::Error::other("current time out of range"))?;
    let mut local = std::mem::MaybeUninit::<libc::tm>::zeroed();
    // SAFETY: both pointers are valid for the call; localtime_r fully initializes `local` on success.
    if unsafe { libc::localtime_r(&now, local.as_mut_ptr()) }.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: localtime_r returned non-null, so `local` is initialized.
    let mut local = unsafe { local.assume_init() };
    local.tm_hour = 0;
    local.tm_min = 0;
    local.tm_sec = 0;
    local.tm_isdst = -1;
    local.tm_mday -= libc::c_int::try_from(days_ago.min(36_500)).unwrap_or(36_500);
    // SAFETY: `local` is a valid, initialized tm; mktime normalizes out-of-range fields in place.
    let midnight = unsafe { libc::mktime(&mut local) };
    if midnight == -1 {
        return Err(io::Error::other("cannot compute local midnight"));
    }
    Ok(u64::try_from(midnight).map_or(UNIX_EPOCH, |seconds| UNIX_EPOCH + Duration::from_secs(seconds)))
}

/// Returns the newest run's `summary.md`, ordering runs by the start timestamp in their directory names.
///
/// # Errors
///
/// Returns an error when the runs directory exists but cannot be read.
pub fn latest_summary(home: &Path) -> io::Result<Option<PathBuf>> {
    let runs = home.join(".cache/sup/runs");
    let entries = match fs::read_dir(&runs) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut newest: Option<(u128, PathBuf)> = None;
    for entry in entries {
        let entry = entry?;
        let Some(started) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.split('-').next())
            .and_then(|stamp| stamp.parse::<u128>().ok())
        else {
            continue;
        };
        let summary = entry.path().join(SUMMARY_FILE);
        let is_file = fs::symlink_metadata(&summary).is_ok_and(|metadata| metadata.is_file());
        if is_file && newest.as_ref().is_none_or(|(best, _)| started > *best) {
            newest = Some((started, summary));
        }
    }
    Ok(newest.map(|(_, path)| path))
}

/// Filename of the Markdown run summary written into each run directory.
pub const SUMMARY_FILE: &str = "summary.md";
