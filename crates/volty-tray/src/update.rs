use crate::update_progress::{Phase, UpdateProgress};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    ffi::OsStr,
    fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const RELEASE_API_URL: &str =
    "https://api.github.com/repos/PhilippPolterauer/volty-rs/releases/latest";
const RELEASE_DOWNLOAD_ROOT: &str =
    "https://github.com/PhilippPolterauer/volty-rs/releases/download";
const RELEASE_ASSET_PREFIX: &str = "Volty-";
const RELEASE_ASSET_SUFFIX: &str = "-x86_64-pc-windows-msvc.exe";
const MAX_RELEASE_METADATA_BYTES: usize = 2 * 1024 * 1024;
const MAX_CHECKSUM_BYTES: usize = 1024;
const MAX_EXECUTABLE_BYTES: u64 = 128 * 1024 * 1024;
const PROCESS_SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
const FILE_REPLACEMENT_TIMEOUT: Duration = Duration::from_secs(120);
const USER_AGENT: &str = concat!("Volty/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    executable_name: String,
    executable_url: String,
    checksum_url: String,
}

#[derive(Debug)]
struct UpdateError(String);

impl UpdateError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for UpdateError {}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GitHubReleaseAsset>,
}

#[derive(Deserialize)]
struct GitHubReleaseAsset {
    name: String,
    browser_download_url: String,
}

pub fn check_for_update() -> Result<Option<UpdateInfo>, Box<dyn Error>> {
    let response = http_agent()
        .get(RELEASE_API_URL)
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|error| {
            UpdateError::new(format!(
                "could not query the latest GitHub release: {error}"
            ))
        })?;
    let mut reader = response.into_reader();
    let metadata = read_limited(&mut reader, MAX_RELEASE_METADATA_BYTES, "release metadata")?;
    let release: GitHubRelease = serde_json::from_slice(&metadata)
        .map_err(|error| UpdateError::new(format!("invalid GitHub release response: {error}")))?;

    if release.draft || release.prerelease {
        return Err(UpdateError::new(
            "GitHub returned a draft or prerelease from its latest-release endpoint",
        )
        .into());
    }

    let version = parse_release_version(&release.tag_name)?;
    let current_version = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|error| UpdateError::new(format!("invalid built-in version: {error}")))?;
    if version <= current_version {
        return Ok(None);
    }

    let executable_name = format!(
        "{RELEASE_ASSET_PREFIX}{}{RELEASE_ASSET_SUFFIX}",
        release.tag_name
    );
    let checksum_name = format!("{executable_name}.sha256");
    let executable_url = format!(
        "{RELEASE_DOWNLOAD_ROOT}/{}/{executable_name}",
        release.tag_name
    );
    let checksum_url = format!(
        "{RELEASE_DOWNLOAD_ROOT}/{}/{checksum_name}",
        release.tag_name
    );

    find_release_asset(&release.assets, &executable_name, &executable_url)?;
    find_release_asset(&release.assets, &checksum_name, &checksum_url)?;

    Ok(Some(UpdateInfo {
        version: version.to_string(),
        executable_name,
        executable_url,
        checksum_url,
    }))
}

