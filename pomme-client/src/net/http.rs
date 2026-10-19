//! Every HTTP client pomme builds. reqwest's default verifier needs Java
//! classes in an Android APK, so Android verifies against the device's own CA
//! store instead.

#[cfg(any(test, target_os = "android"))]
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

/// The device's CA stores, newest location first: the updatable conscrypt
/// module (Android 14+), then the system image.
#[cfg(target_os = "android")]
const ANDROID_CA_DIRS: [&str; 2] = [
    "/apex/com.android.conscrypt/cacerts",
    "/system/etc/security/cacerts",
];

pub fn builder() -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder();
    #[cfg(target_os = "android")]
    let builder = builder.tls_certs_only(system_roots().iter().cloned());
    builder
}

pub fn blocking_builder() -> reqwest::blocking::ClientBuilder {
    let builder = reqwest::blocking::Client::builder();
    #[cfg(target_os = "android")]
    let builder = builder.tls_certs_only(system_roots().iter().cloned());
    builder
}

/// The shared client for requests without their own settings. A stalled
/// network fails a request instead of hanging it; a slow but moving
/// download still finishes.
pub fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(30))
            .build()
            .expect("the HTTP client builds")
    })
}

/// `reqwest::get` on the shared client.
pub async fn get(url: impl reqwest::IntoUrl) -> reqwest::Result<reqwest::Response> {
    client().get(url).send().await
}

#[cfg(target_os = "android")]
fn system_roots() -> &'static [reqwest::Certificate] {
    static ROOTS: OnceLock<Vec<reqwest::Certificate>> = OnceLock::new();
    ROOTS.get_or_init(|| read_roots(&ANDROID_CA_DIRS.map(Path::new)))
}

/// Every PEM certificate in the first of `dirs` that exists.
#[cfg(any(test, target_os = "android"))]
fn read_roots(dirs: &[&Path]) -> Vec<reqwest::Certificate> {
    let Some(entries) = dirs.iter().find_map(|dir| std::fs::read_dir(dir).ok()) else {
        tracing::warn!("No CA certificate directory found");
        return Vec::new();
    };
    let (mut roots, mut skipped) = (Vec::new(), 0);
    for entry in entries.flatten() {
        // `from_pem` defers parsing to the client build, where one bad file
        // would fail it; the bundle reader parses now.
        match std::fs::read(entry.path())
            .ok()
            .and_then(|pem| reqwest::Certificate::from_pem_bundle(&pem).ok())
        {
            Some(certs) if !certs.is_empty() => roots.extend(certs),
            _ => skipped += 1,
        }
    }
    if skipped > 0 {
        tracing::warn!("Skipped {skipped} unreadable CA certificates");
    }
    roots
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::read_roots;
    use crate::test_util::test_temp_dir;

    #[test]
    fn reads_the_first_existing_dir_and_skips_junk() {
        let dir = test_temp_dir("http_roots");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("root.0"), include_str!("isrg_root_x1.pem")).unwrap();
        std::fs::write(dir.join("junk.0"), "not a certificate").unwrap();

        let missing = dir.join("missing");
        assert_eq!(read_roots(&[&missing, &dir]).len(), 1);
        assert!(read_roots(&[&missing]).is_empty());
        assert!(read_roots(&[] as &[&Path]).is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
