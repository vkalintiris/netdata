//! Static files of the dashboard, ported from `find_filename_to_serve()`, `web_server_static_file()` and
//! `append_slash_to_url_and_redirect()` in `src/web/server/web_client.c`.

use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

use netdata_agent_web::content_type::ContentType;
use netdata_agent_web::status;
use nix::errno::Errno;
use nix::fcntl::OFlag;

use crate::acl;
use crate::router::{FILENAME_MAX, Route};
use crate::server::{self, Reply};

const REDIRECT_BODY: &str = "<!DOCTYPE html><html><body onload=\"window.location.href = window.location.origin + window.location.pathname + '/' + window.location.search + window.location.hash\">Redirecting. In case your browser does not support redirection, please click <a onclick=\"window.location.href = window.location.origin + window.location.pathname + '/' + window.location.search + window.location.hash\">here</a>.</body></html>";

/// `append_slash_to_url_and_redirect()`: a 301 to the last path segment of the URL as received plus a slash,
/// keeping the query string.
pub fn append_slash_and_redirect(url: &[u8]) -> Reply {
    let segment_start = |end: usize| {
        let mut e = end;
        while e > 0 && url[e] != b'/' {
            e -= 1;
        }
        if url.get(e) == Some(&b'/') { e + 1 } else { e }
    };
    let mut location = b"Location: ".to_vec();
    match url.iter().position(|&c| c == b'?') {
        Some(q) if q > 0 => {
            location.extend_from_slice(&url[segment_start(q - 1)..q]);
            location.push(b'/');
            location.extend_from_slice(&url[q..]);
        }
        _ => {
            location.extend_from_slice(&url[segment_start(url.len().saturating_sub(1))..]);
            location.push(b'/');
        }
    }
    location.extend_from_slice(b"\r\n");
    Reply {
        code: status::MOVED_PERM,
        content_type: ContentType::TextHtml,
        body: REDIRECT_BODY.as_bytes().to_vec(),
        headers: location,
        ..Reply::default()
    }
}

/// `snprintfz()` into a `FILENAME_MAX` buffer: at most `FILENAME_MAX - 1` bytes.
fn bounded(path: String) -> String {
    let mut path = path;
    // Paths are ASCII here: the filename passed validation and the web directory comes from the configuration.
    if path.len() >= FILENAME_MAX && path.is_char_boundary(FILENAME_MAX - 1) {
        path.truncate(FILENAME_MAX - 1);
    }
    path
}

/// `find_filename_to_serve()`: the path to send, its metadata, and whether it is a directory's `index.html`. A
/// fallback to a directory marks the route as ending in a slash, which suppresses the redirect.
fn find_filename_to_serve(
    route: &mut Route<'_>,
    filename: &str,
) -> Option<(String, Metadata, bool)> {
    let web = &route.shared.web_dir;
    let mark_slash = |route: &mut Route<'_>| {
        if !filename.is_empty() {
            route.trailing_slash = true;
        }
    };
    // What to try when the first path does not exist.
    enum Fallback {
        None,
        Unversioned,
        VersionDir(u8),
        WebDir,
    }
    let (mut dst, fallback) = match (route.has_extension, route.version) {
        (true, None) => (format!("{web}/{filename}"), Fallback::None),
        (true, Some(v)) => (format!("{web}/v{v}/{filename}"), Fallback::Unversioned),
        (false, Some(v)) if !filename.is_empty() => {
            (format!("{web}/{filename}"), Fallback::VersionDir(v))
        }
        (false, Some(v)) => (format!("{web}/v{v}"), Fallback::None),
        (false, None) => (format!("{web}/{filename}"), Fallback::WebDir),
    };
    dst = bounded(dst);
    let mut meta = fs::metadata(&dst);
    if meta.is_err() {
        dst = match fallback {
            Fallback::None => return None,
            Fallback::Unversioned => format!("{web}/{filename}"),
            Fallback::VersionDir(v) => {
                mark_slash(route);
                format!("{web}/v{v}")
            }
            Fallback::WebDir => {
                mark_slash(route);
                web.clone()
            }
        };
        dst = bounded(dst);
        meta = fs::metadata(&dst);
    }
    let mut meta = meta.ok()?;
    let mut is_dir = false;
    if meta.is_dir() {
        if dst.len() > FILENAME_MAX - 11 {
            return None;
        }
        dst.push_str("/index.html");
        meta = fs::metadata(&dst).ok()?;
        is_dir = true;
    }
    Some((dst, meta, is_dir))
}

