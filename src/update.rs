//! "Check for updates": the only code in the app that uses the network.
//!
//! Nothing here runs by itself. A click on the menu item asks GitHub which
//! release is the latest; a second click, when there is a newer one,
//! downloads its installer and starts it. Both happen on a thread of their
//! own, never on the main one.
//!
//! - The only host asked is `github.com`, over HTTPS, and nothing is sent
//!   but the request itself with the user agent `TrayJuice`.
//! - The latest version is read from the address GitHub redirects
//!   `/releases/latest` to. The download address is then built here from
//!   that version, which is digits and dots only; no address from a reply
//!   is ever opened.
//! - Only a copy that the installer put in place is replaced by a newer
//!   installer. A copy from the zip, Scoop or winget gets the release page
//!   opened in the browser instead, and so does any failure on the way.

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_DISABLE_REDIRECTS, WINHTTP_FLAG_SECURE,
    WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_LOCATION,
    WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetOption, WinHttpSetTimeouts,
};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

const HOST: &str = "github.com";
const REPO: &str = "/vvhatislove/TrayJuice";
const USER_AGENT: &str = "TrayJuice";
const TIMEOUT_MS: i32 = 15_000;
/// An installer is under a megabyte; anything near this size is not ours.
const MAX_DOWNLOAD: usize = 32 * 1024 * 1024;

/// Major, minor, patch.
pub type Version = [u32; 3];

/// What the menu item currently offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    /// Nothing is known yet: a click checks.
    Idle,
    Checking,
    UpToDate,
    /// The check did not get an answer: a click tries again.
    Failed,
    /// `installed` tells whether this copy can be replaced by an installer.
    Available {
        version: Version,
        installed: bool,
    },
    Downloading,
}

impl Offer {
    /// Text of the menu item.
    pub fn label(&self) -> String {
        match self {
            Offer::Idle => "Check for updates".to_string(),
            Offer::Checking => "Checking for updates...".to_string(),
            Offer::UpToDate => format!("TrayJuice {} is up to date", text(current())),
            Offer::Failed => "Could not check for updates".to_string(),
            Offer::Available {
                version,
                installed: true,
            } => format!("Install TrayJuice {}", text(*version)),
            Offer::Available { version, .. } => format!("Get TrayJuice {}", text(*version)),
            Offer::Downloading => "Downloading the update...".to_string(),
        }
    }

    /// Whether a click does anything right now.
    pub fn clickable(&self) -> bool {
        !matches!(self, Offer::Checking | Offer::Downloading)
    }
}

/// The version of this build.
pub fn current() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).unwrap_or([0; 3])
}

pub fn text(version: Version) -> String {
    format!("{}.{}.{}", version[0], version[1], version[2])
}