pub fn install_and_launch(update: UpdateInfo) -> Result<(), Box<dyn Error>> {
    let updates_directory = updates_directory()?;
    fs::create_dir_all(&updates_directory).map_err(|error| {
        UpdateError::new(format!(
            "could not create update staging directory {}: {error}",
            updates_directory.display()
        ))
    })?;
    let nonce = unique_suffix()?;
    let staged_executable = updates_directory.join(format!("volty-update-{nonce}.exe"));
    let helper_executable = updates_directory.join(format!("volty-update-helper-{nonce}.exe"));

    let progress = UpdateProgress::start(&update.version);

    let result = (|| {
        let checksum_response = http_agent()
            .get(&update.checksum_url)
            .set("User-Agent", USER_AGENT)
            .call()
            .map_err(|error| {
                UpdateError::new(format!("could not download the update checksum: {error}"))
            })?;
        let mut checksum_reader = checksum_response.into_reader();
        let checksum_file =
            read_limited(&mut checksum_reader, MAX_CHECKSUM_BYTES, "update checksum")?;
        let expected_hash = parse_sha256_checksum(&checksum_file, &update.executable_name)?;

        download_verified_executable(
            &update.executable_url,
            &staged_executable,
            &expected_hash,
            &progress,
        )?;
        progress.set_phase(Phase::Restarting);

        let current_executable = std::env::current_exe().map_err(|error| {
            UpdateError::new(format!("could not locate the running executable: {error}"))
        })?;
        copy_file_new(&current_executable, &helper_executable).map_err(|error| {
            UpdateError::new(format!(
                "could not prepare the update helper at {}: {error}",
                helper_executable.display()
            ))
        })?;

        let mut command = std::process::Command::new(&helper_executable);
        command
            .arg("--apply-update")
            .arg(std::process::id().to_string())
            .arg(&staged_executable)
            .arg(&current_executable)
            .arg(&expected_hash);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        }
        command.spawn().map_err(|error| {
            UpdateError::new(format!("could not start the update helper: {error}"))
        })?;
        Ok::<(), UpdateError>(())
    })();

    if let Err(error) = result {
        let cleanup_error = remove_staged_file(&staged_executable);
        let helper_cleanup_error = remove_staged_file(&helper_executable);
        let cleanup = [cleanup_error, helper_cleanup_error]
            .into_iter()
            .filter_map(Result::err)
            .map(|error| error.to_string())
            .collect::<Vec<_>>();
        if cleanup.is_empty() {
            return Err(error.into());
        }
        return Err(UpdateError::new(format!(
            "{error}; temporary-file cleanup also failed: {}",
            cleanup.join("; ")
        ))
        .into());
    }

    Ok(())
}

pub fn apply_update_cli(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    let request = parse_apply_arguments(arguments)?;
    let helper_executable = std::env::current_exe().map_err(|error| {
        UpdateError::new(format!(
            "could not locate the running update helper: {error}"
        ))
    })?;
    wait_for_process_exit(request.parent_process_id)?;

    let result = replace_executable(&request);
    match result {
        Ok(backup) => {
            if let Err(error) = launch_updated_executable(
                &request.target_executable,
                &helper_executable,
                &request.staged_executable,
                Some(&backup),
            ) {
                restore_backup(&backup, &request.target_executable).map_err(|restore_error| {
                    UpdateError::new(format!(
                        "could not start the updated Volty binary ({error}) or restore the previous binary ({restore_error})"
                    ))
                })?;
                launch_updated_executable(
                    &request.target_executable,
                    &helper_executable,
                    &request.staged_executable,
                    None,
                )
                .map_err(|restart_error| {
                    UpdateError::new(format!(
                        "could not start Volty after restoring the previous binary: {restart_error}"
                    ))
                })?;
                show_update_error(&format!(
                    "The update could not be started, so the previous version was restored.\n\n{error}"
                ));
            }
        }
        Err(error) => {
            launch_updated_executable(
                &request.target_executable,
                &helper_executable,
                &request.staged_executable,
                None,
            )
            .map_err(|restart_error| {
                UpdateError::new(format!(
                    "update failed ({error}) and the previous Volty binary could not be restarted ({restart_error})"
                ))
            })?;
            show_update_error(&format!(
                "The update failed. The previous version was restarted.\n\n{error}"
            ));
        }
    }

    Ok(())
}