/// Reads a regular file the way `web_server_static_file()` does: non-blocking open, `fstat()`, whole contents.
fn read_regular(path: &str, meta: &Metadata) -> io::Result<(Vec<u8>, i64)> {
    let invalid = || io::Error::from_raw_os_error(Errno::EINVAL as i32);
    if !meta.is_file() {
        return Err(invalid());
    }
    let mut file = File::options()
        .read(true)
        .custom_flags(OFlag::O_NONBLOCK.bits())
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(invalid());
    }
    if meta.len() > u64::from(u32::MAX - 2) {
        return Err(io::Error::from_raw_os_error(Errno::EFBIG as i32));
    }
    let size = meta.len() as usize;
    let mut data = vec![0; size];
    // read_exact retries EINTR; a short file (truncated while reading) is EIO in C, UnexpectedEof here: both 404.
    file.read_exact(&mut data)?;
    Ok((data, meta.mtime()))
}

/// `web_server_static_file()`.
pub fn serve(route: &mut Route<'_>, filename: &[u8]) -> Reply {
    if !acl::can(route.acl, acl::bits::DASHBOARD) {
        return server::permission_denied_acl();
    }
    let start = filename
        .iter()
        .position(|&c| c != b'/')
        .unwrap_or(filename.len());
    let filename = &filename[start..];
    if filename
        .iter()
        .any(|&c| !c.is_ascii_alphanumeric() && !matches!(c, b'/' | b'.' | b'-' | b'_'))
    {
        return Reply::html(
            status::BAD_REQUEST,
            "Filename contains invalid characters: ",
            filename,
        );
    }
    if filename.windows(2).any(|w| w == b"..") {
        return Reply::html(
            status::BAD_REQUEST,
            "Relative filenames are not supported: ",
            filename,
        );
    }
    // Validated as ASCII above.
    let name = String::from_utf8_lossy(filename).into_owned();
    let Some((path, meta, is_dir)) = find_filename_to_serve(route, &name) else {
        return Reply::html(
            status::NOT_FOUND,
            "File does not exist, or is not accessible: ",
            filename,
        );
    };
    if is_dir && !route.trailing_slash {
        return append_slash_and_redirect(route.url_as_received);
    }
    match read_regular(&path, &meta) {
        Ok((body, mtime)) => Reply {
            code: status::OK,
            content_type: ContentType::for_filename(path.as_bytes()),
            body,
            date: mtime,
            expires: crate::server::now() + 86400,
            no_cacheable: false,
            ..Reply::default()
        },
        Err(err)
            if matches!(
                err.raw_os_error().map(Errno::from_raw),
                Some(Errno::EBUSY | Errno::EAGAIN)
            ) =>
        {
            let mut reply = Reply::html(
                status::REDIR_TEMP,
                "File is currently busy, please try again later: ",
                filename,
            );
            reply.headers = [b"Location: /", filename, b"\r\n"].concat();
            reply
        }
        Err(_) => Reply::html(status::NOT_FOUND, "Cannot open file: ", filename),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_location_matches_c() {
        let cases: [(&[u8], &[u8]); 5] = [
            (b"/v2", b"Location: v2/\r\n"),
            (b"/host/box?x=1", b"Location: box/?x=1\r\n"),
            (b"/a/b/", b"Location: /\r\n"),
            (b"?q", b"Location: ?q/\r\n"),
            (b"x", b"Location: x/\r\n"),
        ];
        for (url, location) in cases {
            assert_eq!(append_slash_and_redirect(url).headers, location);
        }
    }
}
