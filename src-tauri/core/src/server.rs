use crate::share::{Share, ShareKind};
use crate::state::AppState;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use std::io::SeekFrom;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

/// Builds the axum router for serving landing pages and downloads.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/d/:token", get(landing_page))
        // Accept GET for password-free shares (Cloudflare-friendly direct link)
        // and POST for password-protected shares (password sent in form body).
        .route("/d/:token/download", get(download).post(download))
        // Inline view of the file with a real content type. Used as the Open
        // Graph preview image for image shares (WhatsApp/Slack/etc. fetch it).
        .route("/d/:token/raw", get(raw))
        .with_state(state)
}

/// Looks up a share for the public link routes. Hidden shares don't exist here.
fn link_share(state: &AppState, token: &str) -> Option<Share> {
    let registry = state.registry.lock().unwrap();
    registry.get(token).filter(|s| s.kind == ShareKind::Link).cloned()
}

/// Maps a file extension to a content type for inline serving / link previews.
fn content_type_for(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// True if the file looks like a raster image we can use as a preview thumbnail.
fn is_image(name: &str) -> bool {
    matches!(
        content_type_for(name),
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp"
    )
}

/// Human-friendly file-kind label for the preview subtitle, e.g. "PNG image".
fn kind_label(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "PNG image",
        "jpg" | "jpeg" => "JPEG image",
        "gif" => "GIF image",
        "webp" => "WebP image",
        "bmp" => "Bitmap image",
        "svg" => "SVG image",
        "pdf" => "PDF document",
        "mp4" | "webm" | "mov" => "Video",
        "mp3" | "wav" => "Audio",
        "zip" | "gz" | "tar" | "rar" | "7z" => "Archive",
        "txt" | "md" => "Text document",
        "doc" | "docx" => "Word document",
        "xls" | "xlsx" => "Spreadsheet",
        "ppt" | "pptx" => "Presentation",
        _ => "File",
    }
}

/// Reconstructs the public base URL (scheme://host) from request headers so
/// Open Graph tags can carry absolute URLs. Cloudflare quick tunnels are HTTPS
/// and set `x-forwarded-proto`; fall back to https for any tunneled request.
fn base_url(headers: &HeaderMap) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("https");
    Some(format!("{scheme}://{host}"))
}

