//! NextCloud WebDAV client — file operations for report/theme storage.
//!
//! All operations use HTTP Basic Auth with the app-user/app-password
//! credentials from configuration.

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// Build the base WebDAV URL for a project's file path.
///
/// Format: `<nc_url>/remote.php/dav/files/<user>/<encoded_project_path>`
///
/// Only the project path portion is URL-encoded (e.g. spaces and special
/// chars become `%20` and `%26`), while path separators remain as `/`.
/// This mirrors the shell script's `urlEncode` function applied to just
/// the project folder string.
pub fn webdav_base_url(nc_url: &str, user: &str, project_path: &str) -> String {
    // Encode each path component separately so `/` stays as `/`
    let encoded_path: Vec<String> = project_path.split('/').map(|s| urlencoding::encode(s).into_owned()).collect();
    let encoded_path = encoded_path.join("/");
    format!(
        "{}/remote.php/dav/files/{}/{}",
        nc_url.trim_end_matches('/'),
        user,
        encoded_path
    )
}

/// Check if a file exists on WebDAV via HEAD request.
///
/// Returns `true` if the resource exists (HTTP 200/204), `false` otherwise.
/// If `filename` is empty, checks the base URL itself (useful for verifying
/// that the WebDAV folder is reachable).
pub async fn nc_has_file(
    base_url: &str,
    filename: &str,
    user: &str,
    password: &str,
) -> Result<bool> {
    let url = if filename.is_empty() {
        base_url.to_string()
    } else {
        format!("{}/{}", base_url, filename)
    };
    let client = reqwest::Client::new();

    let response = client
        .head(&url)
        .basic_auth(user, Some(password))
        .send()
        .await
        .with_context(|| format!("HEAD request failed for: {}", url))?;

    Ok(response.status().is_success())
}

/// Fetch a file from WebDAV and save it locally.
pub async fn fetch_file(
    base_url: &str,
    filename: &str,
    user: &str,
    password: &str,
    local_path: &Path,
) -> Result<()> {
    let url = format!("{}/{}", base_url, filename);
    let client = reqwest::Client::new();

    let response = client
        .get(&url)
        .basic_auth(user, Some(password))
        .send()
        .await
        .with_context(|| format!("GET request failed for: {}", url))?;

    if !response.status().is_success() {
        anyhow::bail!(
            "WebDAV returned {} for {}",
            response.status(),
            filename
        );
    }

    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("Failed to read response body for {}", filename))?;

    fs::write(local_path, &bytes)
        .with_context(|| format!("Failed to write file to {:?}", local_path))?;

    Ok(())
}

/// Fetch a file from WebDAV and return its contents as a string.
pub async fn fetch_file_body(
    base_url: &str,
    filename: &str,
    user: &str,
    password: &str,
) -> Result<String> {
    let url = format!("{}/{}", base_url, filename);
    let client = reqwest::Client::new();

    let response = client
        .get(&url)
        .basic_auth(user, Some(password))
        .send()
        .await
        .with_context(|| format!("GET request failed for: {}", url))?;

    if !response.status().is_success() {
        anyhow::bail!(
            "WebDAV returned {} for {}",
            response.status(),
            filename
        );
    }

    let body = response
        .text()
        .await
        .with_context(|| format!("Failed to read response body for {}", filename))?;

    Ok(body)
}

/// Upload data to WebDAV from a local file.
///
/// Uses HTTP PUT with Basic Auth. The `encoded_filename` should already
/// be URL-encoded if it contains special characters.
pub async fn put_file(
    base_url: &str,
    encoded_filename: &str,
    local_path: &Path,
    user: &str,
    password: &str,
) -> Result<()> {
    let url = format!("{}/{}", base_url, encoded_filename);
    let client = reqwest::Client::new();

    let body = fs::read(local_path)
        .with_context(|| format!("Failed to read local file for upload: {:?}", local_path))?;

    let response = client
        .put(&url)
        .basic_auth(user, Some(password))
        .body(body)
        .send()
        .await
        .with_context(|| format!("PUT request failed for: {}", url))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "WebDAV upload returned {} for {}: {}",
            status,
            encoded_filename,
            body
        );
    }

    Ok(())
}

/// Upload a string body to WebDAV.
///
/// Uses HTTP PUT with Basic Auth. This is the same as `put_file` but works
/// directly with a string, avoiding the need for a temp file.
pub async fn put_file_body(
    base_url: &str,
    encoded_filename: &str,
    body: &str,
    user: &str,
    password: &str,
) -> Result<()> {
    let url = format!("{}/{}", base_url, encoded_filename);
    let client = reqwest::Client::new();

    let response = client
        .put(&url)
        .basic_auth(user, Some(password))
        .body(body.to_string())
        .send()
        .await
        .with_context(|| format!("PUT request failed for: {}", url))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "WebDAV upload returned {} for {}: {}",
            status,
            encoded_filename,
            body
        );
    }

    Ok(())
}

/// Copy a file on WebDAV using the HTTP COPY verb.
///
/// `source_encoded` should be the URL-encoded source path (relative to base_url).
/// `destination` is the full destination path (relative to base_url, no encoding needed
/// — it will be encoded here).
pub async fn copy_file(
    base_url: &str,
    source_encoded: &str,
    destination: &str,
    user: &str,
    password: &str,
) -> Result<()> {
    let source_url = format!("{}/{}", base_url, source_encoded);
    let dest_encoded = urlencoding::encode(destination);
    let dest_url = format!("{}/{}", base_url, dest_encoded);
    let client = reqwest::Client::new();

    let response = client
        .request(reqwest::Method::from_bytes(b"COPY")?, &source_url)
        .header("Destination", &dest_url)
        .basic_auth(user, Some(password))
        .send()
        .await
        .with_context(|| format!("COPY request failed for {} → {}", source_url, dest_url))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "WebDAV COPY returned {} for {}: {}",
            status,
            destination,
            body
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webdav_base_url_basic() {
        let url = webdav_base_url(
            "https://nextcloud.example.com",
            "liam",
            "Personal/My Project",
        );
        assert_eq!(
            url,
            "https://nextcloud.example.com/remote.php/dav/files/liam/Personal/My%20Project"
        );
    }

    #[test]
    fn test_webdav_base_url_with_special_chars() {
        let url = webdav_base_url(
            "https://nextcloud.example.com",
            "user",
            "Personal/Project & Co",
        );
        assert_eq!(
            url,
            "https://nextcloud.example.com/remote.php/dav/files/user/Personal/Project%20%26%20Co"
        );
    }

    #[test]
    fn test_webdav_base_url_single_segment() {
        let url = webdav_base_url("https://nc.example.com", "user", "MyFolder");
        assert_eq!(
            url,
            "https://nc.example.com/remote.php/dav/files/user/MyFolder"
        );
    }

    #[test]
    fn test_webdav_base_url_strips_trailing_slash() {
        let url = webdav_base_url("https://example.com/", "user", "path");
        assert_eq!(
            url,
            "https://example.com/remote.php/dav/files/user/path"
        );
    }

    #[test]
    fn test_url_encoding() {
        // Verify urlencoding works for common special chars
        assert_eq!(urlencoding::encode("Hello World"), "Hello%20World");
        assert_eq!(urlencoding::encode("a&b"), "a%26b");
        assert_eq!(urlencoding::encode("a+b"), "a%2Bb"); // + is NOT space in URL encoding
    }
}