pub fn finish_update_cli(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if arguments.len() != 4 {
        return Err(UpdateError::new(
            "invalid update cleanup arguments; expected helper PID, helper path, backup path, and staged path",
        )
        .into());
    }

    let helper_process_id = parse_process_id(&arguments[0])?;
    let helper_executable = PathBuf::from(&arguments[1]);
    let backup_executable = (arguments[2] != "-").then(|| PathBuf::from(&arguments[2]));
    let staged_executable = PathBuf::from(&arguments[3]);

    validate_managed_path(
        &helper_executable,
        "volty-update-helper-",
        &updates_directory()?,
    )?;
    validate_managed_path(&staged_executable, "volty-update-", &updates_directory()?)?;
    if let Some(backup) = &backup_executable {
        let current_executable = std::env::current_exe().map_err(|error| {
            UpdateError::new(format!("could not locate the updated executable: {error}"))
        })?;
        let target_directory = current_executable
            .parent()
            .ok_or_else(|| UpdateError::new("updated executable path has no parent directory"))?;
        validate_managed_path(backup, ".volty-backup-", target_directory)?;
    }

    wait_for_process_exit(helper_process_id)?;
    let mut failures = Vec::new();
    for path in [
        Some(&helper_executable),
        Some(&staged_executable),
        backup_executable.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if let Err(error) = remove_staged_file(path) {
            failures.push(format!("{}: {error}", path.display()));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(UpdateError::new(format!(
            "could not remove temporary update files: {}",
            failures.join("; ")
        ))
        .into())
    }
}

struct ApplyRequest {
    parent_process_id: u32,
    staged_executable: PathBuf,
    target_executable: PathBuf,
    expected_hash: String,
}

fn parse_apply_arguments(arguments: &[String]) -> Result<ApplyRequest, UpdateError> {
    if arguments.len() != 4 {
        return Err(UpdateError::new(
            "invalid update helper arguments; expected parent PID, staged executable, target executable, and SHA-256",
        ));
    }

    let parent_process_id = parse_process_id(&arguments[0])?;
    let staged_executable = PathBuf::from(&arguments[1]);
    let target_executable = PathBuf::from(&arguments[2]);
    let expected_hash = arguments[3].to_ascii_lowercase();
    if expected_hash.len() != 64 || !expected_hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(UpdateError::new(
            "update helper received an invalid SHA-256",
        ));
    }

    let updates_directory = updates_directory()?;
    validate_managed_path(&staged_executable, "volty-update-", &updates_directory)?;
    if !is_executable_path(&target_executable) {
        return Err(UpdateError::new(
            "update target must be an absolute path to a .exe file",
        ));
    }
    if !target_executable.is_file() {
        return Err(UpdateError::new(format!(
            "the existing executable is missing: {}",
            target_executable.display()
        )));
    }

    Ok(ApplyRequest {
        parent_process_id,
        staged_executable,
        target_executable,
        expected_hash,
    })
}

fn replace_executable(request: &ApplyRequest) -> Result<PathBuf, UpdateError> {
    verify_file_hash(&request.staged_executable, &request.expected_hash)?;
    let target_directory = request
        .target_executable
        .parent()
        .ok_or_else(|| UpdateError::new("update target has no parent directory"))?;
    let nonce = unique_suffix()?;
    let replacement = target_directory.join(format!(".volty-update-{nonce}.exe"));
    let backup = target_directory.join(format!(".volty-backup-{nonce}.exe"));

    copy_file_new(&request.staged_executable, &replacement).map_err(|error| {
        UpdateError::new(format!(
            "could not stage the new executable beside {}: {error}",
            request.target_executable.display()
        ))
    })?;
    if let Err(error) = copy_file_new(&request.target_executable, &backup) {
        let cleanup_error = remove_staged_file(&replacement).err();
        return Err(UpdateError::new(match cleanup_error {
            Some(cleanup_error) => format!(
                "could not back up the existing executable: {error}; could not remove replacement staging file: {cleanup_error}"
            ),
            None => format!("could not back up the existing executable: {error}"),
        }));
    }

    if let Err(error) = move_file_replace(&replacement, &request.target_executable) {
        let replacement_cleanup = remove_staged_file(&replacement).err();
        let backup_cleanup = remove_staged_file(&backup).err();
        let cleanup_errors = [replacement_cleanup, backup_cleanup]
            .into_iter()
            .flatten()
            .map(|error| error.to_string())
            .collect::<Vec<_>>();
        let cleanup_context = if cleanup_errors.is_empty() {
            String::new()
        } else {
            format!(
                "; temporary-file cleanup failed: {}",
                cleanup_errors.join("; ")
            )
        };
        return Err(UpdateError::new(format!(
            "could not replace {}: {error}{cleanup_context}",
            request.target_executable.display()
        )));
    }

    Ok(backup)
}

