//! The app's version, asking GitHub for the latest release, and updating
//! the app in place when it runs as an AppImage.
//!
//! Nothing is fetched unless the user asks, so startup never waits on the
//! network.

use std::fs;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest as _, Sha256};

/// The version this build was made from.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The project's home on GitHub.
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

// GitHub rejects API requests without a user agent.
const USER_AGENT: &str = concat!("dictionary/", env!("CARGO_PKG_VERSION"));
const TIMEOUT: Duration = Duration::from_secs(10);
/// How long downloading an update may take in all.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// A published release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// The version, without the tag's leading `v`.
    pub version: String,
    /// The release's page on GitHub.
    pub url: String,
    /// The release's AppImage for this architecture, if it has one that can
    /// be verified.
    pub appimage: Option<Asset>,
}

impl Release {
    /// Whether this release is newer than the running build.
    pub fn is_newer(&self) -> bool {
        is_newer(&self.version, VERSION)
    }
}

/// A file attached to a release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Lowercase hex, as GitHub computed it on upload.
    pub sha256: String,
}

#[derive(Deserialize)]
struct ReleaseJson {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<AssetJson>,
}

#[derive(Deserialize)]
struct AssetJson {
    name: String,
    browser_download_url: String,
    size: u64,
    /// `sha256:<hex>`; missing on assets uploaded before GitHub added it.
    digest: Option<String>,
}
/// Fetches the latest release. `Ok(None)` means nothing is published yet.
/// This blocks, so run it on a background thread.
pub fn latest_release() -> Result<Option<Release>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        // A 404 (no releases) is an answer, not an error.
        .http_status_as_error(false)
        .build()
        .into();
    let repo = REPOSITORY.trim_start_matches("https://github.com/");
    let mut response = agent
        .get(format!(
            "https://api.github.com/repos/{repo}/releases/latest"
        ))
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("Couldn’t reach GitHub: {e}"))?;
    match response.status().as_u16() {
        200 => {}
        404 => return Ok(None),
        status => return Err(format!("GitHub answered with HTTP {status}.")),
    }
    let release: ReleaseJson = response
        .body_mut()
        .read_json()
        .map_err(|e| format!("Couldn’t read GitHub’s answer: {e}"))?;
    Ok(Some(Release {
        version: release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name)
            .to_owned(),
        url: release.html_url,
        appimage: appimage_asset(release.assets, std::env::consts::ARCH),
    }))
}

/// The AppImage built for `arch` (named like `uname -m`, as is the Rust
/// name), if GitHub has a checksum for it.
fn appimage_asset(assets: Vec<AssetJson>, arch: &str) -> Option<Asset> {
    let suffix = format!("-{arch}.AppImage");
    assets.into_iter().find_map(|asset| {
        let sha256 = asset.digest?.strip_prefix("sha256:")?.to_ascii_lowercase();
        asset.name.ends_with(&suffix).then_some(Asset {
            name: asset.name,
            url: asset.browser_download_url,
            size: asset.size,
            sha256,
        })
    })
}

/// The AppImage this process runs from, if it was started from one.
///
/// The AppImage runtime sets `APPIMAGE` to the file and `APPDIR` to where it
/// mounted it. Child processes inherit both, so the binary must also lie
/// inside `APPDIR`; otherwise a copy installed some other way, started from
/// another AppImage such as a terminal, would replace that AppImage.
pub fn running_appimage() -> Option<PathBuf> {
    let appimage = PathBuf::from(std::env::var_os("APPIMAGE")?);
    let appdir = std::env::var_os("APPDIR")?;
    let exe = std::env::current_exe().ok()?;
    (appimage.is_absolute() && appimage.is_file() && exe.starts_with(appdir)).then_some(appimage)
}

/// Downloads `asset` and puts it in place of the AppImage at `target`,
/// reporting the fraction done to `progress`.
///
/// The download goes to a file next to `target` and replaces it only once
/// its size and SHA-256 match, so a failed update leaves the old AppImage
/// as it was. The running app is unaffected, since it keeps the old file
/// open. This blocks, so run it on a background thread.
pub fn install_appimage(
    asset: &Asset,
    target: &Path,
    progress: impl Fn(f32),
) -> Result<(), String> {
    let dir = target
        .parent()
        .ok_or_else(|| format!("{} has no folder.", target.display()))?;
    let partial = dir.join(format!(".{}.part", asset.name));
    let result = download(asset, &partial, progress).and_then(|()| {
        fs::rename(&partial, target)
            .map_err(|e| format!("Couldn’t replace {}: {e}", target.display()))
    });
    if result.is_err() {
        fs::remove_file(&partial).ok();
    }
    result
}

