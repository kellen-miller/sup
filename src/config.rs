use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG: &str = include_str!("../config.toml");

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub aliases: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub run: RunConfig,
    pub jobs: Vec<RawJob>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunConfig {
    #[serde(default = "default_retention")]
    pub retention_days: i64,
    #[serde(default = "default_true")]
    pub cleanup_logs: bool,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            retention_days: 30,
            cleanup_logs: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawJob {
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    pub phase: String,
    pub command: Vec<String>,
    #[serde(default)]
    pub required_commands: Vec<String>,
    #[serde(default)]
    pub required_paths: Vec<String>,
    #[serde(default)]
    pub required_env: Vec<String>,
    #[serde(default = "default_true")]
    pub optional: bool,
    #[serde(default)]
    pub log_name: Option<String>,
    #[serde(default)]
    pub auth: AuthMode,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyConfig {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    aliases: HashMap<String, Vec<String>>,
    #[serde(default)]
    run: RunConfig,
    jobs: Vec<LegacyJob>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyJob {
    name: String,
    #[serde(default)]
    label: Option<String>,
    phase: String,
    command: Vec<String>,
    #[serde(default)]
    required_commands: Vec<String>,
    #[serde(default)]
    required_paths: Vec<String>,
    #[serde(default)]
    required_env: Vec<String>,
    #[serde(default = "default_true", deserialize_with = "deserialize_legacy_bool")]
    optional: bool,
    #[serde(default)]
    log_name: Option<String>,
    #[serde(default)]
    auth: AuthMode,
    #[serde(default, deserialize_with = "deserialize_optional_legacy_bool")]
    sudo_preflight: Option<bool>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum LegacyBool {
    Bool(bool),
    String(String),
}

fn legacy_bool(value: LegacyBool) -> Result<bool, String> {
    match value {
        LegacyBool::Bool(value) => Ok(value),
        LegacyBool::String(value) => match value.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" => Ok(true),
            "false" | "no" | "off" => Ok(false),
            _ => Err(format!("expected a boolean spelling, got {value:?}")),
        },
    }
}

fn deserialize_legacy_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    legacy_bool(LegacyBool::deserialize(deserializer)?).map_err(serde::de::Error::custom)
}

fn deserialize_optional_legacy_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<LegacyBool>::deserialize(deserializer)?
        .map(legacy_bool)
        .transpose()
        .map_err(serde::de::Error::custom)
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMode {
    #[default]
    None,
    Preflight,
    OnDemand,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Job {
    pub name: String,
    pub label: String,
    pub phase: String,
    pub command: Vec<String>,
    pub required_commands: Vec<String>,
    pub required_paths: Vec<PathBuf>,
    pub required_env: Vec<String>,
    pub optional: bool,
    pub log_name: String,
    pub auth: AuthMode,
}

#[derive(Clone, Debug)]
pub struct LoadedConfig {
    pub jobs: Vec<Job>,
    pub aliases: HashMap<String, Vec<String>>,
    pub retention_days: i64,
    pub cleanup_logs: bool,
    pub source: String,
}

fn default_retention() -> i64 {
    30
}
fn default_true() -> bool {
    true
}
fn default_version() -> u32 {
    1
}

pub fn resolve_path(
    explicit: Option<&Path>,
    environ: &HashMap<String, String>,
) -> Result<(PathBuf, bool), String> {
    if let Some(path) = explicit {
        return Ok((path.to_path_buf(), true));
    }
    let base = environ
        .get("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            environ
                .get("HOME")
                .filter(|value| !value.is_empty())
                .map(|v| PathBuf::from(v).join(".config"))
        })
        .ok_or_else(|| "cannot locate config: HOME and XDG_CONFIG_HOME are unset".to_owned())?;
    Ok((base.join("sup/config.toml"), false))
}

pub fn load(
    path: Option<&Path>,
    home: &Path,
    environ: &HashMap<String, String>,
) -> Result<LoadedConfig, String> {
    let (resolved, explicit) = resolve_path(path, environ)?;
    let (text, source) = match fs::read_to_string(&resolved) {
        Ok(text) => (text, resolved.display().to_string()),
        Err(error) if !explicit && error.kind() == std::io::ErrorKind::NotFound => {
            (DEFAULT_CONFIG.to_owned(), "embedded defaults".to_owned())
        }
        Err(error) => {
            return Err(format!(
                "cannot read config {}: {error}",
                resolved.display()
            ));
        }
    };
    let raw: Config = toml::from_str(&text)
        .map_err(|error| format!("invalid TOML config {}: {error}", source))?;
    normalize(raw, home, environ, source)
}

pub fn normalize(
    raw: Config,
    home: &Path,
    environ: &HashMap<String, String>,
    source: String,
) -> Result<LoadedConfig, String> {
    if raw.version != 1 {
        return Err(format!(
            "unsupported config version {}; expected 1",
            raw.version
        ));
    }
    if raw.jobs.is_empty() {
        return Err("config must contain at least one job".into());
    }
    let mut jobs = Vec::new();
    let mut names = HashSet::new();
    let mut logs = HashSet::new();
    let mut expanded_env = environ.clone();
    expanded_env.insert("HOME".into(), home.display().to_string());
    for job in raw.jobs {
        if !valid_name(&job.name) {
            return Err(format!("invalid job name {:?}", job.name));
        }
        if !names.insert(job.name.clone()) {
            return Err(format!("duplicate job name: {}", job.name));
        }
        if !matches!(job.phase.as_str(), "core" | "parallel") {
            return Err(format!(
                "job {} has invalid phase {:?}",
                job.name, job.phase
            ));
        }
        if job.command.is_empty() || job.command[0].is_empty() {
            return Err(format!("job {} command must be a non-empty argv", job.name));
        }
        let log_name = job.log_name.unwrap_or_else(|| format!("{}.log", job.name));
        if !valid_filename(&log_name) {
            return Err(format!(
                "job {} log_name must be a simple filename",
                job.name
            ));
        }
        if !logs.insert(log_name.clone()) {
            return Err(format!("duplicate log filename: {log_name}"));
        }
        jobs.push(Job {
            name: job.name.clone(),
            label: job.label.unwrap_or(job.name),
            phase: job.phase,
            command: job
                .command
                .iter()
                .map(|v| expand(v, &expanded_env))
                .collect(),
            required_commands: job.required_commands,
            required_paths: job
                .required_paths
                .iter()
                .map(|v| PathBuf::from(expand(v, &expanded_env)))
                .collect(),
            required_env: job.required_env,
            optional: job.optional,
            log_name,
            auth: job.auth,
        });
    }
    for (alias, targets) in &raw.aliases {
        if !valid_name(alias) {
            return Err(format!("invalid alias name {alias:?}"));
        }
        if names.contains(alias) {
            return Err(format!("alias {alias} collides with a job name"));
        }
        for target in targets {
            if !names.contains(target) {
                return Err(format!("alias {alias} references unknown job {target}"));
            }
        }
    }
    Ok(LoadedConfig {
        jobs,
        aliases: raw.aliases,
        retention_days: raw.run.retention_days,
        cleanup_logs: raw.run.cleanup_logs,
        source,
    })
}

pub fn migrate_yaml(
    text: &str,
    home: &Path,
    environ: &HashMap<String, String>,
) -> Result<(String, Vec<String>), String> {
    let options = serde_saphyr::options! {
        duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
        reject_unsupported_tags: true,
    };
    let legacy: LegacyConfig = serde_saphyr::from_str_with_options(text, options)
        .map_err(|error| format!("invalid legacy YAML: {error}"))?;
    let mut notices = Vec::new();
    let mut jobs = Vec::new();
    for job in legacy.jobs {
        let mut auth = job.auth;
        if let Some(preflight) = job.sudo_preflight {
            if preflight && direct_brew_mutation(&job.command) {
                auth = AuthMode::OnDemand;
                notices.push(format!(
                    "job {} auth changed from preflight to on-demand for direct Homebrew mutation",
                    job.name
                ));
            } else if preflight {
                auth = AuthMode::Preflight;
                if job.command.iter().any(|part| part.contains("brew")) {
                    notices.push(format!("job {} uses a shell wrapper with preflight; use on-demand if it invokes Homebrew", job.name));
                }
            } else {
                auth = AuthMode::None;
            }
        }
        jobs.push(RawJob {
            name: job.name,
            label: job.label,
            phase: job.phase,
            command: job.command,
            required_commands: job.required_commands,
            required_paths: job.required_paths,
            required_env: job.required_env,
            optional: job.optional,
            log_name: job.log_name,
            auth,
        });
    }
    let raw = Config {
        version: legacy.version,
        aliases: legacy.aliases,
        run: legacy.run,
        jobs,
    };
    normalize(raw.clone(), home, environ, "migration input".into())?;
    let serialized =
        toml::to_string_pretty(&raw).map_err(|error| format!("cannot encode TOML: {error}"))?;
    Ok((serialized, notices))
}

pub fn selectors(
    jobs: &[Job],
    aliases: &HashMap<String, Vec<String>>,
    only: &[String],
    skip: &[String],
) -> Result<Vec<Job>, String> {
    let expand = |values: &[String]| -> Result<HashSet<String>, String> {
        let mut found = HashSet::new();
        for value in values {
            for selector in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                if jobs.iter().any(|job| job.name == selector) {
                    found.insert(selector.to_owned());
                } else if let Some(names) = aliases.get(selector) {
                    found.extend(names.iter().cloned());
                } else {
                    return Err(format!("unknown selector: {selector}"));
                }
            }
        }
        Ok(found)
    };
    let mut selected: HashSet<String> = if only.is_empty() {
        jobs.iter().map(|j| j.name.clone()).collect()
    } else {
        expand(only)?
    };
    for name in expand(skip)? {
        selected.remove(&name);
    }
    Ok(jobs
        .iter()
        .filter(|job| selected.contains(&job.name))
        .cloned()
        .collect())
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
}
fn valid_filename(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
        && !value.contains('/')
        && !value.contains('\\')
        && Path::new(value).file_name().and_then(|n| n.to_str()) == Some(value)
}
fn direct_brew_mutation(command: &[String]) -> bool {
    let Some(exe) = command.first() else {
        return false;
    };
    let basename = Path::new(exe)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(exe);
    basename == "brew"
        && command
            .get(1)
            .is_some_and(|arg| matches!(arg.as_str(), "install" | "upgrade" | "reinstall"))
}
fn expand(value: &str, vars: &HashMap<String, String>) -> String {
    let bytes = value.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'$' {
            let c = value[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
            continue;
        }
        if bytes.get(i + 1) == Some(&b'$') {
            out.push('$');
            i += 2;
            continue;
        }
        let start = i;
        i += 1;
        let (name, end) = if bytes.get(i) == Some(&b'{') {
            if let Some(close) = value[i + 1..].find('}') {
                let name = &value[i + 1..i + 1 + close];
                if valid_var(name) {
                    (Some(name), i + close + 2)
                } else {
                    (None, start + 1)
                }
            } else {
                (None, start + 1)
            }
        } else {
            let mut end = i;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            if end > i && !bytes[i].is_ascii_digit() {
                (Some(&value[i..end]), end)
            } else {
                (None, start + 1)
            }
        };
        if let Some(name) = name {
            out.push_str(
                vars.get(name)
                    .map(String::as_str)
                    .unwrap_or(&value[start..end]),
            );
            i = end;
        } else {
            out.push('$');
            i = start + 1;
        }
    }
    out
}
fn valid_var(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}