fn launch_updated_executable(
    target: &Path,
    helper: &Path,
    staged: &Path,
    backup: Option<&Path>,
) -> io::Result<()> {
    let mut command = std::process::Command::new(target);
    command
        .arg("--finish-update")
        .arg(std::process::id().to_string())
        .arg(helper)
        .arg(backup.unwrap_or_else(|| Path::new("-")))
        .arg(staged);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    command.spawn().map(|_| ())
}

fn restore_backup(backup: &Path, target: &Path) -> io::Result<()> {
    move_file_replace(backup, target)
}

fn wait_for_process_exit(process_id: u32) -> io::Result<()> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, WAIT_FAILED, WAIT_OBJECT_0},
        System::Threading::{OpenProcess, WaitForSingleObject},
    };

    // SAFETY: OpenProcess receives a PID and requests only the synchronization right.
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE_ACCESS, 0, process_id) };
    if process.is_null() {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
            return Ok(());
        }
        return Err(error);
    }

    // SAFETY: process is an owned process handle with SYNCHRONIZE access.
    let wait_result =
        unsafe { WaitForSingleObject(process, windows_sys::Win32::System::Threading::INFINITE) };
    let wait_error = (wait_result == WAIT_FAILED).then(io::Error::last_os_error);
    // SAFETY: this helper uniquely owns the valid handle returned by OpenProcess.
    let close_result = unsafe { CloseHandle(process) };
    let close_error = (close_result == 0).then(io::Error::last_os_error);

    if let Some(error) = wait_error {
        return Err(error);
    }
    if wait_result != WAIT_OBJECT_0 {
        return Err(io::Error::other(format!(
            "unexpected process wait result {wait_result}"
        )));
    }
    if let Some(error) = close_error {
        return Err(error);
    }
    Ok(())
}

fn download_verified_executable(
    url: &str,
    destination: &Path,
    expected_hash: &str,
    progress: &UpdateProgress,
) -> Result<(), UpdateError> {
    let response = http_agent()
        .get(url)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|error| UpdateError::new(format!("could not download the update: {error}")))?;
    if let Some(length) = response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok())
    {
        progress.set_total(length);
    }
    progress.set_phase(Phase::Downloading);
    let mut reader = response.into_reader();
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| {
            UpdateError::new(format!(
                "could not create update staging file {}: {error}",
                destination.display()
            ))
        })?;

    let result = (|| {
        let mut hasher = Sha256::new();
        let mut total_bytes = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = reader.read(&mut buffer).map_err(|error| {
                UpdateError::new(format!("could not read the downloaded update: {error}"))
            })?;
            if count == 0 {
                break;
            }
            total_bytes = total_bytes.saturating_add(count as u64);
            if total_bytes > MAX_EXECUTABLE_BYTES {
                return Err(UpdateError::new(format!(
                    "downloaded update exceeds the {} MiB size limit",
                    MAX_EXECUTABLE_BYTES / (1024 * 1024)
                )));
            }
            file.write_all(&buffer[..count]).map_err(|error| {
                UpdateError::new(format!("could not write the update staging file: {error}"))
            })?;
            hasher.update(&buffer[..count]);
            progress.set_downloaded(total_bytes);
        }
        progress.set_phase(Phase::Verifying);
        if total_bytes == 0 {
            return Err(UpdateError::new("the downloaded update is empty"));
        }
        file.sync_all().map_err(|error| {
            UpdateError::new(format!("could not flush the update staging file: {error}"))
        })?;

        let actual_hash = digest_to_hex(&hasher.finalize()[..]);
        if actual_hash != expected_hash {
            return Err(UpdateError::new(format!(
                "downloaded update SHA-256 mismatch (expected {expected_hash}, received {actual_hash})"
            )));
        }
        Ok(())
    })();

    drop(file);
    if let Err(error) = result {
        let cleanup_error = remove_staged_file(destination).err();
        return Err(match cleanup_error {
            Some(cleanup_error) => UpdateError::new(format!(
                "{error}; could not remove the incomplete update file: {cleanup_error}"
            )),
            None => error,
        });
    }
    Ok(())
}