fn download(asset: &Asset, path: &Path, progress: impl Fn(f32)) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(TIMEOUT))
        .timeout_recv_response(Some(TIMEOUT))
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .build()
        .into();
    let response = agent
        .get(&asset.url)
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| format!("Couldn’t download the update: {e}"))?;
    let mut body = response.into_body().into_reader();
    // Fails when the folder isn't writable, e.g. an AppImage in /opt.
    let mut file =
        fs::File::create(path).map_err(|e| format!("Couldn’t write {}: {e}", path.display()))?;
    let write_error = |e| format!("Couldn’t write {}: {e}", path.display());

    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    let mut received = 0;
    loop {
        let n = body
            .read(&mut buffer)
            .map_err(|e| format!("The download failed: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        file.write_all(&buffer[..n]).map_err(write_error)?;
        received += n as u64;
        progress(received as f32 / asset.size.max(1) as f32);
    }
    if received != asset.size || format!("{:x}", hasher.finalize()) != asset.sha256 {
        return Err("The download was damaged, so the app wasn’t updated.".into());
    }
    file.set_permissions(fs::Permissions::from_mode(0o755))
        .map_err(write_error)?;
    file.sync_all().map_err(write_error)
}

/// Starts the AppImage at `path`, after an update. The caller then quits.
pub fn relaunch(path: &Path) -> Result<(), String> {
    Command::new(path)
        .spawn()
        .map(drop)
        .map_err(|e| format!("Couldn’t restart: {e}"))
}

/// Compares dotted version numbers, e.g. `0.10.0` > `0.9.2`. A pre-release
/// or build suffix (`-rc.1`, `+abc`) is ignored, and missing parts count as 0.
fn is_newer(candidate: &str, current: &str) -> bool {
    fn parts(version: &str) -> Vec<u64> {
        let core = version.split(['-', '+']).next().unwrap_or("");
        let mut parts: Vec<u64> = core.split('.').map(|p| p.parse().unwrap_or(0)).collect();
        while parts.last() == Some(&0) {
            parts.pop();
        }
        parts
    }
    parts(candidate) > parts(current)
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt as _;

    use sha2::{Digest as _, Sha256};

    use super::{Asset, AssetJson, appimage_asset, install_appimage, is_newer};

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.2"));
        assert!(is_newer("1.0", "0.99.99"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.1.0-rc.1", "0.1.0"));
    }

    #[test]
    fn picks_the_appimage_for_the_architecture() {
        let assets: Vec<AssetJson> = serde_json::from_str(
            r#"[
                {"name": "Dictionary-0.2.0-aarch64.AppImage", "size": 1,
                 "browser_download_url": "https://example.com/a", "digest": "sha256:AA"},
                {"name": "Dictionary-0.2.0-x86_64.AppImage", "size": 2,
                 "browser_download_url": "https://example.com/x", "digest": "sha256:BB"},
                {"name": "Dictionary-0.2.0-riscv64.AppImage", "size": 3,
                 "browser_download_url": "https://example.com/r", "digest": null}
            ]"#,
        )
        .unwrap();
        let asset = appimage_asset(assets, "x86_64").unwrap();
        assert_eq!(asset.url, "https://example.com/x");
        assert_eq!(asset.size, 2);
        assert_eq!(asset.sha256, "bb");

        // Without a checksum the download can't be verified.
        let assets: Vec<AssetJson> = serde_json::from_str(
            r#"[{"name": "Dictionary-0.2.0-riscv64.AppImage", "size": 3,
                 "browser_download_url": "https://example.com/r", "digest": null}]"#,
        )
        .unwrap();
        assert_eq!(appimage_asset(assets, "riscv64"), None);
    }

    /// Serves `body` once over HTTP and returns its URL.
    fn serve_once(body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/Dictionary.AppImage",
            listener.local_addr().unwrap()
        );
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            // Requests are small, so one read holds all of it.
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });
        url
    }

    fn asset(url: String, body: &[u8]) -> Asset {
        Asset {
            name: "Dictionary-0.2.0-x86_64.AppImage".into(),
            url,
            size: body.len() as u64,
            sha256: format!("{:x}", Sha256::digest(body)),
        }
    }

    #[test]
    fn replaces_the_appimage() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("Dictionary.AppImage");
        std::fs::write(&target, b"old").unwrap();
        let body = b"new appimage";
        let asset = asset(serve_once(body), body);

        let last = std::cell::Cell::new(0.);
        install_appimage(&asset, &target, |p| last.set(p)).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), body);
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        assert_eq!(last.get(), 1.);
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[test]
    fn keeps_the_appimage_when_the_download_is_damaged() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("Dictionary.AppImage");
        std::fs::write(&target, b"old").unwrap();
        let asset = asset(serve_once(b"tampered"), b"expected");

        assert!(install_appimage(&asset, &target, |_| {}).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
    }
}