/// Escapes a string for safe interpolation into HTML text/attribute contexts.
/// The landing page is served to untrusted remote viewers, so a file name
/// containing HTML metacharacters must not be able to inject markup or script.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Sanitizes a file name for use in a `Content-Disposition` header value:
/// strips quotes (which would break the quoted-string) and control characters
/// (which could inject CRLF and split the response).
fn sanitize_filename(s: &str) -> String {
    s.replace('"', "_")
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

async fn landing_page(
    Path(token): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let share = link_share(&state, &token);
    let Some(share) = share else {
        return (StatusCode::NOT_FOUND, "Link not found or revoked.").into_response();
    };
    let needs_pw = share.password_hash.is_some();
    // For password-free shares use a plain <a> GET link so the browser issues
    // a standard GET request. Cloudflare quick tunnels can show a browser
    // challenge on POST requests, which breaks the download; GET links work
    // reliably through the tunnel. Password-protected shares still need POST
    // to carry the password in the request body.
    let download_widget = if needs_pw {
        format!(
            r#"<form method="post" action="/d/{token}/download">
<input type="password" name="password" placeholder="Password" required /><br/>
<button type="submit">Download</button></form>"#,
            token = html_escape(&token)
        )
    } else {
        format!(
            r#"<a href="/d/{token}/download"><button>Download</button></a>"#,
            token = html_escape(&token)
        )
    };

    let name = html_escape(&share.name);
    let size = human_size(share.size);
    let kind = kind_label(&share.name);
    let subtitle = format!("{kind} · {size}");

    // Open Graph / Twitter tags so chat apps render a rich preview. For
    // password-free image shares the file itself becomes the preview image;
    // protected shares never expose an og:image (it would leak the content).
    let base = base_url(&headers);
    let mut og = format!(
        r#"<meta property="og:site_name" content="Tunneldrop">
<meta property="og:type" content="website">
<meta property="og:title" content="{name}">
<meta property="og:description" content="{desc}">"#,
        name = name,
        desc = html_escape(&format!("{subtitle} — download via Tunneldrop")),
    );
    if let Some(base) = &base {
        og.push_str(&format!(
            "\n<meta property=\"og:url\" content=\"{}/d/{}\">",
            html_escape(base),
            html_escape(&token)
        ));
    }
    let card = if !needs_pw && is_image(&share.name) {
        if let Some(base) = &base {
            og.push_str(&format!(
                "\n<meta property=\"og:image\" content=\"{base}/d/{token}/raw\">\
                 \n<meta property=\"og:image:alt\" content=\"{name}\">",
                base = html_escape(base),
                token = html_escape(&token),
                name = name,
            ));
        }
        "summary_large_image"
    } else {
        "summary"
    };
    og.push_str(&format!("\n<meta name=\"twitter:card\" content=\"{card}\">"));

    let lock = if needs_pw { "🔒 " } else { "" };
    let html = format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{name}</title>
{og}
<style>body{{font-family:system-ui;max-width:32rem;margin:4rem auto;text-align:center;padding:0 1rem}}
h1{{font-size:1.4rem;word-break:break-all}}
.sub{{color:#666;margin:.2rem 0 1.5rem}}
button{{padding:.6rem 1.2rem;font-size:1rem;cursor:pointer;border:0;border-radius:.4rem;background:#1c54d6;color:#fff}}
a{{text-decoration:none}}
input{{padding:.5rem;margin:.5rem;font-size:1rem}}</style></head>
<body><h1>{lock}{name}</h1><p class="sub">{subtitle}</p>
{download_widget}</body></html>"#,
        name = name,
        og = og,
        subtitle = html_escape(&subtitle),
        lock = lock,
        download_widget = download_widget,
    );
    Html(html).into_response()
}

/// Serves the file inline (real content type, `inline` disposition) for use as
/// a link-preview image and in-browser viewing. Refuses password-protected
/// shares so protected content is never exposed without the password.
async fn raw(
    Path(token): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let share = link_share(&state, &token);
    let Some(share) = share else {
        return (StatusCode::NOT_FOUND, "Link not found or revoked.").into_response();
    };
    if share.password_hash.is_some() {
        return (StatusCode::NOT_FOUND, "Not available.").into_response();
    }
    stream_share(&share, &headers, Disposition::Inline).await
}

/// How a browser should treat a served file.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Attachment,
    Inline,
}

enum RangeSpec {
    Full,
    Partial { start: u64, end: u64 },
    Unsatisfiable,
}

/// Reads the forms a resuming download uses, `bytes=N-` and `bytes=N-M`. Any
/// other form is ignored, which the HTTP spec allows: the client gets the file.
fn parse_range(headers: &HeaderMap, size: u64) -> RangeSpec {
    let Some(value) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) else {
        return RangeSpec::Full;
    };
    let Some((start, end)) = value.strip_prefix("bytes=").and_then(|r| r.split_once('-')) else {
        return RangeSpec::Full;
    };
    let Ok(start) = start.parse::<u64>() else {
        return RangeSpec::Full;
    };
    let end = if end.is_empty() {
        u64::MAX
    } else {
        match end.parse::<u64>() {
            Ok(end) if end >= start => end,
            _ => return RangeSpec::Full,
        }
    };
    if start >= size {
        return RangeSpec::Unsatisfiable;
    }
    RangeSpec::Partial { start, end: end.min(size - 1) }
}

/// Serves a share's file, honouring Range requests so an interrupted download
/// can resume. Access checks are the caller's job.
pub async fn stream_share(share: &Share, headers: &HeaderMap, disposition: Disposition) -> Response {
    let mut range = parse_range(headers, share.size);
    if matches!(range, RangeSpec::Unsatisfiable) {
        return (
            StatusCode::RANGE_NOT_SATISFIABLE,
            [(header::CONTENT_RANGE, format!("bytes */{}", share.size))],
        )
            .into_response();
    }

    let mut file = match share.source.open() {
        Ok(f) => tokio::fs::File::from_std(f),
        Err(_) => return (StatusCode::GONE, "File no longer available.").into_response(),
    };

    // An Android provider can hand back a pipe, which can't seek. The client
    // then gets the whole file, which the 200 status tells it.
    if let RangeSpec::Partial { start, .. } = range {
        if file.seek(SeekFrom::Start(start)).await.is_err() {
            range = RangeSpec::Full;
        }
    }

    let (content_type, disposition_kind) = match disposition {
        Disposition::Attachment => ("application/octet-stream", "attachment"),
        Disposition::Inline => (content_type_for(&share.name), "inline"),
    };
    let mut response_headers = vec![
        (header::CONTENT_TYPE, content_type.to_string()),
        (header::ACCEPT_RANGES, "bytes".to_string()),
        (
            header::CONTENT_DISPOSITION,
            format!("{disposition_kind}; filename=\"{}\"", sanitize_filename(&share.name)),
        ),
    ];

    let (status, length) = match range {
        RangeSpec::Partial { start, end } => {
            response_headers.push((
                header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{}", share.size),
            ));
            (StatusCode::PARTIAL_CONTENT, end - start + 1)
        }
        _ => (StatusCode::OK, share.size),
    };
    response_headers.push((header::CONTENT_LENGTH, length.to_string()));

    let mut response = Response::new(Body::from_stream(ReaderStream::new(file.take(length))));
    *response.status_mut() = status;
    for (name, value) in response_headers {
        if let Ok(value) = value.parse() {
            response.headers_mut().insert(name, value);
        }
    }
    response
}

#[derive(serde::Deserialize, Default)]
struct DownloadForm {
    password: Option<String>,
}

async fn download(
    Path(token): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    form: Option<axum::Form<DownloadForm>>,
) -> Response {
    let share = link_share(&state, &token);
    let Some(share) = share else {
        return (StatusCode::NOT_FOUND, "Link not found or revoked.").into_response();
    };

    if let Some(hash) = &share.password_hash {
        let provided = form.and_then(|f| f.0.password).unwrap_or_default();
        if provided.is_empty() {
            // GET request on a password-protected share: bounce back to landing page.
            return Redirect::to(&format!("/d/{token}")).into_response();
        }
        if !crate::password::verify_password(&provided, hash) {
            return (StatusCode::UNAUTHORIZED, "Incorrect password.").into_response();
        }
    }

    let response = stream_share(&share, &headers, Disposition::Attachment).await;

    // NOTE: the count increments when the transfer begins, not on completion;
    // an aborted download is still counted. Resuming one isn't a new download.
    let resuming = matches!(parse_range(&headers, share.size), RangeSpec::Partial { start, .. } if start > 0);
    if response.status().is_success() && !resuming {
        let mut reg = state.registry.lock().unwrap();
        if let Some(s) = reg.get_mut(&token) {
            s.download_count += 1;
        }
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use std::io::Write;
    use std::path::PathBuf;
    use tower::ServiceExt; // for `oneshot`

    fn state_with_share(share: Share) -> AppState {
        let st = AppState::new(0, "cloudflared".into());
        st.registry.lock().unwrap().insert(share);
        st
    }

    fn temp_file(contents: &[u8]) -> (tempfile::NamedTempFile, PathBuf) {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(contents).unwrap();
        let path = f.path().to_path_buf();
        (f, path)
    }

    async fn get_with_range(app: Router, uri: &str, range: Option<&str>) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut req = Request::builder().uri(uri);
        if let Some(range) = range {
            req = req.header(header::RANGE, range);
        }
        let res = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, headers, body.to_vec())
    }

    fn digits_state() -> (tempfile::NamedTempFile, AppState) {
        let (keep, path) = temp_file(b"0123456789");
        let share = Share::new("tok".into(), path, "digits.txt".into(), 10, None);
        (keep, state_with_share(share))
    }

    #[tokio::test]
    async fn hidden_shares_are_not_served_on_public_link_routes() {
        let (_keep, path) = temp_file(b"secret");
        let mut share = Share::new("hid".into(), path, "secret.txt".into(), 6, None);
        share.kind = ShareKind::Hidden;
        let st = state_with_share(share);
        for uri in ["/d/hid", "/d/hid/download", "/d/hid/raw"] {
            let (status, _, _) = get_with_range(build_router(st.clone()), uri, None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        }
    }

    #[tokio::test]
    async fn range_returns_206_with_the_requested_bytes() {
        let (_keep, st) = digits_state();
        let (status, headers, body) =
            get_with_range(build_router(st), "/d/tok/download", Some("bytes=4-")).await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(headers[header::CONTENT_RANGE], "bytes 4-9/10");
        assert_eq!(body, b"456789");
    }

    #[tokio::test]
    async fn bounded_range_returns_exact_slice() {
        let (_keep, st) = digits_state();
        let (status, headers, body) =
            get_with_range(build_router(st), "/d/tok/download", Some("bytes=2-3")).await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(headers[header::CONTENT_LENGTH], "2");
        assert_eq!(body, b"23");
    }

    #[tokio::test]
    async fn range_end_past_the_file_is_clamped() {
        let (_keep, st) = digits_state();
        let (status, headers, body) =
            get_with_range(build_router(st), "/d/tok/download", Some("bytes=8-100")).await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(headers[header::CONTENT_RANGE], "bytes 8-9/10");
        assert_eq!(body, b"89");
    }

    #[tokio::test]
    async fn range_past_the_end_returns_416() {
        let (_keep, st) = digits_state();
        let (status, headers, _) =
            get_with_range(build_router(st), "/d/tok/download", Some("bytes=10-")).await;
        assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(headers[header::CONTENT_RANGE], "bytes */10");
    }

    #[tokio::test]
    async fn unsupported_range_forms_return_the_whole_file() {
        let (_keep, st) = digits_state();
        for range in ["bytes=-3", "bytes=1-2,4-5", "bytes=5-2", "lines=1-2"] {
            let (status, _, body) =
                get_with_range(build_router(st.clone()), "/d/tok/download", Some(range)).await;
            assert_eq!(status, StatusCode::OK, "{range}");
            assert_eq!(body, b"0123456789", "{range}");
        }
    }

    #[tokio::test]
    async fn full_download_advertises_ranges_and_counts_once() {
        let (_keep, st) = digits_state();
        let (status, headers, _) =
            get_with_range(build_router(st.clone()), "/d/tok/download", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::ACCEPT_RANGES], "bytes");
        let count = || st.registry.lock().unwrap().get("tok").unwrap().download_count;
        assert_eq!(count(), 1);

        get_with_range(build_router(st.clone()), "/d/tok/download", Some("bytes=4-")).await;
        assert_eq!(count(), 1, "resuming is not a new download");
    }

    #[tokio::test]
    async fn raw_route_keeps_its_inline_headers_and_supports_range() {
        let (_keep, st) = digits_state();
        let (status, headers, body) =
            get_with_range(build_router(st), "/d/tok/raw", Some("bytes=4-5")).await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert!(headers[header::CONTENT_DISPOSITION].to_str().unwrap().starts_with("inline"));
        assert!(headers[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/plain"));
        assert_eq!(body, b"45");
    }

    #[tokio::test]
    async fn landing_page_shows_name() {
        let (_keep, path) = temp_file(b"hello");
        let share = Share::new("tok1".into(), path, "report.pdf".into(), 5, None);
        let app = build_router(state_with_share(share));
        let res = app
            .oneshot(Request::builder().uri("/d/tok1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("report.pdf"));
    }

    #[test]
    fn html_escape_neutralizes_markup() {
        assert_eq!(
            html_escape("<script>alert('x')&\"</script>"),
            "&lt;script&gt;alert(&#x27;x&#x27;)&amp;&quot;&lt;/script&gt;"
        );
    }

    #[test]
    fn sanitize_filename_strips_quotes_and_controls() {
        assert_eq!(sanitize_filename("ev\"il\r\n.txt"), "ev_il.txt");
    }

    #[tokio::test]
    async fn landing_page_escapes_malicious_name() {
        let (_keep, path) = temp_file(b"x");
        let share = Share::new(
            "tokx".into(),
            path,
            "<script>alert(1)</script>.txt".into(),
            1,
            None,
        );
        let app = build_router(state_with_share(share));
        let res = app
            .oneshot(Request::builder().uri("/d/tokx").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8_lossy(&body);
        assert!(!html.contains("<script>alert(1)"), "raw script must not appear");
        assert!(html.contains("&lt;script&gt;"), "name must be HTML-escaped");
    }

    #[tokio::test]
    async fn unknown_token_is_404() {
        let app = build_router(AppState::new(0, "cloudflared".into()));
        let res = app
            .oneshot(Request::builder().uri("/d/nope").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn download_streams_file_and_increments_count() {
        let (_keep, path) = temp_file(b"hello world");
        let share = Share::new("tok2".into(), path, "a.txt".into(), 11, None);
        let st = state_with_share(share);
        let app = build_router(st.clone());
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/d/tok2/download")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"hello world");
        assert_eq!(st.registry.lock().unwrap().get("tok2").unwrap().download_count, 1);
    }

    #[tokio::test]
    async fn protected_download_rejects_wrong_password() {
        let (_keep, path) = temp_file(b"secret");
        let hash = crate::password::hash_password("letmein");
        let share = Share::new("tok3".into(), path, "s.txt".into(), 6, Some(hash));
        let app = build_router(state_with_share(share));
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/d/tok3/download")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("password=wrong"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn protected_download_accepts_correct_password() {
        let (_keep, path) = temp_file(b"secret");
        let hash = crate::password::hash_password("letmein");
        let share = Share::new("tok4".into(), path, "s.txt".into(), 6, Some(hash));
        let app = build_router(state_with_share(share));
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/d/tok4/download")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("password=letmein"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"secret");
    }

    #[tokio::test]
    async fn get_download_works_for_unprotected_share() {
        let (_keep, path) = temp_file(b"get works");
        let share = Share::new("tok5".into(), path, "b.txt".into(), 9, None);
        let app = build_router(state_with_share(share));
        let res = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/d/tok5/download")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"get works");
    }

    #[tokio::test]
    async fn get_download_on_protected_share_redirects_to_landing() {
        let (_keep, path) = temp_file(b"secret");
        let hash = crate::password::hash_password("pw");
        let share = Share::new("tok6".into(), path, "s.txt".into(), 6, Some(hash));
        let app = build_router(state_with_share(share));
        let res = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/d/tok6/download")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn landing_page_uses_get_link_for_unprotected_share() {
        let (_keep, path) = temp_file(b"hi");
        let share = Share::new("tok7".into(), path, "f.txt".into(), 2, None);
        let app = build_router(state_with_share(share));
        let res = app
            .oneshot(Request::builder().uri("/d/tok7").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains(r#"href="/d/tok7/download""#), "must use GET link, not POST form");
        assert!(!html.contains(r#"method="post""#), "must not use POST form for unprotected share");
    }

    #[tokio::test]
    async fn landing_page_uses_post_form_for_protected_share() {
        let (_keep, path) = temp_file(b"hi");
        let hash = crate::password::hash_password("pw");
        let share = Share::new("tok8".into(), path, "g.txt".into(), 2, Some(hash));
        let app = build_router(state_with_share(share));
        let res = app
            .oneshot(Request::builder().uri("/d/tok8").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains(r#"method="post""#), "protected share must use POST form");
        assert!(html.contains(r#"type="password""#), "protected share must have password field");
    }

    #[tokio::test]
    async fn landing_page_emits_og_tags() {
        let (_keep, path) = temp_file(b"hi");
        let share = Share::new("ogtok".into(), path, "report.pdf".into(), 2, None);
        let app = build_router(state_with_share(share));
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/d/ogtok")
                    .header("host", "x.trycloudflare.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains(r#"property="og:title" content="report.pdf""#));
        assert!(html.contains("PDF document"), "subtitle should describe the kind");
        // A non-image share must not advertise an og:image.
        assert!(!html.contains("og:image"));
    }

    #[tokio::test]
    async fn image_share_advertises_og_image() {
        let (_keep, path) = temp_file(b"img");
        let share = Share::new("imgtok".into(), path, "photo.png".into(), 3, None);
        let app = build_router(state_with_share(share));
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/d/imgtok")
                    .header("host", "x.trycloudflare.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8_lossy(&body);
        assert!(
            html.contains(r#"content="https://x.trycloudflare.com/d/imgtok/raw""#),
            "image share must point og:image at the /raw endpoint"
        );
        assert!(html.contains(r#"content="summary_large_image""#));
    }

    #[tokio::test]
    async fn protected_image_share_has_no_og_image() {
        let (_keep, path) = temp_file(b"img");
        let hash = crate::password::hash_password("pw");
        let share = Share::new("ptok".into(), path, "secret.png".into(), 3, Some(hash));
        let app = build_router(state_with_share(share));
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/d/ptok")
                    .header("host", "x.trycloudflare.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8_lossy(&body);
        assert!(!html.contains("og:image"), "protected share must not expose og:image");
    }

    #[tokio::test]
    async fn raw_serves_image_inline_with_content_type() {
        let (_keep, path) = temp_file(b"\x89PNG fake");
        let share = Share::new("rawtok".into(), path, "p.png".into(), 9, None);
        let app = build_router(state_with_share(share));
        let res = app
            .oneshot(Request::builder().uri("/d/rawtok/raw").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers().get("content-type").unwrap(), "image/png");
        assert!(res
            .headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("inline"));
    }

    #[tokio::test]
    async fn raw_refuses_protected_share() {
        let (_keep, path) = temp_file(b"secret");
        let hash = crate::password::hash_password("pw");
        let share = Share::new("rawp".into(), path, "s.png".into(), 6, Some(hash));
        let app = build_router(state_with_share(share));
        let res = app
            .oneshot(Request::builder().uri("/d/rawp/raw").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
}