fn verify_file_hash(path: &Path, expected_hash: &str) -> Result<(), UpdateError> {
    let mut file = fs::File::open(path).map_err(|error| {
        UpdateError::new(format!(
            "could not reopen staged update {}: {error}",
            path.display()
        ))
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total_bytes = 0u64;
    loop {
        let count = file.read(&mut buffer).map_err(|error| {
            UpdateError::new(format!("could not verify staged update: {error}"))
        })?;
        if count == 0 {
            break;
        }
        total_bytes = total_bytes.saturating_add(count as u64);
        if total_bytes > MAX_EXECUTABLE_BYTES {
            return Err(UpdateError::new(
                "staged update exceeds the maximum executable size",
            ));
        }
        hasher.update(&buffer[..count]);
    }
    if total_bytes == 0 {
        return Err(UpdateError::new("staged update is empty"));
    }
    let actual_hash = digest_to_hex(&hasher.finalize()[..]);
    if actual_hash != expected_hash {
        return Err(UpdateError::new(
            "staged update changed after checksum verification",
        ));
    }
    Ok(())
}

fn parse_release_version(tag: &str) -> Result<Version, UpdateError> {
    let version = tag
        .strip_prefix('v')
        .ok_or_else(|| UpdateError::new(format!("release tag is not versioned: {tag}")))?;
    let parsed = Version::parse(version)
        .map_err(|error| UpdateError::new(format!("invalid release tag {tag}: {error}")))?;
    if parsed.to_string() != version {
        return Err(UpdateError::new(format!(
            "release tag is not in canonical version form: {tag}"
        )));
    }
    Ok(parsed)
}

fn find_release_asset(
    assets: &[GitHubReleaseAsset],
    expected_name: &str,
    expected_url: &str,
) -> Result<(), UpdateError> {
    let matching = assets
        .iter()
        .filter(|asset| asset.name == expected_name)
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return Err(UpdateError::new(format!(
            "release is missing a unique {expected_name} asset"
        )));
    }
    if matching[0].browser_download_url != expected_url {
        return Err(UpdateError::new(format!(
            "release asset {expected_name} has an unexpected download URL"
        )));
    }
    Ok(())
}

fn parse_sha256_checksum(contents: &[u8], expected_name: &str) -> Result<String, UpdateError> {
    let contents = std::str::from_utf8(contents)
        .map_err(|error| UpdateError::new(format!("checksum file is not UTF-8: {error}")))?;
    let mut lines = contents.lines().filter(|line| !line.trim().is_empty());
    let line = lines
        .next()
        .ok_or_else(|| UpdateError::new("checksum file is empty"))?;
    if lines.next().is_some() {
        return Err(UpdateError::new(
            "checksum file must contain exactly one non-empty line",
        ));
    }
    let mut fields = line.split_whitespace();
    let hash = fields
        .next()
        .ok_or_else(|| UpdateError::new("checksum file is missing its hash"))?;
    let name = fields
        .next()
        .ok_or_else(|| UpdateError::new("checksum file is missing its filename"))?;
    if fields.next().is_some() || name.trim_start_matches('*') != expected_name {
        return Err(UpdateError::new(
            "checksum file does not name the expected executable",
        ));
    }
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(UpdateError::new(
            "checksum file does not contain a valid SHA-256 hash",
        ));
    }
    Ok(hash.to_ascii_lowercase())
}