/// Parses `1.2.3` or `v1.2.3`: three groups of digits and nothing else.
fn parse_version(text: &str) -> Option<Version> {
    let text = text.strip_prefix('v').unwrap_or(text);
    let mut parts = text.split('.');
    let mut version = [0u32; 3];
    for slot in &mut version {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 6 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    parts.next().is_none().then_some(version)
}

/// The version in the address `/releases/latest` redirects to. Anything
/// that is not a tag page of this very repository is refused.
fn version_in_location(location: &str) -> Option<Version> {
    let prefix = format!("https://{HOST}{REPO}/releases/tag/");
    parse_version(location.strip_prefix(&prefix)?)
}

fn release_page(version: Version) -> String {
    format!("https://{HOST}{REPO}/releases/tag/v{}", text(version))
}

/// File name of the installer of a release. It is fixed by the release
/// workflow and by `installer/trayjuice.iss`.
fn installer_name(version: Version) -> String {
    format!("TrayJuice-{}-setup.exe", text(version))
}

fn installer_request(version: Version) -> String {
    let v = text(version);
    format!("{REPO}/releases/download/v{v}/{}", installer_name(version))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// A WinHTTP handle that is closed when dropped.
struct Handle(*mut c_void);

impl Handle {
    fn new(raw: *mut c_void) -> Option<Self> {
        (!raw.is_null()).then_some(Self(raw))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { WinHttpCloseHandle(self.0) };
    }
}

/// Sends `GET https://github.com<path>` and returns the request once the
/// response headers are in. The session and connection handles come along
/// because they have to outlive it.
fn get(path: &str, follow_redirects: bool) -> Option<(Handle, Handle, Handle)> {
    unsafe {
        let session = Handle::new(WinHttpOpen(
            wide(USER_AGENT).as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            std::ptr::null(),
            std::ptr::null(),
            0,
        ))?;
        WinHttpSetTimeouts(session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS);
        let connection = Handle::new(WinHttpConnect(session.0, wide(HOST).as_ptr(), 443, 0))?;
        let request = Handle::new(WinHttpOpenRequest(
            connection.0,
            wide("GET").as_ptr(),
            wide(path).as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        if !follow_redirects {
            let feature = WINHTTP_DISABLE_REDIRECTS;
            let done = WinHttpSetOption(
                request.0,
                WINHTTP_OPTION_DISABLE_FEATURE,
                (&raw const feature).cast(),
                size_of_val(&feature) as u32,
            );
            if done == 0 {
                return None;
            }
        }
        let sent = WinHttpSendRequest(request.0, std::ptr::null(), 0, std::ptr::null(), 0, 0, 0);
        if sent == 0 || WinHttpReceiveResponse(request.0, std::ptr::null_mut()) == 0 {
            return None;
        }
        Some((session, connection, request))
    }
}

fn status_code(request: &Handle) -> Option<u32> {
    let mut code = 0u32;
    let mut len = size_of::<u32>() as u32;
    let done = unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            (&raw mut code).cast(),
            &mut len,
            std::ptr::null_mut(),
        )
    };
    (done != 0).then_some(code)
}

fn location(request: &Handle) -> Option<String> {
    let mut buf = [0u16; 512];
    let mut len = size_of_val(&buf) as u32;
    let done = unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_LOCATION,
            std::ptr::null(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
        )
    };
    let units = (len as usize / 2).min(buf.len());
    (done != 0).then(|| String::from_utf16_lossy(&buf[..units]))
}

/// Asks GitHub for the latest release and compares it with this build.
/// Blocks for the length of one request.
pub fn check() -> Offer {
    let latest = get(&format!("{REPO}/releases/latest"), false)
        .and_then(|(_session, _connection, request)| location(&request))
        .and_then(|location| version_in_location(&location));
    match latest {
        None => Offer::Failed,
        Some(version) if version > current() => Offer::Available {
            version,
            installed: is_installed(),
        },
        Some(_) => Offer::UpToDate,
    }
}

/// Downloads the installer of `version` into the temp folder. `None` when
/// the release has no installer or the download fails.
pub fn download_installer(version: Version) -> Option<PathBuf> {
    let (_session, _connection, request) = get(&installer_request(version), true)?;
    if status_code(&request)? != 200 {
        return None;
    }
    let mut data = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let mut read = 0u32;
        let done = unsafe {
            WinHttpReadData(
                request.0,
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                &mut read,
            )
        };
        if done == 0 {
            return None;
        }
        if read == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..read as usize]);
        if data.len() > MAX_DOWNLOAD {
            return None;
        }
    }
    // A Windows executable, not an error page.
    if !data.starts_with(b"MZ") {
        return None;
    }
    let path = std::env::temp_dir().join(installer_name(version));
    std::fs::write(&path, data).ok()?;
    Some(path)
}

/// Starts a downloaded installer without its questions. It waits for this
/// app to quit, replaces it and starts it again.
pub fn run_installer(path: &Path) -> bool {
    std::process::Command::new(path)
        .arg("/SILENT")
        .spawn()
        .is_ok()
}

