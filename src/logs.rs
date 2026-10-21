use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

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
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs((retention_days as u64).saturating_mul(86_400)))
        .unwrap_or(UNIX_EPOCH);
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