fn read_limited(
    reader: &mut impl Read,
    limit: usize,
    description: &str,
) -> Result<Vec<u8>, UpdateError> {
    let mut contents = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut contents)
        .map_err(|error| UpdateError::new(format!("could not read {description}: {error}")))?;
    if contents.len() > limit {
        return Err(UpdateError::new(format!(
            "{description} exceeds the {limit}-byte limit"
        )));
    }
    Ok(contents)
}

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build()
}

fn digest_to_hex(digest: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(digest.len() * 2);
    for byte in digest {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

fn updates_directory() -> Result<PathBuf, UpdateError> {
    let local_app_data = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| UpdateError::new("LOCALAPPDATA is not set"))?;
    Ok(PathBuf::from(local_app_data).join("Volty").join("updates"))
}

fn validate_managed_path(
    path: &Path,
    file_prefix: &str,
    expected_directory: &Path,
) -> Result<(), UpdateError> {
    if !path.is_absolute()
        || path.parent() != Some(expected_directory)
        || !path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.starts_with(file_prefix) && name.ends_with(".exe"))
    {
        return Err(UpdateError::new(format!(
            "refusing unexpected update helper path {}",
            path.display()
        )));
    }
    Ok(())
}

fn parse_process_id(value: &str) -> Result<u32, UpdateError> {
    let process_id = value
        .parse::<u32>()
        .map_err(|error| UpdateError::new(format!("invalid update process ID: {error}")))?;
    if process_id == 0 {
        return Err(UpdateError::new("update process ID must be nonzero"));
    }
    Ok(process_id)
}

fn is_executable_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

fn copy_file_new(source: &Path, destination: &Path) -> io::Result<()> {
    let mut source_file = fs::File::open(source)?;
    let mut destination_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let result =
        io::copy(&mut source_file, &mut destination_file).and_then(|_| destination_file.sync_all());
    drop(destination_file);
    if let Err(error) = result {
        return match remove_staged_file(destination) {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(io::Error::other(format!(
                "{error}; could not remove incomplete copy {}: {cleanup_error}",
                destination.display()
            ))),
        };
    }
    Ok(())
}

fn move_file_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let deadline = Instant::now() + FILE_REPLACEMENT_TIMEOUT;
    loop {
        // SAFETY: both paths are NUL-terminated UTF-16 buffers that remain live for the call.
        if unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } != 0
        {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_SHARING_VIOLATION as i32)
            || Instant::now() >= deadline
        {
            return Err(error);
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn unique_suffix() -> Result<String, UpdateError> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| UpdateError::new(format!("system clock is before Unix epoch: {error}")))?
        .as_nanos();
    Ok(format!("{}-{timestamp}", std::process::id()))
}

fn remove_staged_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn show_update_error(message: &str) {
    crate::app_icon::show_message_box(
        message,
        "Volty update failed",
        windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
            | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
    );
}