/// Opens the page of a release in the default browser.
pub fn open_release_page(version: Version) {
    let (verb, url) = (wide("open"), wide(&release_page(version)));
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            url.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// `true` when this executable sits in the folder the installer recorded
/// under `HKCU\Software\TrayJuice`.
fn is_installed() -> bool {
    let (key, value) = (wide(r"Software\TrayJuice"), wide("InstallDir"));
    let mut buf = [0u16; 1024];
    let mut bytes = size_of_val(&buf) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != ERROR_SUCCESS {
        return false;
    }
    let len = (bytes as usize / 2).min(buf.len());
    let recorded = String::from_utf16_lossy(&buf[..len]);
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    exe.parent()
        .is_some_and(|dir| same_folder(&dir.to_string_lossy(), &recorded))
}

/// Compares two folder paths the way Windows does for plain names: without
/// regard to case or a trailing separator.
fn same_folder(a: &str, b: &str) -> bool {
    let clean = |s: &str| {
        s.trim_end_matches('\0')
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    !clean(a).is_empty() && clean(a) == clean(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions_strictly() {
        assert_eq!(parse_version("0.1.1"), Some([0, 1, 1]));
        assert_eq!(parse_version("v12.0.345"), Some([12, 0, 345]));
        for bad in [
            "",
            "v",
            "1.2",
            "1.2.3.4",
            "1.2.x",
            "1..3",
            "1.2.3-beta",
            "1.2.3/",
            " 1.2.3",
            "1.2.+3",
            "1.2.9999999",
            "../1.2.3",
            "vv1.2.3",
        ] {
            assert_eq!(parse_version(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn compares_versions_by_number() {
        assert!([0, 1, 10] > [0, 1, 9]);
        assert!([0, 2, 0] > [0, 1, 99]);
        assert!([1, 0, 0] > [0, 99, 99]);
        assert!([0, 1, 1] <= [0, 1, 1]);
        assert!(current() > [0, 0, 0]);
    }

    #[test]
    fn reads_the_version_only_from_our_own_tag_page() {
        let at = |location| version_in_location(location);
        assert_eq!(
            at("https://github.com/vvhatislove/TrayJuice/releases/tag/v0.1.2"),
            Some([0, 1, 2])
        );
        assert_eq!(
            at("https://github.com/vvhatislove/TrayJuice/releases"),
            None
        );
        assert_eq!(
            at("https://github.com/someone/TrayJuice/releases/tag/v9.9.9"),
            None
        );
        assert_eq!(
            at("https://example.com/vvhatislove/TrayJuice/releases/tag/v9.9.9"),
            None
        );
        assert_eq!(
            at("https://github.com/vvhatislove/TrayJuice/releases/tag/v1.2.3/../x"),
            None
        );
        assert_eq!(at("https://github.com/login"), None);
    }

    #[test]
    fn builds_the_addresses_itself() {
        assert_eq!(
            installer_request([0, 1, 2]),
            "/vvhatislove/TrayJuice/releases/download/v0.1.2/TrayJuice-0.1.2-setup.exe"
        );
        assert_eq!(
            release_page([0, 1, 2]),
            "https://github.com/vvhatislove/TrayJuice/releases/tag/v0.1.2"
        );
    }

    #[test]
    fn labels_the_menu_item() {
        assert_eq!(Offer::Idle.label(), "Check for updates");
        let offer = |installed| Offer::Available {
            version: [0, 2, 0],
            installed,
        };
        assert_eq!(offer(true).label(), "Install TrayJuice 0.2.0");
        assert_eq!(offer(false).label(), "Get TrayJuice 0.2.0");
        assert!(Offer::UpToDate.label().contains(env!("CARGO_PKG_VERSION")));
        assert!(offer(true).clickable());
        assert!(!Offer::Checking.clickable());
        assert!(!Offer::Downloading.clickable());
    }

    #[test]
    fn compares_folders_like_windows() {
        assert!(same_folder(r"C:\Apps\TrayJuice", r"c:\apps\trayjuice\"));
        assert!(same_folder("C:\\Apps\\TrayJuice\0", r"C:\Apps\TrayJuice"));
        assert!(!same_folder(r"C:\Apps\TrayJuice", r"C:\Apps\Other"));
        assert!(!same_folder("", ""));
    }
}