#[cfg(test)]
mod tests {
    use super::{
        digest_to_hex, download_verified_executable, find_release_asset, is_executable_path,
        parse_release_version, parse_sha256_checksum, remove_staged_file, unique_suffix,
        GitHubReleaseAsset,
    };
    use crate::update_progress::{Phase, UpdateProgress};
    use semver::Version;
    use sha2::{Digest, Sha256};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        path::Path,
        thread,
    };

    fn serve_download(body: Vec<u8>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        });
        (format!("http://{address}/update.exe"), server)
    }

    fn staged_test_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "volty-update-test-{}.exe",
            unique_suffix().unwrap()
        ))
    }

    #[test]
    fn parses_canonical_release_tags() {
        assert_eq!(
            parse_release_version("v1.2.3").unwrap(),
            Version::parse("1.2.3").unwrap()
        );
        for invalid in ["1.2.3", "v01.2.3", "v1.2", "v1.2.3garbage"] {
            assert!(parse_release_version(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn parses_only_a_checksum_for_the_expected_executable() {
        let contents = format!("{}  volty.exe\n", "a".repeat(64));
        assert_eq!(
            parse_sha256_checksum(contents.as_bytes(), "volty.exe").unwrap(),
            "a".repeat(64)
        );
        assert!(parse_sha256_checksum(contents.as_bytes(), "other.exe").is_err());
        assert!(parse_sha256_checksum(b"abc  volty.exe\n", "volty.exe").is_err());
        assert!(parse_sha256_checksum(
            format!(
                "{}  volty.exe\n{}  other.exe\n",
                "a".repeat(64),
                "b".repeat(64)
            )
            .as_bytes(),
            "volty.exe"
        )
        .is_err());
    }

    #[test]
    fn release_assets_must_use_the_expected_github_download_urls() {
        let assets = [GitHubReleaseAsset {
            name: "Volty-v1.2.3-x86_64-pc-windows-msvc.exe".to_owned(),
            browser_download_url:
                "https://github.com/PhilippPolterauer/volty-rs/releases/download/v1.2.3/Volty-v1.2.3-x86_64-pc-windows-msvc.exe"
                    .to_owned(),
        }];
        assert!(find_release_asset(
            &assets,
            "Volty-v1.2.3-x86_64-pc-windows-msvc.exe",
            "https://github.com/PhilippPolterauer/volty-rs/releases/download/v1.2.3/Volty-v1.2.3-x86_64-pc-windows-msvc.exe"
        )
        .is_ok());
        assert!(find_release_asset(
            &assets,
            "Volty-v1.2.3-x86_64-pc-windows-msvc.exe",
            "https://attacker.invalid/Volty-v1.2.3-x86_64-pc-windows-msvc.exe"
        )
        .is_err());
    }

    #[test]
    fn update_target_accepts_the_versioned_release_filename() {
        assert!(is_executable_path(Path::new(
            r"C:\Users\Volty\Volty-v0.5.0-x86_64-pc-windows-msvc.exe"
        )));
        assert!(is_executable_path(Path::new(r"C:\Apps\volty.EXE")));
        assert!(!is_executable_path(Path::new(r"C:\Apps\volty.txt")));
        assert!(!is_executable_path(Path::new("volty.exe")));
    }

    #[test]
    fn sha256_hex_uses_lowercase_encoding() {
        let digest = Sha256::digest(b"Volty");
        assert_eq!(
            digest_to_hex(&digest[..]),
            "14010c781dc09c557bd3615e4e745cd43f81a3d4e6649b5089a26ea3c87bc3e1"
        );
    }

    #[test]
    fn downloads_and_verifies_update_while_reporting_progress() {
        let body = b"test update executable".to_vec();
        let expected_hash = digest_to_hex(&Sha256::digest(&body)[..]);
        let (url, server) = serve_download(body.clone());
        let destination = staged_test_path();
        let progress = UpdateProgress::without_window();

        download_verified_executable(&url, &destination, &expected_hash, &progress).unwrap();

        server.join().unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), body);
        let (phase, downloaded, total) = progress.snapshot();
        assert!(phase == Phase::Verifying);
        assert_eq!(downloaded, body.len() as u64);
        assert_eq!(total, body.len() as u64);
        remove_staged_file(&destination).unwrap();
    }

    #[test]
    fn rejects_bad_checksum_and_removes_partial_update() {
        let body = b"test update executable".to_vec();
        let (url, server) = serve_download(body);
        let destination = staged_test_path();
        let progress = UpdateProgress::without_window();

        let error = download_verified_executable(&url, &destination, &"0".repeat(64), &progress)
            .unwrap_err();

        server.join().unwrap();
        assert!(error.to_string().contains("SHA-256 mismatch"));
        assert!(!destination.exists());
    }
}
